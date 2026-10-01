//! xlsx への差し込み。
//!
//! 文字列は `xl/sharedStrings.xml`（共有文字列）に入っていて、セルは番号で参照する。同じ文字列を複数のセルが
//! 共有するので、表を直接書き換えず、差し込み欄を含むセルだけを「インライン文字列」に置き換える
//! （セルの書式 `s` はそのまま）。欄だけのセルで値が数値なら、文字列ではなく数値のセルにする
//! （Excel で合計などの計算に使えるように）。数式の結果が古いまま残らないよう、開いたときに再計算させる。

use serde_json::Value;

use crate::package::Package;
use crate::placeholder::{display, find};
use crate::xml::{self, Element, Node};
use crate::Result;

type Resolver<'a> = dyn FnMut(&str) -> std::result::Result<Value, String> + 'a;

pub fn process(pkg: &mut Package, resolve: &mut Resolver) -> Result<()> {
    let shared = match pkg.get("xl/sharedStrings.xml") {
        Some(b) => shared_strings(&xml::parse(b)?.root),
        None => vec![],
    };
    let sheets: Vec<String> = pkg
        .names()
        .filter(|n| {
            n.starts_with("xl/worksheets/") && n.ends_with(".xml") && !n.contains("/_rels/")
        })
        .map(String::from)
        .collect();

    let mut any = false;
    for name in sheets {
        let mut doc = xml::parse(pkg.get(&name).expect("listed"))?;
        if walk(&mut doc.root, &shared, resolve)? {
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

fn walk(el: &mut Element, shared: &[String], resolve: &mut Resolver) -> Result<bool> {
    let mut changed = false;
    if el.local() == "c" {
        return cell(el, shared, resolve);
    }
    for child in &mut el.children {
        if let Node::Element(c) = child {
            changed |= walk(c, shared, resolve)?;
        }
    }
    Ok(changed)
}

fn child<'a>(el: &'a Element, local: &str) -> Option<&'a Element> {
    el.children.iter().find_map(|n| match n {
        Node::Element(e) if e.local() == local => Some(e),
        _ => None,
    })
}

fn prefix(name: &str) -> &str {
    name.rfind(':').map_or("", |i| &name[..=i])
}

fn cell(c: &mut Element, shared: &[String], resolve: &mut Resolver) -> Result<bool> {
    let text = match c.attr("t") {
        Some("s") => child(c, "v")
            .and_then(|v| v.text().trim().parse::<usize>().ok())
            .and_then(|i| shared.get(i))
            .cloned(),
        Some("inlineStr") => child(c, "is").map(string_item_text),
        _ => None,
    };
    let Some(text) = text else { return Ok(false) };
    let matches = find(&text);
    if matches.is_empty() {
        return Ok(false);
    }

    let resolve_one = |expr: &str, resolve: &mut Resolver| {
        resolve(expr).map_err(|message| crate::Error::Expr {
            expr: expr.to_string(),
            message,
        })
    };

    // 欄だけのセル（前後の空白は許す）: 値の型を保つ
    let whole = matches.len() == 1
        && text[..matches[0].start].trim().is_empty()
        && text[matches[0].end..].trim().is_empty();
    let p = prefix(&c.name).to_string();
    if whole {
        match resolve_one(&matches[0].expr, resolve)? {
            Value::Number(n) => {
                set_value(c, &p, None, &n.to_string());
                return Ok(true);
            }
            Value::Bool(b) => {
                set_value(c, &p, Some("b"), if b { "1" } else { "0" });
                return Ok(true);
            }
            other => {
                set_inline(c, &p, &display(&other));
                return Ok(true);
            }
        }
    }

    let mut out = String::new();
    let mut last = 0;
    for m in &matches {
        out.push_str(&text[last..m.start]);
        out.push_str(&display(&resolve_one(&m.expr, resolve)?));
        last = m.end;
    }
    out.push_str(&text[last..]);
    set_inline(c, &p, &out);
    Ok(true)
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
