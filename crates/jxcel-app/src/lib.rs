//! アプリの操作ロジック。Tauri には依存せず、`src-tauri` はこれを薄く包むだけにする。
//!
//! 開いているファイルの状態（モデル・履歴・保存先・未保存フラグ）を `Session` が持ち、
//! UI からの編集はすべてここを通る。編集のたびに更新後のモデル全体を返す（MVP の単純化）。

pub mod forms;

use jxcel_core::diff::Change;
use jxcel_core::types::TypeRegistry;
use jxcel_core::{
    new_id, Column, DataSchema, DataType, Export, JxcelFile, Macro, Row, Sheet, TemplateKind,
};
use jxcel_git::archive::Archive;
use jxcel_git::CommitInfo;
use jxcel_macro::ComputedValues;
use serde::Serialize;
use serde_json::Value;

use std::path::PathBuf;
use std::time::Duration;
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
    #[error("マクロ: {0}")]
    Macro(#[from] jxcel_macro::Error),
    #[error("書き出し: {0}")]
    Export(#[from] jxcel_export::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// UI に返す現在の状態。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub file: JxcelFile,
    /// 計算列の値（保存はされず、状態を返すたびに式から計算する）
    pub computed: ComputedValues,
    pub path: Option<String>,
    pub dirty: bool,
}

/// 状態を返すたびに計算列を評価するので、暴走する式で操作が止まらないよう短くする
/// （時間切れは、計算セルのエラーとして表示される）。
const COMPUTE_TIMEOUT: Duration = Duration::from_secs(2);

/// マクロ実行の結果。書き込みがあればファイルに反映済みで、`snapshot` はその後の状態。
#[derive(Debug, Clone, Serialize)]
pub struct RunOutput {
    pub snapshot: Snapshot,
    pub logs: Vec<String>,
    pub result: Option<Value>,
    /// 書き込み操作の数（0 ならファイルは変わっていない）
    pub ops: usize,
}

/// 書き出しの結果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub out_dir: String,
    pub written: Vec<WrittenFile>,
    /// 絞り込み条件に合わず、書き出さなかった行数
    pub skipped: usize,
    /// 書き出せなかった行（他の行は書き出されている）
    pub errors: Vec<ExportRowError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WrittenFile {
    pub row_no: usize,
    /// 実際に書いたファイル名（同名のファイルが既にあれば番号が付く）
    pub filename: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRowError {
    pub row_no: usize,
    pub message: String,
}

/// 日時列のオフセット一括変換の結果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertResult {
    pub snapshot: Snapshot,
    /// 値が変わったセルの数
    pub converted: usize,
    /// すでにそのオフセットで、変わらなかったセルの数
    pub unchanged: usize,
    /// 変換できず、そのままにしたセルの数（うるう秒・範囲外など。変換の対象になった値だけを数える）
    pub skipped: usize,
}

/// 書き出しのプレビュー。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreview {
    /// テンプレートに含まれる差し込み欄の式
    pub placeholders: Vec<String>,
    /// テンプレートの行ループ（`{{#each 式}}`）
    pub loops: Vec<ExportPreviewLoop>,
    pub total_rows: usize,
    pub rows: Vec<ExportPreviewRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreviewLoop {
    /// 繰り返す配列の式
    pub source: String,
    /// 入れ子のループなら、外側のループの番号（`loops` の添字）
    pub parent: Option<usize>,
    /// ループの中の差し込み欄の式
    pub exprs: Vec<String>,
    /// プレビューした行ごとの、繰り返しの回数（要素数）か、対象の式のエラー（入れ子のループは外側の要素すべての合計）
    pub counts: Vec<jxcel_macro::CellResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreviewRow {
    pub row_no: usize,
    /// 絞り込み条件で除外される行か
    pub excluded: bool,
    /// 出力ファイル名（値）か、そのエラー
    pub filename: jxcel_macro::CellResult,
    /// `placeholders` と同じ順の、欄ごとの値かエラー
    pub values: Vec<jxcel_macro::CellResult>,
}

const MACRO_TEMPLATE: &str = r#"// jx からファイルの読み書きができます（型は Jxcel）。
// 列は名前で指定し、行の ID は _id で参照します。
export default function (jx: Jxcel) {
  const rows = jx.sheet("シート1").schema("データ").rows();
  jx.log(`${rows.length} 行`);
}
"#;

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
        let computed = jxcel_macro::compute(
            &d.file,
            &jxcel_macro::Options {
                timeout: COMPUTE_TIMEOUT,
                ..Default::default()
            },
        );
        Ok(Snapshot {
            file: d.file.clone(),
            computed,
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
            macros: Default::default(),
            exports: Default::default(),
            forms: Default::default(),
            templates: Default::default(),
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
            f.forms.retain(|form| form.sheet != sheet);
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
            // 計算列は値を保存しない
            if column.computed.is_some() {
                for r in &mut s.rows {
                    r.cells.remove(&column.id);
                }
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
            // 通常の列を計算列に変えたら、それまでに保存していた値は捨てる（値は式から決まる）
            if slot.computed.is_some() {
                let id = slot.id.clone();
                s.rows.iter_mut().for_each(|r| {
                    r.cells.remove(&id);
                });
            }
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
            // 消した列は、フォームの入力欄からも外す
            for form in &mut f.forms {
                if form.sheet == sheet && form.schema == schema {
                    form.columns.retain(|c| c != &column);
                }
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
            if col.computed.is_some() {
                return Err(Error::Invalid(format!(
                    "「{}」は計算列なので編集できません",
                    col.name
                )));
            }
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

    /// 日時型の列の値を、同じ時刻のまま別のオフセット（`+09:00`・`Z` など）の表記に直す。
    /// 値の入っているセルだけが対象で、変換できない値はそのまま残す。変更はまとめて 1 回の編集になる。
    pub fn convert_datetime_offset(
        &mut self,
        sheet: &str,
        schema: &str,
        column: &str,
        offset: &str,
    ) -> Result<ConvertResult> {
        let minutes = jxcel_core::types::parse_offset(offset).ok_or_else(|| {
            Error::Invalid(format!(
                "オフセットは「+09:00」「-05:30」「Z」の形で指定してください: {offset}"
            ))
        })?;
        let zulu = offset.trim().eq_ignore_ascii_case("z");
        let (sheet, schema, column) = (sheet.to_string(), schema.to_string(), column.to_string());
        let (mut converted, mut unchanged, mut skipped) = (0, 0, 0);
        let was_dirty = self.doc()?.dirty;
        let mut snapshot = self.edit(|f, _| {
            let s = find_schema(f, &sheet, &schema)?;
            let col = s
                .columns
                .iter()
                .find(|c| c.id == column)
                .ok_or(Error::NotFound("列"))?;
            if !matches!(col.ty, DataType::DateTime) || col.computed.is_some() {
                return Err(Error::Invalid(format!(
                    "「{}」は日時型の列ではありません（計算列は変換できません）",
                    col.name
                )));
            }
            for r in &mut s.rows {
                let Some(Value::String(v)) = r.cells.get(&column) else {
                    continue;
                };
                match jxcel_core::types::datetime_to_offset(v, minutes, zulu) {
                    Some(n) if &n == v => unchanged += 1,
                    Some(n) => {
                        r.cells.insert(column.clone(), Value::String(n));
                        converted += 1;
                    }
                    None => skipped += 1,
                }
            }
            Ok(())
        })?;
        if converted == 0 {
            // 何も変わらなかったときは、未保存の状態にしない
            self.doc()?.dirty = was_dirty;
            snapshot = self.snapshot()?;
        }
        Ok(ConvertResult {
            snapshot,
            converted,
            unchanged,
            skipped,
        })
    }

    // ---- マクロ ----

    /// マクロを追加する。`source` を渡すとその内容（サンプルなど）で、無ければ雛形で作る。
    pub fn add_macro(&mut self, name: &str, source: Option<&str>) -> Result<Snapshot> {
        let (name, source) = (
            name.to_string(),
            source.unwrap_or(MACRO_TEMPLATE).to_string(),
        );
        self.edit(move |f, _| {
            f.macros.push(Macro::new(name, source));
            Ok(())
        })
    }

    /// 同梱のサンプルマクロ（ファイルを開いていなくても取得できる）。
    pub fn macro_samples() -> Vec<jxcel_macro::samples::Sample> {
        jxcel_macro::samples::samples()
    }

    pub fn update_macro(&mut self, id: &str, name: &str, source: &str) -> Result<Snapshot> {
        // 変更がなければ未保存にしない（エディタが同じ内容を送り直しても dirty にならない）
        let current = self
            .doc()?
            .file
            .macros
            .iter()
            .find(|m| m.id == id)
            .ok_or(Error::NotFound("マクロ"))?;
        if current.name == name && current.source == source {
            return self.snapshot();
        }
        let (id, name, source) = (id.to_string(), name.to_string(), source.to_string());
        self.edit(move |f, _| {
            let m = f
                .macros
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or(Error::NotFound("マクロ"))?;
            m.name = name;
            m.source = source;
            Ok(())
        })
    }

    pub fn delete_macro(&mut self, id: &str) -> Result<Snapshot> {
        let id = id.to_string();
        self.edit(move |f, _| {
            let before = f.macros.len();
            f.macros.retain(|m| m.id != id);
            (f.macros.len() != before)
                .then_some(())
                .ok_or(Error::NotFound("マクロ"))
        })
    }

    /// マクロを実行する。書き込みが検証を通れば反映して未保存にし、失敗したら何も変えない。
    /// `source` を渡すと、保存前のエディタの内容で実行できる（渡さなければ保存済みのソース）。
    pub fn run_macro(&mut self, id: &str, source: Option<&str>) -> Result<RunOutput> {
        let d = self.doc()?;
        let saved = d
            .file
            .macros
            .iter()
            .find(|m| m.id == id)
            .ok_or(Error::NotFound("マクロ"))?;
        let code = source.unwrap_or(&saved.source).to_string();
        let run = jxcel_macro::run(&code, &d.file, &jxcel_macro::Options::default())?;
        if run.ops > 0 {
            d.file = run.file;
            d.dirty = true;
        }
        Ok(RunOutput {
            snapshot: self.snapshot()?,
            logs: run.logs,
            result: run.result,
            ops: run.ops,
        })
    }

    // ---- 書き出し（テンプレートへの差し込み） ----

    /// テンプレート（xlsx / docx）を取り込んで書き出しを追加する。対象の表は先頭のシートの先頭のスキーマ。
    pub fn add_export(&mut self, template_path: &str, name: Option<&str>) -> Result<Snapshot> {
        let path = std::path::Path::new(template_path);
        let kind = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(TemplateKind::from_extension)
            .ok_or_else(|| {
                Error::Invalid("テンプレートは .xlsx か .docx のファイルを選んでください".into())
            })?;
        let bytes = std::fs::read(path)?;
        // 取り込む前に、差し込み欄を読めるファイルか確かめる
        jxcel_export::scan(kind, &bytes)?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("テンプレート")
            .to_string();
        let name = name
            .map(String::from)
            .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(String::from))
            .unwrap_or_else(|| "書き出し".into());

        self.edit(move |f, _| {
            let sheet = f
                .sheets
                .first()
                .ok_or_else(|| Error::Invalid("シートがありません".into()))?;
            let schema = sheet.schemas.first().ok_or_else(|| {
                Error::Invalid("書き出しの対象にする表（スキーマ）がありません".into())
            })?;
            let filename = schema
                .columns
                .iter()
                .find(|c| c.computed.is_none())
                .map_or_else(|| "{{_no}}".to_string(), |c| format!("{{{{{}}}}}", c.name));
            let id = new_id();
            f.exports.push(Export {
                id: id.clone(),
                name,
                kind,
                template_name: file_name,
                sheet: sheet.id.clone(),
                schema: schema.id.clone(),
                filename,
                filter: None,
            });
            f.templates.insert(id, bytes);
            Ok(())
        })
    }

    /// 設定を更新する（対象の表・出力ファイル名・絞り込み）。`filter` が空なら絞り込みなし。
    pub fn update_export(
        &mut self,
        id: &str,
        name: &str,
        sheet: &str,
        schema: &str,
        filename: &str,
        filter: Option<&str>,
    ) -> Result<Snapshot> {
        let (id, name, sheet, schema, filename) = (
            id.to_string(),
            name.to_string(),
            sheet.to_string(),
            schema.to_string(),
            filename.to_string(),
        );
        let filter = filter
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(String::from);
        self.edit(move |f, _| {
            if !f
                .sheets
                .iter()
                .any(|s| s.id == sheet && s.schemas.iter().any(|c| c.id == schema))
            {
                return Err(Error::NotFound("書き出しの対象の表"));
            }
            let e = f
                .exports
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or(Error::NotFound("書き出し"))?;
            e.name = name;
            e.sheet = sheet;
            e.schema = schema;
            e.filename = filename;
            e.filter = filter;
            Ok(())
        })
    }

    /// テンプレートのファイルを差し替える（種類は同じこと）。
    pub fn replace_export_template(&mut self, id: &str, template_path: &str) -> Result<Snapshot> {
        let path = std::path::Path::new(template_path);
        let id = id.to_string();
        let kind = self
            .doc()?
            .file
            .exports
            .iter()
            .find(|e| e.id == id)
            .ok_or(Error::NotFound("書き出し"))?
            .kind;
        let actual = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(TemplateKind::from_extension);
        if actual != Some(kind) {
            return Err(Error::Invalid(format!(
                "この書き出しのテンプレートは .{} です。同じ種類のファイルを選んでください",
                kind.extension()
            )));
        }
        let bytes = std::fs::read(path)?;
        jxcel_export::scan(kind, &bytes)?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("テンプレート")
            .to_string();
        self.edit(move |f, _| {
            let e = f
                .exports
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or(Error::NotFound("書き出し"))?;
            e.template_name = file_name;
            f.templates.insert(id, bytes);
            Ok(())
        })
    }

    pub fn delete_export(&mut self, id: &str) -> Result<Snapshot> {
        let id = id.to_string();
        self.edit(move |f, _| {
            let before = f.exports.len();
            f.exports.retain(|e| e.id != id);
            f.templates.remove(&id);
            (f.exports.len() != before)
                .then_some(())
                .ok_or(Error::NotFound("書き出し"))
        })
    }

    fn export_spec<'a>(file: &'a JxcelFile, id: &str) -> Result<jxcel_export::Spec<'a>> {
        let e = file
            .exports
            .iter()
            .find(|e| e.id == id)
            .ok_or(Error::NotFound("書き出し"))?;
        let template = file
            .templates
            .get(id)
            .ok_or(Error::NotFound("テンプレート"))?;
        Ok(jxcel_export::Spec {
            kind: e.kind,
            template,
            sheet: &e.sheet,
            schema: &e.schema,
            filename: &e.filename,
            filter: e.filter.as_deref(),
        })
    }

    /// 書き出す前の確認。テンプレートの差し込み欄と、先頭 `limit` 行の値・出力ファイル名を返す。
    pub fn export_preview(&mut self, id: &str, limit: usize) -> Result<ExportPreview> {
        let d = self.doc()?;
        let spec = Self::export_spec(&d.file, id)?;
        let p = jxcel_export::preview(&d.file, &spec, limit, &jxcel_macro::Options::default())?;
        use jxcel_macro::CellResult;
        let cell = |r: std::result::Result<Value, String>| match r {
            Ok(v) => CellResult::Value(v),
            Err(e) => CellResult::Error(e),
        };
        Ok(ExportPreview {
            placeholders: p.placeholders,
            loops: p
                .loops
                .into_iter()
                .map(|l| ExportPreviewLoop {
                    source: l.source,
                    parent: l.parent,
                    exprs: l.exprs,
                    counts: l
                        .counts
                        .into_iter()
                        .map(|c| cell(c.map(|n| Value::from(n as u64))))
                        .collect(),
                })
                .collect(),
            total_rows: p.total_rows,
            rows: p
                .rows
                .into_iter()
                .map(|r| ExportPreviewRow {
                    row_no: r.row_no,
                    excluded: r.excluded,
                    filename: cell(r.filename.map(Value::String)),
                    values: r.values.into_iter().map(cell).collect(),
                })
                .collect(),
        })
    }

    /// 全行を書き出す。ファイルは `out_dir` に作り、既にあるファイルは上書きしない（番号を付ける）。
    /// ファイルの内容（jxcel ファイル）は変わらない。
    pub fn run_export(&mut self, id: &str, out_dir: &str) -> Result<ExportResult> {
        let d = self.doc()?;
        let spec = Self::export_spec(&d.file, id)?;
        let report = jxcel_export::export(&d.file, &spec, &jxcel_macro::Options::default())?;

        let dir = std::path::Path::new(out_dir);
        std::fs::create_dir_all(dir)?;
        let mut written = vec![];
        for f in report.files {
            let filename = write_new_file(dir, &f.filename, &f.bytes)?;
            written.push(WrittenFile {
                row_no: f.row_no,
                filename,
            });
        }
        Ok(ExportResult {
            out_dir: out_dir.to_string(),
            written,
            skipped: report.skipped,
            errors: report
                .errors
                .into_iter()
                .map(|e| ExportRowError {
                    row_no: e.row_no,
                    message: e.message,
                })
                .collect(),
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

/// 既存のファイルを上書きせずに書く。同名があれば「name (2).ext」のように番号を付ける。
/// `create_new` で作るので、確認と作成の間に同名のファイルができても上書きしない。
fn write_new_file(dir: &std::path::Path, filename: &str, bytes: &[u8]) -> Result<String> {
    use std::io::Write;
    let (stem, ext) = match filename.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (filename.to_string(), String::new()),
    };
    let mut name = filename.to_string();
    for n in 2.. {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(&name))
        {
            Ok(mut f) => {
                f.write_all(bytes)?;
                return Ok(name);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                name = format!("{stem} ({n}){ext}")
            }
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!("番号は尽きない")
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
    use jxcel_macro::CellResult;
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
    fn macros_edit_run_save_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.jxcel");
        let path = path.to_str().unwrap();

        let mut s = Session::new();
        let snap = s.new_file("x").unwrap();
        let (sh, sc, row, col) = ids(&snap);
        s.update_column(&sh, &sc, Column::new(&col, "数量", DataType::Int))
            .unwrap();
        s.set_cell(&sh, &sc, &row, &col, json!(1)).unwrap();
        s.save(Some(path), "初回").unwrap();

        // 追加・編集: テンプレートが入り、編集で未保存になる
        let snap = s.add_macro("集計", None).unwrap();
        let id = snap.file.macros[0].id.clone();
        assert!(snap.file.macros[0].source.contains("export default"));
        let src = r#"export default (jx: Jxcel) => {
            const s = jx.sheet("シート1").schema("データ");
            s.rows().forEach((r: any) => s.update(r._id, { 数量: r.数量 + 10 }));
            jx.log("done");
            return s.rows().length;
        }"#;
        s.update_macro(&id, "加算", src).unwrap();
        s.save(None, "マクロ追加").unwrap();
        // 同じ内容の更新は未保存にしない
        assert!(!s.update_macro(&id, "加算", src).unwrap().dirty);
        assert!(s.update_macro(&id, "加算2", src).unwrap().dirty);
        s.update_macro(&id, "加算", src).unwrap();
        s.save(None, "名前を戻す").unwrap();

        // 実行: 反映されて未保存になり、ログと戻り値が返る
        let out = s.run_macro(&id, None).unwrap();
        assert_eq!(
            (out.ops, out.logs, out.result),
            (1, vec!["done".to_string()], Some(json!(1)))
        );
        assert!(out.snapshot.dirty);
        assert_eq!(
            out.snapshot.file.sheets[0].schemas[0].rows[0].cells[&col],
            json!(11)
        );

        // 保存前のエディタの内容で実行できる。何も書き込まなければ未保存にならない
        s.save(None, "実行結果").unwrap();
        let out = s
            .run_macro(&id, Some("export default (jx: Jxcel) => 42"))
            .unwrap();
        assert_eq!((out.ops, out.result), (0, Some(json!(42))));
        assert!(!out.snapshot.dirty);

        // 失敗したマクロは何も変えない（途中の書き込みも残らない）
        let bad = r#"export default (jx: Jxcel) => {
            const s = jx.sheet("シート1").schema("データ");
            s.update(s.rows()[0]._id, { 数量: 999 });
            s.add({ 数量: "x" });
        }"#;
        assert!(matches!(s.run_macro(&id, Some(bad)), Err(Error::Macro(_))));
        let cur = s.current().unwrap();
        assert_eq!(cur.file.sheets[0].schemas[0].rows[0].cells[&col], json!(11));
        assert!(!cur.dirty);

        // 開き直してもマクロが残り、履歴の差分にも出る
        let mut s2 = Session::new();
        let snap = s2.open(path).unwrap();
        assert_eq!(
            (snap.file.macros[0].name.as_str(), snap.file.macros.len()),
            ("加算", 1)
        );
        let log = s2.history_log().unwrap();
        let d = s2.history_diff(&log[log.len() - 1].id, &log[0].id).unwrap();
        assert!(
            d.iter().any(|c| matches!(c, Change::MacroAdded { .. })),
            "{d:?}"
        );

        // 削除
        s.delete_macro(&id).unwrap();
        assert!(s.current().unwrap().file.macros.is_empty());
        assert!(matches!(s.delete_macro(&id), Err(Error::NotFound(_))));
        assert!(matches!(s.run_macro(&id, None), Err(Error::NotFound(_))));
    }

    #[test]
    fn add_macro_from_a_sample() {
        let mut s = Session::new();
        s.new_file("x").unwrap();
        let samples = Session::macro_samples();
        assert!(samples.len() >= 5);
        let sample = &samples[0];
        let snap = s.add_macro(&sample.name, Some(&sample.source)).unwrap();
        assert_eq!(snap.file.macros[0].name, sample.name);
        assert_eq!(snap.file.macros[0].source, sample.source);
        // 新規ファイルの既定の名前（シート1 / データ）に合わせてあるので、そのまま実行できる
        let id = snap.file.macros[0].id.clone();
        let sample = samples
            .iter()
            .find(|x| x.id == "remove-empty-rows")
            .unwrap();
        s.update_macro(&id, "空行", &sample.source).unwrap();
        let out = s.run_macro(&id, None).unwrap();
        // 新規ファイルの 1 行（空）が消える
        assert_eq!((out.ops, out.result), (1, Some(json!(1))));
    }

    #[test]
    fn computed_columns_recompute_and_are_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.jxcel");
        let path = path.to_str().unwrap();

        let mut s = Session::new();
        let snap = s.new_file("x").unwrap();
        let (sh, sc, row, col) = ids(&snap);
        s.update_column(&sh, &sc, Column::new(&col, "数量", DataType::Int))
            .unwrap();
        s.set_cell(&sh, &sc, &row, &col, json!(3)).unwrap();

        // 計算列を足す。値は式から出て、スナップショットに載る
        let src = "export default (row: any) => row.数量 * 10";
        let snap = s
            .add_column(
                &sh,
                &sc,
                Column::new("dbl", "十倍", DataType::Int).computed(src),
            )
            .unwrap();
        assert_eq!(
            snap.computed[&sc]["dbl"][&row],
            CellResult::Value(json!(30))
        );
        // 値はファイルのモデルには入らない
        assert!(!snap.file.sheets[0].schemas[0].rows[0]
            .cells
            .contains_key("dbl"));

        // 元のセルを編集すると、計算結果も変わる
        let snap = s.set_cell(&sh, &sc, &row, &col, json!(4)).unwrap();
        assert_eq!(
            snap.computed[&sc]["dbl"][&row],
            CellResult::Value(json!(40))
        );
        // 計算列のセルは編集できない（状態も変わらない）
        let e = s.set_cell(&sh, &sc, &row, "dbl", json!(1)).unwrap_err();
        assert!(
            matches!(&e, Error::Invalid(m) if m.contains("計算列")),
            "{e}"
        );
        assert_eq!(
            s.current().unwrap().computed[&sc]["dbl"][&row],
            CellResult::Value(json!(40))
        );

        // 行を足すと、その行の計算結果も出る（数量が空なら 0 になる式）
        let snap = s.add_row(&sh, &sc).unwrap();
        let new_row = snap.file.sheets[0].schemas[0].rows[1].id.clone();
        assert_eq!(
            snap.computed[&sc]["dbl"][&new_row],
            CellResult::Value(json!(0))
        );

        // 保存して開き直しても、式は残り、値は計算し直される
        s.save(Some(path), "計算列").unwrap();
        let mut s2 = Session::new();
        let snap = s2.open(path).unwrap();
        let col = snap.file.sheets[0].schemas[0]
            .columns
            .iter()
            .find(|c| c.id == "dbl")
            .unwrap();
        assert_eq!(col.computed.as_ref().unwrap().source, src);
        assert_eq!(
            snap.computed[&sc]["dbl"][&row],
            CellResult::Value(json!(40))
        );

        // 式を直すと履歴の差分に出て、値は新しい式で計算される
        let new_src = "export default (row: any) => row.数量 * 100";
        let snap = s
            .update_column(
                &sh,
                &sc,
                Column::new("dbl", "十倍", DataType::Int).computed(new_src),
            )
            .unwrap();
        assert_eq!(
            snap.computed[&sc]["dbl"][&row],
            CellResult::Value(json!(400))
        );
        s.save(None, "式を変更").unwrap();
        let log = s.history_log().unwrap();
        let d = s.history_diff(&log[1].id, &log[0].id).unwrap();
        assert!(
            d.iter()
                .any(|c| matches!(c, Change::ColumnChanged { column, .. } if column == "dbl")),
            "{d:?}"
        );

        // 通常の列を計算列に変えると、保存していた値は消える
        let snap = s
            .update_column(
                &sh,
                &sc,
                Column::new(col_id(&snap), "数量", DataType::Int)
                    .computed("export default () => 7"),
            )
            .unwrap();
        assert!(!snap.file.sheets[0].schemas[0].rows[0]
            .cells
            .contains_key(&col_id(&snap)));
        assert_eq!(
            snap.computed[&sc][&col_id(&snap)][&row],
            CellResult::Value(json!(7))
        );
    }

    /// 最初の列（new_file の「列1」）の ID
    fn col_id(snap: &Snapshot) -> String {
        snap.file.sheets[0].schemas[0].columns[0].id.clone()
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

    // ---- 書き出し ----

    fn fixture(name: &str) -> String {
        format!(
            "{}/../jxcel-export/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    /// テンプレートの欄に合わせた列名の「請求」表を持つファイルを、ディスクに書いて返す。
    fn invoice_file(dir: &std::path::Path) -> String {
        let names = [
            "請求番号",
            "取引先",
            "数量",
            "単価",
            "区分",
            "備考",
            "品名",
            "発行日",
            "完了",
        ];
        let types = [
            DataType::String,
            DataType::String,
            DataType::Int,
            DataType::Int,
            DataType::String,
            DataType::String,
            DataType::String,
            DataType::Date,
            DataType::Bool,
        ];
        let mut schema = DataSchema::new(
            "請求",
            names
                .iter()
                .zip(types)
                .map(|(n, t)| Column::new(format!("c{n}"), *n, t))
                .collect(),
        );
        for (i, (no, client, qty)) in [("INV-001", "株式会社A", 3), ("INV-002", "B商事", 10)]
            .iter()
            .enumerate()
        {
            let cells = [
                ("請求番号", json!(no)),
                ("取引先", json!(client)),
                ("数量", json!(qty)),
                ("単価", json!(100)),
                ("区分", json!("A")),
                ("備考", json!("メモ")),
                ("品名", json!("ねじ")),
                ("発行日", json!("2024-01-31")),
                ("完了", json!(true)),
            ]
            .into_iter()
            .map(|(k, v)| (format!("c{k}"), v))
            .collect();
            schema.rows.push(Row {
                id: format!("r{i}"),
                cells,
            });
        }
        let file = JxcelFile {
            name: "請求".into(),
            sheets: vec![Sheet {
                id: "sh".into(),
                name: "請求".into(),
                schemas: vec![schema],
            }],
            ..Default::default()
        };
        let path = dir.join("invoices.jxcel");
        std::fs::write(&path, file.to_zip().unwrap()).unwrap();
        path.to_str().unwrap().to_string()
    }

    /// 明細（配列の列）を持つ「請求」表のファイルを、ディスクに書いて返す。
    fn loop_file(dir: &std::path::Path) -> String {
        let lines = DataType::Array {
            item: Box::new(DataType::Object {
                fields: vec![
                    Column::new("f1", "品目", DataType::String),
                    Column::new("f2", "数", DataType::Int),
                ],
            }),
        };
        let mut schema = DataSchema::new(
            "請求",
            vec![
                Column::new("c1", "取引先", DataType::String),
                Column::new("c2", "合計", DataType::Int),
                Column::new("c3", "単価", DataType::Int),
                Column::new("c4", "請求番号", DataType::String),
                Column::new("c5", "発行日", DataType::Date),
                Column::new("c6", "明細", lines),
            ],
        );
        for (i, (client, lines)) in [
            (
                "A社",
                json!([{"f1": "ねじ", "f2": 2}, {"f1": "ナット", "f2": 3}]),
            ),
            ("B社", json!([])),
        ]
        .into_iter()
        .enumerate()
        {
            schema.rows.push(Row {
                id: format!("r{i}"),
                cells: [
                    ("c1", json!(client)),
                    ("c2", json!(500)),
                    ("c3", json!(100)),
                    ("c4", json!(format!("INV-{i}"))),
                    ("c5", json!("2026-10-02")),
                    ("c6", lines),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            });
        }
        let file = JxcelFile {
            name: "請求".into(),
            sheets: vec![Sheet {
                id: "sh".into(),
                name: "請求".into(),
                schemas: vec![schema],
            }],
            ..Default::default()
        };
        let path = dir.join("loops.jxcel");
        std::fs::write(&path, file.to_zip().unwrap()).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn convert_datetime_offsets_in_bulk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conv.jxcel").to_str().unwrap().to_string();
        let mut s = Session::new();
        let snap = s.new_file("t").unwrap();
        let sheet = snap.file.sheets[0].id.clone();
        let snap = s
            .add_schema(
                &sheet,
                "受付",
                vec![
                    Column::new("at", "日時", DataType::DateTime),
                    Column::new("name", "名前", DataType::String),
                    Column::new("calc", "計算", DataType::DateTime)
                        .computed("export default () => null"),
                ],
            )
            .unwrap();
        let schema = snap.file.sheets[0].schemas.last().unwrap().id.clone();
        for (i, v) in [
            "2026-10-02T01:30:00Z",
            "2026-10-02T10:30:00+09:00",
            "2026-10-02T23:59:60Z",
        ]
        .iter()
        .enumerate()
        {
            let row = s.add_row(&sheet, &schema).unwrap().file.sheets[0]
                .schemas
                .last()
                .unwrap()
                .rows[i]
                .id
                .clone();
            // うるう秒は検証を通るので入る
            s.set_cell(&sheet, &schema, &row, "at", json!(v)).unwrap();
        }
        s.add_row(&sheet, &schema).unwrap(); // 値のない行は対象外
        s.save(Some(path.as_str()), "元").unwrap();
        assert!(!s.current().unwrap().dirty);

        let r = s
            .convert_datetime_offset(&sheet, &schema, "at", "+09:00")
            .unwrap();
        assert_eq!((r.converted, r.unchanged, r.skipped), (1, 1, 1));
        let rows = &r.snapshot.file.sheets[0].schemas.last().unwrap().rows;
        assert_eq!(rows[0].cells["at"], json!("2026-10-02T10:30:00+09:00"));
        assert_eq!(rows[1].cells["at"], json!("2026-10-02T10:30:00+09:00"));
        assert_eq!(rows[2].cells["at"], json!("2026-10-02T23:59:60Z")); // 変換できないものはそのまま
        assert!(!rows[3].cells.contains_key("at"));
        assert!(r.snapshot.dirty);

        // もう一度やっても変わらず、未保存にもならない
        s.save(Some(path.as_str()), "変換後").unwrap();
        let again = s
            .convert_datetime_offset(&sheet, &schema, "at", "+09:00")
            .unwrap();
        assert_eq!((again.converted, again.unchanged, again.skipped), (0, 2, 1));
        assert!(!again.snapshot.dirty);

        // Z に戻せる（履歴にも残る）
        let back = s
            .convert_datetime_offset(&sheet, &schema, "at", "z")
            .unwrap();
        assert_eq!(
            back.snapshot.file.sheets[0].schemas.last().unwrap().rows[0].cells["at"],
            json!("2026-10-02T01:30:00Z")
        );

        // 誤りは拒否し、何も変えない
        for (col, off) in [
            ("at", "JST"),
            ("at", "+9:00"),
            ("name", "+09:00"),
            ("calc", "+09:00"),
            ("nope", "+09:00"),
        ] {
            assert!(
                s.convert_datetime_offset(&sheet, &schema, col, off)
                    .is_err(),
                "{col} {off}"
            );
        }
    }

    #[test]
    fn export_row_loops_repeat_rows_over_a_nested_column() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let mut s = Session::new();
        let snap = s.open(&loop_file(dir.path())).unwrap();
        let schema = snap.file.sheets[0].schemas[0].id.clone();

        for (template, ext) in [("loop.docx", "docx"), ("loop.xlsx", "xlsx")] {
            let snap = s.add_export(&fixture(template), None).unwrap();
            let id = snap.file.exports.last().unwrap().id.clone();
            s.update_export(&id, "請求書", "sh", &schema, "{{取引先}}", None)
                .unwrap();

            // プレビュー: ループの対象・中の欄・行ごとの件数が分かる
            let p = s.export_preview(&id, 5).unwrap();
            assert_eq!(p.loops.len(), 1, "{ext}");
            assert_eq!(p.loops[0].source, "明細");
            assert!(p.loops[0].exprs.iter().any(|e| e == "品目"));
            assert_eq!(
                p.loops[0].counts,
                [
                    jxcel_macro::CellResult::Value(json!(2)),
                    jxcel_macro::CellResult::Value(json!(0))
                ]
            );

            let r = s.run_export(&id, out.to_str().unwrap()).unwrap();
            assert_eq!((r.written.len(), r.errors.len()), (2, 0), "{ext}: {r:?}");
            let bytes = std::fs::read(out.join(format!("A社.{ext}"))).unwrap();
            let pkg = jxcel_export::package::Package::read(&bytes).unwrap();
            let part = if ext == "docx" {
                "word/document.xml"
            } else {
                "xl/worksheets/sheet1.xml"
            };
            let xml = String::from_utf8(pkg.get(part).unwrap().to_vec()).unwrap();
            // 配列の列のフィールドが、ID ではなく名前で見える
            assert!(xml.contains("ねじ") && xml.contains("ナット"), "{ext}");
            assert!(!xml.contains("{{"), "{ext}: 差し込み欄が残っている");
            s.delete_export(&id).unwrap();
        }
    }

    #[test]
    fn export_nested_row_loops_over_nested_array_columns() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        // 明細（配列）の各要素が、付属（配列）を持つ
        let parts = DataType::Array {
            item: Box::new(DataType::Object {
                fields: vec![Column::new("g1", "名", DataType::String)],
            }),
        };
        let lines = DataType::Array {
            item: Box::new(DataType::Object {
                fields: vec![
                    Column::new("f1", "品目", DataType::String),
                    Column::new("f2", "数", DataType::Int),
                    Column::new("f3", "付属", parts),
                ],
            }),
        };
        let mut schema = DataSchema::new(
            "請求",
            vec![
                Column::new("c1", "請求番号", DataType::String),
                Column::new("c6", "明細", lines),
            ],
        );
        schema.rows.push(Row {
            id: "r0".into(),
            cells: [
                ("c1", json!("INV-7")),
                (
                    "c6",
                    json!([
                        {"f1": "ねじ", "f2": 3, "f3": [{"g1": "a"}, {"g1": "b"}]},
                        {"f1": "板", "f2": 1, "f3": []},
                    ]),
                ),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        });
        let f = JxcelFile {
            name: "入れ子".into(),
            sheets: vec![Sheet {
                id: "sh".into(),
                name: "請求書".into(),
                schemas: vec![schema],
            }],
            ..Default::default()
        };
        let path = dir.path().join("n.jxcel");
        std::fs::write(&path, f.to_zip().unwrap()).unwrap();

        let mut s = Session::new();
        let snap = s.open(path.to_str().unwrap()).unwrap();
        let schema_id = snap.file.sheets[0].schemas[0].id.clone();
        for (template, ext) in [("nested.docx", "docx"), ("nested.xlsx", "xlsx")] {
            let snap = s.add_export(&fixture(template), None).unwrap();
            let id = snap.file.exports.last().unwrap().id.clone();
            s.update_export(&id, "入れ子", "sh", &schema_id, "{{請求番号}}", None)
                .unwrap();
            let p = s.export_preview(&id, 5).unwrap();
            assert_eq!(p.loops.len(), 2, "{ext}");
            assert_eq!((p.loops[0].parent, p.loops[1].parent), (None, Some(0)));
            // 明細は 2 件、付属は（ねじの 2 件 + 板の 0 件）の合計 2 件
            assert_eq!(
                p.loops[0].counts,
                [jxcel_macro::CellResult::Value(json!(2))]
            );
            assert_eq!(
                p.loops[1].counts,
                [jxcel_macro::CellResult::Value(json!(2))]
            );

            let r = s.run_export(&id, out.to_str().unwrap()).unwrap();
            assert_eq!((r.written.len(), r.errors.len()), (1, 0), "{ext}: {r:?}");
            let bytes = std::fs::read(out.join(format!("INV-7.{ext}"))).unwrap();
            let pkg = jxcel_export::package::Package::read(&bytes).unwrap();
            let part = if ext == "docx" {
                "word/document.xml"
            } else {
                "xl/worksheets/sheet1.xml"
            };
            let xml = String::from_utf8(pkg.get(part).unwrap().to_vec()).unwrap();
            assert!(!xml.contains("{{"), "{ext}: 差し込み欄が残っている");
            // 内側のループが、外側の要素の付属を回る（ねじの a・b。板は付属なし）
            for needle in ["ねじ", "板", ">a<", ">b<"] {
                assert!(xml.contains(needle), "{ext}: {needle}");
            }
            s.delete_export(&id).unwrap();
        }
    }

    #[test]
    fn export_import_preview_run_save_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let file = invoice_file(dir.path());
        let mut s = Session::new();
        let snap = s.open(&file).unwrap();
        assert!(!snap.dirty && snap.file.exports.is_empty());
        // 履歴のないファイルは、最初の保存から履歴が始まる。取り込み前の状態を 1 件目にしておく
        s.save(None, "取り込み前").unwrap();

        // テンプレートを取り込む: 先頭の表が対象、ファイル名は先頭の列、未保存になる
        let snap = s.add_export(&fixture("invoice.docx"), None).unwrap();
        assert!(snap.dirty);
        let e = &snap.file.exports[0];
        assert_eq!(
            (
                e.name.as_str(),
                e.template_name.as_str(),
                e.filename.as_str()
            ),
            ("invoice", "invoice.docx", "{{請求番号}}")
        );
        assert_eq!((e.sheet.as_str(), e.kind), ("sh", TemplateKind::Docx));
        let id = e.id.clone();

        // プレビュー: 差し込み欄と、先頭行の値
        let p = s.export_preview(&id, 5).unwrap();
        assert!(p.placeholders.iter().any(|x| x == "数量 * 単価"));
        assert_eq!(p.total_rows, 2);
        let i = p.placeholders.iter().position(|x| x == "取引先").unwrap();
        assert_eq!(
            p.rows[0].values[i],
            jxcel_macro::CellResult::Value(json!("株式会社A"))
        );
        assert_eq!(
            p.rows[0].filename,
            jxcel_macro::CellResult::Value(json!("INV-001.docx"))
        );

        // 設定を直して、全行を書き出す
        s.update_export(
            &id,
            "請求書",
            "sh",
            &snap.file.sheets[0].schemas[0].id,
            "{{請求番号}}_{{取引先}}",
            Some("数量 > 5"),
        )
        .unwrap();
        let r = s.run_export(&id, out.to_str().unwrap()).unwrap();
        assert_eq!(
            (r.written.len(), r.skipped, r.errors.len()),
            (1, 1, 0),
            "{r:?}"
        );
        assert_eq!(r.written[0].filename, "INV-002_B商事.docx");
        assert!(out.join("INV-002_B商事.docx").is_file());
        // 書き出しは jxcel ファイルの内容を変えない
        assert!(s.current().unwrap().dirty); // 取り込みと設定変更で元々未保存。書き出しで増えも減りもしない

        // もう一度書き出しても、既存のファイルは上書きしない（番号が付く）
        let before = std::fs::read(out.join("INV-002_B商事.docx")).unwrap();
        let r2 = s.run_export(&id, out.to_str().unwrap()).unwrap();
        assert_eq!(r2.written[0].filename, "INV-002_B商事 (2).docx");
        assert_eq!(
            std::fs::read(out.join("INV-002_B商事.docx")).unwrap(),
            before
        );

        // 保存して開き直しても、設定とテンプレートが残り、そのまま書き出せる
        s.save(None, "書き出し設定").unwrap();
        let mut s2 = Session::new();
        let snap = s2.open(&file).unwrap();
        assert_eq!(snap.file.exports.len(), 1);
        let out2 = dir.path().join("out2");
        let r3 = s2.run_export(&id, out2.to_str().unwrap()).unwrap();
        assert_eq!(r3.written.len(), 1);
        assert_eq!(
            std::fs::read(out2.join("INV-002_B商事.docx")).unwrap(),
            before
        );

        // 履歴の差分に、書き出しの追加が出る
        let log = s2.history_log().unwrap();
        assert_eq!(log.len(), 2);
        let d = s2.history_diff(&log[1].id, &log[0].id).unwrap();
        assert!(
            d.iter().any(|c| matches!(c, Change::ExportAdded { .. })),
            "{d:?}"
        );

        // 削除
        let snap = s2.delete_export(&id).unwrap();
        assert!(snap.file.exports.is_empty() && snap.file.templates.is_empty());
        assert!(matches!(s2.delete_export(&id), Err(Error::NotFound(_))));
        assert!(matches!(
            s2.run_export(&id, out2.to_str().unwrap()),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn export_template_validation_and_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let file = invoice_file(dir.path());
        let mut s = Session::new();
        s.open(&file).unwrap();

        // 拡張子が違う・中身が違うファイルは取り込めない（状態は変わらない）
        let txt = dir.path().join("t.txt");
        std::fs::write(&txt, "x").unwrap();
        assert!(
            matches!(s.add_export(txt.to_str().unwrap(), None), Err(Error::Invalid(m)) if m.contains(".docx"))
        );
        let fake = dir.path().join("fake.docx");
        std::fs::write(&fake, "zipではない").unwrap();
        assert!(matches!(
            s.add_export(fake.to_str().unwrap(), None),
            Err(Error::Export(_))
        ));
        // docx の中身を xlsx と名乗らせても弾く
        let wrong = dir.path().join("wrong.xlsx");
        std::fs::copy(fixture("invoice.docx"), &wrong).unwrap();
        assert!(matches!(
            s.add_export(wrong.to_str().unwrap(), None),
            Err(Error::Export(_))
        ));
        assert!(!s.current().unwrap().dirty);
        assert!(s.current().unwrap().file.exports.is_empty());

        // 差し替え: 同じ種類だけ
        let snap = s
            .add_export(&fixture("invoice.docx"), Some("請求書"))
            .unwrap();
        let id = snap.file.exports[0].id.clone();
        assert!(
            matches!(s.replace_export_template(&id, &fixture("invoice.xlsx")), Err(Error::Invalid(m)) if m.contains(".docx"))
        );
        let snap = s
            .replace_export_template(&id, &fixture("invoice.docx"))
            .unwrap();
        assert_eq!(snap.file.exports[0].name, "請求書");
        // 存在しない表を対象にはできない
        assert!(matches!(
            s.update_export(&id, "x", "sh", "nope", "{{_no}}", None),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn export_xlsx_template_writes_numeric_cells() {
        let dir = tempfile::tempdir().unwrap();
        let file = invoice_file(dir.path());
        let mut s = Session::new();
        s.open(&file).unwrap();
        let snap = s.add_export(&fixture("invoice.xlsx"), None).unwrap();
        let id = snap.file.exports[0].id.clone();
        let r = s
            .run_export(&id, dir.path().join("o").to_str().unwrap())
            .unwrap();
        assert_eq!(
            r.written
                .iter()
                .map(|w| w.filename.as_str())
                .collect::<Vec<_>>(),
            ["INV-001.xlsx", "INV-002.xlsx"]
        );
        assert!(dir.path().join("o/INV-001.xlsx").is_file());
    }
}
