use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::types::{DataType, TypeRegistry};

/// File → Sheet → DataSchema（1:N）→ Row
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JxcelFile {
    pub name: String,
    pub sheets: Vec<Sheet>,
    /// ファイルに保存される TS マクロ。旧形式のファイルには無いので省略可。
    #[serde(default)]
    pub macros: Vec<Macro>,
    /// 書き出し（テンプレートへの差し込み）の設定。
    #[serde(default)]
    pub exports: Vec<Export>,
    /// LAN で配信する入力フォームの設定（順序つき）。
    pub forms: Vec<Form>,
    /// 書き出しのテンプレート本体（xlsx / docx のバイト列。キーは `Export::id`）。
    /// 大きいので UI へ返す JSON には含めない。永続化では `exports/<id>.<拡張子>` として別に保存する。
    #[serde(skip)]
    pub templates: BTreeMap<String, Vec<u8>>,
}

/// テンプレートの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateKind {
    Xlsx,
    Docx,
}

impl TemplateKind {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "xlsx" => Some(Self::Xlsx),
            "docx" => Some(Self::Docx),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Xlsx => "xlsx",
            Self::Docx => "docx",
        }
    }
}

/// 書き出しの設定: 表の各行をテンプレートに差し込み、行ごとに 1 ファイルを作る。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Export {
    pub id: String,
    pub name: String,
    pub kind: TemplateKind,
    /// 取り込んだテンプレートの元のファイル名（表示用）
    pub template_name: String,
    /// 対象の表（シート ID・スキーマ ID）
    pub sheet: String,
    pub schema: String,
    /// 出力ファイル名（`{{ 式 }}` が使える。拡張子は付けなくてよい）
    pub filename: String,
    /// 空でなければ、この式が真になる行だけを書き出す
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

/// 入力フォームの設定: 表の一部の列を、LAN 内のブラウザから行として入力できるようにする。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Form {
    pub id: String,
    /// フォームの題名（回答者にも見える）
    pub name: String,
    /// 回答の追加先の表（シート ID・スキーマ ID）
    pub sheet: String,
    pub schema: String,
    /// 入力欄にする列の ID（表示順）
    pub columns: Vec<String>,
    /// 題名の下に出す説明（空なら出さない）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// TypeScript で書いたマクロ。`source` はユーザーが書いたままの TS（実行時に JS へ変換する）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Macro {
    pub id: String,
    pub name: String,
    pub source: String,
}

impl Macro {
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            id: crate::new_id(),
            name: name.into(),
            source: source.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub id: String,
    pub name: String,
    pub schemas: Vec<DataSchema>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataSchema {
    pub id: String,
    pub name: String,
    pub columns: Vec<Column>,
    /// 表示順。永続化時は行本体と分離され、並べ替えが大量の差分を生まない。
    pub rows: Vec<Row>,
}

/// 列（およびオブジェクト型のフィールド）。`id` は不変、`name` は表示名で変更可。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: DataType,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    /// 計算列。値は保存せず、読み込み時に式から計算する（元データと式だけが真実）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computed: Option<Computed>,
}

/// 計算列の式。`export default function (row, jx) { return ... }` の形の TypeScript。
/// 永続化では `computed/<スキーマ>.<列>.ts` に分け、列定義の JSON には印だけを残す。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Computed {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    /// 列 ID → 値
    pub cells: BTreeMap<String, Value>,
}

impl Column {
    pub fn new(id: impl Into<String>, name: impl Into<String>, ty: DataType) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            ty,
            required: false,
            computed: None,
        }
    }

    /// 計算列にする。
    pub fn computed(mut self, source: impl Into<String>) -> Self {
        self.computed = Some(Computed {
            source: source.into(),
        });
        self
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn validate(&self, value: &Value, reg: &TypeRegistry) -> Result<(), String> {
        if value.is_null() && self.required {
            return Err(format!("{} は必須です", self.name));
        }
        self.ty.validate(value, reg)
    }
}

impl Row {
    pub fn new(cells: BTreeMap<String, Value>) -> Self {
        Self {
            id: crate::new_id(),
            cells,
        }
    }
}

/// 検証エラー 1 件。
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub row_id: String,
    pub column_id: String,
    pub message: String,
}

impl DataSchema {
    pub fn new(name: impl Into<String>, columns: Vec<Column>) -> Self {
        Self {
            id: crate::new_id(),
            name: name.into(),
            columns,
            rows: vec![],
        }
    }

    /// 全行を検証する。未定義の列を含む行・必須列の欠落も違反とする。
    pub fn validate(&self, reg: &TypeRegistry) -> Vec<Violation> {
        let mut out = vec![];
        for row in &self.rows {
            for key in row.cells.keys() {
                if !self.columns.iter().any(|c| &c.id == key) {
                    out.push(Violation {
                        row_id: row.id.clone(),
                        column_id: key.clone(),
                        message: "未定義の列".into(),
                    });
                }
            }
            for col in &self.columns {
                // 計算列は値を保存しないので、検証の対象外（結果の型は計算時に検査する）
                if col.computed.is_some() {
                    continue;
                }
                let v = row.cells.get(&col.id).unwrap_or(&Value::Null);
                if let Err(message) = col.validate(v, reg) {
                    out.push(Violation {
                        row_id: row.id.clone(),
                        column_id: col.id.clone(),
                        message,
                    });
                }
            }
        }
        out
    }
}
