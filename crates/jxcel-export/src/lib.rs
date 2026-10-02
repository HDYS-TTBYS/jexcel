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

/// 行ループの印。表の 1 行の中に `{{#each 式}}` と書くと、その行が式の配列の要素の数だけ繰り返される。
pub const LOOP_MARKER: &str = "#each";

/// 差し込む値の取得元。行ループは、テンプレートの中の出現順に 0, 1, 2… の番号で呼ばれる。
pub trait Source {
    /// ループの外の欄 `{{ 式 }}`。
    fn value(&mut self, expr: &str) -> std::result::Result<Value, String>;
    /// `index` 番目のループの要素数。`source` は `{{#each 式}}` の式。対象が配列でなければ Err。
    fn loop_len(&mut self, index: usize, source: &str) -> std::result::Result<usize, String>;
    /// `index` 番目のループの `item` 番目（0 から）の要素での、ループの中の欄の値。
    fn item_value(
        &mut self,
        index: usize,
        item: usize,
        expr: &str,
    ) -> std::result::Result<Value, String>;
}

/// ループを使わないテンプレート用。式を解決する関数だけを渡す（ループがあるとエラー）。
struct Plain<'a, 'b>(&'a mut Resolver<'b>);

impl Source for Plain<'_, '_> {
    fn value(&mut self, expr: &str) -> std::result::Result<Value, String> {
        (self.0)(expr)
    }
    fn loop_len(&mut self, _: usize, _: &str) -> std::result::Result<usize, String> {
        Err("行ループ {{#each}} は、この呼び出しでは使えません".into())
    }
    fn item_value(&mut self, _: usize, _: usize, _: &str) -> std::result::Result<Value, String> {
        Err("行ループ {{#each}} は、この呼び出しでは使えません".into())
    }
}

/// `{{#each 式}}` の式の部分（印でなければ `None`）。
pub fn loop_source(expr: &str) -> Option<&str> {
    let rest = expr.strip_prefix(LOOP_MARKER)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
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

#[derive(Debug, Clone, PartialEq)]
pub struct LoopPlan {
    /// `{{#each 式}}` の式
    pub source: String,
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
    fn loop_len(&mut self, index: usize, source: &str) -> std::result::Result<usize, String> {
        if source.is_empty() {
            return Err("{{#each}} の後ろに、繰り返す配列の式を書いてください".into());
        }
        if self.0.loops.len() == index {
            self.0.loops.push(LoopPlan {
                source: source.to_string(),
                exprs: vec![],
            });
        }
        // 欄を 1 回ずつ集めるために、要素は 1 つあるものとして進める
        Ok(1)
    }
    fn item_value(
        &mut self,
        index: usize,
        _: usize,
        expr: &str,
    ) -> std::result::Result<Value, String> {
        let l = &mut self.0.loops[index];
        if !l.exprs.iter().any(|s| s == expr) {
            l.exprs.push(expr.to_string());
        }
        Ok(Value::Null)
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
