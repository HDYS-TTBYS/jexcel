//! アプリの操作ロジック。Tauri には依存せず、`src-tauri` はこれを薄く包むだけにする。
//!
//! 開いているファイルの状態（モデル・履歴・保存先・未保存フラグ）を `Session` が持ち、
//! UI からの編集はすべてここを通る。編集のたびに更新後のモデル全体を返す（MVP の単純化）。

use jxcel_core::diff::Change;
use jxcel_core::types::TypeRegistry;
use jxcel_core::{new_id, Column, DataSchema, JxcelFile, Row, Sheet};
use jxcel_git::archive::Archive;
use jxcel_git::CommitInfo;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("ファイルが開かれていません")]
    NoFile,
    #[error("保存先が未指定です")]
    NoPath,
    #[error("{0} が見つかりません")]
    NotFound(&'static str),
    #[error("{0}")]
    Invalid(String),
    #[error("入出力エラー: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Core(#[from] jxcel_core::Error),
    #[error(transparent)]
    Git(#[from] jxcel_git::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// UI に返す現在の状態。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub file: JxcelFile,
    pub path: Option<String>,
    pub dirty: bool,
}

struct Doc {
    file: JxcelFile,
    archive: Archive,
    path: Option<PathBuf>,
    dirty: bool,
}

#[derive(Default)]
pub struct Session {
    doc: Option<Doc>,
    registry: TypeRegistry,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    fn doc(&mut self) -> Result<&mut Doc> {
        self.doc.as_mut().ok_or(Error::NoFile)
    }

    fn snapshot(&self) -> Result<Snapshot> {
        let d = self.doc.as_ref().ok_or(Error::NoFile)?;
        Ok(Snapshot {
            file: d.file.clone(),
            path: d.path.as_ref().map(|p| p.display().to_string()),
            dirty: d.dirty,
        })
    }

    /// 編集を適用して未保存にし、更新後の状態を返す。
    fn edit(
        &mut self,
        f: impl FnOnce(&mut JxcelFile, &TypeRegistry) -> Result<()>,
    ) -> Result<Snapshot> {
        let reg = self.registry.clone();
        let d = self.doc()?;
        // 失敗した編集で中途半端な状態を残さない
        let mut next = d.file.clone();
        f(&mut next, &reg)?;
        d.file = next;
        d.dirty = true;
        self.snapshot()
    }

    // ---- ファイル ----

    pub fn new_file(&mut self, name: &str) -> Result<Snapshot> {
        let mut schema = DataSchema::new(
            "データ",
            vec![Column::new(new_id(), "列1", jxcel_core::DataType::String)],
        );
        schema.rows.push(Row::new(Default::default()));
        let file = JxcelFile {
            name: name.into(),
            sheets: vec![Sheet {
                id: new_id(),
                name: "シート1".into(),
                schemas: vec![schema],
            }],
        };
        self.doc = Some(Doc {
            file,
            archive: Archive::create()?,
            path: None,
            // 保存先がないことと、未保存の変更があることは別。編集するまでは変更なし。
            dirty: false,
        });
        self.snapshot()
    }

    pub fn open(&mut self, path: &str) -> Result<Snapshot> {
        let bytes = std::fs::read(path)?;
        let (archive, file) = Archive::open(&bytes)?;
        self.doc = Some(Doc {
            file,
            archive,
            path: Some(path.into()),
            dirty: false,
        });
        self.snapshot()
    }

    /// 保存して履歴に記録する。`path` を渡すと保存先を変更する（名前を付けて保存）。
    pub fn save(&mut self, path: Option<&str>, message: &str) -> Result<Snapshot> {
        let d = self.doc()?;
        if let Some(p) = path {
            d.path = Some(p.into());
        }
        let dest = d.path.clone().ok_or(Error::NoPath)?;
        let message = if message.trim().is_empty() {
            "保存"
        } else {
            message
        };
        let bytes = d.archive.save(&d.file, message)?;
        // 書き込み途中の破損を避けるため一時ファイル経由で置き換える
        let tmp = dest.with_extension("jxcel.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &dest)?;
        d.dirty = false;
        self.snapshot()
    }

    pub fn current(&self) -> Result<Snapshot> {
        self.snapshot()
    }

    // ---- 構造の編集 ----

    pub fn add_sheet(&mut self, name: &str) -> Result<Snapshot> {
        let name = name.to_string();
        self.edit(move |f, _| {
            f.sheets.push(Sheet {
                id: new_id(),
                name,
                schemas: vec![],
            });
            Ok(())
        })
    }

    pub fn rename_sheet(&mut self, sheet: &str, name: &str) -> Result<Snapshot> {
        let (sheet, name) = (sheet.to_string(), name.to_string());
        self.edit(move |f, _| {
            find_sheet(f, &sheet)?.name = name;
            Ok(())
        })
    }

    pub fn delete_sheet(&mut self, sheet: &str) -> Result<Snapshot> {
        let sheet = sheet.to_string();
        self.edit(move |f, _| {
            let before = f.sheets.len();
            f.sheets.retain(|s| s.id != sheet);
            (f.sheets.len() != before)
                .then_some(())
                .ok_or(Error::NotFound("シート"))
        })
    }

    pub fn add_schema(
        &mut self,
        sheet: &str,
        name: &str,
        columns: Vec<Column>,
    ) -> Result<Snapshot> {
        let (sheet, name) = (sheet.to_string(), name.to_string());
        self.edit(move |f, _| {
            find_sheet(f, &sheet)?
                .schemas
                .push(DataSchema::new(name, columns));
            Ok(())
        })
    }

    pub fn add_column(&mut self, sheet: &str, schema: &str, column: Column) -> Result<Snapshot> {
        let (sheet, schema) = (sheet.to_string(), schema.to_string());
        self.edit(move |f, _| {
            let s = find_schema(f, &sheet, &schema)?;
            if s.columns.iter().any(|c| c.id == column.id) {
                return Err(Error::Invalid("列 ID が重複しています".into()));
            }
            s.columns.push(column);
            Ok(())
        })
    }

    /// 列の名前・型・必須を更新する。既存の値が新しい型に合わない場合は拒否する。
    pub fn update_column(&mut self, sheet: &str, schema: &str, column: Column) -> Result<Snapshot> {
        let (sheet, schema) = (sheet.to_string(), schema.to_string());
        self.edit(move |f, reg| {
            let s = find_schema(f, &sheet, &schema)?;
            let slot = s
                .columns
                .iter_mut()
                .find(|c| c.id == column.id)
                .ok_or(Error::NotFound("列"))?;
            *slot = column;
            match s.validate(reg).into_iter().next() {
                Some(v) => Err(Error::Invalid(format!(
                    "既存の値が新しい定義に合いません: {}",
                    v.message
                ))),
                None => Ok(()),
            }
        })
    }

    pub fn delete_column(&mut self, sheet: &str, schema: &str, column: &str) -> Result<Snapshot> {
        let (sheet, schema, column) = (sheet.to_string(), schema.to_string(), column.to_string());
        self.edit(move |f, _| {
            let s = find_schema(f, &sheet, &schema)?;
            let before = s.columns.len();
            s.columns.retain(|c| c.id != column);
            if s.columns.len() == before {
                return Err(Error::NotFound("列"));
            }
            for r in &mut s.rows {
                r.cells.remove(&column);
            }
            Ok(())
        })
    }

    // ---- 行・セルの編集 ----

    pub fn add_row(&mut self, sheet: &str, schema: &str) -> Result<Snapshot> {
        let (sheet, schema) = (sheet.to_string(), schema.to_string());
        self.edit(move |f, _| {
            find_schema(f, &sheet, &schema)?
                .rows
                .push(Row::new(Default::default()));
            Ok(())
        })
    }

    pub fn delete_row(&mut self, sheet: &str, schema: &str, row: &str) -> Result<Snapshot> {
        let (sheet, schema, row) = (sheet.to_string(), schema.to_string(), row.to_string());
        self.edit(move |f, _| {
            let s = find_schema(f, &sheet, &schema)?;
            let before = s.rows.len();
            s.rows.retain(|r| r.id != row);
            (s.rows.len() != before)
                .then_some(())
                .ok_or(Error::NotFound("行"))
        })
    }

    /// セルを更新する。型に合わない値は拒否し、モデルは変更しない。`null` で値を消す。
    pub fn set_cell(
        &mut self,
        sheet: &str,
        schema: &str,
        row: &str,
        column: &str,
        value: Value,
    ) -> Result<Snapshot> {
        let (sheet, schema, row, column) = (
            sheet.to_string(),
            schema.to_string(),
            row.to_string(),
            column.to_string(),
        );
        self.edit(move |f, reg| {
            let s = find_schema(f, &sheet, &schema)?;
            let col = s
                .columns
                .iter()
                .find(|c| c.id == column)
                .ok_or(Error::NotFound("列"))?;
            col.validate(&value, reg).map_err(Error::Invalid)?;
            let r = s
                .rows
                .iter_mut()
                .find(|r| r.id == row)
                .ok_or(Error::NotFound("行"))?;
            if value.is_null() {
                r.cells.remove(&column);
            } else {
                r.cells.insert(column, value);
            }
            Ok(())
        })
    }

    // ---- 履歴 ----

    pub fn history_log(&mut self) -> Result<Vec<CommitInfo>> {
        Ok(self.doc()?.archive.history().log()?)
    }

    pub fn history_diff(&mut self, from: &str, to: &str) -> Result<Vec<Change>> {
        Ok(self.doc()?.archive.history().diff(from, to)?)
    }

    /// 過去のリビジョンの内容を現在の状態として読み込む（未保存。保存で新しいコミットになる）。
    pub fn restore(&mut self, rev: &str) -> Result<Snapshot> {
        let d = self.doc()?;
        let file = d.archive.history().load(rev)?;
        d.file = file;
        d.dirty = true;
        self.snapshot()
    }
}

fn find_sheet<'a>(f: &'a mut JxcelFile, id: &str) -> Result<&'a mut Sheet> {
    f.sheets
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or(Error::NotFound("シート"))
}

fn find_schema<'a>(f: &'a mut JxcelFile, sheet: &str, schema: &str) -> Result<&'a mut DataSchema> {
    find_sheet(f, sheet)?
        .schemas
        .iter_mut()
        .find(|s| s.id == schema)
        .ok_or(Error::NotFound("スキーマ"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jxcel_core::DataType;
    use serde_json::json;

    fn ids(s: &Snapshot) -> (String, String, String, String) {
        let sh = &s.file.sheets[0];
        let sc = &sh.schemas[0];
        (
            sh.id.clone(),
            sc.id.clone(),
            sc.rows[0].id.clone(),
            sc.columns[0].id.clone(),
        )
    }

    #[test]
    fn edit_validate_save_history_restore() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jxcel");
        let path = path.to_str().unwrap();

        let mut s = Session::new();
        assert!(matches!(s.current(), Err(Error::NoFile)));
        let snap = s.new_file("台帳").unwrap();
        // 新規直後は未保存の変更なし（保存先がないだけ）。最初の編集で変更ありになる
        assert!(!snap.dirty);
        assert_eq!(snap.path, None);
        let (sh, sc, row, col) = ids(&snap);

        // 型を Int に変更して不正値を拒否
        s.update_column(&sh, &sc, Column::new(&col, "数量", DataType::Int))
            .unwrap();
        assert!(matches!(
            s.set_cell(&sh, &sc, &row, &col, json!("x")),
            Err(Error::Invalid(_))
        ));
        let snap = s.set_cell(&sh, &sc, &row, &col, json!(1)).unwrap();
        assert!(snap.dirty);
        assert!(matches!(s.save(None, ""), Err(Error::NoPath)));

        let snap = s.save(Some(path), "初回").unwrap();
        assert!(!snap.dirty);

        s.set_cell(&sh, &sc, &row, &col, json!(2)).unwrap();
        s.save(None, "数量変更").unwrap();

        // 別セッションで開き直しても履歴が残っている
        let mut s2 = Session::new();
        let snap = s2.open(path).unwrap();
        assert!(!snap.dirty);
        let log = s2.history_log().unwrap();
        assert_eq!(log.len(), 2);
        let d = s2.history_diff(&log[1].id, &log[0].id).unwrap();
        assert!(matches!(&d[..], [Change::CellChanged { new, .. }] if *new == json!(2)));

        // 復元は未保存状態になり、保存で新しいコミットになる
        let snap = s2.restore(&log[1].id).unwrap();
        assert!(snap.dirty);
        assert_eq!(snap.file.sheets[0].schemas[0].rows[0].cells[&col], json!(1));
        s2.save(None, "復元").unwrap();
        assert_eq!(s2.history_log().unwrap().len(), 3);
    }

    #[test]
    fn untouched_new_file_can_still_be_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.jxcel");
        let mut s = Session::new();
        assert!(!s.new_file("x").unwrap().dirty);
        // 変更なしでも、保存先を指定して保存できる（履歴は初回コミットから始まる）
        let snap = s.save(Some(path.to_str().unwrap()), "").unwrap();
        assert!(!snap.dirty);
        assert_eq!(snap.path.as_deref(), path.to_str());
        assert_eq!(s.history_log().unwrap().len(), 1);
    }

    #[test]
    fn failed_edit_leaves_state_untouched_and_column_change_is_guarded() {
        let mut s = Session::new();
        let snap = s.new_file("x").unwrap();
        let (sh, sc, row, col) = ids(&snap);
        s.set_cell(&sh, &sc, &row, &col, json!("abc")).unwrap();

        // 既存値に合わない型への変更は拒否される
        assert!(s
            .update_column(&sh, &sc, Column::new(&col, "列1", DataType::Int))
            .is_err());
        let cur = s.current().unwrap();
        assert_eq!(
            cur.file.sheets[0].schemas[0].columns[0].ty,
            DataType::String
        );

        assert!(matches!(s.add_row(&sh, "nope"), Err(Error::NotFound(_))));
        assert!(matches!(
            s.add_column(&sh, &sc, Column::new(&col, "dup", DataType::Int)),
            Err(Error::Invalid(_))
        ));

        // 追加・削除
        s.add_sheet("S2").unwrap();
        s.add_row(&sh, &sc).unwrap();
        assert_eq!(s.current().unwrap().file.sheets[0].schemas[0].rows.len(), 2);
        s.set_cell(&sh, &sc, &row, &col, Value::Null).unwrap();
        s.delete_row(&sh, &sc, &row).unwrap();
        s.delete_column(&sh, &sc, &col).unwrap();
        s.delete_sheet(&sh).unwrap();
        assert_eq!(s.current().unwrap().file.sheets.len(), 1);
    }
}
