//! jxcel のファイル形式・型システム・構造差分。UI や Tauri には依存しない。

pub mod diff;
pub mod error;
pub mod model;
pub mod tree;
pub mod types;

pub use error::{Error, Result};
pub use model::{Column, DataSchema, JxcelFile, Macro, Row, Sheet};
pub use types::{DataType, TypeRegistry};

/// 永続化フォーマットのバージョン。互換性を壊す変更で上げる。
pub const FORMAT_VERSION: u32 = 1;

/// 衝突しない安定 ID（ULID）を生成する。
pub fn new_id() -> String {
    ulid::Ulid::new().to_string()
}
