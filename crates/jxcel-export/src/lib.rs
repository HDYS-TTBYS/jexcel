//! xlsx / docx テンプレートへの差し込み（行ごとにファイルを生成する）。
//!
//! テンプレートの `{{ 式 }}` を、表の 1 行ごとに評価した値で置き換える。式は列名をそのまま変数として使える
//! TS/JS の式（`{{数量 * 単価}}`）で、`std` も使える。評価は `jxcel-macro` の QuickJS で行う。

pub mod docx;
pub mod package;
pub mod placeholder;
pub mod run;
pub mod xlsx;
pub mod xml;

pub use run::{
    export, preview, sanitize_filename, GeneratedFile, Preview, PreviewLoop, PreviewRow, Report,
    RowError, Spec,
};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("XML の処理に失敗しました: {0}")]
    Xml(String),
    #[error("zip の処理に失敗しました: {0}")]
    Zip(String),
    #[error("テンプレートとして使えないファイルです: {0}")]
    NotATemplate(String),
    #[error("差し込み欄「{expr}」: {message}")]
    Expr { expr: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// テンプレートの種類（xlsx / docx）。
pub use jxcel_core::TemplateKind as Kind;

/// この種類のファイルに必ずある部品（テンプレートの取り違えを早めに見つける）。
fn required_part(kind: Kind) -> &'static str {
    match kind {
        Kind::Xlsx => "xl/workbook.xml",
        Kind::Docx => "word/document.xml",
    }
}

/// 差し込み欄の式を値に解決する関数。失敗はメッセージで返す。
pub type Resolver<'a> = dyn FnMut(&str) -> std::result::Result<Value, String> + 'a;

/// 行ループの印。表の行の中に `{{#each 式}}` と書くと、その行が式の配列の要素の数だけ繰り返される。
/// `{{/each}}` が別の行にあれば、その間の行（印の行を含む）がひとまとめに繰り返される。
pub const LOOP_MARKER: &str = "#each";
/// ループを閉じる印。
pub const LOOP_END: &str = "/each";

/// 差し込む値の取得元。ループは、テンプレートの中での出現順（外側が先）に 0, 1, 2… の番号を持つ。
/// ループは入れ子にできる: `path` は、外側から順に「何番目の要素の中か」を並べたもの。
pub trait Source {
    /// ループの外の欄 `{{ 式 }}`。
    fn value(&mut self, expr: &str) -> std::result::Result<Value, String>;
    /// `index` 番目のループの要素数。`source` は `{{#each 式}}` の式、`parent` は外側のループの番号、
    /// `path` は外側のループの要素の位置（外側がなければ空）。対象が配列でなければ Err。
    fn loop_len(
        &mut self,
        index: usize,
        parent: Option<usize>,
        path: &[usize],
        source: &str,
    ) -> std::result::Result<usize, String>;
    /// `index` 番目のループの、`path`（最後が自分の要素の位置）の要素での、ループの中の欄の値。
    fn item_value(
        &mut self,
        index: usize,
        path: &[usize],
        expr: &str,
    ) -> std::result::Result<Value, String>;
}

/// ループを使わないテンプレート用。式を解決する関数だけを渡す（ループがあるとエラー）。
struct Plain<'a, 'b>(&'a mut Resolver<'b>);

impl Source for Plain<'_, '_> {
    fn value(&mut self, expr: &str) -> std::result::Result<Value, String> {
        (self.0)(expr)
    }
    fn loop_len(
        &mut self,
        _: usize,
        _: Option<usize>,
        _: &[usize],
        _: &str,
    ) -> std::result::Result<usize, String> {
        Err("行ループ {{#each}} は、この呼び出しでは使えません".into())
    }
    fn item_value(&mut self, _: usize, _: &[usize], _: &str) -> std::result::Result<Value, String> {
        Err("行ループ {{#each}} は、この呼び出しでは使えません".into())
    }
}

/// `{{#each 式}}` の式の部分（印でなければ `None`）。
pub fn loop_source(expr: &str) -> Option<&str> {
    let rest = expr.strip_prefix(LOOP_MARKER)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
}

/// ループの印。
#[derive(Debug, Clone, PartialEq)]
pub enum Marker {
    /// `{{#each 式}}`
    Open(String),
    /// `{{/each}}`
    Close,
}

/// 欄の式がループの印なら、その種類。
pub fn marker(expr: &str) -> Option<Marker> {
    if expr.trim() == LOOP_END {
        return Some(Marker::Close);
    }
    loop_source(expr).map(|s| Marker::Open(s.to_string()))
}

pub(crate) fn marker_error(message: &str) -> Error {
    Error::Expr {
        expr: "#each".into(),
        message: message.into(),
    }
}

/// 繰り返しの範囲（表の行の添字。両端を含む）。入れ子の内側のものは `children`。
#[derive(Debug, Clone)]
pub(crate) struct Block {
    pub source: String,
    pub first: usize,
    pub last: usize,
    /// ループの番号（`number` で振る）
    pub index: usize,
    pub children: Vec<Block>,
}

/// 表の行ごとの印（出現順）から、繰り返しの範囲を作る。
///
/// `{{/each}}` が 1 つもなければ、印のある行 1 行ずつが 1 行ループ（従来の書き方。入れ子はない）。
/// `{{/each}}` があれば、すべての `{{#each}}` を `{{/each}}` と組にして（入れ子にできる）、間の行をひとまとめにする。
/// 1 行だけのループを入れ子にするときは、同じ行で `{{/each}}` も書く。
pub(crate) fn parse_blocks(marks: &[Vec<Marker>]) -> Result<Vec<Block>> {
    let closes = marks
        .iter()
        .flatten()
        .filter(|m| **m == Marker::Close)
        .count();
    let mut top: Vec<Block> = vec![];
    if closes == 0 {
        for (i, row) in marks.iter().enumerate() {
            match row.len() {
                0 => {}
                1 => {
                    let Marker::Open(source) = &row[0] else {
                        unreachable!("閉じる印はない")
                    };
                    top.push(Block {
                        source: source.clone(),
                        first: i,
                        last: i,
                        index: 0,
                        children: vec![],
                    });
                }
                _ => {
                    return Err(marker_error(
                        "1 つの行に {{#each}} は 1 つだけ書けます（入れ子にするときは {{/each}} で閉じます）",
                    ))
                }
            }
        }
        return Ok(top);
    }
    let mut stack: Vec<(String, usize, Vec<Block>)> = vec![];
    for (i, row) in marks.iter().enumerate() {
        for m in row {
            match m {
                Marker::Open(source) => stack.push((source.clone(), i, vec![])),
                Marker::Close => {
                    let Some((source, first, children)) = stack.pop() else {
                        return Err(marker_error("{{/each}} に対応する {{#each}} がありません"));
                    };
                    let block = Block {
                        source,
                        first,
                        last: i,
                        index: 0,
                        children,
                    };
                    match stack.last_mut() {
                        Some(parent) => parent.2.push(block),
                        None => top.push(block),
                    }
                }
            }
        }
    }
    if !stack.is_empty() {
        return Err(marker_error(
            "{{#each}} を {{/each}} で閉じてください（{{/each}} を使うときは、すべてのループを閉じます）",
        ));
    }
    check_siblings(&top)?;
    Ok(top)
}

fn check_siblings(blocks: &[Block]) -> Result<()> {
    for w in blocks.windows(2) {
        if w[1].first <= w[0].last {
            return Err(marker_error(
                "同じ行で、1 つのループが終わって次のループが始まっています。行を分けてください",
            ));
        }
    }
    for b in blocks {
        check_siblings(&b.children)?;
    }
    Ok(())
}

/// 外側が先の順（出現順）に、ループの番号を振る。
pub(crate) fn number(blocks: &mut [Block], next: &mut usize) {
    for b in blocks {
        b.index = *next;
        *next += 1;
        number(&mut b.children, next);
    }
}

/// 数が合わないとき用: 範囲の数（入れ子を含む）。
pub(crate) fn count_blocks(blocks: &[Block]) -> usize {
    blocks.iter().map(|b| 1 + count_blocks(&b.children)).sum()
}

/// 差し込みを行った新しいファイルを返す（行ループなし）。
pub fn render(kind: Kind, template: &[u8], resolve: &mut Resolver) -> Result<Vec<u8>> {
    render_with(kind, template, &mut Plain(resolve))
}

/// 行ループを含めて差し込みを行った新しいファイルを返す。
pub fn render_with(kind: Kind, template: &[u8], source: &mut dyn Source) -> Result<Vec<u8>> {
    let mut pkg = package::Package::read(template)?;
    if pkg.get(required_part(kind)).is_none() {
        return Err(Error::NotATemplate(format!(
            "{} 形式のファイルではありません（{} がありません）",
            kind.extension(),
            required_part(kind)
        )));
    }
    match kind {
        Kind::Docx => docx::process(&mut pkg, source)?,
        Kind::Xlsx => xlsx::process(&mut pkg, source)?,
    }
    pkg.write()
}

/// テンプレートの差し込み欄の一覧。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// ループの外の欄の式（出現順・重複なし）
    pub placeholders: Vec<String>,
    /// 行ループ（出現順）
    pub loops: Vec<LoopPlan>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoopPlan {
    /// `{{#each 式}}` の式
    pub source: String,
    /// 入れ子のループなら、外側のループの番号
    pub parent: Option<usize>,
    /// ループの中の欄の式（出現順・重複なし）
    pub exprs: Vec<String>,
}

#[derive(Default)]
struct Recorder(Plan);

impl Source for Recorder {
    fn value(&mut self, expr: &str) -> std::result::Result<Value, String> {
        if !self.0.placeholders.iter().any(|s| s == expr) {
            self.0.placeholders.push(expr.to_string());
        }
        Ok(Value::Null)
    }
    fn loop_len(
        &mut self,
        index: usize,
        parent: Option<usize>,
        _: &[usize],
        source: &str,
    ) -> std::result::Result<usize, String> {
        if source.is_empty() {
            return Err("{{#each}} の後ろに、繰り返す配列の式を書いてください".into());
        }
        // 入れ子の表の中のループは、呼ばれる順が番号の順と限らない
        while self.0.loops.len() <= index {
            self.0.loops.push(LoopPlan::default());
        }
        let l = &mut self.0.loops[index];
        l.source = source.to_string();
        l.parent = parent;
        // 欄を 1 回ずつ集めるために、要素は 1 つあるものとして進める
        Ok(1)
    }
    fn item_value(
        &mut self,
        index: usize,
        _: &[usize],
        expr: &str,
    ) -> std::result::Result<Value, String> {
        let l = &mut self.0.loops[index];
        if !l.exprs.iter().any(|s| s == expr) {
            l.exprs.push(expr.to_string());
        }
        Ok(Value::Null)
    }
}

impl Plan {
    /// 同じ外側（`parent`）を持つループの中での、`index` 番目のループの位置（評価結果の並びの添字）。
    pub fn sibling_pos(&self, index: usize) -> usize {
        let parent = self.loops[index].parent;
        self.loops[..index]
            .iter()
            .filter(|l| l.parent == parent)
            .count()
    }

    /// 一番外側から `index` 番目のループまでの番号の並び。
    pub fn chain(&self, index: usize) -> Vec<usize> {
        let mut chain = vec![index];
        while let Some(p) = self.loops[*chain.last().expect("非空")].parent {
            chain.push(p);
        }
        chain.reverse();
        chain
    }
}

/// テンプレートに含まれる差し込み欄を調べる。
/// 置換と同じ処理で数えるので、Word がランに分割した欄も見つかる。
pub fn plan(kind: Kind, template: &[u8]) -> Result<Plan> {
    let mut rec = Recorder::default();
    render_with(kind, template, &mut rec)?;
    Ok(rec.0)
}

/// ループの外の差し込み欄の式を、出現順・重複なしで返す。
pub fn scan(kind: Kind, template: &[u8]) -> Result<Vec<String>> {
    Ok(plan(kind, template)?.placeholders)
}
