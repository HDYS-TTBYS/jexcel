//! TS マクロの変換と実行。
//!
//! マクロは `export default function (jx) { ... }` の形の TypeScript モジュール。
//! 型は剥がすだけ（型検査はエディタ側）で JS にして、QuickJS で実行する。
//! 実行環境にはファイルもネットワークもなく、メモリと時間に上限がある。
//!
//! マクロは「ファイルのコピー」に対して動き、書き込みは操作ログとして記録される。
//! 実行後にそのログを検証付きで本体に再生するので、失敗したマクロは何も残さない。

use jxcel_core::types::TypeRegistry;
use jxcel_core::{new_id, JxcelFile, Row};
use rquickjs::{CatchResultExt, Context, Function, Module, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use thiserror::Error;

pub mod samples;

const PRELUDE: &str = include_str!("prelude.js");
/// 標準ライブラリ `std`。マクロにも計算列の式にも、グローバルとして見える。
const STD: &str = include_str!("std.js");

#[derive(Debug, Error)]
pub enum Error {
    #[error("構文エラー:\n{0}")]
    Syntax(String),
    #[error("実行エラー: {0}")]
    Runtime(String),
    #[error("実行時間が上限（{0:?}）を超えました")]
    Timeout(Duration),
    #[error("マクロの結果を適用できません: {0}")]
    Apply(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct Options {
    pub timeout: Duration,
    pub memory_limit: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            memory_limit: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MacroRun {
    /// マクロ適用後のファイル。何も書き込まなければ元と同じ。
    pub file: JxcelFile,
    pub logs: Vec<String>,
    /// `default` 関数の戻り値（JSON で表せるもの）
    pub result: Option<Value>,
    /// 書き込み操作の数
    pub ops: usize,
}

/// TypeScript を JS（ES モジュール）に変換する。型を剥がすだけで、型検査はしない。
pub fn transpile(source: &str) -> Result<String> {
    use oxc_allocator::Allocator;
    use oxc_codegen::Codegen;
    use oxc_parser::Parser;
    use oxc_semantic::SemanticBuilder;
    use oxc_span::SourceType;
    use oxc_transformer::{TransformOptions, Transformer};
    use std::path::Path;

    let allocator = Allocator::default();
    let path = Path::new("macro.ts");
    let source_type = SourceType::ts().with_module(true);

    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(Error::Syntax(render(parsed.diagnostics, source)));
    }
    let mut program = parsed.program;

    let semantic = SemanticBuilder::new().build(&program);
    if !semantic.diagnostics.is_empty() {
        return Err(Error::Syntax(render(semantic.diagnostics, source)));
    }
    let scoping = semantic.semantic.into_scoping();

    let transformed = Transformer::new(&allocator, path, &TransformOptions::default())
        .build_with_scoping(scoping, &mut program);
    if !transformed.diagnostics.is_empty() {
        return Err(Error::Syntax(render(transformed.diagnostics, source)));
    }
    Ok(Codegen::new().build(&program).code)
}

fn render(
    diagnostics: impl IntoIterator<Item = oxc_diagnostics::OxcDiagnostic>,
    source: &str,
) -> String {
    diagnostics
        .into_iter()
        .map(|d| d.with_source_code(source.to_string()).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Deserialize)]
struct Output {
    ops: Vec<Op>,
    logs: Vec<String>,
    result: Value,
    #[serde(default)]
    computed: Vec<RawCell>,
    /// `evaluate_exprs` の結果（行ごと）
    #[serde(default)]
    exprs: Vec<RawRowExprs>,
}

#[derive(Deserialize)]
struct RawRowExprs {
    top: Vec<RawExpr>,
    loops: Vec<RawLoop>,
}

#[derive(Deserialize)]
struct RawLoop {
    e: Option<String>,
    items: Option<Vec<Vec<RawExpr>>>,
}

#[derive(Deserialize)]
struct RawExpr {
    v: Option<Value>,
    e: Option<String>,
}

/// JS 側から返る、計算列の 1 セル分の結果。
#[derive(Deserialize)]
struct RawCell {
    schema: String,
    column: String,
    row: String,
    v: Option<Value>,
    e: Option<String>,
}

/// 計算列の 1 セルの結果。値か、そのセルだけのエラー。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum CellResult {
    #[serde(rename = "v")]
    Value(Value),
    #[serde(rename = "e")]
    Error(String),
}

/// スキーマ ID → 列 ID → 行 ID → 結果。
pub type ComputedValues = BTreeMap<String, BTreeMap<String, BTreeMap<String, CellResult>>>;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Op {
    Add {
        sheet: String,
        schema: String,
        row: String,
        cells: BTreeMap<String, Value>,
    },
    Update {
        sheet: String,
        schema: String,
        row: String,
        cells: BTreeMap<String, Value>,
    },
    Remove {
        sheet: String,
        schema: String,
        row: String,
    },
}

/// マクロを実行し、結果を検証したうえで適用後のファイルを返す。
/// 計算列があれば先に評価するので、マクロからは計算列の値も読める（実行開始時点の値）。
pub fn run(source: &str, file: &JxcelFile, opts: &Options) -> Result<MacroRun> {
    let js = transpile(source)?;
    let output = execute(file, Some(&js), None, opts)?;
    apply(file, output)
}

/// 計算列を評価する。値は保存しないので、読み込むたびに呼ぶ。
///
/// 計算列のないファイルでは何も実行しない。式の誤りや型違いは、そのセルのエラーになる
/// （他のセルは計算を続ける）。実行基盤の失敗（時間切れなど）は、全ての計算セルのエラーにする。
pub fn compute(file: &JxcelFile, opts: &Options) -> ComputedValues {
    let formulas = collect_formulas(file);
    if formulas.is_empty() {
        return ComputedValues::new();
    }
    match execute(file, None, None, opts) {
        Ok(output) => into_values(file, output.computed),
        Err(e) => {
            let message = e.to_string();
            let mut out = ComputedValues::new();
            for f in &formulas {
                let cells = out
                    .entry(f.schema.clone())
                    .or_default()
                    .entry(f.column.clone())
                    .or_default();
                for row in file_rows(file, &f.sheet, &f.schema) {
                    cells.insert(row, CellResult::Error(message.clone()));
                }
            }
            out
        }
    }
}

fn file_rows(file: &JxcelFile, sheet: &str, schema: &str) -> Vec<String> {
    file.sheets
        .iter()
        .find(|s| s.id == sheet)
        .and_then(|s| s.schemas.iter().find(|s| s.id == schema))
        .map(|s| s.rows.iter().map(|r| r.id.clone()).collect())
        .unwrap_or_default()
}

/// 結果を列の型で検査する。合わなければそのセルのエラーにする。
fn into_values(file: &JxcelFile, cells: Vec<RawCell>) -> ComputedValues {
    let registry = TypeRegistry::default();
    let mut out = ComputedValues::new();
    for c in cells {
        let column = file
            .sheets
            .iter()
            .flat_map(|s| &s.schemas)
            .find(|s| s.id == c.schema)
            .and_then(|s| s.columns.iter().find(|x| x.id == c.column));
        let result = match (c.e, c.v, column) {
            (Some(e), _, _) => CellResult::Error(e),
            (None, Some(v), Some(col)) => match col.ty.validate(&v, &registry) {
                Ok(()) => CellResult::Value(v),
                Err(why) => CellResult::Error(format!("{}: {why}", col.name)),
            },
            (None, v, _) => CellResult::Value(v.unwrap_or(Value::Null)),
        };
        out.entry(c.schema)
            .or_default()
            .entry(c.column)
            .or_default()
            .insert(c.row, result);
    }
    out
}

struct Formula {
    sheet: String,
    schema: String,
    column: String,
    /// 変換後の JS。構文エラーなら、そのメッセージ。
    js: std::result::Result<String, String>,
}

/// ファイル内の計算列を、シート・スキーマ・列の並び順で集める（評価もこの順）。
fn collect_formulas(file: &JxcelFile) -> Vec<Formula> {
    let mut out = vec![];
    for sheet in &file.sheets {
        for schema in &sheet.schemas {
            for col in &schema.columns {
                let Some(comp) = &col.computed else { continue };
                let js = if comp.source.trim().is_empty() {
                    Err("式が空です".to_string())
                } else {
                    transpile(&comp.source).map_err(|e| e.to_string())
                };
                out.push(Formula {
                    sheet: sheet.id.clone(),
                    schema: schema.id.clone(),
                    column: col.id.clone(),
                    js,
                });
            }
        }
    }
    out
}

/// 差し込み欄の式を評価する依頼。
struct ExprRequest<'a> {
    sheet: &'a str,
    schema: &'a str,
    exprs: &'a [String],
    loops: &'a [LoopRequest],
}

/// 行ループ `{{#each 式}}` の依頼。`source` が配列になる式、`exprs` がループの中の式。
#[derive(Debug, Clone, PartialEq)]
pub struct LoopRequest {
    pub source: String,
    pub exprs: Vec<String>,
}

/// 1 つの行ループの評価結果。要素ごとに、`LoopRequest::exprs` と同じ順の結果が並ぶ。
pub type LoopResult = std::result::Result<Vec<Vec<CellResult>>, String>;

/// 表の 1 行分の評価結果。
#[derive(Debug, Clone, PartialEq)]
pub struct RowEval {
    /// 通常の式（`evaluate_exprs` の 1 行と同じ）
    pub top: Vec<CellResult>,
    /// 行ループごとの結果（`loops` と同じ順）
    pub loops: Vec<LoopResult>,
}

fn cell_of(c: RawExpr) -> CellResult {
    match (c.e, c.v) {
        (Some(e), _) => CellResult::Error(e),
        (None, v) => CellResult::Value(v.unwrap_or(Value::Null)),
    }
}

/// テンプレートの差し込み欄 `{{ 式 }}` を、表の全行について評価する（行の並び順 × 式の順）。
///
/// 式は列名をそのまま変数として使える（`{{数量 * 単価}}`）。`row`・`jx`（読み取り専用）・`std` も使える。
/// `_no`（1 から始まる行番号）と `_id`（行 ID）も使える。式の誤りや例外は、その行・その式だけのエラー。
/// 計算列は先に評価されるので、その値も使える。
pub fn evaluate_exprs(
    file: &JxcelFile,
    sheet_id: &str,
    schema_id: &str,
    exprs: &[String],
    opts: &Options,
) -> Result<Vec<Vec<CellResult>>> {
    Ok(
        evaluate_template(file, sheet_id, schema_id, exprs, &[], opts)?
            .into_iter()
            .map(|r| r.top)
            .collect(),
    )
}

/// `evaluate_exprs` に、行ループ（配列を評価し、要素ごとに式を評価する）を加えたもの。
///
/// ループの中の式では、要素がオブジェクトならそのキーが変数になり（行の列より優先）、
/// `_item`（要素）・`_i`（0 から）・`_n`（1 から）も使える。対象の式が `null` なら要素なし、
/// 配列以外はその行・そのループのエラー。
pub fn evaluate_template(
    file: &JxcelFile,
    sheet_id: &str,
    schema_id: &str,
    exprs: &[String],
    loops: &[LoopRequest],
    opts: &Options,
) -> Result<Vec<RowEval>> {
    let req = ExprRequest {
        sheet: sheet_id,
        schema: schema_id,
        exprs,
        loops,
    };
    let output = execute(file, None, Some(&req), opts)?;
    Ok(output
        .exprs
        .into_iter()
        .map(|row| RowEval {
            top: row.top.into_iter().map(cell_of).collect(),
            loops: row
                .loops
                .into_iter()
                .map(|l| match (l.e, l.items) {
                    (Some(e), _) => Err(e),
                    (None, items) => Ok(items
                        .unwrap_or_default()
                        .into_iter()
                        .map(|it| it.into_iter().map(cell_of).collect())
                        .collect()),
                })
                .collect(),
        })
        .collect())
}

fn execute(
    file: &JxcelFile,
    main_js: Option<&str>,
    exprs: Option<&ExprRequest>,
    opts: &Options,
) -> Result<Output> {
    let data = serde_json::to_string(file).expect("serialize");
    let formulas = collect_formulas(file);

    let rt = Runtime::new().map_err(|e| Error::Runtime(e.to_string()))?;
    rt.set_memory_limit(opts.memory_limit);
    rt.set_max_stack_size(512 * 1024);
    let deadline = Instant::now() + opts.timeout;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let context = Context::full(&rt).map_err(|e| Error::Runtime(e.to_string()))?;

    let outcome = context.with(|ctx| -> std::result::Result<String, String> {
        let globals = ctx.globals();
        globals
            .set(
                "__newId",
                Function::new(ctx.clone(), new_id).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        globals.set("__data", data).map_err(|e| e.to_string())?;
        ctx.eval::<(), _>(PRELUDE)
            .catch(&ctx)
            .map_err(|e| e.to_string())?;
        ctx.eval::<(), _>(STD)
            .catch(&ctx)
            .map_err(|e| e.to_string())?;
        ctx.eval::<(), _>(
            "globalThis.__state = __makeState(JSON.parse(__data)); globalThis.__fns = [];",
        )
        .catch(&ctx)
        .map_err(|e| e.to_string())?;

        // 計算式: 1 列 1 モジュールとして読み込み、default の関数を __fns[i] に置く。
        // 構文エラーなどはその列のエラーとして JS 側へ渡す（他の列は計算する）。
        if !formulas.is_empty() {
            let fns: rquickjs::Array = globals.get("__fns").map_err(|e| e.to_string())?;
            let mut specs = vec![];
            for (i, f) in formulas.iter().enumerate() {
                let loaded = f.js.clone().and_then(|js| {
                    let (module, promise) = Module::declare(ctx.clone(), format!("formula{i}"), js)
                        .catch(&ctx)
                        .map_err(|e| e.to_string())?
                        .eval()
                        .catch(&ctx)
                        .map_err(|e| e.to_string())?;
                    promise
                        .finish::<()>()
                        .catch(&ctx)
                        .map_err(|e| e.to_string())?;
                    let func: Function = module.get("default").map_err(|_| {
                        "export default function (row, jx) { return ... } の形で書いてください"
                            .to_string()
                    })?;
                    fns.set(i, func).map_err(|e| e.to_string())
                });
                let mut spec =
                    serde_json::json!({ "sheet": f.sheet, "schema": f.schema, "column": f.column });
                if let Err(message) = loaded {
                    spec["error"] = Value::String(message);
                }
                specs.push(spec);
            }
            let specs = serde_json::to_string(&specs).expect("serialize");
            globals.set("__specs", specs).map_err(|e| e.to_string())?;
            ctx.eval::<(), _>("globalThis.__state.computeAll(JSON.parse(__specs));")
                .catch(&ctx)
                .map_err(|e| e.to_string())?;
        }

        // 差し込み欄の式（計算列の後に評価するので、計算列の値も使える）
        if let Some(req) = exprs {
            let loops: Vec<Value> = req
                .loops
                .iter()
                .map(|l| serde_json::json!({ "source": l.source, "exprs": l.exprs }))
                .collect();
            let spec = serde_json::json!({ "sheet": req.sheet, "schemaId": req.schema, "exprs": req.exprs, "loops": loops });
            globals
                .set("__exprspec", serde_json::to_string(&spec).expect("serialize"))
                .map_err(|e| e.to_string())?;
            ctx.eval::<(), _>("globalThis.__state.evalExprs(JSON.parse(__exprspec));")
                .catch(&ctx)
                .map_err(|e| e.to_string())?;
        }

        // マクロ本体（計算列だけを評価する場合は無い）
        let result: rquickjs::Value = match main_js {
            None => rquickjs::Value::new_undefined(ctx.clone()),
            Some(js) => {
                let (module, promise) = Module::declare(ctx.clone(), "macro", js)
                    .catch(&ctx)
                    .map_err(|e| e.to_string())?
                    .eval()
                    .catch(&ctx)
                    .map_err(|e| e.to_string())?;
                promise
                    .finish::<()>()
                    .catch(&ctx)
                    .map_err(|e| e.to_string())?;
                let main: Function = module.get("default").map_err(|_| {
                    "export default function (jx) { ... } の形で書いてください".to_string()
                })?;
                let jx: rquickjs::Value = ctx
                    .eval("globalThis.__state.jx")
                    .catch(&ctx)
                    .map_err(|e| e.to_string())?;
                main.call((jx,)).catch(&ctx).map_err(|e| e.to_string())?
            }
        };

        // 結果は JSON で受け渡す（undefined は null になる）
        globals.set("__result", result).map_err(|e| e.to_string())?;
        ctx.eval::<String, _>("globalThis.__state.finish(globalThis.__result)")
            .catch(&ctx)
            .map_err(|e| e.to_string())
    });

    match outcome {
        Ok(json) => serde_json::from_str(&json).map_err(|e| Error::Runtime(e.to_string())),
        // 割り込み（時間切れ）は JS 例外としては報告されないので、時刻で判定する
        Err(_) if Instant::now() > deadline => Err(Error::Timeout(opts.timeout)),
        Err(msg) => Err(Error::Runtime(msg)),
    }
}

fn apply(original: &JxcelFile, output: Output) -> Result<MacroRun> {
    let mut file = original.clone();
    let ops = output.ops.len();
    let mut touched: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();

    for op in output.ops {
        let (sheet, schema, row_id) = match &op {
            Op::Add {
                sheet, schema, row, ..
            }
            | Op::Update {
                sheet, schema, row, ..
            }
            | Op::Remove { sheet, schema, row } => (sheet, schema, row),
        };
        let target = file
            .sheets
            .iter_mut()
            .find(|s| &s.id == sheet)
            .and_then(|s| s.schemas.iter_mut().find(|s| &s.id == schema))
            .ok_or_else(|| Error::Apply("対象のスキーマが見つかりません".into()))?;
        match &op {
            Op::Add { cells, .. } => target.rows.push(Row {
                id: row_id.clone(),
                cells: cells
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            }),
            Op::Update { cells, .. } => {
                let row = target
                    .rows
                    .iter_mut()
                    .find(|r| &r.id == row_id)
                    .ok_or_else(|| Error::Apply(format!("行 {row_id} が見つかりません")))?;
                for (k, v) in cells {
                    if v.is_null() {
                        row.cells.remove(k);
                    } else {
                        row.cells.insert(k.clone(), v.clone());
                    }
                }
            }
            Op::Remove { .. } => target.rows.retain(|r| &r.id != row_id),
        }
        touched
            .entry((sheet.clone(), schema.clone()))
            .or_default()
            .insert(row_id.clone());
    }

    // 触った行だけを検証する（マクロと無関係な既存の行で失敗させない）
    let registry = TypeRegistry::default();
    for ((sheet, schema), rows) in touched {
        let target = file
            .sheets
            .iter()
            .find(|s| s.id == sheet)
            .and_then(|s| s.schemas.iter().find(|s| s.id == schema))
            .expect("checked above");
        if let Some(v) = target
            .validate(&registry)
            .into_iter()
            .find(|v| rows.contains(&v.row_id))
        {
            let col = target
                .columns
                .iter()
                .find(|c| c.id == v.column_id)
                .map_or(v.column_id.as_str(), |c| c.name.as_str());
            return Err(Error::Apply(format!(
                "{}「{}」: {}",
                target.name, col, v.message
            )));
        }
    }

    Ok(MacroRun {
        file,
        logs: output.logs,
        result: (!output.result.is_null()).then_some(output.result),
        ops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jxcel_core::{Column, DataSchema, DataType, Sheet};
    use serde_json::json;
    use std::collections::BTreeMap;

    /// 「倉庫」シートの「在庫」スキーマ（品名: 文字列・必須、数量: 整数）に 2 行
    fn sample() -> JxcelFile {
        let mut schema = DataSchema::new(
            "在庫",
            vec![
                Column::new("name", "品名", DataType::String).required(),
                Column::new("qty", "数量", DataType::Int),
            ],
        );
        schema.id = "s1".into();
        for (id, name, qty) in [("r1", "ねじ", 10), ("r2", "ナット", 5)] {
            schema.rows.push(Row {
                id: id.into(),
                cells: [
                    ("name".to_string(), json!(name)),
                    ("qty".to_string(), json!(qty)),
                ]
                .into(),
            });
        }
        JxcelFile {
            name: "台帳".into(),
            sheets: vec![Sheet {
                id: "sh1".into(),
                name: "倉庫".into(),
                schemas: vec![schema],
            }],
            macros: Default::default(),
            exports: Default::default(),
            forms: Default::default(),
            templates: Default::default(),
        }
    }

    fn run_ok(src: &str) -> MacroRun {
        run(src, &sample(), &Options::default()).unwrap_or_else(|e| panic!("{e}"))
    }

    fn run_err(src: &str) -> Error {
        run(src, &sample(), &Options::default()).unwrap_err()
    }

    #[test]
    fn transpile_strips_types() {
        let js = transpile("interface P { n: number }\nexport default function (jx: any): number { const p: P = { n: 1 }; return p.n as number; }")
            .unwrap();
        assert!(
            !js.contains("interface") && !js.contains(": number") && !js.contains("as number"),
            "{js}"
        );
        assert!(js.contains("export default"));
    }

    #[test]
    fn read_rows_log_and_return_value() {
        let r = run_ok(
            r#"
            export default function (jx: Jxcel) {
              const rows = jx.sheet("倉庫").schema("在庫").rows();
              console.log("件数", rows.length);
              jx.log({ first: rows[0].品名 });
              return rows.reduce((sum: number, r: any) => sum + (r.数量 as number), 0);
            }"#,
        );
        assert_eq!(r.result, Some(json!(15)));
        assert_eq!(
            r.logs,
            vec!["件数 2".to_string(), r#"{"first":"ねじ"}"#.to_string()]
        );
        assert_eq!(r.ops, 0);
        assert_eq!(r.file, sample());
    }

    #[test]
    fn add_update_remove() {
        let r = run_ok(
            r#"
            export default function (jx: Jxcel) {
              const s = jx.sheet("倉庫").schema("在庫");
              const id = s.add({ 品名: "座金", 数量: 3 });
              s.update(id, { 数量: 4 });          // 同じ実行内で追加した行も更新できる
              s.update("r1", { 数量: s.get("r1").数量 + 1 });
              s.remove("r2");
              return s.rows().map((r: any) => r.品名 + ":" + r.数量).join(",");
            }"#,
        );
        assert_eq!(r.result, Some(json!("ねじ:11,座金:4")));
        assert_eq!(r.ops, 4);
        let rows = &r.file.sheets[0].schemas[0].rows;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cells["qty"], json!(11));
        assert_eq!(rows[1].cells["name"], json!("座金"));
        assert_eq!(rows[1].cells["qty"], json!(4));
        // 元のファイルは変わらない
        assert_eq!(sample().sheets[0].schemas[0].rows.len(), 2);
    }

    #[test]
    fn invalid_writes_discard_everything() {
        // 型違い
        let e = run_err(
            r#"export default (jx: Jxcel) => { jx.sheet("倉庫").schema("在庫").add({ 品名: "x", 数量: "多い" }); }"#,
        );
        assert!(matches!(&e, Error::Apply(m) if m.contains("数量")), "{e}");
        // 必須の欠落
        let e = run_err(
            r#"export default (jx: Jxcel) => { jx.sheet("倉庫").schema("在庫").add({ 数量: 1 }); }"#,
        );
        assert!(matches!(&e, Error::Apply(m) if m.contains("品名")), "{e}");
        // 途中まで成功していても、後の失敗で全て取り消される（run が Err を返すので元のファイルは無傷）
        let e = run_err(
            r#"export default (jx: Jxcel) => {
                 const s = jx.sheet("倉庫").schema("在庫");
                 s.update("r1", { 数量: 99 });
                 s.update("r2", { 数量: 1.5 });
               }"#,
        );
        assert!(matches!(e, Error::Apply(_)));
    }

    #[test]
    fn validation_ignores_untouched_invalid_rows() {
        // 既存の不正な行（必須の欠落）があっても、マクロが触らなければ失敗させない
        let mut f = sample();
        f.sheets[0].schemas[0].rows.push(Row {
            id: "bad".into(),
            cells: Default::default(),
        });
        let src = r#"export default (jx: Jxcel) => { jx.sheet("倉庫").schema("在庫").update("r1", { 数量: 11 }); }"#;
        assert!(run(src, &f, &Options::default()).is_ok());
    }

    #[test]
    fn errors_are_reported() {
        assert!(matches!(
            run_err("export default function ("),
            Error::Syntax(_)
        ));
        assert!(
            matches!(run_err(r#"export default (jx: Jxcel) => jx.sheet("無い")"#), Error::Runtime(m) if m.contains("無い"))
        );
        assert!(
            matches!(run_err(r#"export default (jx: Jxcel) => jx.sheet("倉庫").schema("在庫").add({ 無い列: 1 })"#), Error::Runtime(m) if m.contains("無い列"))
        );
        assert!(
            matches!(run_err(r#"export default (jx: Jxcel) => { throw new Error("boom"); }"#), Error::Runtime(m) if m.contains("boom"))
        );
        assert!(
            matches!(run_err("const x = 1;"), Error::Runtime(m) if m.contains("export default"))
        );
    }

    #[test]
    fn runaway_macro_is_stopped() {
        let opts = Options {
            timeout: Duration::from_millis(200),
            ..Options::default()
        };
        let started = Instant::now();
        let e = run("export default () => { while (true) {} }", &sample(), &opts).unwrap_err();
        assert!(matches!(e, Error::Timeout(_)), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn memory_hog_is_stopped() {
        let opts = Options {
            memory_limit: 8 * 1024 * 1024,
            ..Options::default()
        };
        let e = run(
            "export default () => { const a: number[] = []; while (true) a.push(1); }",
            &sample(),
            &opts,
        )
        .unwrap_err();
        assert!(matches!(e, Error::Runtime(_) | Error::Timeout(_)), "{e}");
    }

    #[test]
    fn sandbox_has_no_io() {
        // QuickJS 付属の std / os モジュール（ファイル・プロセス・環境変数にアクセスできる）は見えない。
        // `std` は jxcel の標準ライブラリで同名だが、入出力の関数は持たない。
        let r = run_ok(
            r#"export default () => [
                typeof require, typeof process, typeof fetch, typeof XMLHttpRequest, typeof os,
                typeof (std as any).open, typeof (std as any).loadFile, typeof (std as any).getenv,
                typeof (std as any).popen, typeof (std as any).evalScript, typeof (std as any).urlGet,
                typeof (std as any).exit, typeof (std as any).writeFile,
            ].join(",")"#,
        );
        assert_eq!(r.result, Some(json!(vec!["undefined"; 13].join(","))));
    }

    // ---- 計算列 ----

    /// サンプルの「在庫」スキーマに計算列を足す（列 ID は `id`）。
    fn with_computed(mut f: JxcelFile, id: &str, name: &str, ty: DataType, src: &str) -> JxcelFile {
        f.sheets[0].schemas[0]
            .columns
            .push(Column::new(id, name, ty).computed(src));
        f
    }

    fn cell(c: &ComputedValues, column: &str, row: &str) -> CellResult {
        c["s1"][column][row].clone()
    }

    fn quick() -> Options {
        Options {
            timeout: Duration::from_millis(300),
            ..Options::default()
        }
    }

    #[test]
    fn computed_values_from_row() {
        let f = with_computed(
            sample(),
            "dbl",
            "倍",
            DataType::Int,
            "export default (row: JxcelRow, jx: Jxcel): number => row.数量 * 2;",
        );
        let f = with_computed(
            f,
            "label",
            "表示",
            DataType::String,
            "export default (row: any) => `${row.品名}(${row.数量})`",
        );
        let c = compute(&f, &Options::default());
        assert_eq!(cell(&c, "dbl", "r1"), CellResult::Value(json!(20)));
        assert_eq!(cell(&c, "dbl", "r2"), CellResult::Value(json!(10)));
        assert_eq!(
            cell(&c, "label", "r1"),
            CellResult::Value(json!("ねじ(10)"))
        );
    }

    #[test]
    fn no_computed_columns_means_nothing_runs() {
        assert!(compute(&sample(), &Options::default()).is_empty());
    }

    #[test]
    fn a_bad_row_fails_only_that_cell() {
        let mut f = sample();
        f.sheets[0].schemas[0].rows[1].cells.remove("qty"); // r2 は数量が空
        let f = with_computed(
            f,
            "ratio",
            "比",
            DataType::Int,
            "export default (row: any) => { if (row.数量 === null) throw new Error('数量が空です'); return row.数量 * 2; }",
        );
        let c = compute(&f, &Options::default());
        assert_eq!(cell(&c, "ratio", "r1"), CellResult::Value(json!(20)));
        assert!(
            matches!(cell(&c, "ratio", "r2"), CellResult::Error(m) if m.contains("数量が空です"))
        );
    }

    #[test]
    fn result_must_match_the_column_type() {
        // Int 列なのに、小数や文字列を返した行だけがエラーになる
        let f = with_computed(
            sample(),
            "half",
            "半分",
            DataType::Int,
            "export default (row: any) => row.品名 === 'ねじ' ? row.数量 / 4 : row.数量 / 5",
        );
        let c = compute(&f, &Options::default());
        assert!(
            matches!(cell(&c, "half", "r1"), CellResult::Error(m) if m.contains("半分")),
            "{:?}",
            cell(&c, "half", "r1")
        );
        assert_eq!(cell(&c, "half", "r2"), CellResult::Value(json!(1)));
    }

    #[test]
    fn syntax_error_marks_that_column_only() {
        let f = with_computed(
            sample(),
            "bad",
            "壊れ",
            DataType::Int,
            "export default (row => ",
        );
        let f = with_computed(
            f,
            "ok",
            "正常",
            DataType::Int,
            "export default (row: any) => row.数量",
        );
        let c = compute(&f, &Options::default());
        assert!(matches!(cell(&c, "bad", "r1"), CellResult::Error(m) if m.contains("構文エラー")));
        assert_eq!(cell(&c, "ok", "r1"), CellResult::Value(json!(10)));
        // 式が空、default の関数でない場合もそのセルのエラー
        let f = with_computed(sample(), "empty", "空", DataType::Int, "  ");
        assert!(
            matches!(cell(&compute(&f, &Options::default()), "empty", "r1"), CellResult::Error(m) if m.contains("空"))
        );
        let f = with_computed(
            sample(),
            "nofn",
            "非関数",
            DataType::Int,
            "export default 5;",
        );
        assert!(matches!(
            cell(&compute(&f, &Options::default()), "nofn", "r1"),
            CellResult::Error(_)
        ));
    }

    #[test]
    fn later_columns_can_use_earlier_computed_columns() {
        let f = with_computed(
            sample(),
            "dbl",
            "倍",
            DataType::Int,
            "export default (row: any) => row.数量 * 2",
        );
        let f = with_computed(
            f,
            "quad",
            "四倍",
            DataType::Int,
            "export default (row: any) => row.倍 * 2",
        );
        let c = compute(&f, &Options::default());
        assert_eq!(cell(&c, "quad", "r1"), CellResult::Value(json!(40)));
        // 後ろの計算列は、まだ計算されていないので空（null）として見える
        let f = with_computed(
            sample(),
            "first",
            "先",
            DataType::Int,
            "export default (row: any) => row.後 === null ? -1 : 1",
        );
        let f = with_computed(f, "後", "後", DataType::Int, "export default () => 5");
        let c = compute(&f, &Options::default());
        assert_eq!(cell(&c, "first", "r1"), CellResult::Value(json!(-1)));
    }

    #[test]
    fn formulas_can_look_up_other_rows_read_only() {
        // 全体に占める割合（%）: 同じ表の全行を読んで集計する
        let f = with_computed(
            sample(),
            "pct",
            "割合",
            DataType::Int,
            r#"export default (row: any, jx: Jxcel) => {
                 const total = jx.sheet("倉庫").schema("在庫").rows().reduce((s: number, r: any) => s + r.数量, 0);
                 return Math.round(row.数量 * 100 / total);
               }"#,
        );
        let c = compute(&f, &Options::default());
        assert_eq!(cell(&c, "pct", "r1"), CellResult::Value(json!(67)));
        assert_eq!(cell(&c, "pct", "r2"), CellResult::Value(json!(33)));
        // 計算式の中では書き込めない
        let f = with_computed(
            sample(),
            "w",
            "書込",
            DataType::Int,
            r#"export default (row: any, jx: Jxcel) => { jx.sheet("倉庫").schema("在庫").remove("r1"); return 1; }"#,
        );
        let c = compute(&f, &Options::default());
        assert!(
            matches!(cell(&c, "w", "r1"), CellResult::Error(m) if m.contains("書き換えられません"))
        );
    }

    #[test]
    fn runaway_formula_times_out_for_all_cells() {
        let f = with_computed(
            sample(),
            "loop",
            "無限",
            DataType::Int,
            "export default () => { while (true) {} }",
        );
        let started = Instant::now();
        let c = compute(&f, &quick());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(matches!(cell(&c, "loop", "r1"), CellResult::Error(m) if m.contains("実行時間")));
        assert!(matches!(cell(&c, "loop", "r2"), CellResult::Error(_)));
    }

    #[test]
    fn macros_read_computed_values_but_cannot_write_them() {
        let f = with_computed(
            sample(),
            "dbl",
            "倍",
            DataType::Int,
            "export default (row: any) => row.数量 * 2",
        );
        let r = run(
            r#"export default (jx: Jxcel) => jx.sheet("倉庫").schema("在庫").rows().map((r: any) => r.倍)"#,
            &f,
            &Options::default(),
        )
        .unwrap();
        assert_eq!(r.result, Some(json!([20, 10])));
        assert_eq!(r.ops, 0);

        let e = run(
            r#"export default (jx: Jxcel) => { jx.sheet("倉庫").schema("在庫").update("r1", { 倍: 1 }); }"#,
            &f,
            &Options::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&e, Error::Runtime(m) if m.contains("計算列")),
            "{e}"
        );

        // 通常の列への書き込みは普通にでき、計算列の値はファイルに入らない
        let r = run(
            r#"export default (jx: Jxcel) => { jx.sheet("倉庫").schema("在庫").update("r1", { 数量: 11 }); }"#,
            &f,
            &Options::default(),
        )
        .unwrap();
        assert!(!r.file.sheets[0].schemas[0].rows[0]
            .cells
            .contains_key("dbl"));
        assert_eq!(
            cell(&compute(&r.file, &Options::default()), "dbl", "r1"),
            CellResult::Value(json!(22))
        );
    }

    #[test]
    fn cell_results_serialize_as_ui_expects() {
        // UI の `CellResult = { v: unknown } | { e: string }` と対応している。変えるなら ui/src/types.ts も
        assert_eq!(
            serde_json::to_value(CellResult::Value(json!(30))).unwrap(),
            json!({ "v": 30 })
        );
        assert_eq!(
            serde_json::to_value(CellResult::Value(Value::Null)).unwrap(),
            json!({ "v": null })
        );
        assert_eq!(
            serde_json::to_value(CellResult::Error("だめ".into())).unwrap(),
            json!({ "e": "だめ" })
        );
    }

    // ---- 標準ライブラリ std ----

    /// 式を並べて 1 回の実行で評価し、結果の配列を返す。`rows` は「在庫」表の全行。
    fn eval_all(file: &JxcelFile, exprs: &[&str]) -> Vec<Value> {
        let src = format!(
            "export default (jx: Jxcel) => {{ const rows = jx.sheet('倉庫').schema('在庫').rows(); return [{}]; }}",
            exprs.join(",\n")
        );
        match run(&src, file, &Options::default()) {
            Ok(r) => r.result.unwrap().as_array().unwrap().clone(),
            Err(e) => panic!("{e}"),
        }
    }

    fn check_table(file: &JxcelFile, cases: &[(&str, Value)]) {
        let exprs: Vec<&str> = cases.iter().map(|(e, _)| *e).collect();
        let got = eval_all(file, &exprs);
        for ((expr, want), got) in cases.iter().zip(got) {
            assert_eq!(&got, want, "{expr}");
        }
    }

    /// 品名・数量・区分のある 5 行（区分: 金属 / 樹脂、数量が空の行と、区分が空の行を含む）
    fn stock() -> JxcelFile {
        let mut f = sample();
        let s = &mut f.sheets[0].schemas[0];
        s.columns
            .push(Column::new("kind", "区分", DataType::String));
        s.rows.clear();
        for (id, name, qty, kind) in [
            ("a", "ねじ", Some(10), Some("金属")),
            ("b", "ナット", Some(5), Some("金属")),
            ("c", "ワッシャー", None, Some("樹脂")),
            ("d", "ばね", Some(20), None),
            ("e", "ピン", Some(2), Some("金属")),
        ] {
            let mut cells: std::collections::BTreeMap<String, Value> =
                [("name".to_string(), json!(name))].into();
            if let Some(q) = qty {
                cells.insert("qty".into(), json!(q));
            }
            if let Some(k) = kind {
                cells.insert("kind".into(), json!(k));
            }
            s.rows.push(Row {
                id: id.into(),
                cells,
            });
        }
        f
    }

    #[test]
    fn std_aggregates_and_rounding() {
        check_table(
            &stock(),
            &[
                ("std.sum([1, 2, null, '3'])", json!(6)),
                ("std.sum([])", json!(0)),
                ("std.avg([1, 2, null])", json!(1.5)),
                ("std.avg([null])", json!(null)),
                ("std.min([3, 1, null, 2])", json!(1)),
                ("std.max([3, 1, null, 2])", json!(3)),
                ("std.max([])", json!(null)),
                ("std.median([3, 1, 2])", json!(2)),
                ("std.median([4, 1, 3, 2])", json!(2.5)),
                ("std.count([1, null, 0, '', undefined])", json!(3)),
                ("std.round(1.005, 2)", json!(1.01)),
                ("std.round(2.5)", json!(3)),
                ("std.round(-2.5)", json!(-3)),
                ("std.round(1234.5678, -2)", json!(1200)),
                ("std.round(null)", json!(null)),
                ("std.floor(1.239, 2)", json!(1.23)),
                ("std.floor(-1.5)", json!(-2)),
                ("std.ceil(1.231, 2)", json!(1.24)),
                ("std.clamp(15, 0, 10)", json!(10)),
                ("std.clamp(-1, 0, 10)", json!(0)),
                ("std.coalesce(null, undefined, 0, 5)", json!(0)),
                ("std.coalesce(null)", json!(null)),
            ],
        );
        // 数値でない値は黙って 0 にせず、例外にする
        let e = run(
            "export default () => std.sum([1, 'abc'])",
            &sample(),
            &Options::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&e, Error::Runtime(m) if m.contains("数値ではありません")),
            "{e}"
        );
    }

    #[test]
    fn std_table_helpers() {
        check_table(
            &stock(),
            &[
                (
                    "std.pluck(rows, '品名')",
                    json!(["ねじ", "ナット", "ワッシャー", "ばね", "ピン"]),
                ),
                ("std.sumBy(rows, '数量')", json!(37)),
                ("std.avgBy(rows, '数量')", json!(9.25)),
                ("std.countBy(rows, '数量')", json!(4)),
                (
                    "std.where(rows, '区分', '金属').map((r: any) => r.品名)",
                    json!(["ねじ", "ナット", "ピン"]),
                ),
                (
                    "std.where(rows, '区分', null).map((r: any) => r.品名)",
                    json!(["ばね"]),
                ),
                ("std.find(rows, '品名', 'ばね')._id", json!("d")),
                ("std.find(rows, '品名', 'なし')", json!(null)),
                ("std.lookup(rows, '品名', 'ナット', '数量')", json!(5)),
                ("std.lookup(rows, '品名', 'なし', '数量', -1)", json!(-1)),
                ("std.lookup(rows, '品名', 'なし', '数量')", json!(null)),
                // 出現順を保ち、空の区分も 1 つのグループになる
                (
                    "std.groupBy(rows, '区分').map((g: any) => [g.key, std.sumBy(g.rows, '数量')])",
                    json!([["金属", 17], ["樹脂", 0], [null, 20]]),
                ),
                (
                    "std.uniq([1, 2, 1, '1', null, null])",
                    json!([1, 2, "1", null]),
                ),
                (
                    "std.sortBy(rows, '数量').map((r: any) => r.品名)",
                    json!(["ピン", "ナット", "ねじ", "ばね", "ワッシャー"]),
                ),
                (
                    "std.sortBy(rows, '数量', 'desc').map((r: any) => r.品名)",
                    json!(["ばね", "ねじ", "ナット", "ピン", "ワッシャー"]),
                ),
                // 文字列は文字コード順（五十音順ではない）なので「樹脂」(U+6A39) が「金属」(U+91D1) より前。
                // 同じ値は元の順を保つ（金属 3 件は ねじ → ナット → ピン のまま）。空は最後
                (
                    "std.sortBy(rows, '区分').map((r: any) => r.品名)",
                    json!(["ワッシャー", "ねじ", "ナット", "ピン", "ばね"]),
                ),
            ],
        );
        // 存在しない列は、黙って空にせず例外
        let e = run("export default (jx: Jxcel) => std.pluck(jx.sheet('倉庫').schema('在庫').rows(), '無い列')", &sample(), &Options::default()).unwrap_err();
        assert!(
            matches!(&e, Error::Runtime(m) if m.contains("無い列")),
            "{e}"
        );
    }

    #[test]
    fn std_dates() {
        check_table(
            &sample(),
            &[
                ("std.date.addDays('2024-02-28', 2)", json!("2024-03-01")),
                ("std.date.addDays('2023-02-28', 2)", json!("2023-03-02")),
                ("std.date.addDays('2024-01-01', -1)", json!("2023-12-31")),
                ("std.date.addMonths('2024-01-31', 1)", json!("2024-02-29")),
                ("std.date.addMonths('2023-01-31', 1)", json!("2023-02-28")),
                ("std.date.addMonths('2024-03-31', -1)", json!("2024-02-29")),
                ("std.date.addMonths('2024-11-15', 3)", json!("2025-02-15")),
                ("std.date.addYears('2024-02-29', 1)", json!("2025-02-28")),
                ("std.date.diffDays('2024-03-01', '2024-02-01')", json!(29)),
                ("std.date.diffDays('2024-02-01', '2024-03-01')", json!(-29)),
                ("std.date.startOfMonth('2024-02-10')", json!("2024-02-01")),
                ("std.date.endOfMonth('2024-02-10')", json!("2024-02-29")),
                ("std.date.endOfMonth('2023-12-05')", json!("2023-12-31")),
                ("std.date.weekday('2024-01-31')", json!(3)),
                ("std.date.weekday('1970-01-01')", json!(4)),
                ("std.date.isWeekend('2024-02-03')", json!(true)),
                ("std.date.isWeekend('2024-02-05')", json!(false)),
                ("std.date.addWorkdays('2024-01-31', 3)", json!("2024-02-05")),
                (
                    "std.date.addWorkdays('2024-02-05', -1)",
                    json!("2024-02-02"),
                ),
                // 祝日（2/5 を休みにすると、さらに 1 日後ろへ）
                (
                    "std.date.addWorkdays('2024-01-31', 3, ['2024-02-05'])",
                    json!("2024-02-06"),
                ),
                (
                    "std.date.workdaysBetween('2024-01-29', '2024-02-04')",
                    json!(5),
                ),
                (
                    "std.date.workdaysBetween('2024-02-04', '2024-01-29')",
                    json!(-5),
                ),
                (
                    "std.date.format('2024-01-31', 'YYYY年M月D日(ddd)')",
                    json!("2024年1月31日(水)"),
                ),
                (
                    "std.date.format('2024-03-05', 'YY/MM/DD')",
                    json!("24/03/05"),
                ),
                (
                    "std.date.format('2024-03-05T10:20:30+09:00', 'YYYY-MM-DD')",
                    json!("2024-03-05"),
                ),
                ("std.date.toWareki('2024-01-31')", json!("令和6年1月31日")),
                ("std.date.toWareki('2019-05-01')", json!("令和元年5月1日")),
                ("std.date.toWareki('2019-04-30')", json!("平成31年4月30日")),
                ("std.date.toWareki('1989-01-07')", json!("昭和64年1月7日")),
                ("std.date.toWareki('1912-07-30')", json!("大正元年7月30日")),
                ("std.date.isValid('2024-02-30')", json!(false)),
                ("std.date.isValid('2024-02-29')", json!(true)),
                ("std.date.addDays(null, 1)", json!(null)),
                ("std.date.diffDays(null, '2024-01-01')", json!(null)),
                ("std.date.today().length", json!(10)),
            ],
        );
        let e = run(
            "export default () => std.date.addDays('2024-02-30', 1)",
            &sample(),
            &Options::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&e, Error::Runtime(m) if m.contains("存在しない日付")),
            "{e}"
        );
    }

    #[test]
    fn std_decimals_are_exact() {
        check_table(
            &sample(),
            &[
                // number なら 0.30000000000000004 になる計算が、誤差なく出る
                ("0.1 + 0.2 === 0.3", json!(false)),
                ("std.dec.add('0.1', '0.2')", json!("0.3")),
                ("std.dec.add('1.5', '2')", json!("3.5")),
                ("std.dec.sub('1', '1.5')", json!("-0.5")),
                ("std.dec.sub('1.5', '1.5')", json!("0.0")),
                ("std.dec.mul('1.10', '3')", json!("3.30")),
                ("std.dec.mul('-0.5', '0.5')", json!("-0.25")),
                ("std.dec.div('1', '3', 4)", json!("0.3333")),
                ("std.dec.div('2', '3', 2)", json!("0.67")),
                ("std.dec.div('-2', '3', 2)", json!("-0.67")),
                ("std.dec.div('10', '4', 0)", json!("3")),
                ("std.dec.round('2.675', 2)", json!("2.68")),
                ("std.dec.round('-2.5', 0)", json!("-3")),
                ("std.dec.round('2.4', 0)", json!("2")),
                ("std.dec.round('1234.5678', 1)", json!("1234.6")),
                ("std.dec.fixed('1.5', 2)", json!("1.50")),
                ("std.dec.sum(['0.1', '0.2', '0.3', null])", json!("0.6")),
                ("std.dec.sum([])", json!("0")),
                ("std.dec.cmp('1.0', '1')", json!(0)),
                ("std.dec.cmp('1.01', '1.1')", json!(-1)),
                ("std.dec.cmp('-1', '-2')", json!(1)),
                ("std.dec.toNumber('1.5')", json!(1.5)),
                ("std.dec.add(0.1, 0.2)", json!("0.3")),
                ("std.dec.round(null)", json!(null)),
            ],
        );
        let e = run(
            "export default () => std.dec.div('1', '0', 2)",
            &sample(),
            &Options::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&e, Error::Runtime(m) if m.contains("0 で割れません")),
            "{e}"
        );
        let e = run(
            "export default () => std.dec.add('abc', '1')",
            &sample(),
            &Options::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&e, Error::Runtime(m) if m.contains("10進数ではありません")),
            "{e}"
        );
    }

    #[test]
    fn std_text() {
        check_table(
            &sample(),
            &[
                (
                    "std.text.normalize('  Ａ１２３　ﾊﾟｿｺﾝ   一覧 ')",
                    json!("A123 パソコン 一覧"),
                ),
                ("std.text.normalize(null)", json!(null)),
                ("std.text.trim('\\u3000 abc \\u3000')", json!("abc")),
                (
                    "std.text.toHalfWidth('ＡＢＣ　１２３！')",
                    json!("ABC 123!"),
                ),
                ("std.text.toFullWidth('ABC 12')", json!("ＡＢＣ　１２")),
                ("std.text.toFullWidth('ﾊﾟｿｺﾝ')", json!("パソコン")),
                ("std.text.toKatakana('ひらがな')", json!("ヒラガナ")),
                ("std.text.toHiragana('カタカナ')", json!("かたかな")),
                ("std.text.isBlank(null)", json!(true)),
                ("std.text.isBlank(' \\u3000')", json!(true)),
                ("std.text.isBlank('a')", json!(false)),
                ("std.text.isBlank(0)", json!(false)),
                ("std.text.zeroPad(7, 3)", json!("007")),
                (
                    "std.text.formatNumber(1234567.891, 1)",
                    json!("1,234,567.9"),
                ),
                ("std.text.formatNumber(-1234)", json!("-1,234")),
                ("std.text.formatNumber(999)", json!("999")),
                ("std.text.formatNumber(1000)", json!("1,000")),
                ("std.text.formatNumber(0.5, 2)", json!("0.50")),
                ("std.text.yen(1234567)", json!("¥1,234,567")),
                ("std.text.yen(-500)", json!("-¥500")),
                ("std.text.yen(null)", json!(null)),
            ],
        );
    }

    #[test]
    fn std_is_available_in_computed_columns() {
        // 計算列の式からも std が使える（税込み価格を 10 進数で誤差なく計算し、四捨五入する）
        let f = with_computed(
            sample(),
            "inc",
            "税込",
            DataType::Decimal,
            "export default (row: any) => std.dec.round(std.dec.mul(String(row.数量), '1.10'), 0)",
        );
        let c = compute(&f, &Options::default());
        assert_eq!(cell(&c, "inc", "r1"), CellResult::Value(json!("11")));
        assert_eq!(cell(&c, "inc", "r2"), CellResult::Value(json!("6"))); // 5.5 → 6（四捨五入）
    }

    #[test]
    fn std_cannot_be_tampered_with() {
        // std は凍結されている。書き換えようとしても、他のマクロ・計算列に影響しない（そもそも実行ごとに作り直す）
        let r = run_ok("export default () => { try { (std as any).sum = () => 0; } catch (e) {} return std.sum([1, 2]); }");
        assert_eq!(r.result, Some(json!(3)));
    }

    /// 型定義（エディタの補完・ホバー用）と、実際の std が食い違わないことを検査する。
    /// 型定義にあるのに実在しない関数（補完は出るのに実行で失敗する）も、
    /// 実在するのに型定義が無い関数（補完・説明が出ない）も、ここで見つかる。
    #[test]
    fn std_matches_its_type_definitions() {
        let dts = include_str!("../../../ui/src/jxcelApi.d.ts.txt");
        // interface ごとに「2 文字インデントの関数名」を集める（コメント行とプロパティは除く）
        let mut declared: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut current: Option<&str> = None;
        for line in dts.lines() {
            match line.trim_end() {
                "interface JxcelStd {" => current = Some(""),
                "interface JxcelStdDate {" => current = Some("date"),
                "interface JxcelStdDec {" => current = Some("dec"),
                "interface JxcelStdText {" => current = Some("text"),
                "}" => current = None,
                l => {
                    let (Some(ns), Some(body)) = (current, l.strip_prefix("  ")) else {
                        continue;
                    };
                    let name: String = body
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() && body[name.len()..].starts_with(['(', '<']) {
                        declared.entry(ns).or_default().push(name);
                    }
                }
            }
        }
        // 取りこぼしていないこと（パースが壊れて空のまま通ってしまうのを防ぐ）
        assert!(
            declared[""].len() >= 20
                && declared["date"].len() >= 10
                && declared["dec"].len() >= 8
                && declared["text"].len() >= 8,
            "{declared:?}"
        );

        let src = format!(
            r#"export default () => {{
                const declared: Record<string, string[]> = {};
                const problems: string[] = [];
                for (const [ns, names] of Object.entries(declared)) {{
                    const obj: any = ns ? (std as any)[ns] : std;
                    for (const n of names) if (typeof obj[n] !== "function") problems.push("型定義にあるが実在しない: " + (ns ? ns + "." : "") + n);
                    for (const k of Object.keys(obj)) if (typeof obj[k] === "function" && !names.includes(k)) problems.push("型定義にない: " + (ns ? ns + "." : "") + k);
                }}
                return problems;
            }}"#,
            serde_json::to_string(&declared).unwrap()
        );
        let r = run(&src, &sample(), &Options::default()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.result, Some(json!([])), "std と型定義が食い違っています");
    }

    // ---- 差し込み欄の式 ----

    fn exprs(file: &JxcelFile, exprs: &[&str]) -> Vec<Vec<CellResult>> {
        let list: Vec<String> = exprs.iter().map(|s| s.to_string()).collect();
        evaluate_exprs(file, "sh1", "s1", &list, &Options::default())
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn v(x: Value) -> CellResult {
        CellResult::Value(x)
    }

    #[test]
    fn exprs_use_column_names_as_variables() {
        let got = exprs(
            &sample(),
            &[
                "品名",
                "数量 * 2",
                "品名 + '(' + 数量 + ')'",
                "_no",
                "_id",
                "row.数量",
                "std.sum([数量, 1])",
            ],
        );
        assert_eq!(got.len(), 2); // 行の数
        assert_eq!(
            got[0],
            [
                v(json!("ねじ")),
                v(json!(20)),
                v(json!("ねじ(10)")),
                v(json!(1)),
                v(json!("r1")),
                v(json!(10)),
                v(json!(11))
            ]
        );
        assert_eq!(got[1][3], v(json!(2)));
        assert_eq!(got[1][1], v(json!(10)));
    }

    #[test]
    fn exprs_see_computed_columns_and_are_read_only() {
        let f = with_computed(
            sample(),
            "dbl",
            "倍",
            DataType::Int,
            "export default (row: any) => row.数量 * 2",
        );
        let got = exprs(
            &f,
            &["倍 + 1", "jx.sheet('倉庫').schema('在庫').remove('r1')"],
        );
        assert_eq!(got[0][0], v(json!(21)));
        assert!(
            matches!(&got[0][1], CellResult::Error(m) if m.contains("書き換えられません")),
            "{:?}",
            got[0][1]
        );
    }

    #[test]
    fn expr_errors_are_isolated_per_row_and_expression() {
        let mut f = sample();
        f.sheets[0].schemas[0].rows[1].cells.remove("qty");
        let got = exprs(&f, &["数量.toFixed(1)", "存在しない列", "1 +", "品名"]);
        // r1 は数量があるので成功、r2 は数量が null なので例外。他の式には影響しない
        assert_eq!(got[0][0], v(json!("10.0")));
        assert!(matches!(&got[1][0], CellResult::Error(_)));
        assert!(
            matches!(&got[0][1], CellResult::Error(m) if m.contains("存在しない列")),
            "{:?}",
            got[0][1]
        );
        // 構文エラーは全行のその式だけがエラー
        assert!(got.iter().all(|r| matches!(&r[2], CellResult::Error(_))));
        assert_eq!(got[1][3], v(json!("ナット")));
    }

    #[test]
    fn expr_trailing_line_comment_does_not_break_evaluation() {
        // 式の後ろに // コメントがあっても、閉じ括弧が飲み込まれない
        let got = exprs(&sample(), &["数量 // 在庫数"]);
        assert_eq!(got[0][0], v(json!(10)));
    }

    #[test]
    fn exprs_report_an_unknown_target_and_time_out() {
        let e = evaluate_exprs(
            &sample(),
            "sh1",
            "nope",
            &["1".to_string()],
            &Options::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&e, Error::Runtime(m) if m.contains("書き出し対象")),
            "{e}"
        );
        let e = evaluate_exprs(
            &sample(),
            "sh1",
            "s1",
            &["(() => { while (true) {} })()".to_string()],
            &quick(),
        )
        .unwrap_err();
        assert!(matches!(e, Error::Timeout(_)), "{e}");
    }

    #[test]
    fn template_loops_evaluate_per_row_and_per_item() {
        let mut f = sample();
        // 明細（配列）を持つ列と、別の表（明細表）を引く式の両方をループの対象にできる
        f.sheets[0].schemas[0]
            .columns
            .push(Column::new("lines", "明細", DataType::Any));
        f.sheets[0].schemas[0].rows[0].cells.insert(
            "lines".into(),
            json!([{"品目": "A", "数": 2}, {"品目": "B", "数": 3}]),
        );
        let loops = vec![
            LoopRequest {
                source: "明細".into(),
                exprs: vec![
                    "品目 + ':' + 数".into(),
                    "品名 + _n".into(), // 行の列も見える
                    "_i".into(),
                    "_item.数 * 数量".into(), // _item と、行の列の両方
                    "存在しない".into(),
                ],
            },
            LoopRequest {
                source: "[1, 2].map(x => x * 数量)".into(), // 要素がオブジェクトでない
                exprs: vec!["_item".into(), "_n".into()],
            },
            LoopRequest {
                source: "品名".into(), // 配列ではない
                exprs: vec![],
            },
        ];
        let got = evaluate_template(
            &f,
            "sh1",
            "s1",
            &["_no".to_string()],
            &loops,
            &Options::default(),
        )
        .unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].top, [v(json!(1))]);
        let l0 = got[0].loops[0].as_ref().unwrap();
        assert_eq!(l0.len(), 2);
        assert_eq!(
            l0[0][..4],
            [
                v(json!("A:2")),
                v(json!("ねじ1")),
                v(json!(0)),
                v(json!(20))
            ]
        );
        assert!(matches!(&l0[0][4], CellResult::Error(_))); // 失敗はその式だけ
        assert_eq!(
            l0[1][..4],
            [
                v(json!("B:3")),
                v(json!("ねじ2")),
                v(json!(1)),
                v(json!(30))
            ]
        );
        let l1 = got[0].loops[1].as_ref().unwrap();
        assert_eq!(
            l1,
            &[
                vec![v(json!(10)), v(json!(1))],
                vec![v(json!(20)), v(json!(2))]
            ]
        );
        assert!(got[0].loops[2]
            .as_ref()
            .unwrap_err()
            .contains("配列ではありません"));
        // 明細が無い行（null）は、要素なし
        assert_eq!(got[1].loops[0].as_ref().unwrap().len(), 0);
    }

    #[test]
    fn template_loop_source_errors_are_isolated() {
        let loops = vec![
            LoopRequest {
                source: "1 +".into(),
                exprs: vec![],
            },
            LoopRequest {
                source: "存在しない".into(),
                exprs: vec![],
            },
            LoopRequest {
                source: "[1]".into(),
                exprs: vec!["_n".into()],
            },
        ];
        let got =
            evaluate_template(&sample(), "sh1", "s1", &[], &loops, &Options::default()).unwrap();
        assert!(got[0].loops[0].is_err() && got[0].loops[1].is_err());
        assert_eq!(got[0].loops[2].as_ref().unwrap(), &[vec![v(json!(1))]]);
        let big = vec![LoopRequest {
            source: "Array(10001).fill(0)".into(),
            exprs: vec![],
        }];
        let got =
            evaluate_template(&sample(), "sh1", "s1", &[], &big, &Options::default()).unwrap();
        assert!(got[0].loops[0].as_ref().unwrap_err().contains("多すぎます"));
    }

    #[test]
    fn template_exprs_see_nested_fields_by_name() {
        let mut f = sample();
        let lines = DataType::Array {
            item: Box::new(DataType::Object {
                fields: vec![
                    Column::new("f_item", "品目", DataType::String),
                    Column::new("f_n", "数", DataType::Int),
                ],
            }),
        };
        f.sheets[0].schemas[0]
            .columns
            .push(Column::new("lines", "明細", lines));
        f.sheets[0].schemas[0].rows[0]
            .cells
            .insert("lines".into(), json!([{"f_item": "A", "f_n": 2}]));
        let loops = vec![LoopRequest {
            source: "明細".into(),
            exprs: vec!["品目 + 数".into()],
        }];
        let got = evaluate_template(
            &f,
            "sh1",
            "s1",
            &["明細[0].品目".to_string()],
            &loops,
            &Options::default(),
        )
        .unwrap();
        assert_eq!(got[0].top, [v(json!("A"))]);
        assert_eq!(got[0].loops[0].as_ref().unwrap(), &[vec![v(json!("A2"))]]);
    }
}
