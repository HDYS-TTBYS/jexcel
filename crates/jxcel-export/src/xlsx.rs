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
//! 別の行に `{{/each}}` と書くと、その間の行（印の行を含む）がひとまとめに繰り返される。ループは入れ子にできる
//! （`{{/each}}` を使う形で書く）。数式は、同じ繰り返しの回の中の行を指す参照が、その回のコピーを指す。
//! 定義名（印刷範囲・印刷タイトル・名前付き範囲）の中の参照も、同じ規則でずらす。
//! ほかのシートの数式からの参照・シート名つきの自シートへの参照・共有数式のずらしは行わない。

use serde_json::Value;

use crate::package::Package;
use crate::placeholder::{display, find, Match};
use crate::xml::{self, Element, Node};
use crate::{marker, number, parse_blocks, Block, Error, Marker, Result, Source};

/// 差し込みの途中経過。ループは文書（シート → 行）の出現順に番号を振る。
struct Ctx<'a> {
    src: &'a mut dyn Source,
    next_loop: usize,
    shared: Vec<String>,
    /// セルの書式の番号 → 日付の表示形式か
    date_styles: Vec<bool>,
    /// 1904 年始まりの日付システムか
    date1904: bool,
    /// いま処理しているシートの名前
    sheet_name: Option<String>,
}

/// いまの位置。
#[derive(Clone)]
enum Scope {
    Top,
    /// 外側から順に（ループの番号, 要素の位置）。ループの中の行
    Item(Vec<(usize, usize)>),
    /// 要素が 0 件のときに残す空の行（欄は空になる）
    Blank,
}

impl Scope {
    /// 外側のループの要素の位置の並び。
    fn path(&self) -> Vec<usize> {
        match self {
            Scope::Item(f) => f.iter().map(|x| x.1).collect(),
            _ => vec![],
        }
    }
    fn innermost(&self) -> Option<usize> {
        match self {
            Scope::Item(f) => f.last().map(|x| x.0),
            _ => None,
        }
    }
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
        sheet_name: None,
    };
    let names = sheet_names(pkg)?;
    // 行がずれたシートの記録（シート名 → 記録）。定義名の参照をずらすのに使う
    let mut moved = Moved::new(sheet_order(pkg)?);
    let mut any = false;
    for name in &sheets {
        let mut doc = xml::parse(pkg.get(name).expect("listed"))?;
        let mut moves = None;
        ctx.sheet_name = names.get(name).cloned();
        if sheet(&mut doc.root, &mut ctx, &mut moves)? {
            pkg.set(name, xml::write(&doc)?);
            any = true;
        }
        if let (Some(m), Some(sheet_name)) = (moves, names.get(name)) {
            moved.insert(sheet_name.clone(), m);
        }
    }
    // 行がずれたシートを、ほかのシートの数式が名前つきで指している参照をずらす
    if !moved.is_empty() {
        for name in &sheets {
            let mut doc = xml::parse(pkg.get(name).expect("listed"))?;
            if shift_other_sheet_refs(&mut doc.root, names.get(name).map(String::as_str), &moved) {
                pkg.set(name, xml::write(&doc)?);
            }
        }
    }
    // 図形・画像・グラフの置き場所（行）。行がずれたシートの、描画の部品のアンカーを動かす
    for (part, sheet_name) in &names {
        if let Some(m) = moved.get(sheet_name) {
            shift_drawing_anchors(pkg, part, m)?;
        }
    }
    // グラフの系列の参照（`<c:f>Sheet1!$B$2:$B$9</c:f>`）
    if !moved.is_empty() {
        let charts: Vec<String> = pkg
            .names()
            .filter(|n| {
                n.starts_with("xl/charts/") && n.ends_with(".xml") && !n.contains("/_rels/")
            })
            .map(String::from)
            .collect();
        for name in charts {
            let mut doc = xml::parse(pkg.get(&name).expect("listed"))?;
            if shift_other_sheet_refs(&mut doc.root, None, &moved) {
                pkg.set(&name, xml::write(&doc)?);
            }
        }
    }
    if any {
        if let Some(b) = pkg.get("xl/workbook.xml") {
            let mut doc = xml::parse(b)?;
            force_recalculation(&mut doc.root);
            if !moved.is_empty() {
                shift_defined_names(&mut doc.root, &moved);
            }
            pkg.set("xl/workbook.xml", xml::write(&doc)?);
        }
    }
    if let Some(message) = moved.error.take() {
        return Err(expr_error("3D 参照", message));
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

fn sheet(root: &mut Element, ctx: &mut Ctx, moved: &mut Option<Moves>) -> Result<bool> {
    let mut changed = false;
    let mut moves = Moves::default();
    if let Some(sd) = child_mut(root, "sheetData") {
        changed |= sheet_data(sd, ctx, &mut moves)?;
    }
    if moves.active {
        fix_references(root, &moves, ctx.sheet_name.as_deref());
    }
    if let Some(hf) = child_mut(root, "headerFooter") {
        changed |= header_footer(hf, ctx)?;
    }
    if changed {
        drop_cached_results(root);
    }
    if moves.active {
        *moved = Some(moves);
    }
    Ok(changed)
}

/// シートの関係の定義（`xl/worksheets/_rels/sheet1.xml.rels`）から、描画の部品（`xl/drawings/drawing1.xml`）を探し、
/// アンカーの行（0 から数える）を、行ループでずれたことに合わせて動かす。
/// 始まりは最初のコピー、終わりは最後のコピーの位置にするので、ループを含む範囲に掛かる図は伸びる。
fn shift_drawing_anchors(pkg: &mut Package, sheet_part: &str, moves: &Moves) -> Result<()> {
    let (dir, file) = sheet_part.rsplit_once('/').unwrap_or(("", sheet_part));
    let rels_name = format!("{dir}/_rels/{file}.rels");
    let Some(bytes) = pkg.get(&rels_name) else {
        return Ok(());
    };
    let rels = xml::parse(bytes)?;
    let mut drawings = vec![];
    for n in &rels.root.children {
        let Node::Element(e) = n else { continue };
        if e.attr("Type").is_some_and(|t| t.ends_with("/drawing")) {
            if let Some(t) = e.attr("Target") {
                drawings.push(resolve_part_path(dir, t));
            }
        }
    }
    for name in drawings {
        let Some(bytes) = pkg.get(&name) else {
            continue;
        };
        let mut doc = xml::parse(bytes)?;
        if shift_anchor_rows(&mut doc.root, moves) {
            pkg.set(&name, xml::write(&doc)?);
        }
    }
    Ok(())
}

/// 部品 `dir` からの相対パス（`../drawings/drawing1.xml`）か、絶対パス（`/xl/drawings/…`）を、部品名にする。
fn resolve_part_path(dir: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// `<xdr:from><xdr:row>`・`<xdr:to><xdr:row>`（0 から数える行）を動かす。変えたら true。
fn shift_anchor_rows(el: &mut Element, moves: &Moves) -> bool {
    let mut changed = false;
    for n in &mut el.children {
        let Node::Element(e) = n else { continue };
        if matches!(e.local(), "from" | "to") {
            let is_to = e.local() == "to";
            for m in &mut e.children {
                let Node::Element(r) = m else { continue };
                if r.local() != "row" {
                    continue;
                }
                let Ok(row) = r.text().trim().parse::<u32>() else {
                    continue;
                };
                // アンカーは 0 から、参照は 1 から数える
                let orig = row + 1;
                let new = if is_to {
                    moves.end(orig)
                } else {
                    moves.point(orig)
                } - 1;
                if new != row {
                    r.set_text(&new.to_string());
                    changed = true;
                }
            }
        } else {
            changed |= shift_anchor_rows(e, moves);
        }
    }
    changed
}

/// 数式の文字列を持つ要素（セルの `<f>`、条件付き書式・入力規則の `<formula>`・`<formula1>`・`<formula2>`、
/// グラフの `<c:f>`）か。
fn is_formula_element(e: &Element) -> bool {
    matches!(e.local(), "f" | "formula" | "formula1" | "formula2")
}

fn formula_texts(el: &Element, out: &mut Vec<String>) {
    for n in &el.children {
        let Node::Element(e) = n else { continue };
        if is_formula_element(e) {
            out.push(e.text());
        } else {
            formula_texts(e, out);
        }
    }
}

fn map_formula_elements(el: &mut Element, f: &mut dyn FnMut(&str) -> String) {
    for n in &mut el.children {
        let Node::Element(e) = n else { continue };
        if is_formula_element(e) {
            let text = e.text();
            if !text.is_empty() {
                let shifted = f(&text);
                if shifted != text {
                    e.set_text(&shifted);
                }
            }
        } else {
            map_formula_elements(e, f);
        }
    }
}

/// このシート（またはグラフ）の数式の中の、ほかの（行がずれた）シートへの参照をずらす。変えたら true。
/// 共有数式は、先に 1 つずつの数式に展開する（参照先が動くと、共有の相対位置が崩れるため）。
fn shift_other_sheet_refs(root: &mut Element, own_name: Option<&str>, moved: &Moved) -> bool {
    let mut texts = vec![];
    formula_texts(root, &mut texts);
    if !texts
        .iter()
        .any(|t| !t.is_empty() && shift_sheet_refs(t, moved, own_name) != *t)
    {
        return false;
    }
    if let Some(sd) = child_mut(root, "sheetData") {
        let nodes = std::mem::take(&mut sd.children);
        let (rows, others): (Vec<_>, Vec<_>) = nodes
            .into_iter()
            .partition(|n| matches!(n, Node::Element(_)));
        let mut rows: Vec<Element> = rows
            .into_iter()
            .filter_map(|n| match n {
                Node::Element(e) => Some(e),
                _ => None,
            })
            .collect();
        expand_shared(&mut rows);
        sd.children = others;
        sd.children.extend(rows.into_iter().map(Node::Element));
    }
    map_formula_elements(root, &mut |t| shift_sheet_refs(t, moved, own_name));
    drop_cached_results(root);
    true
}

/// ブックの中のシート名を、タブの並び（`workbook.xml` の `<sheets>` の順）で。
fn sheet_order(pkg: &Package) -> Result<Vec<String>> {
    let Some(wb) = pkg.get("xl/workbook.xml") else {
        return Ok(vec![]);
    };
    let wb = xml::parse(wb)?;
    let mut out = vec![];
    if let Some(sheets) = child(&wb.root, "sheets") {
        for n in &sheets.children {
            if let Node::Element(e) = n {
                if let Some(name) = e.attr("name") {
                    out.push(name.to_string());
                }
            }
        }
    }
    Ok(out)
}

/// 部品のパス（`xl/worksheets/sheet1.xml`）→ シート名。`workbook.xml` と、その関係の定義から引く。
fn sheet_names(pkg: &Package) -> Result<std::collections::HashMap<String, String>> {
    let mut out = std::collections::HashMap::new();
    let (Some(wb), Some(rels)) = (
        pkg.get("xl/workbook.xml"),
        pkg.get("xl/_rels/workbook.xml.rels"),
    ) else {
        return Ok(out);
    };
    let wb = xml::parse(wb)?;
    let rels = xml::parse(rels)?;
    let mut targets = std::collections::HashMap::new();
    for n in &rels.root.children {
        if let Node::Element(e) = n {
            if let (Some(id), Some(t)) = (e.attr("Id"), e.attr("Target")) {
                let path = match t.strip_prefix('/') {
                    Some(abs) => abs.to_string(),
                    None => format!("xl/{t}"),
                };
                targets.insert(id.to_string(), path);
            }
        }
    }
    if let Some(sheets) = child(&wb.root, "sheets") {
        for n in &sheets.children {
            let Node::Element(e) = n else { continue };
            let rid = e
                .attrs
                .iter()
                .find(|(k, _)| k == "id" || k.ends_with(":id"))
                .map(|(_, v)| v.as_str());
            if let (Some(name), Some(path)) = (e.attr("name"), rid.and_then(|r| targets.get(r))) {
                out.insert(path.clone(), name.to_string());
            }
        }
    }
    Ok(out)
}

/// 定義名（印刷範囲・印刷タイトル・名前付き範囲）の中の、行がずれたシートへの参照をずらす。
fn shift_defined_names(workbook: &mut Element, moved: &Moved) {
    let Some(names) = child_mut(workbook, "definedNames") else {
        return;
    };
    for n in &mut names.children {
        let Node::Element(e) = n else { continue };
        if e.local() != "definedName" {
            continue;
        }
        let text = e.text();
        let shifted = shift_sheet_refs(&text, moved, None);
        if shifted != text {
            e.set_text(&shifted);
        }
    }
}

/// `Sheet1!$A$1:$C$8` のような、シート名つきの参照（範囲・行だけの範囲 `Sheet1!$1:$3`）をずらす。
/// 対象は `moved` にあるシートだけ。文字列リテラルの中は触らない。
fn shift_sheet_refs(f: &str, moved: &Moved, skip: Option<&str>) -> String {
    let chars: Vec<char> = f.chars().collect();
    let mut out = String::with_capacity(f.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            // 文字列リテラル（"" は引用符の文字）
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '"' {
                    if chars.get(i + 1) == Some(&'"') {
                        out.push('"');
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
        // シート名: 'Sheet 1'（'' は引用符の文字）か、そのまま書ける名前
        let (sheet, next) = if c == '\'' {
            let mut j = i + 1;
            let mut name = String::new();
            while j < chars.len() {
                if chars[j] == '\'' {
                    if chars.get(j + 1) == Some(&'\'') {
                        name.push('\'');
                        j += 2;
                        continue;
                    }
                    break;
                }
                name.push(chars[j]);
                j += 1;
            }
            (name, j + 1)
        } else if c.is_alphanumeric() || c == '_' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_alphanumeric() || "_.".contains(chars[j])) {
                j += 1;
            }
            (chars[i..j].iter().collect(), j)
        } else {
            out.push(c);
            i += 1;
            continue;
        };
        // 3D 参照: `Sheet1:Sheet3!A1`（名前に空白があれば `'Sheet 1:Sheet 3'!A1`）。範囲の中のシートすべての同じ参照
        let mut bang = next;
        let mut span: Option<(String, String)> = None;
        if chars.get(next) == Some(&':') {
            if let Some(b) = three_d_end(&chars, next + 1) {
                let last: String = chars[next + 1..b].iter().collect();
                span = Some((sheet.clone(), last.trim_matches('\'').replace("''", "'")));
                bang = b;
            }
        } else if c == '\'' && chars.get(next) == Some(&'!') {
            if let Some((x, y)) = sheet.split_once(':') {
                span = Some((x.to_string(), y.to_string()));
            }
        }
        if chars.get(bang) != Some(&'!') {
            out.extend(&chars[i..next.min(chars.len())]);
            i = next.min(chars.len());
            continue;
        }
        let start = i;
        out.extend(&chars[i..=bang]);
        i = bang + 1;
        // `!` の後ろの参照（`$A$1`、`$A$1:$C$8`、`$1:$3`）
        let take = |from: usize| {
            let mut j = from;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '$') {
                j += 1;
            }
            j
        };
        let a_end = take(i);
        let a: String = chars[i..a_end].iter().collect();
        let (b, end) = if chars.get(a_end) == Some(&':') {
            let b_end = take(a_end + 1);
            (
                Some(chars[a_end + 1..b_end].iter().collect::<String>()),
                b_end,
            )
        } else {
            (None, a_end)
        };
        if let Some((x, y)) = span {
            // 範囲の中のシートのどれでも、ずらした結果が同じなら、その結果にする。違えば、黙って壊さずエラー
            let pos = |n: &str| moved.order.iter().position(|o| same_sheet(o, n));
            if let (Some(p), Some(q)) = (pos(&x), pos(&y)) {
                let mapped: Vec<String> = moved.order[p.min(q)..=p.max(q)]
                    .iter()
                    .map(|n| match moved.get(n) {
                        Some(m) => map_sheet_ref(&a, b.as_deref(), m),
                        None => chars[i..end].iter().collect(),
                    })
                    .collect();
                if mapped.iter().all(|m| *m == mapped[0]) {
                    out.push_str(&mapped[0]);
                } else {
                    let whole: String = chars[start..end].iter().collect();
                    moved.fail(format!(
                        "{whole} は、範囲内のシートで行のずれ方が違うため、ずらせません（行ループのあるシートを 3D 参照に含めないでください）"
                    ));
                    out.extend(&chars[i..end]);
                }
            } else {
                out.extend(&chars[i..end]);
            }
            i = end;
            continue;
        }
        match moved
            .get(&sheet)
            .filter(|_| skip.is_none_or(|k| !same_sheet(k, &sheet)))
        {
            Some(m) => out.push_str(&map_sheet_ref(&a, b.as_deref(), m)),
            None => out.extend(&chars[i..end]),
        }
        i = end;
    }
    out
}

/// 3D 参照の 2 つ目のシート名（`from` から）のあとの `!` の位置。
fn three_d_end(chars: &[char], from: usize) -> Option<usize> {
    let mut j = from;
    if chars.get(j) == Some(&'\'') {
        j += 1;
        while j < chars.len() {
            if chars[j] == '\'' {
                if chars.get(j + 1) == Some(&'\'') {
                    j += 2;
                    continue;
                }
                j += 1;
                break;
            }
            j += 1;
        }
    } else {
        while j < chars.len() && (chars[j].is_alphanumeric() || "_.".contains(chars[j])) {
            j += 1;
        }
    }
    (j > from && chars.get(j) == Some(&'!')).then_some(j)
}

/// 行だけの参照（`$3`）の行番号。
fn parse_row_only(s: &str) -> Option<(&str, u32)> {
    let digits = s.strip_prefix('$').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let abs = if s.starts_with('$') { "$" } else { "" };
    Some((abs, digits.parse().ok()?))
}

fn map_sheet_ref(a: &str, b: Option<&str>, moves: &Moves) -> String {
    let rows = |a: &str, b: &str| -> Option<String> {
        let ((xa, ra), (xb, rb)) = (parse_row_only(a)?, parse_row_only(b)?);
        let (ra, rb) = (moves.point(ra), moves.end(rb));
        Some(format!("{xa}{ra}:{xb}{rb}"))
    };
    match b {
        Some(b) => rows(a, b).unwrap_or_else(|| map_range(&format!("{a}:{b}"), moves, &[])),
        None => map_range(a, moves, &[]),
    }
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

/// 行がずれたシートの記録（シート名 → 記録）と、ブックの中のシートの並び。3D 参照（`Sheet1:Sheet3!A1`）を
/// 範囲の中のシートに展開するために並びを持つ。
struct Moved {
    by_name: std::collections::HashMap<String, Moves>,
    /// ブックの中のシート名の並び（`workbook.xml` の `<sheets>` の順）
    order: Vec<String>,
    /// ずらせない参照があったときのメッセージ（最初の 1 件）
    error: std::cell::RefCell<Option<String>>,
}

impl Moved {
    fn new(order: Vec<String>) -> Self {
        Self {
            by_name: Default::default(),
            order,
            error: Default::default(),
        }
    }

    fn insert(&mut self, name: String, m: Moves) {
        self.by_name.insert(name, m);
    }

    fn get(&self, name: &str) -> Option<&Moves> {
        self.by_name.get(name)
    }

    fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    fn fail(&self, message: String) {
        let mut e = self.error.borrow_mut();
        if e.is_none() {
            *e = Some(message);
        }
    }
}

/// 行ループで行がずれたことの記録（元の行番号 → 新しい行番号）。
#[derive(Default)]
struct Moves {
    /// ループがあって行がずれたか
    active: bool,
    /// 元の行（1 から。添字は行番号 - 1）の、繰り返しの最初のコピーと最後のコピーの新しい行番号
    first: Vec<u32>,
    last: Vec<u32>,
    /// 元のシートの最後の行番号と、新しい最後の行との差
    delta: i64,
    /// 繰り返し 1 回ごとの記録（数式・結合セルが、同じ回の別の行を指すために使う）
    owns: Vec<Own>,
    /// 繰り返しの範囲（ループの番号, 先頭の行, 末尾の行）
    blocks: Vec<(usize, u32, u32)>,
}

/// 繰り返しの 1 回分。範囲の各行（元の行番号 `lo`〜`hi`）の、この回での新しい行番号（最初, 最後）。
struct Own {
    lo: u32,
    hi: u32,
    rows: Vec<(u32, u32)>,
    /// 外側の繰り返しの回（`Moves::owns` の添字）
    parent: Option<usize>,
    /// この回のループの番号
    block: usize,
}

impl Moves {
    /// 元の行番号 `row` の新しい行番号（繰り返しの行は最初のコピー）。
    fn point(&self, row: u32) -> u32 {
        match self.first.get((row as usize).wrapping_sub(1)) {
            Some(&n) => n,
            None => (i64::from(row) + self.delta).max(1) as u32,
        }
    }

    /// 範囲の終わりの行: 繰り返しの行を含むなら、最後のコピーまで広げる。
    fn end(&self, row: u32) -> u32 {
        match self.last.get((row as usize).wrapping_sub(1)) {
            Some(&n) => n,
            None => (i64::from(row) + self.delta).max(1) as u32,
        }
    }

    /// 繰り返しの回 `id` から外側へたどった、外側が先の並び。
    fn chain(&self, id: Option<usize>) -> Vec<usize> {
        let mut c = vec![];
        let mut cur = id;
        while let Some(i) = cur {
            c.push(i);
            cur = self.owns[i].parent;
        }
        c.reverse();
        c
    }

    fn refs(&self, chain: &[usize]) -> Vec<&Own> {
        chain.iter().map(|&i| &self.owns[i]).collect()
    }
}

/// 1 行分の配置: 元の行を、新しい行番号・位置・繰り返しの回の並びで出力する。
struct Placed {
    orig: usize,
    new: u32,
    scope: Scope,
    chain: Vec<usize>,
}

/// 繰り返しの範囲（行番号で表したもの）。
struct RBlock {
    source: String,
    index: usize,
    lo: u32,
    hi: u32,
    children: Vec<RBlock>,
}

fn to_rblocks(blocks: &[Block], nums: &[u32]) -> Vec<RBlock> {
    blocks
        .iter()
        .map(|b| RBlock {
            source: b.source.clone(),
            index: b.index,
            lo: nums[b.first],
            hi: nums[b.last],
            children: to_rblocks(&b.children, nums),
        })
        .collect()
}

fn flatten(blocks: &[RBlock], out: &mut Vec<(usize, u32, u32)>) {
    for b in blocks {
        out.push((b.index, b.lo, b.hi));
        flatten(&b.children, out);
    }
}

/// 行番号の順に、繰り返しを展開した行の並びを作る（要素数は `Source::loop_len` で聞く）。
struct Layout<'a, 'b> {
    ctx: &'a mut Ctx<'b>,
    moves: &'a mut Moves,
    placed: Vec<Placed>,
    /// 元の行番号 → 行の添字（シートに実在する行）
    exists: &'a std::collections::HashMap<u32, usize>,
    cursor: u32,
}

impl Layout<'_, '_> {
    fn record(&mut self, orig: u32, chain: &[usize]) {
        let i = orig as usize - 1;
        let new = self.cursor;
        if self.moves.first[i] == 0 {
            self.moves.first[i] = new;
        }
        self.moves.last[i] = new;
        for &id in chain {
            let o = &mut self.moves.owns[id];
            if (o.lo..=o.hi).contains(&orig) {
                let r = &mut o.rows[(orig - o.lo) as usize];
                if r.0 == 0 {
                    r.0 = new;
                }
                r.1 = new;
            }
        }
    }

    fn lay(
        &mut self,
        lo: u32,
        hi: u32,
        blocks: &[RBlock],
        scope: &Scope,
        chain: &mut Vec<usize>,
    ) -> Result<()> {
        let mut x = lo;
        while x <= hi {
            if let Some(b) = blocks.iter().find(|b| b.lo == x) {
                let n = match scope {
                    Scope::Blank => 0,
                    _ => self
                        .ctx
                        .src
                        .loop_len(b.index, scope.innermost(), &scope.path(), &b.source)
                        .map_err(|m| expr_error(&format!("#each {}", b.source), m))?,
                };
                for item in 0..n.max(1) {
                    let inner = match (n, scope) {
                        (0, _) | (_, Scope::Blank) => Scope::Blank,
                        (_, Scope::Item(f)) => {
                            let mut f = f.clone();
                            f.push((b.index, item));
                            Scope::Item(f)
                        }
                        (_, Scope::Top) => Scope::Item(vec![(b.index, item)]),
                    };
                    let id = self.moves.owns.len();
                    self.moves.owns.push(Own {
                        lo: b.lo,
                        hi: b.hi,
                        rows: vec![(0, 0); (b.hi - b.lo + 1) as usize],
                        parent: chain.last().copied(),
                        block: b.index,
                    });
                    chain.push(id);
                    let r = self.lay(b.lo, b.hi, &b.children, &inner, chain);
                    chain.pop();
                    r?;
                }
                x = b.hi + 1;
            } else {
                self.record(x, chain);
                if let Some(&orig) = self.exists.get(&x) {
                    self.placed.push(Placed {
                        orig,
                        new: self.cursor,
                        scope: scope.clone(),
                        chain: chain.clone(),
                    });
                }
                self.cursor += 1;
                x += 1;
            }
        }
        Ok(())
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

/// 行の中にあるループの印（セルの出現順）。
fn row_markers(row: &Element, shared: &[String]) -> Vec<Marker> {
    let mut found = vec![];
    for n in &row.children {
        let Node::Element(c) = n else { continue };
        if c.local() != "c" {
            continue;
        }
        if let Some(text) = cell_text(c, shared) {
            for m in find(&text) {
                if let Some(mk) = marker(&m.expr) {
                    found.push(mk);
                }
            }
        }
    }
    found
}

fn row_number(row: &Element, prev: u32) -> u32 {
    row.attr("r")
        .and_then(|r| r.parse().ok())
        .unwrap_or(prev + 1)
}

fn sheet_data(sd: &mut Element, ctx: &mut Ctx, moves: &mut Moves) -> Result<bool> {
    let nodes = std::mem::take(&mut sd.children);
    let mut others: Vec<Node> = vec![];
    let mut rows: Vec<Element> = vec![];
    let mut nums: Vec<u32> = vec![];
    let mut prev = 0;
    for n in nodes {
        match n {
            Node::Element(row) => {
                let r = row_number(&row, prev);
                prev = r;
                nums.push(r);
                rows.push(row);
            }
            other => others.push(other),
        }
    }

    let marks: Vec<Vec<Marker>> = rows.iter().map(|r| row_markers(r, &ctx.shared)).collect();
    let mut blocks = parse_blocks(&marks)?;
    let mut changed = false;

    // ループがなければ、行はそのまま（欄だけ差し込む）
    if blocks.is_empty() {
        let mut out = others;
        for mut row in rows {
            changed |= cells(&mut row, ctx, &Scope::Top)?;
            out.push(Node::Element(row));
        }
        sd.children = out;
        return Ok(changed);
    }

    expand_shared(&mut rows);
    number(&mut blocks, &mut ctx.next_loop);
    let rblocks = to_rblocks(&blocks, &nums);
    if nums.windows(2).any(|w| w[1] <= w[0]) {
        return Err(expr_error(
            "#each",
            "行番号が昇順でないシートでは、行ループを使えません",
        ));
    }
    let max = *nums.last().expect("ループがあれば行がある");
    moves.first = vec![0; max as usize];
    moves.last = vec![0; max as usize];
    flatten(&rblocks, &mut moves.blocks);
    let exists: std::collections::HashMap<u32, usize> =
        nums.iter().enumerate().map(|(i, &r)| (r, i)).collect();

    // 1 回目: 繰り返しを展開した行の並びと、元の行 → 新しい行の対応を作る
    let mut layout = Layout {
        ctx,
        moves,
        placed: vec![],
        exists: &exists,
        cursor: 1,
    };
    layout.lay(1, max, &rblocks, &Scope::Top, &mut vec![])?;
    let placed = std::mem::take(&mut layout.placed);
    let new_max = layout.cursor - 1;
    moves.delta = i64::from(new_max) - i64::from(max);
    moves.active = true;

    // 2 回目: 行番号をずらしながら差し込む
    let mut out = others;
    for p in placed {
        let mut row = rows[p.orig].clone();
        let refs = moves.refs(&p.chain);
        place_row(&mut row, p.new, &refs, moves, ctx.sheet_name.as_deref());
        cells(&mut row, ctx, &p.scope)?;
        out.push(Node::Element(row));
    }
    sd.children = out;
    Ok(true)
}

/// 行の番号とセルの番地を `row` に付け替え、数式の参照をずらす。`own` は、この行が属する繰り返しの回（外側が先）。
fn place_row(row: &mut Element, new_row: u32, own: &[&Own], moves: &Moves, name: Option<&str>) {
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
            if fe.local() == "f" {
                let text = fe.text();
                if !text.is_empty() {
                    fe.set_text(&shift_formula(&text, name, moves, own));
                }
            }
        }
    }
}

/// 行の中のセルに差し込む。
fn cells(row: &mut Element, ctx: &mut Ctx, scope: &Scope) -> Result<bool> {
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

fn resolve(ctx: &mut Ctx, scope: &Scope, expr: &str) -> std::result::Result<Value, String> {
    match scope {
        Scope::Top => ctx.src.value(expr),
        Scope::Item(f) => {
            let index = f.last().expect("ループの中").0;
            ctx.src.item_value(index, &scope.path(), expr)
        }
        Scope::Blank => Ok(Value::Null),
    }
}

fn resolve_match(ctx: &mut Ctx, scope: &Scope, m: &Match) -> Result<Value> {
    resolve(ctx, scope, &m.expr).map_err(|message| expr_error(&m.expr, message))
}

/// 文字列から、ループの印 `{{#each …}}` `{{/each}}` を取り除く（繰り返しの行の中だけで呼ぶ）。
fn without_markers(text: &str) -> (String, bool) {
    let mut out = String::new();
    let mut last = 0;
    let mut any = false;
    for m in find(text) {
        if marker(&m.expr).is_some() {
            out.push_str(&text[last..m.start]);
            last = m.end;
            any = true;
        }
    }
    out.push_str(&text[last..]);
    (out, any)
}

fn cell(c: &mut Element, ctx: &mut Ctx, scope: &Scope) -> Result<bool> {
    let Some(mut text) = cell_text(c, &ctx.shared) else {
        return Ok(false);
    };
    let p = prefix(&c.name).to_string();
    if matches!(scope, Scope::Item(_) | Scope::Blank) {
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
    if matches.iter().any(|m| marker(&m.expr).is_some()) {
        return Err(expr_error(
            "#each",
            "{{#each}} と {{/each}} は、シートの行の中のセルに書いてください",
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
            if marker(&m.expr).is_some() {
                return Err(expr_error(
                    "#each",
                    "{{#each}} と {{/each}} は、シートの行の中のセルに書いてください",
                ));
            }
            let v = resolve_match(ctx, &Scope::Top, m)?;
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

fn map_row(row: u32, pos: Pos, moves: &Moves, own: &[&Own]) -> u32 {
    // 同じ繰り返しの回の中の行は、その回のコピーを指す（内側の回が優先）
    for o in own.iter().rev() {
        if (o.lo..=o.hi).contains(&row) {
            let (first, last) = o.rows[(row - o.lo) as usize];
            return match pos {
                Pos::Start => first,
                Pos::End => last,
            };
        }
    }
    match pos {
        Pos::Start => moves.point(row),
        Pos::End => moves.end(row),
    }
}

fn map_cell(s: &str, pos: Pos, moves: &Moves, own: &[&Own]) -> Option<String> {
    let (col, abs_row, row) = parse_ref(s)?;
    Some(format!("{col}{abs_row}{}", map_row(row, pos, moves, own)))
}

/// 範囲（`A1:B2`）または単独の参照（`A1`）の文字列をずらす。解釈できなければそのまま返す。
fn map_range(text: &str, moves: &Moves, own: &[&Own]) -> String {
    match text.split_once(':') {
        Some((a, b)) => {
            let (Some(x), Some(y)) = (
                map_cell(a, Pos::Start, moves, own),
                map_cell(b, Pos::End, moves, own),
            ) else {
                // 行だけの範囲（`$3:$5`）。列だけの範囲は行がないので動かない
                return match (parse_row_only(a), parse_row_only(b)) {
                    (Some((xa, ra)), Some((xb, rb))) => {
                        let r1 = map_row(ra, Pos::Start, moves, own);
                        let r2 = map_row(rb, Pos::End, moves, own).max(r1);
                        format!("{xa}{r1}:{xb}{r2}")
                    }
                    _ => text.to_string(),
                };
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

/// 数式の中のセル参照（`A1`・`A1:B2`、`Sheet2!A1`）を順に書き換える。`rewrite(シート名, 参照)` が `Some` を
/// 返せば置き換え、`None` ならそのまま。文字列リテラル・関数名・名前・シート名そのものは参照ではない。
fn rewrite_refs(f: &str, mut rewrite: impl FnMut(Option<&str>, &str) -> Option<String>) -> String {
    let chars: Vec<char> = f.chars().collect();
    let mut out = String::with_capacity(f.len());
    let mut i = 0;
    // 直前にあった `シート名!`
    let mut sheet: Option<String> = None;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' | '\'' => {
                // 文字列リテラル（"" は引用符の文字）または引用符つきのシート名
                let q = c;
                let mut name = String::new();
                out.push(c);
                i += 1;
                while i < chars.len() {
                    out.push(chars[i]);
                    if chars[i] == q {
                        if chars.get(i + 1) == Some(&q) {
                            out.push(q);
                            name.push(q);
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    name.push(chars[i]);
                    i += 1;
                }
                sheet = None;
                if q == '\'' && chars.get(i) == Some(&'!') {
                    out.push('!');
                    i += 1;
                    sheet = Some(name);
                }
                continue;
            }
            c if c == '$' || c.is_alphabetic() => {
                let prev = out.chars().last();
                let boundary_ok = !prev.is_some_and(|p| {
                    p.is_alphanumeric() || "_.".contains(p) || (p == '!' && sheet.is_none())
                });
                let start = i;
                let mut j = i;
                while j < chars.len() && (chars[j].is_alphanumeric() || "$_.".contains(chars[j])) {
                    j += 1;
                }
                let token: String = chars[start..j].iter().collect();
                let next = chars.get(j).copied();
                // 3D 参照（`Sheet1:Sheet3!A1`）は、1 つのシートへの参照ではない。どのシート名にも当たらない印を付ける
                if next == Some(':')
                    && boundary_ok
                    && !is_col_token(&token)
                    && !is_row_token(&token)
                {
                    if let Some(bang) = three_d_end(&chars, j + 1) {
                        out.extend(&chars[start..=bang]);
                        i = bang + 1;
                        sheet = Some("\u{0}3D".to_string());
                        continue;
                    }
                }
                // シート名（直後が `!`）
                if next == Some('!') && boundary_ok {
                    out.push_str(&token);
                    out.push('!');
                    i = j + 1;
                    sheet = Some(token);
                    continue;
                }
                // 列だけ・行だけの範囲（`A:C`・`$3:$5`）。`Sheet1:Sheet3!A1`（3D 参照）や関数名は除く
                if boundary_ok
                    && next == Some(':')
                    && (is_col_token(&token) || is_row_token(&token))
                {
                    let mut k = j + 1;
                    while k < chars.len()
                        && (chars[k].is_alphanumeric() || "$_.".contains(chars[k]))
                    {
                        k += 1;
                    }
                    let second: String = chars[j + 1..k].iter().collect();
                    let same_kind = if is_col_token(&token) {
                        is_col_token(&second)
                    } else {
                        is_row_token(&second)
                    };
                    if same_kind && !matches!(chars.get(k), Some('(') | Some('!')) {
                        let text = format!("{token}:{second}");
                        match rewrite(sheet.as_deref(), &text) {
                            Some(r) => out.push_str(&r),
                            None => out.extend(&chars[start..k]),
                        }
                        sheet = None;
                        i = k;
                        continue;
                    }
                }
                // 関数名・名前（直後が `(`）は参照ではない
                if boundary_ok && parse_ref(&token).is_some() && next != Some('(') {
                    let mut text = token.clone();
                    let mut end = j;
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
                            text = format!("{token}:{second}");
                            end = k;
                        }
                    }
                    match rewrite(sheet.as_deref(), &text) {
                        Some(r) => out.push_str(&r),
                        None => out.extend(&chars[start..end]),
                    }
                    sheet = None;
                    i = end;
                    continue;
                }
                out.push_str(&token);
                sheet = None;
                i = j;
                continue;
            }
            c if c.is_ascii_digit() => {
                // 行だけの範囲（`3:5`）。数値の一部（`1.5`・`2E3`）は参照ではない
                let prev = out.chars().last();
                let boundary_ok = !prev.is_some_and(|p| p.is_alphanumeric() || "_.$".contains(p));
                let mut j = i;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if boundary_ok && chars.get(j) == Some(&':') {
                    let mut k = j + 1;
                    if chars.get(k) == Some(&'$') {
                        k += 1;
                    }
                    let digits_from = k;
                    while k < chars.len() && chars[k].is_ascii_digit() {
                        k += 1;
                    }
                    let tail = chars.get(k).copied();
                    if k > digits_from
                        && !tail.is_some_and(|t| t.is_alphanumeric() || "_.(!".contains(t))
                    {
                        let text: String = chars[i..k].iter().collect();
                        match rewrite(sheet.as_deref(), &text) {
                            Some(r) => out.push_str(&r),
                            None => out.push_str(&text),
                        }
                        sheet = None;
                        i = k;
                        continue;
                    }
                }
                sheet = None;
                out.extend(&chars[i..j]);
                i = j;
                continue;
            }
            _ => {}
        }
        sheet = None;
        out.push(c);
        i += 1;
    }
    out
}

/// 列だけの参照（`A`・`$AB`）の列の文字。
fn is_col_token(t: &str) -> bool {
    let b = t.strip_prefix('$').unwrap_or(t);
    (1..=3).contains(&b.len()) && b.bytes().all(|c| c.is_ascii_alphabetic())
}

/// 行だけの参照（`3`・`$3`）の行番号。
fn is_row_token(t: &str) -> bool {
    let b = t.strip_prefix('$').unwrap_or(t);
    !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit())
}

fn same_sheet(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// 数式の中のこのシートのセル参照をずらす。このシート自身を名前つきで指す参照（`Sheet1!B6`）も同じ。
/// ほかのシートへの参照は触らない（`shift_sheet_refs` が、全シートの処理のあとでずらす）。
fn shift_formula(f: &str, own_name: Option<&str>, moves: &Moves, own: &[&Own]) -> String {
    rewrite_refs(f, |sheet, text| match (sheet, own_name) {
        (None, _) => Some(map_range(text, moves, own)),
        (Some(s), Some(me)) if same_sheet(s, me) => Some(map_range(text, moves, own)),
        _ => None,
    })
}

/// 列の文字 → 0 から数えた列番号（`A` → 0）。
fn col_index(col: &str) -> Option<i64> {
    let letters = col.trim_start_matches('$');
    if letters.is_empty() {
        return None;
    }
    letters
        .bytes()
        .try_fold(0i64, |n, b| {
            b.is_ascii_alphabetic()
                .then(|| n * 26 + i64::from(b.to_ascii_uppercase() - b'A' + 1))
        })
        .map(|n| n - 1)
}

fn col_letters(mut n: i64) -> String {
    let mut s = vec![];
    n += 1;
    while n > 0 {
        s.push(b'A' + ((n - 1) % 26) as u8);
        n = (n - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).expect("ascii")
}

/// 共有数式の元の式を `drow` 行・`dcol` 列だけ離れたセルに写したもの（`$` のない側だけ動く）。
fn translate_formula(f: &str, drow: i64, dcol: i64) -> String {
    let one = |r: &str| -> Option<String> {
        let (col, abs_row, row) = parse_ref(r)?;
        let col = if col.starts_with('$') {
            col.to_string()
        } else {
            col_letters((col_index(col)? + dcol).clamp(0, 16383))
        };
        let row = if abs_row == "$" {
            i64::from(row)
        } else {
            (i64::from(row) + drow).max(1)
        };
        Some(format!("{col}{abs_row}{row}"))
    };
    // セル・行だけ・列だけの、どれか
    let part = |r: &str| -> Option<String> {
        if let Some(x) = one(r) {
            return Some(x);
        }
        if let Some((abs, row)) = parse_row_only(r) {
            let row = if abs == "$" {
                i64::from(row)
            } else {
                (i64::from(row) + drow).max(1)
            };
            return Some(format!("{abs}{row}"));
        }
        if is_col_token(r) {
            return Some(if r.starts_with('$') {
                r.to_string()
            } else {
                col_letters((col_index(r)? + dcol).clamp(0, 16383))
            });
        }
        None
    };
    rewrite_refs(f, |_, text| match text.split_once(':') {
        Some((a, b)) => Some(format!("{}:{}", part(a)?, part(b)?)),
        None => part(text),
    })
}

/// 共有数式（`<f t="shared" si="…">`）を、セルごとの通常の数式に展開する。行をずらすと、
/// 共有の範囲の相対位置が崩れるため。`rows` は元の座標の行。
fn expand_shared(rows: &mut [Element]) {
    let pos = |c: &Element| -> Option<(i64, i64)> {
        let r = c.attr("r")?;
        let (col, _, row) = parse_ref(r)?;
        Some((i64::from(row), col_index(col)?))
    };
    let mut masters: std::collections::HashMap<String, (i64, i64, String)> = Default::default();
    for row in rows.iter() {
        for n in &row.children {
            let Node::Element(c) = n else { continue };
            let Some((r, col)) = pos(c) else { continue };
            if let Some(f) = child(c, "f") {
                if let (Some("shared"), Some(si)) = (f.attr("t"), f.attr("si")) {
                    let text = f.text();
                    if !text.is_empty() {
                        masters.insert(si.to_string(), (r, col, text));
                    }
                }
            }
        }
    }
    for row in rows.iter_mut() {
        for n in &mut row.children {
            let Node::Element(c) = n else { continue };
            let at = pos(c);
            let Some(f) = child_mut(c, "f") else { continue };
            if f.attr("t") != Some("shared") {
                continue;
            }
            let Some(si) = f.attr("si").map(String::from) else {
                continue;
            };
            let Some((mr, mc, text)) = masters.get(&si) else {
                continue;
            };
            let formula = if f.text().is_empty() {
                let Some((r, col)) = at else { continue };
                translate_formula(text, r - mr, col - mc)
            } else {
                text.clone()
            };
            f.set_text(&formula);
            for a in ["t", "si", "ref"] {
                f.remove_attr(a);
            }
        }
    }
}

/// 行ループで行がずれたことに合わせ、シートの結合セル・条件付き書式・入力規則などの範囲をずらす。
fn fix_references(root: &mut Element, moves: &Moves, name: Option<&str>) {
    for n in &mut root.children {
        let Node::Element(e) = n else { continue };
        match e.local() {
            "dimension" | "autoFilter" => remap_attr(e, "ref", moves),
            "conditionalFormatting" => {
                remap_attr(e, "sqref", moves);
                shift_rule_formulas(e, name, moves);
            }
            "dataValidations" | "hyperlinks" => {
                for m in &mut e.children {
                    if let Node::Element(x) = m {
                        shift_rule_formulas(x, name, moves);
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

/// 条件付き書式・入力規則の式（`<formula>`・`<formula1>`・`<formula2>`）の中の参照をずらす。
fn shift_rule_formulas(e: &mut Element, name: Option<&str>, moves: &Moves) {
    for n in &mut e.children {
        let Node::Element(c) = n else { continue };
        if matches!(c.local(), "formula" | "formula1" | "formula2") {
            let text = c.text();
            if !text.is_empty() {
                c.set_text(&shift_formula(&text, name, moves, &[]));
            }
        } else {
            shift_rule_formulas(c, name, moves);
        }
    }
}

fn remap_attr(e: &mut Element, attr: &str, moves: &Moves) {
    if let Some(v) = e.attr(attr) {
        let mapped: Vec<String> = v
            .split_whitespace()
            .map(|r| map_range(r, moves, &[]))
            .collect();
        e.set_attr(attr, &mapped.join(" "));
    }
}

/// 結合セル。繰り返しの範囲に収まるものは、繰り返した回ごとに作る。
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
        let rows = r.split_once(':').and_then(|(a, b)| {
            let (_, _, ra) = parse_ref(a)?;
            let (_, _, rb) = parse_ref(b)?;
            Some((ra.min(rb), ra.max(rb)))
        });
        // 範囲を含む一番内側の繰り返し
        let block = rows.and_then(|(ra, rb)| {
            moves
                .blocks
                .iter()
                .filter(|(_, lo, hi)| *lo <= ra && rb <= *hi)
                .min_by_key(|(i, lo, hi)| (hi - lo, std::cmp::Reverse(*i)))
                .map(|b| b.0)
        });
        match block {
            Some(block) => {
                for (id, o) in moves.owns.iter().enumerate() {
                    if o.block != block {
                        continue;
                    }
                    let chain = moves.chain(Some(id));
                    let mut copy = m.clone();
                    copy.set_attr("ref", &map_range(&r, moves, &moves.refs(&chain)));
                    out.push(Node::Element(copy));
                }
            }
            None => {
                let mut copy = m;
                copy.set_attr("ref", &map_range(&r, moves, &[]));
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

    /// 行 `row` が `count` 回に繰り返されたシート（最後の行は `max`）。繰り返しの行の位置は `Own` で返す。
    fn moves(loops: &[(u32, u32)], max: u32) -> (Moves, Vec<Own>) {
        let mut m = Moves {
            active: true,
            first: vec![0; max as usize],
            last: vec![0; max as usize],
            ..Default::default()
        };
        let mut owns = vec![];
        let mut cursor = 1;
        for x in 1..=max {
            match loops.iter().find(|l| l.0 == x) {
                Some(&(_, count)) => {
                    m.first[x as usize - 1] = cursor;
                    for k in 0..count {
                        owns.push(Own {
                            lo: x,
                            hi: x,
                            rows: vec![(cursor + k, cursor + k)],
                            parent: None,
                            block: 0,
                        });
                    }
                    cursor += count;
                    m.last[x as usize - 1] = cursor - 1;
                }
                None => {
                    m.first[x as usize - 1] = cursor;
                    m.last[x as usize - 1] = cursor;
                    cursor += 1;
                }
            }
        }
        m.delta = i64::from(cursor - 1) - i64::from(max);
        (m, owns)
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
        let (m, owns) = moves(&[(5, 3)], 12);
        let s = |f: &str| shift_formula(f, None, &m, &[]);
        assert_eq!(s("SUM(C5:C5)"), "SUM(C5:C7)"); // 範囲が広がる
        assert_eq!(s("SUM(C4:C5)"), "SUM(C4:C7)");
        assert_eq!(s("SUM(C6:C9)"), "SUM(C8:C11)"); // 後ろは下へ
        assert_eq!(s("A1+B2"), "A1+B2"); // 前はそのまま
        assert_eq!(s("$B$6*C$7"), "$B$8*C$9"); // 絶対参照も行はずれる
        assert_eq!(s("IF(A1=\"A6\",B6,0)"), "IF(A1=\"A6\",B8,0)"); // 文字列は触らない
        assert_eq!(s("Sheet2!A6+LOG10(2)+A6"), "Sheet2!A6+LOG10(2)+A8"); // 他のシートと関数名は触らない
        assert_eq!(s("'Sheet 2'!A6"), "'Sheet 2'!A6");
        assert_eq!(s("SUM(A:A)+SUM(6:6)"), "SUM(A:A)+SUM(8:8)"); // 行全体は行がずれ、列全体は動かない
                                                                 // 繰り返しの行の中の数式は、自分の行を参照する
        let own = [&owns[1]]; // 5 行目のテンプレートの 2 つ目（6 行目）
        assert_eq!(shift_formula("B5*C5", None, &m, &own), "B6*C6");
        assert_eq!(shift_formula("SUM(C$4:C5)", None, &m, &own), "SUM(C$4:C6)");
        assert_eq!(shift_formula("D9", None, &m, &own), "D11");
    }

    #[test]
    fn two_loops_accumulate_and_zero_items_keep_one_row() {
        let (m, _) = moves(&[(3, 2), (6, 1)], 9);
        assert_eq!(
            (m.point(2), m.point(3), m.point(4), m.point(6), m.point(7)),
            (2, 3, 5, 7, 8)
        );
        assert_eq!((m.end(3), m.end(6), m.end(7)), (4, 7, 8));
        // 0 件でも 1 行は残る（count は 1）ので、行を消す方向のずれはない
        assert_eq!(map_range("A1:B3", &m, &[]), "A1:B4");
    }

    #[test]
    fn sheet_qualified_references_are_shifted_only_for_moved_sheets() {
        let (m, _) = moves(&[(5, 3)], 12);
        let mut moved = Moved::new(vec![]);
        moved.insert("請求書".to_string(), m);
        moved.insert("Sheet 2".to_string(), moves(&[(2, 2)], 4).0);
        let s = |f: &str| shift_sheet_refs(f, &moved, None);
        assert_eq!(s("請求書!$A$1:$C$8"), "請求書!$A$1:$C$10");
        assert_eq!(s("'請求書'!$B$6"), "'請求書'!$B$8");
        assert_eq!(s("'Sheet 2'!$1:$3"), "'Sheet 2'!$1:$4"); // 行だけの範囲
        assert_eq!(s("'請求書'!$A:$C"), "'請求書'!$A:$C"); // 列だけは行がない
        assert_eq!(s("他!A6+請求書!A6"), "他!A6+請求書!A8"); // 動いていないシートは触らない
        assert_eq!(s("SUM(請求書!A6:B7)*A6"), "SUM(請求書!A8:B9)*A6"); // 関数の中・シート名なしの参照
        assert_eq!(s("\"請求書!A6\""), "\"請求書!A6\""); // 文字列リテラル
        assert_eq!(s("'It''s'!A6"), "'It''s'!A6");
    }

    #[test]
    fn ranges_in_attributes_are_remapped() {
        let (m, _) = moves(&[(2, 3)], 8);
        assert_eq!(map_range("A1:C3", &m, &[]), "A1:C5");
        assert_eq!(map_range("A4", &m, &[]), "A6");
        assert_eq!(map_range("A5:B6", &m, &[]), "A7:B8");
    }

    #[test]
    fn shared_formulas_are_translated_relative_to_the_master() {
        assert_eq!(translate_formula("B2*2", 1, 0), "B3*2");
        assert_eq!(translate_formula("$A2+B$1", 3, 1), "$A5+C$1");
        assert_eq!(translate_formula("SUM(A1:B2)", 2, 2), "SUM(C3:D4)");
        assert_eq!(
            translate_formula("Sheet2!A1+\"A1\"", 1, 0),
            "Sheet2!A2+\"A1\""
        );
        assert_eq!(translate_formula("LOG10(Z9)", 0, 1), "LOG10(AA9)");
        assert_eq!(col_letters(col_index("AZ").unwrap()), "AZ");
    }

    #[test]
    fn own_sheet_qualified_references_are_shifted_but_other_sheets_are_not() {
        let (m, _) = moves(&[(5, 3)], 12);
        let s = |f: &str| shift_formula(f, Some("請求書"), &m, &[]);
        assert_eq!(s("請求書!B6+B6"), "請求書!B8+B8");
        assert_eq!(s("'請求書'!$B$6"), "'請求書'!$B$8");
        assert_eq!(s("他!B6+B6"), "他!B6+B8");
        assert_eq!(s("'It''s'!B6"), "'It''s'!B6");
    }

    #[test]
    fn row_only_and_column_only_ranges_in_formulas() {
        let (m, _) = moves(&[(5, 3)], 12);
        let s = |f: &str| shift_formula(f, None, &m, &[]);
        assert_eq!(s("SUM(3:6)"), "SUM(3:8)"); // 行だけの範囲は、ループを含めば最後のコピーまで
        assert_eq!(s("SUM($6:$7)"), "SUM($8:$9)");
        assert_eq!(s("SUM(A:C)+B6"), "SUM(A:C)+B8"); // 列だけの範囲は動かない
        assert_eq!(s("1.5+2E3+IF(A1,1:2)"), "1.5+2E3+IF(A1,1:2)"); // 数値・関数の引数の中は読み違えない
        assert_eq!(s("Jan:Dec!A6"), "Jan:Dec!A6"); // 3D 参照は参照として読まない
    }

    #[test]
    fn shared_formulas_translate_row_and_column_ranges() {
        assert_eq!(translate_formula("SUM(A:A)", 3, 1), "SUM(B:B)");
        assert_eq!(translate_formula("SUM(2:3)", 2, 1), "SUM(4:5)");
        assert_eq!(
            translate_formula("SUM($A:$B,$2:$3)", 2, 1),
            "SUM($A:$B,$2:$3)"
        );
    }

    #[test]
    fn three_d_references_are_shifted_only_when_every_sheet_in_the_span_agrees() {
        let order: Vec<String> = ["A", "B", "C", "D", "Sheet 5"].map(String::from).into();
        let mut moved = Moved::new(order.clone());
        moved.insert("B".to_string(), moves(&[(5, 3)], 12).0);
        moved.insert("C".to_string(), moves(&[(5, 3)], 12).0);
        let s = |f: &str| shift_sheet_refs(f, &moved, None);
        // 範囲内の動いたシートが同じずれ方（B と C）なら、そのとおりにずらす
        assert_eq!(s("SUM(B:C!A6)"), "SUM(B:C!A8)");
        assert_eq!(moved.error.borrow().as_deref(), None);
        // ずれない位置（ループより前）の参照は、どの範囲でも同じなのでそのまま
        assert_eq!(s("SUM(A:D!A2)"), "SUM(A:D!A2)");
        assert_eq!(moved.error.borrow().as_deref(), None);
        // 動いていないシートを含む範囲で、ずれる行を指すと、結果がそろわない → エラー
        assert_eq!(s("SUM(A:C!A6)"), "SUM(A:C!A6)");
        let e = moved.error.borrow().clone().unwrap();
        assert!(e.contains("A:C!A6") && e.contains("3D 参照"), "{e}");
        // 引用符つき（空白のある名前）・文字列リテラルの中・普通の参照
        let mut m2 = Moved::new(order);
        m2.insert("Sheet 5".to_string(), moves(&[(5, 3)], 12).0);
        let t = |f: &str| shift_sheet_refs(f, &m2, None);
        assert_eq!(t("'Sheet 5:Sheet 5'!A6"), "'Sheet 5:Sheet 5'!A8");
        assert_eq!(t("\"A:C!A6\""), "\"A:C!A6\"");
        assert_eq!(t("'Sheet 5'!A6"), "'Sheet 5'!A8");
    }
}
