//! 行ループ（`{{#each 式}}`）・日付・ヘッダー/フッターの差し込みのテスト。
//! テンプレートは tools/make_export_fixtures.py で生成してコミットしてある。

use jxcel_export::package::Package;
use jxcel_export::xml::{self, Element, Node};
use jxcel_export::{plan, render_with, Error, Kind, LoopPlan, Source};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const DOCX: &[u8] = include_bytes!("fixtures/loop.docx");
const XLSX: &[u8] = include_bytes!("fixtures/loop.xlsx"); // 共有文字列（Excel と同じ形）
const XLSX_INLINE: &[u8] = include_bytes!("fixtures/loop-inline.xlsx");

/// 1 つの行ループを持つテンプレート用の取得元。
struct Fake {
    top: BTreeMap<&'static str, Value>,
    items: Vec<Value>,
    unit: i64,
}

impl Fake {
    fn new(items: Vec<Value>) -> Self {
        Self {
            top: BTreeMap::from([
                ("取引先", json!("A社")),
                ("請求番号", json!("INV-7")),
                ("発行日", json!("2026-10-02")),
                ("合計", json!(999)),
            ]),
            items,
            unit: 100,
        }
    }
}

impl Source for Fake {
    fn value(&mut self, expr: &str) -> Result<Value, String> {
        self.top
            .get(expr)
            .cloned()
            .ok_or_else(|| format!("未定義: {expr}"))
    }
    fn loop_len(&mut self, index: usize, source: &str) -> Result<usize, String> {
        assert_eq!((index, source), (0, "明細"));
        Ok(self.items.len())
    }
    fn item_value(&mut self, index: usize, item: usize, expr: &str) -> Result<Value, String> {
        assert_eq!(index, 0);
        let it = &self.items[item];
        match expr {
            "_n" => Ok(json!(item + 1)),
            "取引先" => self.value(expr),
            "数 * 単価" => Ok(json!(it["数"].as_i64().unwrap() * self.unit)),
            e => it.get(e).cloned().ok_or_else(|| format!("未定義: {e}")),
        }
    }
}

fn items(n: usize) -> Vec<Value> {
    (1..=n)
        .map(|i| json!({"品目": format!("品{i}"), "数": i * 10}))
        .collect()
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

fn elements<'a>(el: &'a Element, local: &str, out: &mut Vec<&'a Element>) {
    for c in &el.children {
        if let Node::Element(e) = c {
            if e.local() == local {
                out.push(e);
            }
            elements(e, local, out);
        }
    }
}

// ---------------------------------------------------------------- docx

/// 表の各行の、セルごとのテキスト。
fn table_rows(zip: &[u8]) -> Vec<Vec<String>> {
    let root = part(zip, "word/document.xml");
    let mut rows = vec![];
    elements(&root, "tr", &mut rows);
    rows.iter()
        .map(|tr| {
            let mut cells = vec![];
            elements(tr, "tc", &mut cells);
            cells
                .iter()
                .map(|tc| {
                    let mut ts = vec![];
                    elements(tc, "t", &mut ts);
                    ts.iter().map(|t| t.text()).collect::<String>()
                })
                .collect()
        })
        .collect()
}

fn docx(items: Vec<Value>) -> Vec<Vec<String>> {
    let out =
        render_with(Kind::Docx, DOCX, &mut Fake::new(items)).unwrap_or_else(|e| panic!("{e}"));
    table_rows(&out)
}

#[test]
fn docx_plan_lists_loop_placeholders_separately() {
    let p = plan(Kind::Docx, DOCX).unwrap();
    assert_eq!(p.placeholders, ["取引先", "合計"]);
    assert_eq!(
        p.loops,
        [LoopPlan {
            source: "明細".into(),
            exprs: vec![
                "品目".into(),
                "数 * 単価".into(),
                "_n".into(),
                "取引先".into()
            ],
        }]
    );
}

#[test]
fn docx_row_repeats_once_per_item_with_the_marker_removed() {
    let rows = docx(items(3));
    assert_eq!(rows.len(), 1 + 3 + 2, "{rows:?}");
    assert_eq!(rows[0], ["品名", "数量", "金額"]);
    // 印は、複数のランに分かれていても取り除かれ、残りの欄は要素ごとに入る
    assert_eq!(rows[1], ["品1", "1000", "1/A社"]);
    assert_eq!(rows[2], ["品2", "2000", "2/A社"]);
    assert_eq!(rows[3], ["品3", "3000", "3/A社"]);
    // ループの外の行は、ループの外の値で、そのまま
    assert_eq!(rows[4], ["合計", "", "999"]);
    assert_eq!(rows[5], ["ループの外の行", "", ""]);
}

#[test]
fn docx_zero_items_removes_the_row_and_one_item_keeps_it() {
    let rows = docx(vec![]);
    assert_eq!(rows.len(), 1 + 2, "{rows:?}");
    assert_eq!(rows[1][0], "合計");
    assert_eq!(docx(items(1)).len(), 1 + 1 + 2);
}

#[test]
fn docx_loop_errors_are_reported() {
    // 対象の式が失敗
    struct Bad;
    impl Source for Bad {
        fn value(&mut self, _: &str) -> Result<Value, String> {
            Ok(Value::Null)
        }
        fn loop_len(&mut self, _: usize, _: &str) -> Result<usize, String> {
            Err("明細 is not defined".into())
        }
        fn item_value(&mut self, _: usize, _: usize, _: &str) -> Result<Value, String> {
            unreachable!()
        }
    }
    let e = render_with(Kind::Docx, DOCX, &mut Bad).unwrap_err();
    assert!(
        matches!(&e, Error::Expr { expr, message } if expr == "#each 明細" && message.contains("not defined")),
        "{e}"
    );

    // 表の外に印を書いた
    let mut doc = jxcel_export::package::Package::read(DOCX).unwrap();
    let xml_text = String::from_utf8(doc.get("word/document.xml").unwrap().to_vec()).unwrap();
    doc.set(
        "word/document.xml",
        xml_text.replace("宛先: ", "宛先: {{#each x}}").into_bytes(),
    );
    let out = doc.write().unwrap();
    let e = render_with(Kind::Docx, &out, &mut Fake::new(items(1))).unwrap_err();
    assert!(e.to_string().contains("表の行の中"), "{e}");
}

// ---------------------------------------------------------------- xlsx

#[derive(Debug, PartialEq)]
struct C {
    t: Option<String>,
    v: String,
    f: Option<String>,
    s: Option<String>,
}

fn shared_strings(zip: &[u8]) -> Vec<String> {
    if Package::read(zip)
        .unwrap()
        .get("xl/sharedStrings.xml")
        .is_none()
    {
        return vec![];
    }
    let root = part(zip, "xl/sharedStrings.xml");
    let mut sis = vec![];
    elements(&root, "si", &mut sis);
    sis.iter()
        .map(|si| {
            let mut ts = vec![];
            elements(si, "t", &mut ts);
            ts.iter().map(|t| t.text()).collect()
        })
        .collect()
}

fn sheet(zip: &[u8]) -> (BTreeMap<String, C>, Element) {
    let shared = shared_strings(zip);
    let root = part(zip, "xl/worksheets/sheet1.xml");
    let mut cs = vec![];
    elements(&root, "c", &mut cs);
    let mut out = BTreeMap::new();
    for c in cs {
        let mut v = String::new();
        let mut f = None;
        for n in &c.children {
            if let Node::Element(e) = n {
                match e.local() {
                    "v" => v.push_str(&e.text()),
                    "f" => f = Some(e.text()),
                    "is" => {
                        let mut ts = vec![];
                        elements(e, "t", &mut ts);
                        v.extend(ts.iter().map(|t| t.text()));
                    }
                    _ => {}
                }
            }
        }
        // 共有文字列のままのセル（欄のないセル）は、文字列を引いて「文字列のセル」として扱う
        let mut t = c.attr("t").map(String::from);
        if t.as_deref() == Some("s") {
            v = shared[v.parse::<usize>().unwrap()].clone();
            t = Some("inlineStr".into());
        }
        out.insert(
            c.attr("r").unwrap().to_string(),
            C {
                t,
                v,
                f,
                s: c.attr("s").map(String::from),
            },
        );
    }
    (out, root)
}

fn xlsx(template: &[u8], items: Vec<Value>) -> (BTreeMap<String, C>, Element) {
    let out =
        render_with(Kind::Xlsx, template, &mut Fake::new(items)).unwrap_or_else(|e| panic!("{e}"));
    sheet(&out)
}

fn text(c: &BTreeMap<String, C>, r: &str) -> String {
    c.get(r).map_or("<なし>".into(), |c| c.v.clone())
}

fn formula(c: &BTreeMap<String, C>, r: &str) -> String {
    c.get(r)
        .and_then(|c| c.f.clone())
        .unwrap_or_else(|| "<なし>".into())
}

fn check_xlsx_loop(template: &[u8]) {
    let (c, root) = xlsx(template, items(3));

    // ループの外（上）。日付の表示形式のセルは Excel の日付（数値）、そうでないセルは文字列のまま
    assert_eq!(text(&c, "A1"), "請求書 INV-7");
    assert_eq!((c["B2"].t.as_deref(), c["B2"].v.as_str()), (None, "46297"));
    assert_eq!(c["B3"].t.as_deref(), Some("inlineStr"));
    assert_eq!(text(&c, "B3"), "2026-10-02");
    assert_eq!(text(&c, "C3"), "2026-10-02 発行");

    // ループの行（5 行目）が 3 行に: 品目・数・数式・_n が行ごとに入り、印は消える
    for (i, row) in (5..=7).enumerate() {
        let n = i + 1;
        assert_eq!(text(&c, &format!("A{row}")), format!("品{n}"), "A{row}");
        assert_eq!(text(&c, &format!("B{row}")), (n * 10).to_string());
        assert_eq!(c[&format!("B{row}")].t, None, "数値のセル");
        assert_eq!(formula(&c, &format!("C{row}")), format!("B{row}*100"));
        assert_eq!(text(&c, &format!("D{row}")), n.to_string());
    }
    // 後ろの行は 2 行下へ。ループの行を含む範囲は広がり、後ろの行への参照はずれる
    assert_eq!(text(&c, "A8"), "合計");
    assert_eq!(formula(&c, "B8"), "SUM(B5:B7)");
    // 数式セルの古い計算結果は捨てる（LibreOffice などが差し込み前の値を表示しないように）
    assert_eq!((c["B8"].v.as_str(), c["B8"].t.as_deref()), ("", None));
    assert_eq!(formula(&c, "C8"), "SUM(C5:C7)");
    assert_eq!(text(&c, "A9"), "ループの外");
    assert_eq!(formula(&c, "B9"), "B8+1");
    assert_eq!(text(&c, "A10"), "結合セル（ループの外）");
    assert!(!c.contains_key("A11"));
    // 元の位置に古い内容が残っていない
    assert!(c.keys().all(|k| !k.ends_with("11")));

    // 結合セル・範囲の定義もずれる
    let mut merges = vec![];
    elements(&root, "mergeCell", &mut merges);
    assert_eq!(
        merges
            .iter()
            .map(|m| m.attr("ref").unwrap())
            .collect::<Vec<_>>(),
        ["A10:B10"]
    );
    let mut dim = vec![];
    elements(&root, "dimension", &mut dim);
    assert_eq!(dim[0].attr("ref"), Some("A1:D10"));

    // 行番号が連続し、重複しない
    let mut rows = vec![];
    elements(&root, "row", &mut rows);
    let nums: Vec<u32> = rows
        .iter()
        .map(|r| r.attr("r").unwrap().parse().unwrap())
        .collect();
    assert_eq!(nums, (1..=10).collect::<Vec<_>>());

    // ヘッダー・フッター（値の & は && にする）
    let mut hf = vec![];
    elements(&root, "headerFooter", &mut hf);
    let mut odd = vec![];
    elements(hf[0], "oddHeader", &mut odd);
    assert!(odd[0].text().contains("請求書 INV-7"), "{}", odd[0].text());
    assert!(!odd[0].text().contains("{{"));
    let mut foot = vec![];
    elements(hf[0], "oddFooter", &mut foot);
    assert!(foot[0].text().contains("A社"), "{}", foot[0].text());
}

#[test]
fn xlsx_row_repeats_with_shared_strings() {
    check_xlsx_loop(XLSX);
}

#[test]
fn xlsx_row_repeats_with_inline_strings() {
    check_xlsx_loop(XLSX_INLINE);
}

#[test]
fn xlsx_one_item_keeps_the_layout_and_zero_items_leave_an_empty_row() {
    let (c, _) = xlsx(XLSX_INLINE, items(1));
    assert_eq!(text(&c, "A5"), "品1");
    assert_eq!(formula(&c, "B6"), "SUM(B5:B5)");
    assert_eq!(text(&c, "A6"), "合計");

    // 0 件: 行は消さずに空にする（数式の参照が壊れないように）
    let (c, root) = xlsx(XLSX_INLINE, vec![]);
    assert_eq!(text(&c, "A5"), "");
    assert_eq!(text(&c, "B5"), "");
    assert_eq!(text(&c, "D5"), "");
    assert_eq!(formula(&c, "B6"), "SUM(B5:B5)");
    assert_eq!(text(&c, "A6"), "合計");
    let mut rows = vec![];
    elements(&root, "row", &mut rows);
    assert_eq!(rows.len(), 8);
}

#[test]
fn xlsx_plan_lists_loop_placeholders_and_header_footer_fields() {
    let p = plan(Kind::Xlsx, XLSX_INLINE).unwrap();
    assert!(p.placeholders.contains(&"請求番号".to_string()));
    assert!(p.placeholders.contains(&"取引先".to_string())); // フッター
    assert_eq!(p.loops.len(), 1);
    assert_eq!(p.loops[0].source, "明細");
    assert_eq!(p.loops[0].exprs, ["品目", "数", "_n"]);
}

#[test]
fn xlsx_marker_outside_a_row_cell_and_double_markers_are_errors() {
    // 1 行に 2 つの印
    let mut pkg = Package::read(XLSX_INLINE).unwrap();
    let s = String::from_utf8(pkg.get("xl/worksheets/sheet1.xml").unwrap().to_vec()).unwrap();
    pkg.set(
        "xl/worksheets/sheet1.xml",
        s.replace("{{&#25968;}}", "{{#each 別}}{{&#25968;}}")
            .into_bytes(),
    );
    let e = render_with(Kind::Xlsx, &pkg.write().unwrap(), &mut Fake::new(items(1))).unwrap_err();
    assert!(e.to_string().contains("1 つだけ"), "{e}");
}

#[test]
fn rendering_with_loops_is_repeatable() {
    let a = render_with(Kind::Xlsx, XLSX, &mut Fake::new(items(3))).unwrap();
    let b = render_with(Kind::Xlsx, XLSX, &mut Fake::new(items(3))).unwrap();
    assert_eq!(a, b);
    let a = render_with(Kind::Docx, DOCX, &mut Fake::new(items(3))).unwrap();
    let b = render_with(Kind::Docx, DOCX, &mut Fake::new(items(3))).unwrap();
    assert_eq!(a, b);
}
