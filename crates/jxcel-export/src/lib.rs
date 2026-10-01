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
    export, preview, sanitize_filename, GeneratedFile, Preview, PreviewRow, Report, RowError, Spec,
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

/// 差し込みを行った新しいファイルを返す。
pub fn render(kind: Kind, template: &[u8], resolve: &mut Resolver) -> Result<Vec<u8>> {
    let mut pkg = package::Package::read(template)?;
    if pkg.get(required_part(kind)).is_none() {
        return Err(Error::NotATemplate(format!(
            "{} 形式のファイルではありません（{} がありません）",
            kind.extension(),
            required_part(kind)
        )));
    }
    match kind {
        Kind::Docx => docx::process(&mut pkg, resolve)?,
        Kind::Xlsx => xlsx::process(&mut pkg, resolve)?,
    }
    pkg.write()
}

/// テンプレートに含まれる差し込み欄の式を、出現順・重複なしで返す。
/// 置換と同じ処理で数えるので、Word がランに分割した欄も見つかる。
pub fn scan(kind: Kind, template: &[u8]) -> Result<Vec<String>> {
    let mut seen: Vec<String> = vec![];
    render(kind, template, &mut |expr| {
        if !seen.iter().any(|s| s == expr) {
            seen.push(expr.to_string());
        }
        Ok(Value::Null)
    })?;
    Ok(seen)
}
