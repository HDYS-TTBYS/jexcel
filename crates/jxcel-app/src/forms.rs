//! LAN への入力フォーム配信。
//!
//! 開いているファイルの表の一部の列を、同じネットワークのブラウザから 1 行として入力できるようにする。
//! 回答は開いている `Session` の表に行として追加される（未保存の変更になる。保存は利用者が行う）。
//!
//! 安全のための方針:
//! - URL に推測できないトークンを含め、トークンを知らなければフォームの存在も分からない
//!   （トークンは配信を始めるたびに作り直し、ファイルには保存しない）。
//! - 回答はファイルの型検証を通ったものだけを受け付け、フォームにない列は書けない。
//! - フォームの定義は JSON で渡し、ブラウザ側が DOM API で組み立てる（HTML の埋め込みをしない）。
//! - 本文は 64KB まで。計算列・ネスト型の列は入力欄にできない。

use crate::{Error, Result, Session, Snapshot};
use jxcel_core::{new_id, Column, DataType, Form, JxcelFile, Row};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FORM_PAGE: &str = include_str!("form.html");
const MAX_BODY: u64 = 64 * 1024;
const WORKERS: usize = 4;

/// 合言葉を渡すリクエストヘッダ（URL に載せない）。
const CODE_HEADER: &str = "X-Jxcel-Code";
/// 回答の修正用トークン（送信の応答で渡す）を送るヘッダ
const EDIT_HEADER: &str = "X-Jxcel-Edit";
/// 修正用トークンを覚えておく回答の数（超えたら新しい回答には渡さない）
const MAX_EDITS: usize = 100_000;
/// 合言葉の長さ（バイト数＝文字数）の範囲
const CODE_LEN: std::ops::RangeInclusive<usize> = 4..=64;
/// 1 つの端末（IP アドレス）からの失敗（合言葉の間違い・存在しない URL）がこの回数に達すると、窓の間ロックする
const FAIL_LIMIT: usize = 8;
const FAIL_WINDOW: Duration = Duration::from_secs(300);
/// 1 つの端末からの送信（回答）の上限。窓の間に超えると 429
const SUBMIT_LIMIT: usize = 30;
const SUBMIT_WINDOW: Duration = Duration::from_secs(60);
/// 覚えておく端末の数の上限（超えたら期限切れを捨て、それでも多ければ全部捨てる）
const MAX_TRACKED: usize = 4096;

/// 入力欄にできる列の種類（ブラウザの入力部品に対応する）。計算列・ネスト型は `None`。
pub fn field_kind(c: &Column) -> Option<&'static str> {
    if c.computed.is_some() {
        return None;
    }
    Some(match &c.ty {
        DataType::String => "text",
        DataType::Int => "int",
        DataType::Float => "float",
        DataType::Decimal => "decimal",
        DataType::Bool => "bool",
        DataType::Date => "date",
        DataType::DateTime => "dateTime",
        DataType::Enum { .. } => "enum",
        _ => return None,
    })
}

/// ブラウザに渡すフォームの定義。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormDef {
    pub title: String,
    pub description: String,
    pub fields: Vec<FormField>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormField {
    pub id: String,
    pub label: String,
    pub kind: &'static str,
    pub required: bool,
    /// 選択肢（`enum` のときだけ）
    pub options: Vec<String>,
}

impl Session {
    /// 開いているファイルのフォーム一覧（ファイルが無ければ空）。
    pub fn forms(&self) -> Vec<Form> {
        self.doc
            .as_ref()
            .map(|d| d.file.forms.clone())
            .unwrap_or_default()
    }

    /// 表を指定してフォームを追加する。入力欄は、入力できる列すべて。
    pub fn add_form(&mut self, sheet: &str, schema: &str, name: &str) -> Result<Snapshot> {
        let (sheet, schema, name) = (sheet.to_string(), schema.to_string(), name.to_string());
        self.edit(move |f, _| {
            let s = crate::find_schema(f, &sheet, &schema)?;
            let columns: Vec<String> = s
                .columns
                .iter()
                .filter(|c| field_kind(c).is_some())
                .map(|c| c.id.clone())
                .collect();
            if columns.is_empty() {
                return Err(Error::Invalid(
                    "入力欄にできる列がありません（計算列・ネスト型は使えません）".into(),
                ));
            }
            f.forms.push(Form {
                id: new_id(),
                name,
                sheet,
                schema,
                columns,
                description: String::new(),
            });
            Ok(())
        })
    }

    pub fn update_form(
        &mut self,
        id: &str,
        name: &str,
        sheet: &str,
        schema: &str,
        columns: Vec<String>,
        description: &str,
    ) -> Result<Snapshot> {
        let (id, name, sheet, schema, description) = (
            id.to_string(),
            name.trim().to_string(),
            sheet.to_string(),
            schema.to_string(),
            description.trim().to_string(),
        );
        self.edit(move |f, _| {
            if name.is_empty() {
                return Err(Error::Invalid("フォームの名前が空です".into()));
            }
            let s = crate::find_schema(f, &sheet, &schema)?;
            for c in &columns {
                let col = s
                    .columns
                    .iter()
                    .find(|x| &x.id == c)
                    .ok_or(Error::NotFound("列"))?;
                if field_kind(col).is_none() {
                    return Err(Error::Invalid(format!(
                        "「{}」は入力欄にできません（計算列・ネスト型）",
                        col.name
                    )));
                }
            }
            if columns.is_empty() {
                return Err(Error::Invalid("入力欄を 1 つ以上選んでください".into()));
            }
            let mut seen = std::collections::HashSet::new();
            if !columns.iter().all(|c| seen.insert(c)) {
                return Err(Error::Invalid("同じ列が複数あります".into()));
            }
            let form = f
                .forms
                .iter_mut()
                .find(|x| x.id == id)
                .ok_or(Error::NotFound("フォーム"))?;
            form.name = name;
            form.sheet = sheet;
            form.schema = schema;
            form.columns = columns;
            form.description = description;
            Ok(())
        })
    }

    pub fn delete_form(&mut self, id: &str) -> Result<Snapshot> {
        let id = id.to_string();
        self.edit(move |f, _| {
            let before = f.forms.len();
            f.forms.retain(|x| x.id != id);
            (f.forms.len() != before)
                .then_some(())
                .ok_or(Error::NotFound("フォーム"))
        })
    }

    /// ブラウザに渡すフォームの定義。入力欄にできない列（後から計算列にした等）は黙って外す。
    pub fn form_definition(&self, id: &str) -> Result<FormDef> {
        let d = self.doc.as_ref().ok_or(Error::NoFile)?;
        let form = d
            .file
            .forms
            .iter()
            .find(|f| f.id == id)
            .ok_or(Error::NotFound("フォーム"))?;
        let schema = schema_of(&d.file, form)?;
        let fields: Vec<FormField> = form
            .columns
            .iter()
            .filter_map(|cid| schema.columns.iter().find(|c| &c.id == cid))
            .filter_map(|c| {
                Some(FormField {
                    id: c.id.clone(),
                    label: c.name.clone(),
                    kind: field_kind(c)?,
                    required: c.required,
                    options: match &c.ty {
                        DataType::Enum { values } => values.clone(),
                        _ => vec![],
                    },
                })
            })
            .collect();
        if fields.is_empty() {
            return Err(Error::Invalid("入力できる欄がありません".into()));
        }
        Ok(FormDef {
            title: form.name.clone(),
            description: form.description.clone(),
            fields,
        })
    }

    /// フォームの回答を、対象の表の末尾に 1 行として追加し、その行の ID を返す。
    /// 型に合わない値・必須の欠落・フォームにない列は拒否し、そのときは何も追加しない。
    pub fn submit_form(&mut self, id: &str, values: &Map<String, Value>) -> Result<String> {
        let reg = self.registry.clone();
        let d = self.doc()?;
        let form = d
            .file
            .forms
            .iter()
            .find(|f| f.id == id)
            .ok_or(Error::NotFound("フォーム"))?
            .clone();
        let cells = {
            let schema = schema_of(&d.file, &form)?;
            coerce(&form, schema, values, &reg)?
        };
        let schema = crate::find_schema(&mut d.file, &form.sheet, &form.schema)?;
        let row = Row::new(cells);
        let row_id = row.id.clone();
        schema.rows.push(row);
        d.dirty = true;
        Ok(row_id)
    }

    /// 回答（行）の、フォームの入力欄になっている列の現在の値（列 ID → 値）。値のない列は含まない。
    /// 行が無い（削除された・復元で消えた）ときは `NotFound`。
    pub fn form_answer(&self, id: &str, row_id: &str) -> Result<Map<String, Value>> {
        let d = self.doc.as_ref().ok_or(Error::NoFile)?;
        let form = d
            .file
            .forms
            .iter()
            .find(|f| f.id == id)
            .ok_or(Error::NotFound("フォーム"))?;
        let schema = schema_of(&d.file, form)?;
        let row = schema
            .rows
            .iter()
            .find(|r| r.id == row_id)
            .ok_or(Error::NotFound("回答"))?;
        let mut out = Map::new();
        for cid in &form.columns {
            let fillable = schema
                .columns
                .iter()
                .find(|c| &c.id == cid)
                .is_some_and(|c| field_kind(c).is_some());
            if let (true, Some(v)) = (fillable, row.cells.get(cid)) {
                out.insert(cid.clone(), v.clone());
            }
        }
        Ok(out)
    }

    /// 送信済みの回答（行）を、フォームの入力欄の分だけ書き換える（入力欄にない列は触らない）。
    /// 検証は送信と同じ。値が空の欄はその列の値を消す。拒否したときは何も変えない。
    pub fn edit_form_answer(
        &mut self,
        id: &str,
        row_id: &str,
        values: &Map<String, Value>,
    ) -> Result<()> {
        let reg = self.registry.clone();
        let d = self.doc()?;
        let form = d
            .file
            .forms
            .iter()
            .find(|f| f.id == id)
            .ok_or(Error::NotFound("フォーム"))?
            .clone();
        let (cells, fillable) = {
            let schema = schema_of(&d.file, &form)?;
            if !schema.rows.iter().any(|r| r.id == row_id) {
                return Err(Error::NotFound("回答"));
            }
            let fillable: Vec<String> = form
                .columns
                .iter()
                .filter(|cid| {
                    schema
                        .columns
                        .iter()
                        .find(|c| &c.id == *cid)
                        .is_some_and(|c| field_kind(c).is_some())
                })
                .cloned()
                .collect();
            (coerce(&form, schema, values, &reg)?, fillable)
        };
        let schema = crate::find_schema(&mut d.file, &form.sheet, &form.schema)?;
        let row = schema
            .rows
            .iter_mut()
            .find(|r| r.id == row_id)
            .ok_or(Error::NotFound("回答"))?;
        for cid in fillable {
            match cells.get(&cid) {
                Some(v) => {
                    row.cells.insert(cid, v.clone());
                }
                None => {
                    row.cells.remove(&cid);
                }
            }
        }
        d.dirty = true;
        Ok(())
    }
}

fn schema_of<'a>(file: &'a JxcelFile, form: &Form) -> Result<&'a jxcel_core::DataSchema> {
    file.sheets
        .iter()
        .find(|s| s.id == form.sheet)
        .and_then(|s| s.schemas.iter().find(|c| c.id == form.schema))
        .ok_or(Error::NotFound("フォームの対象の表"))
}

/// 回答（ブラウザからの値）を列の型に直し、検証する。エラーは欄ごとに集めて 1 つにまとめる。
fn coerce(
    form: &Form,
    schema: &jxcel_core::DataSchema,
    values: &Map<String, Value>,
    reg: &jxcel_core::types::TypeRegistry,
) -> Result<BTreeMap<String, Value>> {
    if let Some(k) = values.keys().find(|k| !form.columns.contains(k)) {
        return Err(Error::Invalid(format!("フォームにない欄です: {k}")));
    }
    let mut cells = BTreeMap::new();
    let mut errors = vec![];
    for cid in &form.columns {
        let Some(col) = schema.columns.iter().find(|c| &c.id == cid) else {
            continue;
        };
        let Some(kind) = field_kind(col) else {
            continue;
        };
        let raw = values.get(cid).unwrap_or(&Value::Null);
        let value = match convert(kind, raw) {
            Ok(v) => v,
            Err(m) => {
                errors.push(format!("{}: {m}", col.name));
                continue;
            }
        };
        if let Err(m) = col.validate(&value, reg) {
            // 必須のメッセージは列名を含むので、そのまま使う
            errors.push(if value.is_null() {
                m
            } else {
                format!("{}: {m}", col.name)
            });
            continue;
        }
        if !value.is_null() {
            cells.insert(cid.clone(), value);
        }
    }
    if errors.is_empty() {
        Ok(cells)
    } else {
        Err(Error::Invalid(errors.join("\n")))
    }
}

/// 符号・整数部・小数部だけの 10 進数（指数表記や桁区切りは不可）。
fn is_plain_decimal(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = body
        .split_once('.')
        .map_or((body, None), |(i, f)| (i, Some(f)));
    !int.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}

/// 入力部品の値（多くは文字列）を、列の型の JSON 値にする。空は null。
fn convert(kind: &str, raw: &Value) -> std::result::Result<Value, String> {
    let text = match raw {
        Value::Null => return Ok(Value::Null),
        Value::String(s) if s.trim().is_empty() => return Ok(Value::Null),
        Value::String(s) => Some(s.trim()),
        _ => None,
    };
    match kind {
        "bool" => raw
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| "真偽値ではありません".into()),
        "int" => match (raw, text) {
            (Value::Number(n), _) if n.is_i64() || n.is_u64() => Ok(raw.clone()),
            (_, Some(t)) => t
                .parse::<i64>()
                .map(|n| json!(n))
                .map_err(|_| "整数ではありません".into()),
            _ => Err("整数ではありません".into()),
        },
        "float" => match (raw, text) {
            (Value::Number(_), _) => Ok(raw.clone()),
            (_, Some(t)) => t
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .map(|n| json!(n))
                .ok_or_else(|| "数値ではありません".into()),
            _ => Err("数値ではありません".into()),
        },
        // Decimal は精度を保つため文字列のまま。数値で来たら文字列表現を使う
        "decimal" => match (raw, text) {
            (Value::Number(n), _) => Ok(json!(n.to_string())),
            (_, Some(t)) if is_plain_decimal(t) => Ok(json!(t)),
            _ => Err("数値で入力してください（例: 12.50）".into()),
        },
        // text / date / dateTime / enum
        _ => match raw {
            // 文字列の列は前後の空白をそのまま残す（空白だけの入力は空として扱う）
            Value::String(s) => Ok(json!(if kind == "text" {
                s.as_str()
            } else {
                text.unwrap_or("")
            })),
            _ => Err("文字列ではありません".into()),
        },
    }
}

// ---- サーバー ----

/// 配信中のフォームの URL。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormUrl {
    pub form_id: String,
    pub name: String,
    pub url: String,
    /// 配信を始めてから受け付けた回答の数
    pub submitted: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormsStatus {
    pub running: bool,
    pub port: Option<u16>,
    /// 合言葉が必要か
    pub protected: bool,
    pub urls: Vec<FormUrl>,
}

impl FormsStatus {
    pub fn stopped() -> Self {
        Self {
            running: false,
            port: None,
            protected: false,
            urls: vec![],
        }
    }
}

#[derive(Default)]
struct Tokens {
    by_form: HashMap<String, String>,
    by_token: HashMap<String, String>,
}

/// 端末ごとの失敗と送信の回数の制限（時刻は引数で受け取るのでテストできる）。
#[derive(Default)]
struct Limiter {
    fails: HashMap<IpAddr, VecDeque<Instant>>,
    submits: HashMap<IpAddr, VecDeque<Instant>>,
}

/// 窓より古い記録を捨てる。
fn expire(q: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
        q.pop_front();
    }
}

fn prune(map: &mut HashMap<IpAddr, VecDeque<Instant>>, now: Instant, window: Duration) {
    if map.len() <= MAX_TRACKED {
        return;
    }
    map.retain(|_, q| {
        expire(q, now, window);
        !q.is_empty()
    });
    if map.len() > MAX_TRACKED {
        map.clear();
    }
}

impl Limiter {
    /// ロック中なら、解けるまでの時間。
    fn locked(&mut self, ip: IpAddr, now: Instant) -> Option<Duration> {
        let q = self.fails.get_mut(&ip)?;
        expire(q, now, FAIL_WINDOW);
        if q.len() < FAIL_LIMIT {
            return None;
        }
        Some(FAIL_WINDOW.saturating_sub(now.duration_since(q[q.len() - FAIL_LIMIT])))
    }

    fn fail(&mut self, ip: IpAddr, now: Instant) {
        prune(&mut self.fails, now, FAIL_WINDOW);
        let q = self.fails.entry(ip).or_default();
        expire(q, now, FAIL_WINDOW);
        q.push_back(now);
    }

    /// 送信を 1 回数える。上限を超えていれば、空くまでの時間。
    fn submit(&mut self, ip: IpAddr, now: Instant) -> std::result::Result<(), Duration> {
        prune(&mut self.submits, now, SUBMIT_WINDOW);
        let q = self.submits.entry(ip).or_default();
        expire(q, now, SUBMIT_WINDOW);
        if q.len() >= SUBMIT_LIMIT {
            return Err(SUBMIT_WINDOW.saturating_sub(now.duration_since(q[0])));
        }
        q.push_back(now);
        Ok(())
    }
}

/// 時間を一定にして比べる（合言葉の長さ以外を、応答の速さから推測されないように）。
fn same_code(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}

struct Shared {
    session: Arc<Mutex<Session>>,
    /// 設定されていれば、定義の取得と回答の送信にヘッダで合言葉が要る
    access_code: Option<String>,
    limiter: Mutex<Limiter>,
    tokens: Mutex<Tokens>,
    submitted: Mutex<HashMap<String, u32>>,
    /// 修正用のトークン → (フォーム ID, 回答の行 ID)。メモリだけ（配信を止めると無効になる）
    edits: Mutex<HashMap<String, (String, String)>>,
    stop: AtomicBool,
    /// 回答を受け付けて表が変わったときに呼ぶ（UI の再描画用）
    on_change: Box<dyn Fn() + Send + Sync>,
}

pub struct FormServer {
    server: Arc<tiny_http::Server>,
    shared: Arc<Shared>,
    port: u16,
    host: String,
}

impl FormServer {
    /// `port` で待ち受けを始める（0 なら空いているポート）。全ネットワークから届く。
    ///
    /// `access_code` を渡すと、フォームの定義の取得と回答の送信に合言葉（ヘッダ）が要る。
    /// ファイルには保存せず、配信を始めるたびに決める（URL のトークンと同じ扱い）。
    pub fn start(
        session: Arc<Mutex<Session>>,
        port: u16,
        access_code: Option<String>,
        on_change: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        let access_code = access_code.filter(|c| !c.is_empty());
        if let Some(c) = &access_code {
            // ヘッダで送るので ASCII の印字可能文字だけ（ブラウザの fetch は日本語をヘッダに入れられない）
            if !CODE_LEN.contains(&c.len()) || !c.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(Error::Invalid(format!(
                    "合言葉は半角の英数字と記号（空白なし）の {}〜{} 文字にしてください",
                    CODE_LEN.start(),
                    CODE_LEN.end()
                )));
            }
        }
        let server = tiny_http::Server::http(("0.0.0.0", port)).map_err(|e| {
            Error::Invalid(format!(
                "ポート {port} で待ち受けできません（使用中の可能性があります）: {e}"
            ))
        })?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .unwrap_or(port);
        let server = Arc::new(server);
        let shared = Arc::new(Shared {
            session,
            access_code,
            limiter: Mutex::default(),
            tokens: Mutex::default(),
            submitted: Mutex::default(),
            edits: Mutex::default(),
            stop: AtomicBool::new(false),
            on_change: Box::new(on_change),
        });
        for _ in 0..WORKERS {
            let (server, shared) = (server.clone(), shared.clone());
            std::thread::spawn(move || {
                while !shared.stop.load(Ordering::Relaxed) {
                    match server.recv_timeout(Duration::from_millis(200)) {
                        Ok(Some(req)) => handle(&shared, req),
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            });
        }
        Ok(Self {
            server,
            shared,
            port,
            host: lan_host(),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// 現在のファイルのフォームの URL 一覧。新しいフォームにはトークンを作る。
    pub fn status(&self) -> FormsStatus {
        let forms = self
            .shared
            .session
            .lock()
            .map(|s| s.forms())
            .unwrap_or_default();
        let mut tokens = self.shared.tokens.lock().unwrap();
        let submitted = self.shared.submitted.lock().unwrap();
        let urls = forms
            .into_iter()
            .map(|f| {
                let token = match tokens.by_form.get(&f.id) {
                    Some(t) => t.clone(),
                    None => {
                        let t = random_token();
                        tokens.by_form.insert(f.id.clone(), t.clone());
                        tokens.by_token.insert(t.clone(), f.id.clone());
                        t
                    }
                };
                FormUrl {
                    submitted: submitted.get(&f.id).copied().unwrap_or(0),
                    url: format!("http://{}:{}/f/{token}", self.host, self.port),
                    form_id: f.id,
                    name: f.name,
                }
            })
            .collect();
        FormsStatus {
            running: true,
            port: Some(self.port),
            protected: self.shared.access_code.is_some(),
            urls,
        }
    }
}

impl Drop for FormServer {
    /// 停止を指示するだけで、スレッドの終了は待たない（本文をゆっくり送ってくる相手で止まらないように）。
    /// 待ち受けは各スレッドが抜けた時点で閉じる。
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.server.unblock();
    }
}

/// 同じネットワークから見えるこの端末のアドレス。分からなければ `localhost`。
fn lan_host() -> String {
    // 実際にはパケットを送らない（経路の選択だけで、自分側のアドレスが分かる）
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("192.0.2.1:9")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "localhost".into())
}

fn random_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("乱数を取得できません");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn respond(req: tiny_http::Request, status: u16, content_type: &str, body: String) {
    respond_with(req, status, content_type, body, &[]);
}

fn respond_with(
    req: tiny_http::Request,
    status: u16,
    content_type: &str,
    body: String,
    extra: &[(&str, String)],
) {
    let mut r = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Referrer-Policy", "no-referrer"));
    for (name, value) in extra {
        r = r.with_header(header(name, value));
    }
    if content_type.starts_with("text/html") {
        r = r.with_header(header(
            "Content-Security-Policy",
            "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
             connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ));
    }
    let _ = req.respond(r);
}

fn respond_json(req: tiny_http::Request, status: u16, body: Value) {
    respond(
        req,
        status,
        "application/json; charset=utf-8",
        body.to_string(),
    );
}

/// 回数の制限に達したときの応答（待つべき秒数を `Retry-After` で伝える）。
fn respond_limited(req: tiny_http::Request, wait: Duration, what: &str) {
    let secs = wait.as_secs() + 1;
    respond_with(
        req,
        429,
        "application/json; charset=utf-8",
        json!({"error": format!("{what}。{secs} 秒ほど待ってからやり直してください"), "retryAfter": secs})
            .to_string(),
        &[("Retry-After", secs.to_string())],
    );
}

fn error_status(e: &Error) -> u16 {
    match e {
        Error::NoFile => 503,
        Error::NotFound(_) => 404,
        Error::Invalid(_) => 400,
        _ => 500,
    }
}

/// リクエストの本文（`{"values": {…}}`）から値を読む。エラーは (状態, メッセージ)。
fn read_values(
    req: &mut tiny_http::Request,
) -> std::result::Result<Map<String, Value>, (u16, &'static str)> {
    let mut body = Vec::new();
    if req
        .as_reader()
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
        .is_err()
        || body.len() as u64 > MAX_BODY
    {
        return Err((413, "送信内容が大きすぎます"));
    }
    match serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| match v.get("values") {
            Some(Value::Object(m)) => Some(m.clone()),
            _ => None,
        }) {
        Some(m) => Ok(m),
        None => Err((400, "送信内容が正しくありません")),
    }
}

fn handle(shared: &Shared, mut req: tiny_http::Request) {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let method = req.method().clone();
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let ip = req
        .remote_addr()
        .map(|a| a.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let code = req
        .headers()
        .iter()
        .find(|h| h.field.equiv(CODE_HEADER))
        .map(|h| h.value.as_str().to_string());
    let edit_token = req
        .headers()
        .iter()
        .find(|h| h.field.equiv(EDIT_HEADER))
        .map(|h| h.value.as_str().to_string());

    // フォームの URL への失敗が続いた端末は、しばらく何も受け付けない
    if parts.first() == Some(&"f") {
        let locked = shared.limiter.lock().unwrap().locked(ip, Instant::now());
        if let Some(wait) = locked {
            return respond_limited(req, wait, "失敗が続いたため、一時的に受け付けません");
        }
    }

    // トークンからフォームを引く。知らないトークンは失敗として数える（トークンの総当たりを止める）
    let form_id = |token: &str| -> Option<String> {
        let found = shared.tokens.lock().unwrap().by_token.get(token).cloned();
        if found.is_none() {
            shared.limiter.lock().unwrap().fail(ip, Instant::now());
        }
        found
    };
    // 合言葉の確認。ヘッダが無いのは数えず（ページを開いた直後の問い合わせ）、間違いは失敗として数える
    let denied = || -> Option<Value> {
        let want = shared.access_code.as_deref()?;
        match code.as_deref() {
            Some(got) if same_code(got, want) => None,
            Some(_) => {
                shared.limiter.lock().unwrap().fail(ip, Instant::now());
                Some(json!({"error": "合言葉が違います", "needCode": true}))
            }
            None => Some(json!({"error": "合言葉を入力してください", "needCode": true})),
        }
    };

    // 修正用トークンから、このフォームの回答の行 ID を引く。知らない・別のフォームのものは失敗として数える
    let edit_row = |form: &str| -> Option<String> {
        let found = edit_token.as_deref().and_then(|t| {
            shared
                .edits
                .lock()
                .unwrap()
                .get(t)
                .filter(|(f, _)| f == form)
                .map(|(_, r)| r.clone())
        });
        if found.is_none() && edit_token.is_some() {
            shared.limiter.lock().unwrap().fail(ip, Instant::now());
        }
        found
    };

    match (&method, parts.as_slice()) {
        (tiny_http::Method::Get, [""]) => respond(
            req,
            200,
            "text/plain; charset=utf-8",
            "jxcel フォーム配信中です。共有された URL から開いてください。".into(),
        ),
        (tiny_http::Method::Get, ["f", token]) => match form_id(token) {
            Some(_) => respond(req, 200, "text/html; charset=utf-8", FORM_PAGE.into()),
            None => respond_json(req, 404, json!({"error": "見つかりません"})),
        },
        (tiny_http::Method::Get, ["f", token, "def"]) => {
            let Some(id) = form_id(token) else {
                return respond_json(req, 404, json!({"error": "フォームが見つかりません"}));
            };
            if let Some(body) = denied() {
                return respond_json(req, 401, body);
            }
            let def = shared
                .session
                .lock()
                .map_err(|_| Error::Invalid("内部状態が壊れています".into()))
                .and_then(|s| s.form_definition(&id));
            match def {
                Ok(d) => respond_json(req, 200, serde_json::to_value(d).unwrap()),
                Err(e) => respond_json(req, error_status(&e), json!({"error": e.to_string()})),
            }
        }
        (tiny_http::Method::Post, ["f", token, "submit"]) => {
            let Some(id) = form_id(token) else {
                return respond_json(req, 404, json!({"error": "フォームが見つかりません"}));
            };
            if let Some(body) = denied() {
                return respond_json(req, 401, body);
            }
            let sent = shared.limiter.lock().unwrap().submit(ip, Instant::now());
            if let Err(wait) = sent {
                return respond_limited(req, wait, "送信が多すぎます");
            }
            let values = match read_values(&mut req) {
                Ok(m) => m,
                Err((status, m)) => return respond_json(req, status, json!({"error": m})),
            };
            let result = shared
                .session
                .lock()
                .map_err(|_| Error::Invalid("内部状態が壊れています".into()))
                .and_then(|mut s| s.submit_form(&id, &values));
            match result {
                Ok(row_id) => {
                    *shared
                        .submitted
                        .lock()
                        .unwrap()
                        .entry(id.clone())
                        .or_insert(0) += 1;
                    // 回答者が後から直せるように、この回答だけを指す修正用トークンを渡す
                    let mut edits = shared.edits.lock().unwrap();
                    let edit = (edits.len() < MAX_EDITS).then(|| {
                        let t = random_token();
                        edits.insert(t.clone(), (id, row_id));
                        t
                    });
                    drop(edits);
                    (shared.on_change)();
                    respond_json(req, 200, json!({"ok": true, "editToken": edit}))
                }
                Err(e) => respond_json(req, error_status(&e), json!({"error": e.to_string()})),
            }
        }
        // 送信済みの回答の現在の値（修正画面の初期値）
        (tiny_http::Method::Get, ["f", token, "answer"]) => {
            let Some(id) = form_id(token) else {
                return respond_json(req, 404, json!({"error": "フォームが見つかりません"}));
            };
            if let Some(body) = denied() {
                return respond_json(req, 401, body);
            }
            let Some(row_id) = edit_row(&id) else {
                return respond_json(req, 404, json!({"error": "修正できる回答が見つかりません"}));
            };
            let found = shared
                .session
                .lock()
                .map_err(|_| Error::Invalid("内部状態が壊れています".into()))
                .and_then(|s| s.form_answer(&id, &row_id));
            match found {
                Ok(values) => respond_json(req, 200, json!({"values": values})),
                Err(Error::NotFound(_)) => {
                    respond_json(req, 404, json!({"error": "修正できる回答が見つかりません"}))
                }
                Err(e) => respond_json(req, error_status(&e), json!({"error": e.to_string()})),
            }
        }
        // 送信済みの回答の修正
        (tiny_http::Method::Post, ["f", token, "edit"]) => {
            let Some(id) = form_id(token) else {
                return respond_json(req, 404, json!({"error": "フォームが見つかりません"}));
            };
            if let Some(body) = denied() {
                return respond_json(req, 401, body);
            }
            let Some(row_id) = edit_row(&id) else {
                return respond_json(req, 404, json!({"error": "修正できる回答が見つかりません"}));
            };
            let sent = shared.limiter.lock().unwrap().submit(ip, Instant::now());
            if let Err(wait) = sent {
                return respond_limited(req, wait, "送信が多すぎます");
            }
            let values = match read_values(&mut req) {
                Ok(m) => m,
                Err((status, m)) => return respond_json(req, status, json!({"error": m})),
            };
            let result = shared
                .session
                .lock()
                .map_err(|_| Error::Invalid("内部状態が壊れています".into()))
                .and_then(|mut s| s.edit_form_answer(&id, &row_id, &values));
            match result {
                Ok(()) => {
                    (shared.on_change)();
                    respond_json(req, 200, json!({"ok": true}))
                }
                Err(Error::NotFound(_)) => {
                    respond_json(req, 404, json!({"error": "修正できる回答が見つかりません"}))
                }
                Err(e) => respond_json(req, error_status(&e), json!({"error": e.to_string()})),
            }
        }
        _ => respond_json(req, 404, json!({"error": "見つかりません"})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;

    /// 名前・数量・単価・区分・日付・済・メモ・計算列を持つ表のあるセッション。
    fn session() -> (Session, String, String) {
        let mut s = Session::new();
        let snap = s.new_file("t").unwrap();
        let sheet = snap.file.sheets[0].id.clone();
        let snap = s
            .add_schema(
                &sheet,
                "受付",
                vec![
                    Column::new("name", "氏名", DataType::String).required(),
                    Column::new("qty", "数量", DataType::Int),
                    Column::new("price", "単価", DataType::Decimal),
                    Column::new("rate", "率", DataType::Float),
                    Column::new(
                        "kind",
                        "区分",
                        DataType::Enum {
                            values: vec!["A".into(), "B".into()],
                        },
                    ),
                    Column::new("day", "日付", DataType::Date),
                    Column::new("at", "日時", DataType::DateTime),
                    Column::new("done", "済", DataType::Bool),
                    Column::new("calc", "計算", DataType::Int).computed("export default () => 1"),
                    Column::new(
                        "tags",
                        "タグ",
                        DataType::Array {
                            item: Box::new(DataType::String),
                        },
                    ),
                ],
            )
            .unwrap();
        let schema = snap.file.sheets[0].schemas.last().unwrap().id.clone();
        (s, sheet, schema)
    }

    fn vals(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn rows(s: &Session, schema: &str) -> Vec<BTreeMap<String, Value>> {
        let snap = s.current().unwrap();
        snap.file.sheets[0]
            .schemas
            .iter()
            .find(|c| c.id == schema)
            .unwrap()
            .rows
            .iter()
            .map(|r| r.cells.clone())
            .collect()
    }

    #[test]
    fn add_form_picks_only_columns_a_browser_can_fill() {
        let (mut s, sheet, schema) = session();
        let snap = s.add_form(&sheet, &schema, "受付").unwrap();
        let form = &snap.file.forms[0];
        // 計算列とネスト型（配列）は入力欄にならない
        assert_eq!(
            form.columns,
            ["name", "qty", "price", "rate", "kind", "day", "at", "done"]
        );
        let def = s.form_definition(&form.id).unwrap();
        assert_eq!(def.title, "受付");
        assert_eq!(def.fields.len(), 8);
        assert!(def.fields[0].required);
        assert_eq!(def.fields[4].kind, "enum");
        assert_eq!(def.fields[4].options, ["A", "B"]);
        assert_eq!(def.fields[6].kind, "dateTime");
    }

    #[test]
    fn update_form_is_validated() {
        let (mut s, sheet, schema) = session();
        let id = s.add_form(&sheet, &schema, "受付").unwrap().file.forms[0]
            .id
            .clone();
        let upd = |s: &mut Session, cols: &[&str], name: &str| {
            s.update_form(
                &id,
                name,
                &sheet,
                &schema,
                cols.iter().map(|c| c.to_string()).collect(),
                "説明",
            )
        };
        assert!(upd(&mut s, &["calc"], "x").is_err()); // 計算列
        assert!(upd(&mut s, &["tags"], "x").is_err()); // ネスト型
        assert!(upd(&mut s, &["nope"], "x").is_err()); // 無い列
        assert!(upd(&mut s, &[], "x").is_err()); // 空
        assert!(upd(&mut s, &["name", "name"], "x").is_err()); // 重複
        assert!(upd(&mut s, &["name"], "  ").is_err()); // 名前が空
        let snap = upd(&mut s, &["qty", "name"], "新しい名前").unwrap();
        assert_eq!(snap.file.forms[0].columns, ["qty", "name"]);
        assert_eq!(snap.file.forms[0].description, "説明");
        // 列を消すと入力欄からも外れ、シートを消すとフォームも消える
        let snap = s.delete_column(&sheet, &schema, "qty").unwrap();
        assert_eq!(snap.file.forms[0].columns, ["name"]);
        let snap = s.delete_sheet(&sheet).unwrap();
        assert!(snap.file.forms.is_empty());
    }

    #[test]
    fn submit_converts_browser_values_and_appends_a_row() {
        let (mut s, sheet, schema) = session();
        let id = s.add_form(&sheet, &schema, "受付").unwrap().file.forms[0]
            .id
            .clone();
        s.submit_form(
            &id,
            &vals(json!({
                "name": "山田",
                "qty": "3",
                "price": "12.50",
                "rate": "0.25",
                "kind": "B",
                "day": "2026-10-02",
                "at": "2026-10-02T01:00:00.000Z",
                "done": true,
            })),
        )
        .unwrap();
        // 空欄は値を持たない
        s.submit_form(
            &id,
            &vals(json!({"name": "佐藤", "qty": "", "done": false})),
        )
        .unwrap();
        let r = rows(&s, &schema);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["qty"], json!(3));
        assert_eq!(r[0]["price"], json!("12.50"));
        assert_eq!(r[0]["rate"], json!(0.25));
        assert_eq!(r[0]["done"], json!(true));
        assert_eq!(r[1]["name"], json!("佐藤"));
        assert!(!r[1].contains_key("qty"));
        assert_eq!(r[1]["done"], json!(false));
        assert!(s.current().unwrap().dirty);
    }

    #[test]
    fn submit_rejects_bad_values_without_adding_anything() {
        let (mut s, sheet, schema) = session();
        let id = s.add_form(&sheet, &schema, "受付").unwrap().file.forms[0]
            .id
            .clone();
        let bad = |s: &mut Session, v: Value| s.submit_form(&id, &vals(v)).unwrap_err().to_string();
        assert!(bad(&mut s, json!({})).contains("氏名 は必須です"));
        assert!(bad(&mut s, json!({"name": "  "})).contains("必須"));
        assert!(bad(&mut s, json!({"name": "a", "qty": "1.5"})).contains("数量"));
        assert!(bad(&mut s, json!({"name": "a", "qty": "abc"})).contains("整数"));
        assert!(bad(&mut s, json!({"name": "a", "price": "1e3"})).contains("単価: 数値で入力"));
        assert!(bad(&mut s, json!({"name": "a", "kind": "C"})).contains("区分"));
        assert!(bad(&mut s, json!({"name": "a", "day": "2026-02-30"})).contains("日付"));
        assert!(bad(&mut s, json!({"name": "a", "at": "2026-10-02T01:00"})).contains("日時"));
        assert!(bad(&mut s, json!({"name": "a", "done": "yes"})).contains("済"));
        // フォームにない列・計算列には書けない
        assert!(bad(&mut s, json!({"name": "a", "calc": 1})).contains("フォームにない"));
        assert!(bad(&mut s, json!({"name": "a", "tags": ["x"]})).contains("フォームにない"));
        // 複数のエラーは欄ごとにまとめて返す
        let m = bad(&mut s, json!({"qty": "x", "kind": "Z"}));
        assert!(
            m.contains("氏名") && m.contains("数量") && m.contains("区分"),
            "{m}"
        );
        assert!(rows(&s, &schema).is_empty());
    }

    #[test]
    fn submit_without_a_file_or_form_fails() {
        let mut s = Session::new();
        assert!(matches!(
            s.submit_form("x", &Map::new()),
            Err(Error::NoFile)
        ));
        let (mut s, ..) = session();
        assert!(matches!(
            s.submit_form("x", &Map::new()),
            Err(Error::NotFound(_))
        ));
    }

    // ---- HTTP ----

    fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String, String) {
        http_with(port, method, path, body, &[])
    }

    fn http_with(
        port: u16,
        method: &str,
        path: &str,
        body: Option<&str>,
        headers: &[(&str, &str)],
    ) -> (u16, String, String) {
        let extra: String = headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let body = body.unwrap_or("");
        write!(
            c,
            "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut raw = Vec::new();
        c.read_to_end(&mut raw).unwrap();
        let raw = String::from_utf8_lossy(&raw).to_string();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, head.to_string(), body.to_string())
    }

    #[test]
    fn serves_a_form_and_accepts_submissions_over_http() {
        let (mut s, sheet, schema) = session();
        s.add_form(&sheet, &schema, "受付").unwrap();
        let shared = Arc::new(Mutex::new(s));
        let changed = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = changed.clone();
        let server = FormServer::start(shared.clone(), 0, None, move || {
            c2.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        let port = server.port();
        let st = server.status();
        assert!(st.running && st.urls.len() == 1);
        let url = &st.urls[0].url;
        let token = url.rsplit('/').next().unwrap();
        assert_eq!(token.len(), 32);
        // 状態を取り直しても同じトークン
        assert_eq!(&server.status().urls[0].url, url);

        // トークンを知らなければ何も見えない
        assert_eq!(http(port, "GET", "/f/deadbeef", None).0, 404);
        assert_eq!(http(port, "GET", "/f/deadbeef/def", None).0, 404);
        assert_eq!(http(port, "POST", "/f/deadbeef/submit", Some("{}")).0, 404);
        assert_eq!(http(port, "GET", "/", None).0, 200);

        let (code, head, body) = http(port, "GET", &format!("/f/{token}"), None);
        assert_eq!(code, 200);
        assert!(body.contains("<form id=\"form\""));
        assert!(head.to_lowercase().contains("content-security-policy"));

        let (code, _, body) = http(port, "GET", &format!("/f/{token}/def"), None);
        assert_eq!(code, 200);
        let def: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(def["title"], "受付");
        assert_eq!(def["fields"][0]["label"], "氏名");

        let submit = format!("/f/{token}/submit");
        let (code, _, body) = http(
            port,
            "POST",
            &submit,
            Some(r#"{"values":{"name":"山田","qty":"2"}}"#),
        );
        assert_eq!(code, 200, "{body}");
        assert_eq!(changed.load(Ordering::SeqCst), 1);
        assert_eq!(rows(&shared.lock().unwrap(), &schema)[0]["qty"], json!(2));
        assert_eq!(server.status().urls[0].submitted, 1);

        // 不正な回答は 400 でメッセージが返り、行は増えない
        let (code, _, body) = http(port, "POST", &submit, Some(r#"{"values":{"qty":"x"}}"#));
        assert_eq!(code, 400);
        assert!(body.contains("氏名") && body.contains("数量"), "{body}");
        assert_eq!(http(port, "POST", &submit, Some("not json")).0, 400);
        assert_eq!(http(port, "POST", &submit, Some(r#"{"values":[]}"#)).0, 400);
        assert_eq!(rows(&shared.lock().unwrap(), &schema).len(), 1);
        assert_eq!(changed.load(Ordering::SeqCst), 1);

        // 大きすぎる本文は拒否
        let big = format!(r#"{{"values":{{"name":"{}"}}}}"#, "a".repeat(70_000));
        assert_eq!(http(port, "POST", &submit, Some(&big)).0, 413);

        // 実行中にフォームを消すと、そのフォームは見えなくなる
        let id = shared.lock().unwrap().forms()[0].id.clone();
        shared.lock().unwrap().delete_form(&id).unwrap();
        assert_eq!(http(port, "GET", &format!("/f/{token}/def"), None).0, 404);
        assert!(server.status().urls.is_empty());

        // 停止するとポートを閉じる
        drop(server);
        let t = std::time::Instant::now();
        while TcpStream::connect(("127.0.0.1", port)).is_ok() {
            assert!(t.elapsed() < Duration::from_secs(5), "ポートが閉じません");
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("closed after {:?}", t.elapsed());
    }

    #[test]
    fn form_is_unavailable_while_no_file_is_open_and_port_conflicts_are_reported() {
        let shared = Arc::new(Mutex::new(Session::new()));
        let a = FormServer::start(shared.clone(), 0, None, || {}).unwrap();
        assert!(FormServer::start(shared, a.port(), None, || {}).is_err());
        assert!(a.status().urls.is_empty());
    }

    #[test]
    fn limiter_locks_after_repeated_failures_and_recovers() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let other: IpAddr = "10.0.0.2".parse().unwrap();
        let t0 = Instant::now();
        let mut l = Limiter::default();
        for i in 0..FAIL_LIMIT - 1 {
            l.fail(ip, t0 + Duration::from_secs(i as u64));
        }
        assert!(l.locked(ip, t0 + Duration::from_secs(10)).is_none());
        l.fail(ip, t0 + Duration::from_secs(10));
        let now = t0 + Duration::from_secs(11);
        let wait = l.locked(ip, now).expect("ロックされる");
        // 解けるのは、上限に達する直前の失敗（最古ではなく FAIL_LIMIT 個前）が窓から出たとき
        assert!(
            wait > Duration::from_secs(280) && wait <= FAIL_WINDOW,
            "{wait:?}"
        );
        // ほかの端末には影響しない
        assert!(l.locked(other, now).is_none());
        // 窓が過ぎれば解ける
        assert!(l
            .locked(ip, t0 + FAIL_WINDOW + Duration::from_secs(11))
            .is_none());
    }

    #[test]
    fn limiter_caps_submissions_per_window() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let t0 = Instant::now();
        let mut l = Limiter::default();
        for _ in 0..SUBMIT_LIMIT {
            l.submit(ip, t0).unwrap();
        }
        let wait = l.submit(ip, t0 + Duration::from_secs(10)).unwrap_err();
        assert_eq!(wait, Duration::from_secs(50));
        assert!(l.submit("10.0.0.2".parse().unwrap(), t0).is_ok());
        assert!(l.submit(ip, t0 + SUBMIT_WINDOW).is_ok());
    }

    #[test]
    fn limiter_forgets_old_hosts_when_the_table_is_full() {
        let t0 = Instant::now();
        let mut l = Limiter::default();
        for i in 0..MAX_TRACKED as u32 + 1 {
            l.fail(IpAddr::V4(Ipv4Addr::from(i)), t0);
        }
        let later = t0 + FAIL_WINDOW + Duration::from_secs(1);
        l.fail("9.9.9.9".parse().unwrap(), later);
        assert_eq!(l.fails.len(), 1);
    }

    #[test]
    fn same_code_compares_whole_strings() {
        assert!(same_code("abcd", "abcd"));
        assert!(!same_code("abcd", "abce"));
        assert!(!same_code("abcd", "abc"));
        assert!(!same_code("abc", "abcd"));
        assert!(same_code("", ""));
    }

    #[test]
    fn access_code_is_required_for_definition_and_submission() {
        let (mut s, sheet, schema) = session();
        s.add_form(&sheet, &schema, "受付").unwrap();
        let shared = Arc::new(Mutex::new(s));
        // 短すぎる・長すぎる合言葉は開始できない
        assert!(FormServer::start(shared.clone(), 0, Some("abc".into()), || {}).is_err());
        assert!(FormServer::start(shared.clone(), 0, Some("a".repeat(65)), || {}).is_err());
        assert!(
            FormServer::start(shared.clone(), 0, Some("ひみつの合言葉".into()), || {}).is_err()
        );
        assert!(FormServer::start(shared.clone(), 0, Some("ab cd".into()), || {}).is_err());
        // 空は「なし」と同じ
        let open = FormServer::start(shared.clone(), 0, Some(String::new()), || {}).unwrap();
        assert!(!open.status().protected);
        drop(open);

        let server =
            FormServer::start(shared.clone(), 0, Some("himitsu-1234".into()), || {}).unwrap();
        let port = server.port();
        let st = server.status();
        assert!(st.protected);
        let token = st.urls[0].url.rsplit('/').next().unwrap().to_string();
        let (def, submit) = (format!("/f/{token}/def"), format!("/f/{token}/submit"));
        let body = r#"{"values":{"name":"山田","qty":"2"}}"#;

        // 画面（HTML）は合言葉なしで開ける。中身は何も入っていない
        let (code, _, page) = http(port, "GET", &format!("/f/{token}"), None);
        assert_eq!(code, 200);
        assert!(!page.contains("受付"));

        // ヘッダが無い: 401 で、行は増えない。失敗には数えない
        for _ in 0..FAIL_LIMIT + 2 {
            let (code, _, b) = http(port, "GET", &def, None);
            assert_eq!(code, 401);
            assert!(b.contains("needCode"), "{b}");
        }
        assert_eq!(http(port, "POST", &submit, Some(body)).0, 401);
        assert!(rows(&shared.lock().unwrap(), &schema).is_empty());

        // 正しい合言葉
        let ok = [(CODE_HEADER, "himitsu-1234")];
        let (code, _, b) = http_with(port, "GET", &def, None, &ok);
        assert_eq!(code, 200, "{b}");
        assert_eq!(http_with(port, "POST", &submit, Some(body), &ok).0, 200);
        assert_eq!(rows(&shared.lock().unwrap(), &schema).len(), 1);

        // 間違いが続くと、正しい合言葉でもロックされる（429 と Retry-After）
        let bad = [(CODE_HEADER, "wrong")];
        for _ in 0..FAIL_LIMIT {
            assert_eq!(http_with(port, "GET", &def, None, &bad).0, 401);
        }
        let (code, head, b) = http_with(port, "GET", &def, None, &ok);
        assert_eq!(code, 429, "{b}");
        assert!(head.to_lowercase().contains("retry-after"), "{head}");
        assert_eq!(http_with(port, "POST", &submit, Some(body), &ok).0, 429);
        assert_eq!(rows(&shared.lock().unwrap(), &schema).len(), 1);
    }

    #[test]
    fn edit_form_answer_rewrites_only_the_form_columns_of_that_row() {
        let (mut s, sheet, schema) = session();
        // フォームは名前・数量だけ。単価はフォームにない列
        let form = s.add_form(&sheet, &schema, "受付").unwrap().file.forms[0].clone();
        s.update_form(
            &form.id,
            "受付",
            &sheet,
            &schema,
            vec!["name".into(), "qty".into()],
            "",
        )
        .unwrap();
        let a = s
            .submit_form(&form.id, &vals(json!({"name": "山田", "qty": "3"})))
            .unwrap();
        let b = s
            .submit_form(&form.id, &vals(json!({"name": "佐藤", "qty": "5"})))
            .unwrap();
        // 管理者がフォームにない列を直接入れた
        let col = s.current().unwrap().file.sheets[0]
            .schemas
            .last()
            .unwrap()
            .columns[2]
            .id
            .clone();
        assert_eq!(col, "price");
        {
            let d = s.doc().unwrap();
            let sch = crate::find_schema(&mut d.file, &sheet, &schema).unwrap();
            sch.rows
                .iter_mut()
                .find(|r| r.id == a)
                .unwrap()
                .cells
                .insert("price".into(), json!("9.99"));
        }
        assert_eq!(
            s.form_answer(&form.id, &a).unwrap(),
            vals(json!({"name": "山田", "qty": 3}))
        );

        // 修正: 数量を変え、名前は同じ。他の行・フォームにない列は変わらない
        s.edit_form_answer(&form.id, &a, &vals(json!({"name": "山田", "qty": "4"})))
            .unwrap();
        let r = rows(&s, &schema);
        assert_eq!(r[0]["qty"], json!(4));
        assert_eq!(r[0]["price"], json!("9.99"));
        assert_eq!(r[1]["qty"], json!(5));
        // 空の欄はその列の値を消す
        s.edit_form_answer(&form.id, &a, &vals(json!({"name": "山田", "qty": ""})))
            .unwrap();
        assert!(!rows(&s, &schema)[0].contains_key("qty"));

        // 不正な値・必須の欠落・フォームにない列は拒否して、何も変えない
        let before = rows(&s, &schema);
        for bad in [
            json!({"name": "山田", "qty": "x"}),
            json!({"name": ""}),
            json!({"name": "山田", "price": "1"}),
        ] {
            assert!(s.edit_form_answer(&form.id, &a, &vals(bad)).is_err());
        }
        assert_eq!(rows(&s, &schema), before);

        // 行が消えた回答・知らないフォームは NotFound
        {
            let d = s.doc().unwrap();
            let sch = crate::find_schema(&mut d.file, &sheet, &schema).unwrap();
            sch.rows.retain(|r| r.id != b);
        }
        assert!(matches!(
            s.edit_form_answer(&form.id, &b, &vals(json!({"name": "x"}))),
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            s.form_answer(&form.id, &b),
            Err(Error::NotFound(_))
        ));
        assert!(matches!(s.form_answer("nope", &a), Err(Error::NotFound(_))));
    }

    #[test]
    fn answers_can_be_edited_over_http_with_the_edit_token() {
        let (mut s, sheet, schema) = session();
        s.add_form(&sheet, &schema, "受付").unwrap();
        let shared = Arc::new(Mutex::new(s));
        let changes = Arc::new(AtomicBool::new(false));
        let c2 = changes.clone();
        let server = FormServer::start(shared.clone(), 0, None, move || {
            c2.store(true, Ordering::Relaxed)
        })
        .unwrap();
        let port = server.port();
        let token = server.status().urls[0]
            .url
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let (submit, edit, answer) = (
            format!("/f/{token}/submit"),
            format!("/f/{token}/edit"),
            format!("/f/{token}/answer"),
        );
        let (code, _, b) = http(
            port,
            "POST",
            &submit,
            Some(r#"{"values":{"name":"山田","qty":"2"}}"#),
        );
        assert_eq!(code, 200, "{b}");
        let et = serde_json::from_str::<Value>(&b).unwrap()["editToken"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(et.len(), 32);
        let h = [(EDIT_HEADER, et.as_str())];

        // 現在の回答を読める
        let (code, _, b) = http_with(port, "GET", &answer, None, &h);
        assert_eq!(code, 200, "{b}");
        let v: Value = serde_json::from_str(&b).unwrap();
        assert_eq!(v["values"]["name"], json!("山田"));
        assert_eq!(v["values"]["qty"], json!(2));

        // 修正できる。行は増えず、表の値が変わり、変更が通知される
        changes.store(false, Ordering::Relaxed);
        let (code, _, b) = http_with(
            port,
            "POST",
            &edit,
            Some(r#"{"values":{"name":"山田太郎","qty":"7"}}"#),
            &h,
        );
        assert_eq!(code, 200, "{b}");
        let r = rows(&shared.lock().unwrap(), &schema);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["name"], json!("山田太郎"));
        assert_eq!(r[0]["qty"], json!(7));
        assert!(changes.load(Ordering::Relaxed));

        // 検証は送信と同じ。不正なら 400 で変わらない
        let (code, _, _) = http_with(
            port,
            "POST",
            &edit,
            Some(r#"{"values":{"name":"山田","qty":"x"}}"#),
            &h,
        );
        assert_eq!(code, 400);
        assert_eq!(
            rows(&shared.lock().unwrap(), &schema)[0]["name"],
            json!("山田太郎")
        );

        // 別の回答者（トークンなし・違うトークン）は修正も閲覧もできない
        let body = r#"{"values":{"name":"乗っ取り"}}"#;
        assert_eq!(http(port, "POST", &edit, Some(body)).0, 404);
        assert_eq!(http(port, "GET", &answer, None).0, 404);
        let bad = [(EDIT_HEADER, "0".repeat(32))];
        let bad = [(bad[0].0, bad[0].1.as_str())];
        assert_eq!(http_with(port, "POST", &edit, Some(body), &bad).0, 404);
        assert_eq!(
            rows(&shared.lock().unwrap(), &schema)[0]["name"],
            json!("山田太郎")
        );

        // 回答が表から消えたら、修正はできない
        {
            let mut guard = shared.lock().unwrap();
            let d = guard.doc().unwrap();
            let (sheet, schema) = (
                d.file.sheets[0].id.clone(),
                d.file.sheets[0].schemas.last().unwrap().id.clone(),
            );
            crate::find_schema(&mut d.file, &sheet, &schema)
                .unwrap()
                .rows
                .clear();
        }
        let (code, _, _) = http_with(port, "POST", &edit, Some(body), &h);
        assert_eq!(code, 404);
    }

    #[test]
    fn edit_tokens_are_per_form_and_need_the_access_code() {
        let (mut s, sheet, schema) = session();
        s.add_form(&sheet, &schema, "受付").unwrap();
        s.add_form(&sheet, &schema, "受付2").unwrap();
        let shared = Arc::new(Mutex::new(s));
        let server =
            FormServer::start(shared.clone(), 0, Some("himitsu-1234".into()), || {}).unwrap();
        let port = server.port();
        let tokens: Vec<String> = server
            .status()
            .urls
            .iter()
            .map(|u| u.url.rsplit('/').next().unwrap().to_string())
            .collect();
        let ok = [(CODE_HEADER, "himitsu-1234")];
        let (_, _, b) = http_with(
            port,
            "POST",
            &format!("/f/{}/submit", tokens[0]),
            Some(r#"{"values":{"name":"山田"}}"#),
            &ok,
        );
        let et = serde_json::from_str::<Value>(&b).unwrap()["editToken"]
            .as_str()
            .unwrap()
            .to_string();
        let both = [(CODE_HEADER, "himitsu-1234"), (EDIT_HEADER, et.as_str())];
        let body = r#"{"values":{"name":"変更"}}"#;
        // 合言葉がなければ、修正用トークンがあっても受け付けない
        let only_edit = [(EDIT_HEADER, et.as_str())];
        assert_eq!(
            http_with(
                port,
                "POST",
                &format!("/f/{}/edit", tokens[0]),
                Some(body),
                &only_edit
            )
            .0,
            401
        );
        // 別のフォームには使えない
        assert_eq!(
            http_with(
                port,
                "POST",
                &format!("/f/{}/edit", tokens[1]),
                Some(body),
                &both
            )
            .0,
            404
        );
        // 元のフォームでは通る
        assert_eq!(
            http_with(
                port,
                "POST",
                &format!("/f/{}/edit", tokens[0]),
                Some(body),
                &both
            )
            .0,
            200
        );
        assert_eq!(
            rows(&shared.lock().unwrap(), &schema)[0]["name"],
            json!("変更")
        );
    }

    #[test]
    fn guessing_tokens_locks_the_host_out() {
        let (mut s, sheet, schema) = session();
        s.add_form(&sheet, &schema, "受付").unwrap();
        let server = FormServer::start(Arc::new(Mutex::new(s)), 0, None, || {}).unwrap();
        let port = server.port();
        let token = server.status().urls[0]
            .url
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        for i in 0..FAIL_LIMIT {
            assert_eq!(http(port, "GET", &format!("/f/{i:032}/def"), None).0, 404);
        }
        // 正しい URL もしばらく開けない（429）。トップは影響しない
        assert_eq!(http(port, "GET", &format!("/f/{token}"), None).0, 429);
        assert_eq!(http(port, "GET", "/", None).0, 200);
    }

    #[test]
    fn submissions_are_rate_limited_per_host() {
        let (mut s, sheet, schema) = session();
        s.add_form(&sheet, &schema, "受付").unwrap();
        let shared = Arc::new(Mutex::new(s));
        let server = FormServer::start(shared.clone(), 0, None, || {}).unwrap();
        let port = server.port();
        let token = server.status().urls[0]
            .url
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let submit = format!("/f/{token}/submit");
        let body = r#"{"values":{"name":"山田","qty":"2"}}"#;
        for _ in 0..SUBMIT_LIMIT {
            assert_eq!(http(port, "POST", &submit, Some(body)).0, 200);
        }
        let (code, head, b) = http(port, "POST", &submit, Some(body));
        assert_eq!(code, 429, "{b}");
        assert!(head.to_lowercase().contains("retry-after"));
        assert_eq!(rows(&shared.lock().unwrap(), &schema).len(), SUBMIT_LIMIT);
        // 定義の取得は止めない（回答の制限だけ）
        assert_eq!(http(port, "GET", &format!("/f/{token}/def"), None).0, 200);
    }
}
