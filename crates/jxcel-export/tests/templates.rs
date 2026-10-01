//! 実際の Word / Excel（python-docx・openpyxl・LibreOffice）で作ったテンプレートを使った差し込みのテスト。
//! テンプレートは tools/make_export_fixtures.py で生成してコミットしてある。

use jxcel_export::package::Package;
use jxcel_export::xml::{self, Element, Node};
use jxcel_export::{render, scan, Error, Kind};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const DOCX: &[u8] = include_bytes!("fixtures/invoice.docx");
const XLSX: &[u8] = include_bytes!("fixtures/invoice.xlsx"); // 共有文字列（Excel と同じ形）
const XLSX_INLINE: &[u8] = include_bytes!("fixtures/invoice-inline.xlsx");

fn values() -> BTreeMap<&'static str, Value> {
    BTreeMap::from([
        ("取引先", json!("株式会社テスト & 協力会社")),
        ("数量", json!(3)),
        ("単価", json!(1200)),
        (r#"区分 == "A" && 数量 > 1 ? "○" : "×""#, json!("○")),
        ("数量 * 単価", json!(3600)),
        ("品名", json!("ねじ")),
        ("備考", json!("一行目\n二行目 <重要>")),
        ("請求番号", json!("INV-001")),
        ("発行日", json!("2024-01-31")),
        ("完了", json!(true)),
    ])
}

fn do_render(kind: Kind, template: &[u8]) -> Vec<u8> {
    let vals = values();
    render(kind, template, &mut |e| {
        vals.get(e).cloned().ok_or_else(|| format!("未定義: {e}"))
    })
    .unwrap_or_else(|e| panic!("{e}"))
}

fn part(zip: &[u8], name: &str) -> Element {
    let pkg = Package::read(zip).unwrap();
    xml::parse(
        pkg.get(name)
            .unwrap_or_else(|| panic!("{name} がありません")),
    )
    .unwrap()
    .root
}

/// 段落ごとのテキスト（`<w:br/>` は改行）。
fn paragraphs(root: &Element) -> Vec<String> {
    fn text_of(el: &Element, out: &mut String) {
        for c in &el.children {
            if let Node::Element(e) = c {
                match e.name.as_str() {
                    "w:t" => out.push_str(&e.text()),
                    "w:br" => out.push('\n'),
                    "w:p" => {}
                    _ => text_of(e, out),
                }
            }
        }
    }
    fn walk(el: &Element, out: &mut Vec<String>) {
        if el.name == "w:p" {
            let mut s = String::new();
            text_of(el, &mut s);
            out.push(s);
        }
        for c in &el.children {
            if let Node::Element(e) = c {
                walk(e, out);
            }
        }
    }
    let mut out = vec![];
    walk(root, &mut out);
    out
}

// ---------------------------------------------------------------- docx

#[test]
fn docx_scan_finds_every_placeholder_even_when_split_across_runs() {
    let found = scan(Kind::Docx, DOCX).unwrap();
    assert_eq!(&found[..2], ["取引先", "数量"], "{found:?}");
    for want in [
        "取引先",
        "数量",
        r#"区分 == "A" && 数量 > 1 ? "○" : "×""#,
        "数量 * 単価",
        "品名",
        "備考",
        "請求番号",
        "発行日",
    ] {
        assert!(
            found.iter().any(|f| f == want),
            "{want} が見つかりません: {found:?}"
        );
    }
    // 重複はまとめる
    assert_eq!(found.iter().filter(|f| *f == "数量").count(), 1);
}

#[test]
fn docx_render_substitutes_text_across_runs_tables_and_headers() {
    let out = do_render(Kind::Docx, DOCX);
    let body = paragraphs(&part(&out, "word/document.xml"));

    // 欄が複数のランに分割されていても置換される（2 つ・3 つのランにまたがる）
    assert!(
        body.contains(&"宛先: 株式会社テスト & 協力会社 御中".to_string()),
        "{body:?}"
    );
    assert!(body.contains(&"ねじ（3）".to_string()), "{body:?}");
    // 1 つのラン内の複数の欄、式の中の & < > "
    assert!(
        body.contains(&"数量 3 個 / 判定 ○ / 合計 3600 円".to_string()),
        "{body:?}"
    );
    // 表のセル
    for cell in ["ねじ", "3", "3600"] {
        assert!(
            body.contains(&cell.to_string()),
            "表のセル {cell}: {body:?}"
        );
    }
    // 改行を含む値は <w:br/> になり、< > は文字として残る
    assert!(
        body.contains(&"備考: 一行目\n二行目 <重要>".to_string()),
        "{body:?}"
    );
    // 欄のない段落・前後の空白は変わらない
    assert!(body.contains(&"下記のとおりご請求申し上げます。".to_string()));
    assert!(
        body.contains(&"  先頭と末尾に空白  ".to_string()),
        "{body:?}"
    );
    // 差し込み欄が残っていない
    assert!(
        body.iter().all(|p| !p.contains("{{") && !p.contains("}}")),
        "{body:?}"
    );

    // ヘッダー・フッター
    assert_eq!(
        paragraphs(&part(&out, "word/header1.xml")),
        ["請求番号 INV-001"]
    );
    assert_eq!(
        paragraphs(&part(&out, "word/footer1.xml")),
        ["発行 2024-01-31"]
    );
}

#[test]
fn docx_render_keeps_everything_else_untouched() {
    let out = do_render(Kind::Docx, DOCX);
    let (a, b) = (Package::read(DOCX).unwrap(), Package::read(&out).unwrap());
    // 同じ部品が同じ順序で残り、差し込み対象でない部品は 1 バイトも変わらない
    assert_eq!(a.names().collect::<Vec<_>>(), b.names().collect::<Vec<_>>());
    for n in a.names() {
        if !["word/document.xml", "word/header1.xml", "word/footer1.xml"].contains(&n) {
            assert_eq!(a.get(n), b.get(n), "{n} が変わっています");
        }
    }
    // 書式（太字・色）は失われない: 分割されていた「先}}」の太字のランは残る
    let doc = String::from_utf8(b.get("word/document.xml").unwrap().to_vec()).unwrap();
    assert!(
        doc.contains("<w:b/>") && doc.contains("C00000"),
        "書式が消えています"
    );
}

#[test]
fn docx_failing_expression_is_reported_with_the_expression() {
    let e = render(Kind::Docx, DOCX, &mut |expr| {
        if expr == "品名" {
            Err("品名が空です".into())
        } else {
            Ok(json!("x"))
        }
    })
    .unwrap_err();
    assert!(
        matches!(&e, Error::Expr { expr, message } if expr == "品名" && message.contains("空です")),
        "{e}"
    );
}

#[test]
fn docx_empty_and_null_values_leave_empty_text() {
    let out = render(Kind::Docx, DOCX, &mut |_| Ok(Value::Null)).unwrap();
    let body = paragraphs(&part(&out, "word/document.xml"));
    assert!(body.contains(&"宛先:  御中".to_string()), "{body:?}");
}

// ---------------------------------------------------------------- xlsx

/// セル番地 → (t 属性, 値)。値は `<v>` の文字、インライン文字列ならそのテキスト。
fn cells(zip: &[u8], sheet: &str) -> BTreeMap<String, (Option<String>, String)> {
    fn walk(el: &Element, out: &mut BTreeMap<String, (Option<String>, String)>) {
        if el.local() == "c" {
            let r = el.attr("r").unwrap().to_string();
            let t = el.attr("t").map(String::from);
            let mut text = String::new();
            for c in &el.children {
                if let Node::Element(e) = c {
                    match e.local() {
                        "v" => text.push_str(&e.text()),
                        "is" => e.children.iter().for_each(|n| {
                            if let Node::Element(t) = n {
                                text.push_str(&t.text());
                            }
                        }),
                        _ => {}
                    }
                }
            }
            out.insert(r, (t, text));
            return;
        }
        for c in &el.children {
            if let Node::Element(e) = c {
                walk(e, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&part(zip, sheet), &mut out);
    out
}

fn check_xlsx(template: &[u8]) {
    let out = do_render(Kind::Xlsx, template);
    let c = cells(&out, "xl/worksheets/sheet1.xml");
    let inline = |s: &str| (Some("inlineStr".to_string()), s.to_string());

    // 文字列に埋め込んだ欄（共有文字列のリッチテキストの複数のランにまたがっていても置換される）
    assert_eq!(c["A1"], inline("請求書 INV-001"), "{c:?}");
    // 欄だけのセル（文字列）、同じ共有文字列を使う別のセルも
    assert_eq!(c["B2"], inline("株式会社テスト & 協力会社"));
    assert_eq!(c["B8"], inline("株式会社テスト & 協力会社"));
    assert_eq!(c["C8"], inline("株式会社テスト & 協力会社様"));
    // 欄だけのセルで値が数値なら、文字列ではなく数値のセル（Excel で計算に使える）
    assert_eq!(c["B3"], (None, "3".to_string()), "数値のまま出ていません");
    assert_eq!(c["B4"], (None, "3600".to_string()));
    // 真偽値
    assert_eq!(c["B5"], (Some("b".to_string()), "1".to_string()));
    // 改行を含む文字列
    assert_eq!(c["B6"], inline("一行目\n二行目 <重要>"));
    // 欄のないセルは、元の文字列（共有文字列の番号）のまま
    assert!(c["B9"].0.as_deref() == Some("s") || c["B9"].0.as_deref() == Some("inlineStr"));
    assert!(
        c.values().all(|(_, v)| !v.contains("{{")),
        "差し込み欄が残っています: {c:?}"
    );

    // 数式のセルは触らず、開いたときに再計算させる
    let raw = String::from_utf8(
        Package::read(&out)
            .unwrap()
            .get("xl/worksheets/sheet1.xml")
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(
        raw.contains("<f>B3*2</f>") || raw.contains("<f aca=\"false\">B3*2</f>"),
        "{raw}"
    );
    let wb = String::from_utf8(
        Package::read(&out)
            .unwrap()
            .get("xl/workbook.xml")
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(wb.contains("fullCalcOnLoad=\"1\""), "{wb}");

    // 2 枚目のシートも
    assert_eq!(
        cells(&out, "xl/worksheets/sheet2.xml")["A1"],
        inline("ねじ")
    );
}

#[test]
fn xlsx_with_shared_strings_and_rich_text() {
    check_xlsx(XLSX);
}

#[test]
fn xlsx_with_inline_strings() {
    check_xlsx(XLSX_INLINE);
}

#[test]
fn xlsx_keeps_cell_styles_and_other_parts() {
    for template in [XLSX, XLSX_INLINE] {
        let out = do_render(Kind::Xlsx, template);
        // 数値書式つきのセル（B4 の #,##0 と太字）のスタイル番号は、置換後も同じ
        let style = |zip: &[u8]| {
            let root = part(zip, "xl/worksheets/sheet1.xml");
            fn find<'a>(e: &'a Element, r: &str) -> Option<&'a Element> {
                if e.local() == "c" && e.attr("r") == Some(r) {
                    return Some(e);
                }
                e.children.iter().find_map(|c| {
                    if let Node::Element(x) = c {
                        find(x, r)
                    } else {
                        None
                    }
                })
            }
            find(&root, "B4").unwrap().attr("s").map(String::from)
        };
        assert!(style(template).is_some());
        assert_eq!(style(template), style(&out), "セルの書式が変わっています");
        let (a, b) = (
            Package::read(template).unwrap(),
            Package::read(&out).unwrap(),
        );
        for n in ["xl/styles.xml", "[Content_Types].xml", "_rels/.rels"] {
            assert_eq!(a.get(n), b.get(n), "{n} が変わっています");
        }
    }
}

#[test]
fn xlsx_scan_lists_each_expression_once() {
    for template in [XLSX, XLSX_INLINE] {
        let found = scan(Kind::Xlsx, template).unwrap();
        for want in [
            "請求番号",
            "取引先",
            "数量",
            "数量 * 単価",
            "完了",
            "備考",
            "品名",
        ] {
            assert!(found.iter().any(|f| f == want), "{want}: {found:?}");
        }
        assert_eq!(
            found.iter().filter(|f| *f == "取引先").count(),
            1,
            "{found:?}"
        );
    }
}

// ---------------------------------------------------------------- 共通

#[test]
fn wrong_kind_or_garbage_is_not_a_template() {
    assert!(matches!(
        render(Kind::Xlsx, DOCX, &mut |_| Ok(json!(1))),
        Err(Error::NotATemplate(_))
    ));
    assert!(matches!(
        render(Kind::Docx, XLSX, &mut |_| Ok(json!(1))),
        Err(Error::NotATemplate(_))
    ));
    assert!(matches!(
        scan(Kind::Docx, "zipではない".as_bytes()),
        Err(Error::NotATemplate(_))
    ));
}

#[test]
fn rendering_is_repeatable() {
    // 同じテンプレートから何度作っても、結果は同じ（テンプレートは書き換わらない）
    assert_eq!(do_render(Kind::Docx, DOCX), do_render(Kind::Docx, DOCX));
    assert_eq!(do_render(Kind::Xlsx, XLSX), do_render(Kind::Xlsx, XLSX));
}
