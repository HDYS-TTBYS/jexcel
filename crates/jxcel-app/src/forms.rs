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
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FORM_PAGE: &str = include_str!("form.html");
const MAX_BODY: u64 = 64 * 1024;
const WORKERS: usize = 4;

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

    /// フォームの回答を、対象の表の末尾に 1 行として追加する。
    /// 型に合わない値・必須の欠落・フォームにない列は拒否し、そのときは何も追加しない。
    pub fn submit_form(&mut self, id: &str, values: &Map<String, Value>) -> Result<()> {
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
        schema.rows.push(Row::new(cells));
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
    pub urls: Vec<FormUrl>,
}

impl FormsStatus {
    pub fn stopped() -> Self {
        Self {
            running: false,
            port: None,
            urls: vec![],
        }
    }
}

#[derive(Default)]
struct Tokens {
    by_form: HashMap<String, String>,
    by_token: HashMap<String, String>,
}

struct Shared {
    session: Arc<Mutex<Session>>,
    tokens: Mutex<Tokens>,
    submitted: Mutex<HashMap<String, u32>>,
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
    pub fn start(
        session: Arc<Mutex<Session>>,
        port: u16,
        on_change: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
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
            tokens: Mutex::default(),
            submitted: Mutex::default(),
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
    let mut r = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Referrer-Policy", "no-referrer"));
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

fn error_status(e: &Error) -> u16 {
    match e {
        Error::NoFile => 503,
        Error::NotFound(_) => 404,
        Error::Invalid(_) => 400,
        _ => 500,
    }
}

fn handle(shared: &Shared, mut req: tiny_http::Request) {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let method = req.method().clone();
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();

    let form_id = |token: &str| -> Option<String> {
        shared.tokens.lock().unwrap().by_token.get(token).cloned()
    };

    match (&method, parts.as_slice()) {
        (tiny_http::Method::Get, [""]) => respond(
            req,
            200,
            "text/plain; charset=utf-8",
            "jxcel フォーム配信中です。共有された URL から開いてください。".into(),
        ),
        (tiny_http::Method::Get, ["f", token]) if form_id(token).is_some() => {
            respond(req, 200, "text/html; charset=utf-8", FORM_PAGE.into())
        }
        (tiny_http::Method::Get, ["f", token, "def"]) => {
            let Some(id) = form_id(token) else {
                return respond_json(req, 404, json!({"error": "フォームが見つかりません"}));
            };
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
            let mut body = Vec::new();
            if req
                .as_reader()
                .take(MAX_BODY + 1)
                .read_to_end(&mut body)
                .is_err()
                || body.len() as u64 > MAX_BODY
            {
                return respond_json(req, 413, json!({"error": "送信内容が大きすぎます"}));
            }
            let values = match serde_json::from_slice::<Value>(&body).ok().and_then(|v| {
                match v.get("values") {
                    Some(Value::Object(m)) => Some(m.clone()),
                    _ => None,
                }
            }) {
                Some(m) => m,
                None => {
                    return respond_json(req, 400, json!({"error": "送信内容が正しくありません"}))
                }
            };
            let result = shared
                .session
                .lock()
                .map_err(|_| Error::Invalid("内部状態が壊れています".into()))
                .and_then(|mut s| s.submit_form(&id, &values));
            match result {
                Ok(()) => {
                    *shared.submitted.lock().unwrap().entry(id).or_insert(0) += 1;
                    (shared.on_change)();
                    respond_json(req, 200, json!({"ok": true}))
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
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let body = body.unwrap_or("");
        write!(
            c,
            "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
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
        let server = FormServer::start(shared.clone(), 0, move || {
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
        let a = FormServer::start(shared.clone(), 0, || {}).unwrap();
        assert!(FormServer::start(shared, a.port(), || {}).is_err());
        assert!(a.status().urls.is_empty());
    }
}
