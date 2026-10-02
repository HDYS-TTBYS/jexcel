//! xlsx への差し込み。
//!
//! 文字列は `xl/sharedStrings.xml`（共有文字列）に入っていて、セルは番号で参照する。同じ文字列を複数のセルが
//! 共有するので、表を直接書き換えず、差し込み欄を含むセルだけを「インライン文字列」に置き換える
//! （セルの書式 `s` はそのまま）。欄だけのセルで値が数値なら、文字列ではなく数値のセルにする
//! （Excel で合計などの計算に使えるように）。欄だけのセルで値が日付（`YYYY-MM-DD`）や日時の文字列で、
//! セルの表示形式が日付なら、Excel の日付（シリアル値）にする（表示形式が日付でなければ文字列のまま）。
//! ページのヘッダー・フッターの欄も差し込む。数式の結果が古いまま残らないよう、開いたときに再計算させる。
//!
//! 行ループ: シートの行のどこかのセルに `{{#each 式}}` と書くと、その行が式の配列の要素の数だけ繰り返される
//! （印は取り除かれ、行の中の欄は要素ごとに差し込まれる）。後ろの行は下へずれ、セルの番地・数式の参照・
//! 結合セルなどもずらす。ループの行を含む範囲（`SUM(C5:C5)` など）は、繰り返した分だけ広がる。
//! 要素が 0 件のときは、数式の参照が壊れないよう、値を空にした行を 1 行残す。
//! ループの入れ子と、ほかのシートへの参照・共有数式・名前の定義のずらしは行わない。

use serde_json::Value;

use crate::package::Package;
use crate::placeholder::{display, find, Match};
use crate::xml::{self, Element, Node};
use crate::{loop_source, Error, Result, Source};

/// 差し込みの途中経過。ループは文書（シート → 行）の出現順に番号を振る。
struct Ctx<'a> {
    src: &'a mut dyn Source,
    next_loop: usize,
    shared: Vec<String>,
    /// セルの書式の番号 → 日付の表示形式か
    date_styles: Vec<bool>,
    /// 1904 年始まりの日付システムか
    date1904: bool,
}

/// いまの位置。
#[derive(Clone, Copy)]
enum Scope {
    Top,
    /// `index` 番目のループの `item` 番目の要素の行
    Item {
        index: usize,
        item: usize,
    },
    /// 要素が 0 件のときに残す空の行（欄は空になる）
    Blank,
}

pub fn process(pkg: &mut Package, src: &mut dyn Source) -> Result<()> {
    let shared = match pkg.get("xl/sharedStrings.xml") {
        Some(b) => shared_strings(&xml::parse(b)?.root),
        None => vec![],
    };
    let date_styles = match pkg.get("xl/styles.xml") {
        Some(b) => date_styles(&xml::parse(b)?.root),
        None => vec![],
    };
    let date1904 = match pkg.get("xl/workbook.xml") {
        Some(b) => {
            let doc = xml::parse(b)?;
            doc.root.children.iter().any(|n| {
                matches!(n, Node::Element(e) if e.local() == "workbookPr"
                    && matches!(e.attr("date1904"), Some("1" | "true")))
            })
        }
        None => false,
    };
    let sheets: Vec<String> = pkg
        .names()
        .filter(|n| {
            n.starts_with("xl/worksheets/") && n.ends_with(".xml") && !n.contains("/_rels/")
        })
        .map(String::from)
        .collect();

    let mut ctx = Ctx {
        src,
        next_loop: 0,
        shared,
        date_styles,
        date1904,
    };
    let mut any = false;
    for name in sheets {
        let mut doc = xml::parse(pkg.get(&name).expect("listed"))?;
        if sheet(&mut doc.root, &mut ctx)? {
            pkg.set(&name, xml::write(&doc)?);
            any = true;
        }
    }
    if any {
        if let Some(b) = pkg.get("xl/workbook.xml") {
            let mut doc = xml::parse(b)?;
            force_recalculation(&mut doc.root);
            pkg.set("xl/workbook.xml", xml::write(&doc)?);
        }
    }
    Ok(())
}

fn expr_error(expr: &str, message: impl Into<String>) -> Error {
    Error::Expr {
        expr: expr.to_string(),
        message: message.into(),
    }
}

/// 共有文字列の一覧。リッチテキスト（複数の `<r>`）はつなげ、ふりがな（`<rPh>`）は除く。
fn shared_strings(sst: &Element) -> Vec<String> {
    sst.children
        .iter()
        .filter_map(|n| {
            if let Node::Element(e) = n {
                Some(e)
            } else {
                None
            }
        })
        .filter(|e| e.local() == "si")
        .map(string_item_text)
        .collect()
}

fn string_item_text(el: &Element) -> String {
    let mut out = String::new();
    for c in &el.children {
        let Node::Element(ce) = c else { continue };
        match ce.local() {
            "t" => out.push_str(&ce.text()),
            "r" => out.push_str(&string_item_text(ce)),
            _ => {}
        }
    }
    out
}

fn child<'a>(el: &'a Element, local: &str) -> Option<&'a Element> {
    el.children.iter().find_map(|n| match n {
        Node::Element(e) if e.local() == local => Some(e),
        _ => None,
    })
}

fn child_mut<'a>(el: &'a mut Element, local: &str) -> Option<&'a mut Element> {
    el.children.iter_mut().find_map(|n| match n {
        Node::Element(e) if e.local() == local => Some(e),
        _ => None,
    })
}

fn prefix(name: &str) -> &str {
    name.rfind(':').map_or("", |i| &name[..=i])
}

// ---- 日付の表示形式 ----

/// `cellXfs` の各書式（セルの `s` の番号）が日付の表示形式かどうか。
fn date_styles(styles: &Element) -> Vec<bool> {
    let custom: Vec<(u32, String)> = child(styles, "numFmts")
        .map(|nf| {
            nf.children
                .iter()
                .filter_map(|n| match n {
                    Node::Element(e) if e.local() == "numFmt" => Some((
                        e.attr("numFmtId")?.parse().ok()?,
                        e.attr("formatCode")?.to_string(),
                    )),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let Some(xfs) = child(styles, "cellXfs") else {
        return vec![];
    };
    xfs.children
        .iter()
        .filter_map(|n| match n {
            Node::Element(e) if e.local() == "xf" => Some(e),
            _ => None,
        })
        .map(|xf| {
            let id: u32 = xf
                .attr("numFmtId")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            match custom.iter().find(|(i, _)| *i == id) {
                Some((_, code)) => is_date_format_code(code),
                None => matches!(id, 14..=22 | 27..=36 | 45..=47 | 50..=58),
            }
        })
        .collect()
}

/// 表示形式の書式コードが日付・時刻か（引用符や `[...]`、エスケープした文字を除いて、y m d h s があるか）。
fn is_date_format_code(code: &str) -> bool {
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                }
            }
            '[' => {
                for q in chars.by_ref() {
                    if q == ']' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                chars.next();
            }
            'y' | 'Y' | 'm' | 'M' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' => return true,
            _ => {}
        }
    }
    false
}

/// 1970-01-01 からの日数（グレゴリオ暦）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `YYYY-MM-DD` または `YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]` を Excel のシリアル値の文字列にする。
/// 日時は書かれたままの時刻（オフセットは無視）。Excel で表せない日付（1900 年より前など）は `None`。
pub fn excel_serial(s: &str, date1904: bool) -> Option<String> {
    let (date, time) = match s.split_once(['T', 't']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let b = date.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (
        date[0..4].parse().ok()?,
        date[5..7].parse().ok()?,
        date[8..10].parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let days = days_from_civil(y, m, d);
    let mut serial = if date1904 {
        days - days_from_civil(1904, 1, 1)
    } else {
        let s = days - days_from_civil(1899, 12, 30);
        // Excel は存在しない 1900-02-29 を数えるので、それより前の日付は 1 つずれる
        if days < days_from_civil(1900, 3, 1) {
            s - 1
        } else {
            s
        }
    };
    if serial < 0 || (!date1904 && days < days_from_civil(1900, 1, 1)) {
        return None;
    }
    let mut frac = 0.0;
    if let Some(t) = time {
        let tb = t.as_bytes();
        if tb.len() < 8 || tb[2] != b':' || tb[5] != b':' {
            return None;
        }
        let (h, mi, sec): (f64, f64, f64) = (
            t[0..2].parse().ok()?,
            t[3..5].parse().ok()?,
            t[6..8].parse().ok()?,
        );
        if h > 23.0 || mi > 59.0 || sec > 60.0 {
            return None;
        }
        frac = (h * 3600.0 + mi * 60.0 + sec) / 86400.0;
    }
    if frac == 0.0 {
        return Some(serial.to_string());
    }
    // 小数部は 10 桁で丸め、桁上がりで日が進むこともある
    let rounded = (frac * 1e10).round() / 1e10;
    if rounded >= 1.0 {
        serial += 1;
        return Some(serial.to_string());
    }
    let text = format!("{rounded:.10}");
    let frac_part = text.trim_start_matches('0').trim_end_matches('0');
    Some(format!("{serial}{frac_part}"))
}

// ---- シート ----

fn sheet(root: &mut Element, ctx: &mut Ctx) -> Result<bool> {
    let mut changed = false;
    let mut moves = Moves::default();
    if let Some(sd) = child_mut(root, "sheetData") {
        changed |= sheet_data(sd, ctx, &mut moves)?;
    }
    if !moves.loops.is_empty() {
        fix_references(root, &moves);
    }
    if let Some(hf) = child_mut(root, "headerFooter") {
        changed |= header_footer(hf, ctx)?;
    }
    if changed {
        drop_cached_results(root);
    }
    Ok(changed)
}

/// 数式セルの計算済みの値を捨てる。Excel は `fullCalcOnLoad` で開いたときに再計算するが、
/// LibreOffice などは保存されている値を信用するので、差し込み前の古い結果が残らないようにする。
fn drop_cached_results(el: &mut Element) {
    if el.local() == "c" && child(el, "f").is_some() {
        el.children
            .retain(|n| !matches!(n, Node::Element(e) if e.local() == "v"));
        if matches!(el.attr("t"), Some("e" | "str" | "n" | "b")) {
            el.remove_attr("t");
        }
        return;
    }
    for n in &mut el.children {
        if let Node::Element(c) = n {
            drop_cached_results(c);
        }
    }
}

/// 行ループで増減した行の記録。
#[derive(Default)]
struct Moves {
    loops: Vec<LoopRows>,
}

struct LoopRows {
    /// テンプレートの行番号（元のシートでの行）
    row: u32,
    /// 繰り返した行の数（要素が 0 件でも 1）
    count: u32,
}

impl Moves {
    fn delta(l: &LoopRows) -> i64 {
        i64::from(l.count) - 1
    }

    /// 元の行番号 `row` の新しい行番号（ループの行そのものは先頭の行）。
    fn point(&self, row: u32) -> u32 {
        let d: i64 = self
            .loops
            .iter()
            .filter(|l| l.row < row)
            .map(Self::delta)
            .sum();
        (i64::from(row) + d).max(1) as u32
    }

    /// 範囲の終わりの行: ループの行を含むなら、繰り返した分だけ広げる。
    fn end(&self, row: u32) -> u32 {
        let d: i64 = self
            .loops
            .iter()
            .filter(|l| l.row <= row)
            .map(Self::delta)
            .sum();
        (i64::from(row) + d).max(1) as u32
    }
}

fn cell_text(c: &Element, shared: &[String]) -> Option<String> {
    match c.attr("t") {
        Some("s") => child(c, "v")
            .and_then(|v| v.text().trim().parse::<usize>().ok())
            .and_then(|i| shared.get(i))
            .cloned(),
        Some("inlineStr") => child(c, "is").map(string_item_text),
        _ => None,
    }
}

/// 行の中にある `{{#each 式}}` の式。複数あればエラー。
fn row_marker(row: &Element, shared: &[String]) -> Result<Option<String>> {
    let mut found = vec![];
    for n in &row.children {
        let Node::Element(c) = n else { continue };
        if c.local() != "c" {
            continue;
        }
        if let Some(text) = cell_text(c, shared) {
            for m in find(&text) {
                if let Some(src) = loop_source(&m.expr) {
                    found.push(src.to_string());
                }
            }
        }
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found.remove(0))),
        _ => Err(expr_error(
            "#each",
            "1 つの行に {{#each}} は 1 つだけ書けます",
        )),
    }
}

fn row_number(row: &Element, prev: u32) -> u32 {
    row.attr("r")
        .and_then(|r| r.parse().ok())
        .unwrap_or(prev + 1)
}

fn sheet_data(sd: &mut Element, ctx: &mut Ctx, moves: &mut Moves) -> Result<bool> {
    let nodes = std::mem::take(&mut sd.children);

    // 1 回目: ループの行と、その要素数を（文書の順に）調べる
    let mut prev = 0;
    let mut plans: Vec<Option<(usize, usize)>> = vec![]; // 行ごとに (ループ番号, 要素数)
    for n in &nodes {
        let Node::Element(row) = n else {
            plans.push(None);
            continue;
        };
        let r = row_number(row, prev);
        prev = r;
        match row_marker(row, &ctx.shared)? {
            None => plans.push(None),
            Some(source) => {
                let index = ctx.next_loop;
                ctx.next_loop += 1;
                let len = ctx
                    .src
                    .loop_len(index, &source)
                    .map_err(|m| expr_error(&format!("#each {source}"), m))?;
                moves.loops.push(LoopRows {
                    row: r,
                    count: len.max(1) as u32,
                });
                plans.push(Some((index, len)));
            }
        }
    }

    // 2 回目: 行番号をずらしながら差し込む
    let mut changed = !moves.loops.is_empty();
    let mut out = Vec::with_capacity(nodes.len());
    let mut prev = 0;
    for (n, plan) in nodes.into_iter().zip(plans) {
        let Node::Element(mut row) = n else {
            out.push(n);
            continue;
        };
        let r = row_number(&row, prev);
        prev = r;
        let new_row = moves.point(r);
        match plan {
            None => {
                place_row(&mut row, new_row, None, moves);
                changed |= cells(&mut row, ctx, Scope::Top)?;
                out.push(Node::Element(row));
            }
            Some((index, len)) => {
                for k in 0..len.max(1) {
                    let mut copy = row.clone();
                    let at = new_row + k as u32;
                    place_row(&mut copy, at, Some((r, at)), moves);
                    let scope = if len == 0 {
                        Scope::Blank
                    } else {
                        Scope::Item { index, item: k }
                    };
                    cells(&mut copy, ctx, scope)?;
                    out.push(Node::Element(copy));
                }
            }
        }
    }
    sd.children = out;
    Ok(changed)
}

/// 行の番号とセルの番地を `row` に付け替え、数式の参照をずらす。`own` は繰り返しの行の（元の行番号, 新しい行番号）。
fn place_row(row: &mut Element, new_row: u32, own: Option<(u32, u32)>, moves: &Moves) {
    row.set_attr("r", &new_row.to_string());
    for n in &mut row.children {
        let Node::Element(c) = n else { continue };
        if c.local() != "c" {
            continue;
        }
        if let Some(r) = c.attr("r") {
            let col: String = r.chars().take_while(char::is_ascii_alphabetic).collect();
            let value = format!("{col}{new_row}");
            c.set_attr("r", &value);
        }
        for f in &mut c.children {
            let Node::Element(fe) = f else { continue };
            if fe.local() == "f" && fe.attr("t") != Some("shared") {
                let text = fe.text();
                if !text.is_empty() {
                    fe.set_text(&shift_formula(&text, moves, own));
                }
            }
        }
    }
}

/// 行の中のセルに差し込む。
fn cells(row: &mut Element, ctx: &mut Ctx, scope: Scope) -> Result<bool> {
    let mut changed = false;
    for n in &mut row.children {
        if let Node::Element(c) = n {
            if c.local() == "c" {
                changed |= cell(c, ctx, scope)?;
            }
        }
    }
    Ok(changed)
}

fn resolve(ctx: &mut Ctx, scope: Scope, expr: &str) -> std::result::Result<Value, String> {
    match scope {
        Scope::Top => ctx.src.value(expr),
        Scope::Item { index, item } => ctx.src.item_value(index, item, expr),
        Scope::Blank => Ok(Value::Null),
    }
}

fn resolve_match(ctx: &mut Ctx, scope: Scope, m: &Match) -> Result<Value> {
    resolve(ctx, scope, &m.expr).map_err(|message| expr_error(&m.expr, message))
}

/// 文字列から、ループの印 `{{#each …}}` を取り除く（繰り返しの行の中だけで呼ぶ）。
fn without_markers(text: &str) -> (String, bool) {
    let mut out = String::new();
    let mut last = 0;
    let mut any = false;
    for m in find(text) {
        if loop_source(&m.expr).is_some() {
            out.push_str(&text[last..m.start]);
            last = m.end;
            any = true;
        }
    }
    out.push_str(&text[last..]);
    (out, any)
}

fn cell(c: &mut Element, ctx: &mut Ctx, scope: Scope) -> Result<bool> {
    let Some(mut text) = cell_text(c, &ctx.shared) else {
        return Ok(false);
    };
    let p = prefix(&c.name).to_string();
    if matches!(scope, Scope::Item { .. } | Scope::Blank) {
        let (cleaned, had_marker) = without_markers(&text);
        if had_marker {
            text = cleaned;
            if find(&text).is_empty() {
                // 印だけのセルは空にする（書式は残す）
                if text.trim().is_empty() {
                    clear_value(c);
                    c.remove_attr("t");
                } else {
                    set_inline(c, &p, &text);
                }
                return Ok(true);
            }
        }
    }
    let matches = find(&text);
    if matches.is_empty() {
        return Ok(false);
    }
    if let Some(m) = matches.iter().find(|m| loop_source(&m.expr).is_some()) {
        let _ = m;
        return Err(expr_error(
            "#each",
            "{{#each}} は、シートの行の中のセルに書いてください",
        ));
    }

    // 欄だけのセル（前後の空白は許す）: 値の型を保つ
    let whole = matches.len() == 1
        && text[..matches[0].start].trim().is_empty()
        && text[matches[0].end..].trim().is_empty();
    if whole {
        match resolve_match(ctx, scope, &matches[0])? {
            Value::Number(n) => {
                set_value(c, &p, None, &n.to_string());
            }
            Value::Bool(b) => {
                set_value(c, &p, Some("b"), if b { "1" } else { "0" });
            }
            Value::String(s) if is_date_cell(c, ctx) => match excel_serial(&s, ctx.date1904) {
                Some(serial) => set_value(c, &p, None, &serial),
                None => set_inline(c, &p, &s),
            },
            other => set_inline(c, &p, &display(&other)),
        }
        return Ok(true);
    }

    let mut out = String::new();
    let mut last = 0;
    for m in &matches {
        out.push_str(&text[last..m.start]);
        out.push_str(&display(&resolve_match(ctx, scope, m)?));
        last = m.end;
    }
    out.push_str(&text[last..]);
    set_inline(c, &p, &out);
    Ok(true)
}

fn is_date_cell(c: &Element, ctx: &Ctx) -> bool {
    c.attr("s")
        .and_then(|s| s.parse::<usize>().ok())
        .and_then(|i| ctx.date_styles.get(i).copied())
        .unwrap_or(false)
}

fn clear_value(c: &mut Element) {
    c.children
        .retain(|n| !matches!(n, Node::Element(e) if e.local() == "v" || e.local() == "is"));
}

/// 数値（`t` なし）または真偽値（`t="b"`）のセルにする。
fn set_value(c: &mut Element, p: &str, t: Option<&str>, v: &str) {
    clear_value(c);
    match t {
        Some(t) => c.set_attr("t", t),
        None => c.remove_attr("t"),
    }
    let mut ve = Element::new(&format!("{p}v"));
    ve.set_text(v);
    c.children.push(Node::Element(ve));
}

fn set_inline(c: &mut Element, p: &str, text: &str) {
    clear_value(c);
    c.set_attr("t", "inlineStr");
    let mut t = Element::new(&format!("{p}t"));
    t.set_attr("xml:space", "preserve");
    t.set_text(text);
    let mut is = Element::new(&format!("{p}is"));
    is.children.push(Node::Element(t));
    c.children.push(Node::Element(is));
}

// ---- ヘッダー・フッター ----

/// ページのヘッダー・フッター（`&C` などの書式コードを含む文字列）の中の欄を差し込む。
/// 書式コードの `&` と区別するため、値の中の `&` は `&&` にする。
fn header_footer(hf: &mut Element, ctx: &mut Ctx) -> Result<bool> {
    let mut changed = false;
    for n in &mut hf.children {
        let Node::Element(e) = n else { continue };
        if !matches!(
            e.local(),
            "oddHeader" | "oddFooter" | "evenHeader" | "evenFooter" | "firstHeader" | "firstFooter"
        ) {
            continue;
        }
        let text = e.text();
        let matches = find(&text);
        if matches.is_empty() {
            continue;
        }
        let mut out = String::new();
        let mut last = 0;
        for m in &matches {
            out.push_str(&text[last..m.start]);
            if loop_source(&m.expr).is_some() {
                return Err(expr_error(
                    "#each",
                    "{{#each}} は、シートの行の中のセルに書いてください",
                ));
            }
            let v = resolve_match(ctx, Scope::Top, m)?;
            out.push_str(&display(&v).replace('&', "&&"));
            last = m.end;
        }
        out.push_str(&text[last..]);
        e.set_text(&out);
        changed = true;
    }
    Ok(changed)
}

// ---- 参照のずらし ----

/// セル参照 `$A$1` の（列の文字, 行番号）。
fn parse_ref(s: &str) -> Option<(&str, &str, u32)> {
    let body = s.strip_prefix('$').unwrap_or(s);
    let col_len = body.chars().take_while(char::is_ascii_alphabetic).count();
    if col_len == 0 || col_len > 3 {
        return None;
    }
    let rest = &body[col_len..];
    let digits = rest.strip_prefix('$').unwrap_or(rest);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let abs_prefix = if s.starts_with('$') { "$" } else { "" };
    let abs_row = if rest.starts_with('$') { "$" } else { "" };
    let col_end = abs_prefix.len() + col_len;
    // 列の文字（先頭の $ を含む）と、行の前の $ を返す
    let col = &s[..col_end];
    Some((col, abs_row, digits.parse().ok()?))
}

#[derive(Clone, Copy)]
enum Pos {
    /// 単独の参照、または範囲の始まり
    Start,
    /// 範囲の終わり
    End,
}

fn map_row(row: u32, pos: Pos, moves: &Moves, own: Option<(u32, u32)>) -> u32 {
    if let Some((orig, new)) = own {
        if row == orig {
            return new;
        }
    }
    match pos {
        Pos::Start => moves.point(row),
        Pos::End => moves.end(row),
    }
}

fn map_cell(s: &str, pos: Pos, moves: &Moves, own: Option<(u32, u32)>) -> Option<String> {
    let (col, abs_row, row) = parse_ref(s)?;
    Some(format!("{col}{abs_row}{}", map_row(row, pos, moves, own)))
}

/// 範囲（`A1:B2`）または単独の参照（`A1`）の文字列をずらす。解釈できなければそのまま返す。
fn map_range(text: &str, moves: &Moves, own: Option<(u32, u32)>) -> String {
    match text.split_once(':') {
        Some((a, b)) => {
            let (Some(x), Some(y)) = (
                map_cell(a, Pos::Start, moves, own),
                map_cell(b, Pos::End, moves, own),
            ) else {
                return text.to_string();
            };
            // 行を消す方向のずれで逆転しないように
            match (parse_ref(&x), parse_ref(&y)) {
                (Some((_, _, r1)), Some((c2, ar, r2))) if r2 < r1 => format!("{x}:{c2}{ar}{r1}"),
                _ => format!("{x}:{y}"),
            }
        }
        None => map_cell(text, Pos::Start, moves, own).unwrap_or_else(|| text.to_string()),
    }
}

/// 数式の中のこのシートのセル参照をずらす。文字列リテラルと、シート名つきの参照（`Sheet2!A1`）は触らない。
fn shift_formula(f: &str, moves: &Moves, own: Option<(u32, u32)>) -> String {
    let chars: Vec<char> = f.chars().collect();
    let mut out = String::with_capacity(f.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' | '\'' => {
                // 文字列リテラル（"" は引用符の文字）または引用符つきのシート名
                let q = c;
                out.push(c);
                i += 1;
                while i < chars.len() {
                    out.push(chars[i]);
                    if chars[i] == q {
                        if chars.get(i + 1) == Some(&q) {
                            out.push(q);
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            '$' | 'A'..='Z' | 'a'..='z' => {
                let prev = out.chars().last();
                let boundary_ok = !prev.is_some_and(|p| p.is_alphanumeric() || "_.!".contains(p));
                let start = i;
                let mut j = i;
                while j < chars.len() && (chars[j].is_alphanumeric() || "$_.".contains(chars[j])) {
                    j += 1;
                }
                let token: String = chars[start..j].iter().collect();
                // 関数名・名前・シート名（直後が `(` や `!`）は参照ではない
                let next = chars.get(j).copied();
                if boundary_ok
                    && parse_ref(&token).is_some()
                    && next != Some('(')
                    && next != Some('!')
                {
                    // 範囲の後半（`:` の後ろ）があれば一緒に扱う
                    if next == Some(':') {
                        let mut k = j + 1;
                        while k < chars.len()
                            && (chars[k].is_alphanumeric() || "$_.".contains(chars[k]))
                        {
                            k += 1;
                        }
                        let second: String = chars[j + 1..k].iter().collect();
                        if parse_ref(&second).is_some() && chars.get(k) != Some(&'(') {
                            out.push_str(&map_range(&format!("{token}:{second}"), moves, own));
                            i = k;
                            continue;
                        }
                    }
                    out.push_str(&map_range(&token, moves, own));
                    i = j;
                    continue;
                }
                out.push_str(&token);
                i = j;
                continue;
            }
            _ => {}
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 行ループで行がずれたことに合わせ、シートの結合セル・条件付き書式・入力規則などの範囲をずらす。
fn fix_references(root: &mut Element, moves: &Moves) {
    for n in &mut root.children {
        let Node::Element(e) = n else { continue };
        match e.local() {
            "dimension" | "autoFilter" => remap_attr(e, "ref", moves),
            "conditionalFormatting" => remap_attr(e, "sqref", moves),
            "dataValidations" | "hyperlinks" => {
                for m in &mut e.children {
                    if let Node::Element(x) = m {
                        remap_attr(
                            x,
                            if x.local() == "hyperlink" {
                                "ref"
                            } else {
                                "sqref"
                            },
                            moves,
                        );
                    }
                }
            }
            "mergeCells" => merge_cells(e, moves),
            _ => {}
        }
    }
}

fn remap_attr(e: &mut Element, attr: &str, moves: &Moves) {
    if let Some(v) = e.attr(attr) {
        let mapped: Vec<String> = v
            .split_whitespace()
            .map(|r| map_range(r, moves, None))
            .collect();
        e.set_attr(attr, &mapped.join(" "));
    }
}

/// 結合セル。ループの行 1 行に収まるものは、繰り返した行ごとに作る。
fn merge_cells(mc: &mut Element, moves: &Moves) {
    let old = std::mem::take(&mut mc.children);
    let mut out = vec![];
    for n in old {
        let Node::Element(m) = n else {
            out.push(n);
            continue;
        };
        let Some(r) = m.attr("ref").map(String::from) else {
            out.push(Node::Element(m));
            continue;
        };
        let single_row = r.split_once(':').and_then(|(a, b)| {
            let (_, _, ra) = parse_ref(a)?;
            let (_, _, rb) = parse_ref(b)?;
            (ra == rb).then_some(ra)
        });
        match single_row.and_then(|row| moves.loops.iter().find(|l| l.row == row)) {
            Some(l) => {
                for k in 0..l.count {
                    let mut copy = m.clone();
                    let at = moves.point(l.row) + k;
                    copy.set_attr("ref", &map_range(&r, moves, Some((l.row, at))));
                    out.push(Node::Element(copy));
                }
            }
            None => {
                let mut copy = m;
                copy.set_attr("ref", &map_range(&r, moves, None));
                out.push(Node::Element(copy));
            }
        }
    }
    let count = out.iter().filter(|n| matches!(n, Node::Element(_))).count();
    mc.children = out;
    if mc.attr("count").is_some() {
        mc.set_attr("count", &count.to_string());
    }
}

/// `<calcPr fullCalcOnLoad="1"/>` にする。無ければ、スキーマの順序（`definedNames` などの後）に挿入する。
fn force_recalculation(workbook: &mut Element) {
    let p = prefix(&workbook.name).to_string();
    for n in &mut workbook.children {
        if let Node::Element(e) = n {
            if e.local() == "calcPr" {
                e.set_attr("fullCalcOnLoad", "1");
                return;
            }
        }
    }
    let after = [
        "sheets",
        "functionGroups",
        "externalReferences",
        "definedNames",
    ];
    let at = workbook
        .children
        .iter()
        .rposition(|n| matches!(n, Node::Element(e) if after.contains(&e.local())))
        .map_or(workbook.children.len(), |i| i + 1);
    let mut calc = Element::new(&format!("{p}calcPr"));
    calc.set_attr("fullCalcOnLoad", "1");
    workbook.children.insert(at, Node::Element(calc));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moves(loops: &[(u32, u32)]) -> Moves {
        Moves {
            loops: loops
                .iter()
                .map(|&(row, count)| LoopRows { row, count })
                .collect(),
        }
    }

    #[test]
    fn serial_numbers_match_excel() {
        assert_eq!(excel_serial("2026-10-02", false).as_deref(), Some("46297"));
        assert_eq!(excel_serial("1900-03-01", false).as_deref(), Some("61"));
        assert_eq!(excel_serial("1900-01-01", false).as_deref(), Some("1"));
        assert_eq!(excel_serial("1970-01-01", false).as_deref(), Some("25569"));
        assert_eq!(excel_serial("2000-02-29", false).as_deref(), Some("36585"));
        assert_eq!(excel_serial("1899-12-31", false), None); // Excel で表せない
        assert_eq!(excel_serial("1904-01-01", true).as_deref(), Some("0"));
        assert_eq!(excel_serial("2026-10-02", true).as_deref(), Some("44835"));
        // 日時は小数部（書かれたままの時刻。オフセットは無視）
        assert_eq!(
            excel_serial("2026-10-02T10:30:00Z", false).as_deref(),
            Some("46297.4375")
        );
        assert_eq!(
            excel_serial("2026-10-02T12:00:00+09:00", false).as_deref(),
            Some("46297.5")
        );
        assert_eq!(
            excel_serial("2026-10-02T00:00:00.000Z", false).as_deref(),
            Some("46297")
        );
        for bad in ["", "2026-13-01", "2026/10/02", "2026-10-02T25:00:00Z", "あ"] {
            assert_eq!(excel_serial(bad, false), None, "{bad}");
        }
    }

    #[test]
    fn date_format_codes() {
        for yes in [
            "yyyy/mm/dd",
            "yyyy\"年\"m\"月\"d\"日\"",
            "[$-411]ggge\"年\"m\"月\"d\"日\"",
            "h:mm:ss",
            "[h]:mm",
            "m/d/yy",
        ] {
            assert!(is_date_format_code(yes), "{yes}");
        }
        for no in [
            "General", "0.00", "#,##0", "@", "0.00E+00", "\"d\"0", "[Red]0.0", "\\d0",
        ] {
            assert!(!is_date_format_code(no), "{no}");
        }
    }

    #[test]
    fn references_shift_around_loop_rows() {
        // 5 行目が 3 行に繰り返される（+2）
        let m = moves(&[(5, 3)]);
        let s = |f: &str| shift_formula(f, &m, None);
        assert_eq!(s("SUM(C5:C5)"), "SUM(C5:C7)"); // 範囲が広がる
        assert_eq!(s("SUM(C4:C5)"), "SUM(C4:C7)");
        assert_eq!(s("SUM(C6:C9)"), "SUM(C8:C11)"); // 後ろは下へ
        assert_eq!(s("A1+B2"), "A1+B2"); // 前はそのまま
        assert_eq!(s("$B$6*C$7"), "$B$8*C$9"); // 絶対参照も行はずれる
        assert_eq!(s("IF(A1=\"A6\",B6,0)"), "IF(A1=\"A6\",B8,0)"); // 文字列は触らない
        assert_eq!(s("Sheet2!A6+LOG10(2)+A6"), "Sheet2!A6+LOG10(2)+A8"); // 他のシートと関数名は触らない
        assert_eq!(s("'Sheet 2'!A6"), "'Sheet 2'!A6");
        assert_eq!(s("SUM(A:A)+SUM(6:6)"), "SUM(A:A)+SUM(6:6)"); // 列・行全体は対象外
                                                                 // 繰り返しの行の中の数式は、自分の行を参照する
        let own = Some((5, 6)); // 5 行目のテンプレートの 2 つ目（6 行目）
        assert_eq!(shift_formula("B5*C5", &m, own), "B6*C6");
        assert_eq!(shift_formula("SUM(C$4:C5)", &m, own), "SUM(C$4:C6)");
        assert_eq!(shift_formula("D9", &m, own), "D11");
    }

    #[test]
    fn two_loops_accumulate_and_zero_items_keep_one_row() {
        let m = moves(&[(3, 2), (6, 1)]);
        assert_eq!(
            (m.point(2), m.point(3), m.point(4), m.point(6), m.point(7)),
            (2, 3, 5, 7, 8)
        );
        assert_eq!((m.end(3), m.end(6), m.end(7)), (4, 7, 8));
        // 0 件でも 1 行は残る（count は 1）ので、行を消す方向のずれはない
        assert_eq!(map_range("A1:B3", &m, None), "A1:B4");
    }

    #[test]
    fn ranges_in_attributes_are_remapped() {
        let m = moves(&[(2, 3)]);
        assert_eq!(map_range("A1:C3", &m, None), "A1:C5");
        assert_eq!(map_range("A4", &m, None), "A6");
        assert_eq!(map_range("A5:B6", &m, None), "A7:B8");
    }
}
