use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::types::{DataType, TypeRegistry};

/// File → Sheet → DataSchema（1:N）→ Row
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JxcelFile {
    pub name: String,
    pub sheets: Vec<Sheet>,
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
        }
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
