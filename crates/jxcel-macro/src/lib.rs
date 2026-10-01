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

const PRELUDE: &str = include_str!("prelude.js");

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
    let output = execute(file, Some(&js), opts)?;
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
    match execute(file, None, opts) {
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

fn execute(file: &JxcelFile, main_js: Option<&str>, opts: &Options) -> Result<Output> {
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
        let r = run_ok(
            r#"export default () => [typeof require, typeof process, typeof fetch, typeof XMLHttpRequest, typeof std, typeof os].join(",")"#,
        );
        assert_eq!(
            r.result,
            Some(json!(
                "undefined,undefined,undefined,undefined,undefined,undefined"
            ))
        );
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
}
