//! 表の全行について差し込みを行い、行ごとにファイルを生成する。

use std::collections::{HashMap, HashSet};

use jxcel_core::JxcelFile;
use jxcel_macro::{CellResult, ItemEval, LoopRequest, LoopResult, Options, RowEval};
use serde_json::Value;

use crate::placeholder::{find, replace_all};
use crate::{plan, render_with_notes, Error, Kind, Plan, Result, Source};

/// 書き出しの指定。
pub struct Spec<'a> {
    pub kind: Kind,
    pub template: &'a [u8],
    /// 対象の表（シート ID・スキーマ ID）
    pub sheet: &'a str,
    pub schema: &'a str,
    /// ファイル名（拡張子は付けなくてよい）。`{{ 式 }}` が使える。空なら行番号。
    pub filename: &'a str,
    /// 空でなければ、この式が真になる行だけを書き出す。
    pub filter: Option<&'a str>,
}

#[derive(Debug)]
pub struct GeneratedFile {
    pub row_id: String,
    /// 1 から始まる行番号
    pub row_no: usize,
    pub filename: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RowError {
    pub row_id: String,
    pub row_no: usize,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub files: Vec<GeneratedFile>,
    /// 絞り込み条件に合わず、書き出さなかった行数
    pub skipped: usize,
    /// 書き出せなかった行（他の行は書き出される）
    pub errors: Vec<RowError>,
    /// 書き出したが、注意が要ること（1900 年より前の日付を Excel の日付にできず文字列で書いた、など）
    pub warnings: Vec<RowError>,
}

/// 評価する式の一覧（テンプレートの欄、ファイル名の欄、絞り込み）。重複は 1 つにまとめる。
struct Exprs {
    list: Vec<String>,
    index: HashMap<String, usize>,
}

impl Exprs {
    fn new() -> Self {
        Self {
            list: vec![],
            index: HashMap::new(),
        }
    }

    fn add(&mut self, expr: &str) {
        if !self.index.contains_key(expr) {
            self.index.insert(expr.to_string(), self.list.len());
            self.list.push(expr.to_string());
        }
    }
}

fn pattern_of(spec: &Spec) -> String {
    if spec.filename.trim().is_empty() {
        "{{_no}}".to_string()
    } else {
        spec.filename.to_string()
    }
}

fn collect(spec: &Spec) -> Result<(Exprs, Plan)> {
    let plan = plan(spec.kind, spec.template)?;
    let mut exprs = Exprs::new();
    for p in &plan.placeholders {
        exprs.add(p);
    }
    for m in find(&pattern_of(spec)) {
        exprs.add(&m.expr);
    }
    if let Some(f) = spec.filter.map(str::trim).filter(|f| !f.is_empty()) {
        exprs.add(f);
    }
    Ok((exprs, plan))
}

fn evaluate(
    file: &JxcelFile,
    spec: &Spec,
    exprs: &Exprs,
    plan: &Plan,
    opts: &Options,
) -> Result<Vec<RowEval>> {
    let loops: Vec<LoopRequest> = plan
        .loops
        .iter()
        .map(|l| LoopRequest {
            source: l.source.clone(),
            exprs: l.exprs.clone(),
            parent: l.parent,
        })
        .collect();
    jxcel_macro::evaluate_template(file, spec.sheet, spec.schema, &exprs.list, &loops, opts)
        .map_err(|e| Error::Expr {
            expr: "（式の評価）".into(),
            message: e.to_string(),
        })
}

/// 1 行分の評価結果から値を引く（テンプレートの差し込み先）。
struct RowSource<'a> {
    exprs: &'a Exprs,
    plan: &'a Plan,
    eval: &'a RowEval,
}

impl RowSource<'_> {
    fn top(&self, expr: &str) -> std::result::Result<Value, String> {
        cell_to_result(&self.eval.top[self.exprs.index[expr]])
    }
}

impl Source for RowSource<'_> {
    fn value(&mut self, expr: &str) -> std::result::Result<Value, String> {
        self.top(expr)
    }

    fn loop_len(
        &mut self,
        index: usize,
        _: Option<usize>,
        path: &[usize],
        _: &str,
    ) -> std::result::Result<usize, String> {
        Ok(self
            .items(index, &path[..self.plan.chain(index).len() - 1])?
            .len())
    }

    fn item_value(
        &mut self,
        index: usize,
        path: &[usize],
        expr: &str,
    ) -> std::result::Result<Value, String> {
        let pos = self.plan.loops[index]
            .exprs
            .iter()
            .position(|e| e == expr)
            .ok_or_else(|| format!("評価していない欄です: {expr}"))?;
        let depth = path.len() - 1;
        let item = self
            .items(index, &path[..depth])?
            .get(path[depth])
            .ok_or("要素の位置が範囲外です")?;
        cell_to_result(&item.values[pos])
    }
}

impl RowSource<'_> {
    /// `index` 番目のループの要素の一覧。`outer` は外側のループの要素の位置（外側から順）。
    fn items(&self, index: usize, outer: &[usize]) -> std::result::Result<&[ItemEval], String> {
        let chain = self.plan.chain(index);
        let mut list: &LoopResult = &self.eval.loops[self.plan.sibling_pos(chain[0])];
        for d in 0..chain.len() {
            let items = list.as_ref().map_err(Clone::clone)?;
            if d == chain.len() - 1 {
                return Ok(items);
            }
            let item = items.get(outer[d]).ok_or("要素の位置が範囲外です")?;
            list = &item.loops[self.plan.sibling_pos(chain[d + 1])];
        }
        unreachable!("chain は空でない")
    }
}

/// `k` 番目のループの要素数（入れ子のループは、外側の要素すべての合計。どれかが失敗ならそのエラー）。
fn total_len(eval: &RowEval, plan: &Plan, k: usize) -> std::result::Result<usize, String> {
    fn walk(list: &LoopResult, chain: &[usize], plan: &Plan) -> std::result::Result<usize, String> {
        let items = list.as_ref().map_err(Clone::clone)?;
        if chain.len() == 1 {
            return Ok(items.len());
        }
        let mut n = 0;
        for it in items {
            n += walk(&it.loops[plan.sibling_pos(chain[1])], &chain[1..], plan)?;
        }
        Ok(n)
    }
    let chain = plan.chain(k);
    walk(&eval.loops[plan.sibling_pos(chain[0])], &chain, plan)
}

fn cell_to_result(c: &CellResult) -> std::result::Result<Value, String> {
    match c {
        CellResult::Value(v) => Ok(v.clone()),
        CellResult::Error(e) => Err(e.clone()),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// ファイル名に使えない文字を置き換え、前後の空白とドットを除く。長すぎる名前は切り詰める。
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut s: String = cleaned
        .trim()
        .trim_matches('.')
        .trim()
        .chars()
        .take(100)
        .collect();
    // Windows の予約名（CON, NUL, COM1 …）は拡張子付きでも使えない
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    if reserved {
        s.insert(0, '_');
    }
    s
}

/// 拡張子を付け、同じ名前があれば「 (2)」「 (3)」と番号を付けて重複させない（大文字小文字は区別しない）。
fn finish_filename(stem: &str, row_no: usize, ext: &str, used: &mut HashSet<String>) -> String {
    let mut stem = sanitize_filename(stem);
    let dotted = format!(".{ext}");
    if stem.to_lowercase().ends_with(&dotted) {
        stem.truncate(stem.len() - dotted.len());
    }
    if stem.is_empty() {
        stem = format!("row{row_no}");
    }
    let mut name = format!("{stem}{dotted}");
    let mut n = 2;
    while !used.insert(name.to_lowercase()) {
        name = format!("{stem} ({n}){dotted}");
        n += 1;
    }
    name
}

/// 全行を書き出す。行ごとの失敗は `errors` に集め、他の行は続ける。
pub fn export(file: &JxcelFile, spec: &Spec, opts: &Options) -> Result<Report> {
    let (exprs, plan) = collect(spec)?;
    let matrix = evaluate(file, spec, &exprs, &plan, opts)?;
    let rows = row_ids(file, spec);
    let pattern = pattern_of(spec);
    let filter = spec.filter.map(str::trim).filter(|f| !f.is_empty());

    let mut report = Report::default();
    let mut used = HashSet::new();
    for (i, (row_id, eval)) in rows.iter().zip(&matrix).enumerate() {
        let row_no = i + 1;
        let fail = |message: String| RowError {
            row_id: row_id.clone(),
            row_no,
            message,
        };
        let source = RowSource {
            exprs: &exprs,
            plan: &plan,
            eval,
        };
        let lookup = |expr: &str| source.top(expr);

        if let Some(f) = filter {
            match lookup(f) {
                Ok(v) if !truthy(&v) => {
                    report.skipped += 1;
                    continue;
                }
                Ok(_) => {}
                Err(e) => {
                    report.errors.push(fail(format!("絞り込み条件: {e}")));
                    continue;
                }
            }
        }

        let stem = match replace_all(&pattern, &mut |e| lookup(e)) {
            Ok(s) => s,
            Err((expr, e)) => {
                report
                    .errors
                    .push(fail(format!("ファイル名の「{expr}」: {e}")));
                continue;
            }
        };
        let mut source = RowSource {
            exprs: &exprs,
            plan: &plan,
            eval,
        };
        match render_with_notes(spec.kind, spec.template, &mut source) {
            Ok((bytes, notes)) => {
                for message in notes {
                    report.warnings.push(fail(message));
                }
                let filename = finish_filename(&stem, row_no, spec.kind.extension(), &mut used);
                report.files.push(GeneratedFile {
                    row_id: row_id.clone(),
                    row_no,
                    filename,
                    bytes,
                });
            }
            Err(e) => report.errors.push(fail(e.to_string())),
        }
    }
    Ok(report)
}

fn row_ids(file: &JxcelFile, spec: &Spec) -> Vec<String> {
    file.sheets
        .iter()
        .find(|s| s.id == spec.sheet)
        .and_then(|s| s.schemas.iter().find(|s| s.id == spec.schema))
        .map(|s| s.rows.iter().map(|r| r.id.clone()).collect())
        .unwrap_or_default()
}

/// プレビュー 1 行分。
#[derive(Debug)]
pub struct PreviewRow {
    pub row_no: usize,
    /// 絞り込みで除外される行か
    pub excluded: bool,
    pub filename: std::result::Result<String, String>,
    /// テンプレートの欄ごとの値（`Preview::placeholders` と同じ順）
    pub values: Vec<std::result::Result<Value, String>>,
}

/// テンプレートの行ループ 1 つ分のプレビュー。
#[derive(Debug)]
pub struct PreviewLoop {
    /// `{{#each 式}}` の式
    pub source: String,
    /// 入れ子のループなら、外側のループの番号
    pub parent: Option<usize>,
    /// ループの中の差し込み欄の式
    pub exprs: Vec<String>,
    /// プレビューした行ごとの要素数（か、対象の式のエラー）。入れ子のループは、外側の要素すべての合計
    pub counts: Vec<std::result::Result<usize, String>>,
}

#[derive(Debug)]
pub struct Preview {
    /// テンプレートに含まれる差し込み欄の式（ループの外）
    pub placeholders: Vec<String>,
    /// 行ループ
    pub loops: Vec<PreviewLoop>,
    pub rows: Vec<PreviewRow>,
    /// 対象の行の総数
    pub total_rows: usize,
}

/// 書き出す前の確認用。テンプレートの欄と、先頭 `limit` 行の値・ファイル名を返す。
pub fn preview(file: &JxcelFile, spec: &Spec, limit: usize, opts: &Options) -> Result<Preview> {
    let (exprs, plan) = collect(spec)?;
    let matrix = evaluate(file, spec, &exprs, &plan, opts)?;
    let pattern = pattern_of(spec);
    let filter = spec.filter.map(str::trim).filter(|f| !f.is_empty());
    let mut used = HashSet::new();
    let rows = matrix
        .iter()
        .take(limit)
        .enumerate()
        .map(|(i, eval)| {
            let lookup = |expr: &str| cell_to_result(&eval.top[exprs.index[expr]]);
            let excluded = filter.is_some_and(|f| lookup(f).is_ok_and(|v| !truthy(&v)));
            let filename = replace_all(&pattern, &mut |e| lookup(e))
                .map(|stem| finish_filename(&stem, i + 1, spec.kind.extension(), &mut used))
                .map_err(|(expr, e)| format!("ファイル名の「{expr}」: {e}"));
            PreviewRow {
                row_no: i + 1,
                excluded,
                filename,
                values: plan.placeholders.iter().map(|p| lookup(p)).collect(),
            }
        })
        .collect();
    let loops = plan
        .loops
        .iter()
        .enumerate()
        .map(|(k, l)| PreviewLoop {
            source: l.source.clone(),
            parent: l.parent,
            exprs: l.exprs.clone(),
            counts: matrix
                .iter()
                .take(limit)
                .map(|e| total_len(e, &plan, k))
                .collect(),
        })
        .collect();
    Ok(Preview {
        placeholders: plan.placeholders,
        loops,
        rows,
        total_rows: matrix.len(),
    })
}
