//! 2 つのファイル状態の構造的な差分。「どの行のどの列がどう変わったか」を返す。
//! 突合は ID で行う（行は rowId、列は columnId、シート/スキーマは各 ID）。

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

use crate::model::{Column, DataSchema, JxcelFile, Row};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Change {
    FileRenamed {
        old: String,
        new: String,
    },
    SheetAdded {
        sheet: String,
        name: String,
    },
    SheetRemoved {
        sheet: String,
        name: String,
    },
    SheetRenamed {
        sheet: String,
        old: String,
        new: String,
    },
    SchemaAdded {
        sheet: String,
        schema: String,
        name: String,
    },
    SchemaRemoved {
        sheet: String,
        schema: String,
        name: String,
    },
    SchemaRenamed {
        sheet: String,
        schema: String,
        old: String,
        new: String,
    },
    ColumnAdded {
        sheet: String,
        schema: String,
        column: String,
        name: String,
    },
    ColumnRemoved {
        sheet: String,
        schema: String,
        column: String,
        name: String,
    },
    /// 名前・型・必須のいずれかが変わった
    ColumnChanged {
        sheet: String,
        schema: String,
        column: String,
        old: Column,
        new: Column,
    },
    RowAdded {
        sheet: String,
        schema: String,
        row: String,
        cells: Row,
    },
    RowRemoved {
        sheet: String,
        schema: String,
        row: String,
        cells: Row,
    },
    CellChanged {
        sheet: String,
        schema: String,
        row: String,
        column: String,
        old: Value,
        new: Value,
    },
    MacroAdded {
        id: String,
        name: String,
    },
    MacroRemoved {
        id: String,
        name: String,
    },
    MacroRenamed {
        id: String,
        old: String,
        new: String,
    },
    /// マクロのソースが変わった
    MacroEdited {
        id: String,
        name: String,
    },
    /// 行の並びだけが変わった（共通する行の相対順序が違う）
    RowsReordered {
        sheet: String,
        schema: String,
    },
}

pub fn diff(old: &JxcelFile, new: &JxcelFile) -> Vec<Change> {
    let mut out = vec![];
    if old.name != new.name {
        out.push(Change::FileRenamed {
            old: old.name.clone(),
            new: new.name.clone(),
        });
    }
    let old_sheets: HashMap<_, _> = old.sheets.iter().map(|s| (&s.id, s)).collect();
    let new_sheets: HashMap<_, _> = new.sheets.iter().map(|s| (&s.id, s)).collect();

    for s in &old.sheets {
        if !new_sheets.contains_key(&s.id) {
            out.push(Change::SheetRemoved {
                sheet: s.id.clone(),
                name: s.name.clone(),
            });
        }
    }
    for ns in &new.sheets {
        let Some(os) = old_sheets.get(&ns.id) else {
            out.push(Change::SheetAdded {
                sheet: ns.id.clone(),
                name: ns.name.clone(),
            });
            continue;
        };
        if os.name != ns.name {
            out.push(Change::SheetRenamed {
                sheet: ns.id.clone(),
                old: os.name.clone(),
                new: ns.name.clone(),
            });
        }
        let old_schemas: HashMap<_, _> = os.schemas.iter().map(|s| (&s.id, s)).collect();
        let new_schema_ids: HashMap<_, _> = ns.schemas.iter().map(|s| (&s.id, ())).collect();
        for s in &os.schemas {
            if !new_schema_ids.contains_key(&s.id) {
                out.push(Change::SchemaRemoved {
                    sheet: ns.id.clone(),
                    schema: s.id.clone(),
                    name: s.name.clone(),
                });
            }
        }
        for nsc in &ns.schemas {
            match old_schemas.get(&nsc.id) {
                None => out.push(Change::SchemaAdded {
                    sheet: ns.id.clone(),
                    schema: nsc.id.clone(),
                    name: nsc.name.clone(),
                }),
                Some(osc) => diff_schema(&ns.id, osc, nsc, &mut out),
            }
        }
    }
    diff_macros(old, new, &mut out);
    out
}

fn diff_macros(old: &JxcelFile, new: &JxcelFile, out: &mut Vec<Change>) {
    for m in &old.macros {
        if !new.macros.iter().any(|n| n.id == m.id) {
            out.push(Change::MacroRemoved {
                id: m.id.clone(),
                name: m.name.clone(),
            });
        }
    }
    for n in &new.macros {
        match old.macros.iter().find(|m| m.id == n.id) {
            None => out.push(Change::MacroAdded {
                id: n.id.clone(),
                name: n.name.clone(),
            }),
            Some(m) => {
                if m.name != n.name {
                    out.push(Change::MacroRenamed {
                        id: n.id.clone(),
                        old: m.name.clone(),
                        new: n.name.clone(),
                    });
                }
                if m.source != n.source {
                    out.push(Change::MacroEdited {
                        id: n.id.clone(),
                        name: n.name.clone(),
                    });
                }
            }
        }
    }
}

fn diff_schema(sheet: &str, old: &DataSchema, new: &DataSchema, out: &mut Vec<Change>) {
    let (sheet, schema) = (sheet.to_string(), new.id.clone());
    if old.name != new.name {
        out.push(Change::SchemaRenamed {
            sheet: sheet.clone(),
            schema: schema.clone(),
            old: old.name.clone(),
            new: new.name.clone(),
        });
    }

    let old_cols: HashMap<_, _> = old.columns.iter().map(|c| (&c.id, c)).collect();
    let new_cols: HashMap<_, _> = new.columns.iter().map(|c| (&c.id, c)).collect();
    for c in &old.columns {
        if !new_cols.contains_key(&c.id) {
            out.push(Change::ColumnRemoved {
                sheet: sheet.clone(),
                schema: schema.clone(),
                column: c.id.clone(),
                name: c.name.clone(),
            });
        }
    }
    for c in &new.columns {
        match old_cols.get(&c.id) {
            None => out.push(Change::ColumnAdded {
                sheet: sheet.clone(),
                schema: schema.clone(),
                column: c.id.clone(),
                name: c.name.clone(),
            }),
            Some(oc) if *oc != c => out.push(Change::ColumnChanged {
                sheet: sheet.clone(),
                schema: schema.clone(),
                column: c.id.clone(),
                old: (*oc).clone(),
                new: c.clone(),
            }),
            Some(_) => {}
        }
    }

    let old_rows: HashMap<_, _> = old.rows.iter().map(|r| (&r.id, r)).collect();
    let new_rows: HashMap<_, _> = new.rows.iter().map(|r| (&r.id, r)).collect();
    for r in &old.rows {
        if !new_rows.contains_key(&r.id) {
            out.push(Change::RowRemoved {
                sheet: sheet.clone(),
                schema: schema.clone(),
                row: r.id.clone(),
                cells: r.clone(),
            });
        }
    }
    for nr in &new.rows {
        let Some(or) = old_rows.get(&nr.id) else {
            out.push(Change::RowAdded {
                sheet: sheet.clone(),
                schema: schema.clone(),
                row: nr.id.clone(),
                cells: nr.clone(),
            });
            continue;
        };
        let mut cols: Vec<&String> = or.cells.keys().chain(nr.cells.keys()).collect();
        cols.sort();
        cols.dedup();
        for col in cols {
            let (o, n) = (
                or.cells.get(col).unwrap_or(&Value::Null),
                nr.cells.get(col).unwrap_or(&Value::Null),
            );
            if o != n {
                out.push(Change::CellChanged {
                    sheet: sheet.clone(),
                    schema: schema.clone(),
                    row: nr.id.clone(),
                    column: col.clone(),
                    old: o.clone(),
                    new: n.clone(),
                });
            }
        }
    }

    // 共通する行だけを取り出して相対順序を比較する
    let common = |rows: &[Row], other: &HashMap<&String, &Row>| -> Vec<String> {
        rows.iter()
            .filter(|r| other.contains_key(&r.id))
            .map(|r| r.id.clone())
            .collect()
    };
    if common(&old.rows, &new_rows) != common(&new.rows, &old_rows) {
        out.push(Change::RowsReordered { sheet, schema });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::tests::sample;
    use crate::types::DataType;
    use serde_json::json;

    #[test]
    fn identical_is_empty() {
        assert!(diff(&sample(), &sample()).is_empty());
    }

    #[test]
    fn cell_row_column_changes() {
        let old = sample();
        let mut new = old.clone();
        let s = &mut new.sheets[0].schemas[0];
        s.rows[0].cells.insert("qty".into(), json!(11));
        s.rows.remove(1);
        s.rows.push(Row {
            id: "r3".into(),
            cells: [("name".to_string(), json!("座金"))].into(),
        });
        s.columns.push(Column::new("loc", "場所", DataType::String));
        s.columns[1].name = "個数".into();

        let d = diff(&old, &new);
        assert!(d.iter().any(
            |c| matches!(c, Change::CellChanged { row, column, old, new, .. }
            if row == "r1" && column == "qty" && *old == json!(10) && *new == json!(11))
        ));
        assert!(d
            .iter()
            .any(|c| matches!(c, Change::RowRemoved { row, .. } if row == "r2")));
        assert!(d
            .iter()
            .any(|c| matches!(c, Change::RowAdded { row, .. } if row == "r3")));
        assert!(d
            .iter()
            .any(|c| matches!(c, Change::ColumnAdded { column, .. } if column == "loc")));
        assert!(d
            .iter()
            .any(|c| matches!(c, Change::ColumnChanged { column, .. } if column == "qty")));
        assert!(!d.iter().any(|c| matches!(c, Change::RowsReordered { .. })));
    }

    #[test]
    fn macro_changes() {
        let old = sample();
        let mut new = old.clone();
        new.macros.push(crate::Macro {
            id: "m1".into(),
            name: "集計".into(),
            source: "a".into(),
        });
        assert_eq!(
            diff(&old, &new),
            vec![Change::MacroAdded {
                id: "m1".into(),
                name: "集計".into()
            }]
        );
        let mut newer = new.clone();
        newer.macros[0].source = "b".into();
        newer.macros[0].name = "合計".into();
        assert_eq!(
            diff(&new, &newer),
            vec![
                Change::MacroRenamed {
                    id: "m1".into(),
                    old: "集計".into(),
                    new: "合計".into()
                },
                Change::MacroEdited {
                    id: "m1".into(),
                    name: "合計".into()
                },
            ]
        );
        assert_eq!(
            diff(&newer, &old),
            vec![Change::MacroRemoved {
                id: "m1".into(),
                name: "合計".into()
            }]
        );
    }

    #[test]
    fn reorder_and_structure() {
        let old = sample();
        let mut new = old.clone();
        new.sheets[0].schemas[0].rows.reverse();
        assert_eq!(
            diff(&old, &new),
            vec![Change::RowsReordered {
                sheet: "sh1".into(),
                schema: "s1".into()
            }]
        );

        let mut new = old.clone();
        new.name = "新台帳".into();
        new.sheets[0].name = "倉庫2".into();
        new.sheets.push(crate::Sheet {
            id: "sh2".into(),
            name: "新".into(),
            schemas: vec![],
        });
        let d = diff(&old, &new);
        assert!(d.iter().any(|c| matches!(c, Change::FileRenamed { .. })));
        assert!(d.iter().any(|c| matches!(c, Change::SheetRenamed { .. })));
        assert!(d
            .iter()
            .any(|c| matches!(c, Change::SheetAdded { sheet, .. } if sheet == "sh2")));

        let mut gone = old.clone();
        gone.sheets.clear();
        assert!(diff(&old, &gone)
            .iter()
            .any(|c| matches!(c, Change::SheetRemoved { .. })));
    }
}
