"""jxcel-export のテスト用テンプレート（docx / xlsx）を生成する。

実際の Word / Excel が作るファイルに近い状況（欄が複数のランに分割される、表、ヘッダー、
数式、共有文字列、数値書式など）を再現する。生成物は crates/jxcel-export/tests/fixtures/ に置いてコミットする。
再生成: python3 tools/make_export_fixtures.py（python-docx と openpyxl が必要）
"""
import pathlib

from docx import Document
from docx.shared import Pt, RGBColor
from openpyxl import Workbook
from openpyxl.styles import Font

OUT = pathlib.Path(__file__).resolve().parent.parent / "crates/jxcel-export/tests/fixtures"
OUT.mkdir(parents=True, exist_ok=True)


def make_docx():
    d = Document()
    d.add_heading("請求書", level=1)

    # 1. 欄が複数のランに分割されている（書式が途中で変わると、Word はこう保存する）
    p = d.add_paragraph()
    p.add_run("宛先: ")
    p.add_run("{{取引")
    r = p.add_run("先}}")
    r.bold = True
    r.font.color.rgb = RGBColor(0xC0, 0x00, 0x00)
    p.add_run(" 御中")

    # 2. 1 つのラン内の欄が複数、式に XML の特殊文字（& < > "）を含む
    p = d.add_paragraph("数量 {{数量}} 個 / 判定 {{ 区分 == \"A\" && 数量 > 1 ? \"○\" : \"×\" }} / 合計 {{ 数量 * 単価 }} 円")

    # 3. 欄が 3 つのランにまたがる
    p = d.add_paragraph()
    p.add_run("{{")
    p.add_run("品名")
    p.add_run("}}（")
    p.add_run("{{数量}}")
    p.add_run("）")

    # 4. 表
    t = d.add_table(rows=2, cols=3)
    t.style = "Table Grid"
    for i, h in enumerate(["品名", "数量", "金額"]):
        t.cell(0, i).text = h
    t.cell(1, 0).text = "{{品名}}"
    t.cell(1, 1).text = "{{数量}}"
    t.cell(1, 2).text = "{{数量 * 単価}}"

    # 5. 複数行の値を入れる欄
    d.add_paragraph("備考: {{備考}}")

    # 6. 欄のない段落（そのまま残ること）と、前後の空白が意味を持つ段落
    d.add_paragraph("下記のとおりご請求申し上げます。")
    p = d.add_paragraph()
    r = p.add_run("  先頭と末尾に空白  ")

    # 7. ヘッダー・フッター
    sec = d.sections[0]
    sec.header.paragraphs[0].text = "請求番号 {{請求番号}}"
    sec.footer.paragraphs[0].text = "発行 {{ 発行日 }}"

    d.save(OUT / "invoice.docx")


def make_xlsx():
    wb = Workbook()
    ws = wb.active
    ws.title = "請求書"
    ws["A1"] = "請求書 {{請求番号}}"            # 文字列に埋め込み
    ws["A2"] = "宛先"
    ws["B2"] = "{{取引先}}"                      # 欄だけ（文字列）
    ws["A3"] = "数量"
    ws["B3"] = "{{数量}}"                        # 欄だけ（数値のまま出す）
    ws["A4"] = "金額"
    ws["B4"] = "{{数量 * 単価}}"                 # 式（数値）
    ws["B4"].number_format = "#,##0"
    ws["B4"].font = Font(bold=True)
    ws["A5"] = "完了"
    ws["B5"] = "{{完了}}"                        # 真偽値
    ws["A6"] = "備考"
    ws["B6"] = "{{備考}}"
    ws["A7"] = "合計(数式)"
    ws["B7"] = "=B3*2"                           # 数式（再計算が必要）
    ws["A8"] = "同じ文字列を共有するセル"
    ws["B8"] = "{{取引先}}"                      # B2 と共有文字列を共有する
    ws["C8"] = "{{取引先}}様"
    ws["A9"] = "欄のないセル"
    ws["B9"] = "そのまま"
    ws2 = wb.create_sheet("明細")
    ws2["A1"] = "{{品名}}"
    # openpyxl はインライン文字列で保存する（一部の生成ツールの形）
    wb.save(OUT / "invoice-inline.xlsx")


def make_shared_strings_xlsx():
    """LibreOffice で保存し直し、Excel と同じ「共有文字列」形式の xlsx にする（こちらが一般的な形）。"""
    import shutil
    import subprocess
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(
            ["soffice", "--headless", "--convert-to", "xlsx", "--outdir", tmp, str(OUT / "invoice-inline.xlsx")],
            check=True, capture_output=True, timeout=180,
        )
        shutil.copy(pathlib.Path(tmp) / "invoice-inline.xlsx", OUT / "invoice.xlsx")
        subprocess.run(
            ["soffice", "--headless", "--convert-to", "xlsx", "--outdir", tmp, str(OUT / "loop-inline.xlsx")],
            check=True, capture_output=True, timeout=180,
        )
        shutil.copy(pathlib.Path(tmp) / "loop-inline.xlsx", OUT / "loop.xlsx")


def make_loop_docx():
    """行ループ（表の行の繰り返し）を使う docx。印は欄と同じく複数のランに分割される。"""
    d = Document()
    d.add_paragraph("宛先: {{取引先}}")
    t = d.add_table(rows=4, cols=3)
    t.style = "Table Grid"
    for i, h in enumerate(["品名", "数量", "金額"]):
        t.cell(0, i).text = h
    # 2 行目: ループの行。印が品名のセルの先頭にあり、Word のように複数のランに分かれている
    p = t.cell(1, 0).paragraphs[0]
    p.add_run("{{#ea")
    p.add_run("ch 明細}}")
    p.add_run("{{品目}}").bold = True
    t.cell(1, 1).text = "{{数 * 単価}}"
    t.cell(1, 2).text = "{{_n}}/{{取引先}}"
    t.cell(2, 0).text = "合計"
    t.cell(2, 2).text = "{{合計}}"
    t.cell(3, 0).text = "ループの外の行"
    d.add_paragraph("末尾")
    d.save(OUT / "loop.docx")


def make_loop_xlsx():
    """行ループ・日付・ヘッダー/フッター・数式・結合セルを使う xlsx（インライン文字列。共有文字列版は別に作る）。"""
    wb = Workbook()
    ws = wb.active
    ws.title = "請求書"
    ws["A1"] = "請求書 {{請求番号}}"
    ws["A2"] = "発行日"
    ws["B2"] = "{{発行日}}"
    ws["B2"].number_format = "yyyy/mm/dd"          # 日付の表示形式 → Excel の日付になる
    ws["A3"] = "文字列の日付"
    ws["B3"] = "{{発行日}}"                          # 標準の書式 → 文字列のまま
    ws["C3"] = "{{発行日}} 発行"                      # 文字列に埋め込み → 文字列のまま
    ws["A4"] = "品目"
    ws["B4"] = "数量"
    ws["C4"] = "金額"
    ws["A5"] = "{{#each 明細}}{{品目}}"              # ループの行
    ws["B5"] = "{{数}}"
    ws["C5"] = "=B5*単価"                            # 自分の行を参照する数式（単価は名前なので触らない）
    ws["C5"] = "=B5*100"
    ws["D5"] = "{{_n}}"
    ws["A6"] = "合計"
    ws["B6"] = "=SUM(B5:B5)"                         # ループの行を含む範囲 → 広がる
    ws["C6"] = "=SUM(C5:C5)"
    ws["A7"] = "ループの外"
    ws["B7"] = "=B6+1"                               # 後ろの行への参照 → ずれる
    ws.merge_cells("A8:B8")
    ws["A8"] = "結合セル（ループの外）"
    ws.oddHeader.center.text = "請求書 {{請求番号}} & 御中"
    ws.oddFooter.right.text = "{{取引先}}"
    wb.save(OUT / "loop-inline.xlsx")


def make_nested_docx():
    """入れ子の行ループ（{{/each}} で閉じる形）。明細ごとに、見出しの行・付属品の行（さらに内側のループ）・小計の行。"""
    d = Document()
    t = d.add_table(rows=5, cols=2)
    t.style = "Table Grid"
    t.cell(0, 0).text = "品名"
    t.cell(0, 1).text = "付属品"
    t.cell(1, 0).text = "{{#each 明細}}{{_n}}. {{品目}}"      # 外側の見出しの行（ここから繰り返し）
    t.cell(2, 0).text = "{{#each 付属}}{{名}}{{/each}}"       # 内側のループ（1 行。同じ行で閉じる）
    t.cell(2, 1).text = "{{品目}}の付属 {{_n}}"                 # 外側のキー（品目）も見える
    t.cell(3, 0).text = "{{/each}}小計 {{数}}"                  # 外側を閉じる行（繰り返しの最後の行）
    t.cell(4, 0).text = "合計"
    d.save(OUT / "nested.docx")


def make_nested_table_docx():
    """繰り返す行の中の表（入れ子の表）の中のループ。印は {{#each}} だけ（従来の 1 行ループ）。"""
    d = Document()
    t = d.add_table(rows=2, cols=2)
    t.style = "Table Grid"
    t.cell(0, 0).text = "品名"
    t.cell(0, 1).text = "付属品"
    t.cell(1, 0).text = "{{#each 明細}}{{品目}}"
    inner = t.cell(1, 1).add_table(rows=1, cols=1)
    inner.cell(0, 0).text = "{{#each 付属}}{{品目}}:{{名}}"
    d.save(OUT / "nested-table.docx")


def make_nested_xlsx():
    """入れ子の行ループ（{{/each}} で閉じる形）・同じ回の中の行を指す数式・繰り返しの中の結合セル。"""
    wb = Workbook()
    ws = wb.active
    ws.title = "明細"
    ws["A1"] = "請求書 {{請求番号}}"
    ws["A2"] = "品目"
    ws["B2"] = "数量"
    ws["C2"] = "金額"
    ws["A3"] = "{{#each 明細}}{{品目}}"           # 外側の見出しの行
    ws["B3"] = "{{数}}"
    ws["C3"] = "=B3*100"
    ws["A4"] = "{{#each 付属}}{{名}}{{/each}}"    # 内側のループ（1 行。同じ行で閉じる）
    ws["B4"] = "{{品目}}"                          # 外側のキー
    ws["C4"] = "=C3"                               # 同じ回の見出しの行を指す
    ws.merge_cells("C4:D4")                        # 内側の繰り返しごとに複製される
    ws["A5"] = "{{/each}}小計"                     # 外側を閉じる行
    ws["B5"] = "=SUM(B3:B4)"                       # 同じ回の行だけの範囲
    ws["C5"] = "=SUM(C3:C4)"
    ws["A6"] = "総合計"
    ws["B6"] = "=SUM(B3:B5)"                       # ループを含む範囲 → 広がる
    ws["C6"] = "=C5+1"                              # 後ろの行からの参照
    # 定義名（印刷範囲・印刷タイトル・名前付き範囲）: ループでずれた行に合わせて直される
    ws.print_area = "A1:D6"
    ws.print_title_rows = "1:2"
    from openpyxl.workbook.defined_name import DefinedName
    wb.defined_names["合計セル"] = DefinedName("合計セル", attr_text="明細!$B$6")
    wb.defined_names["グループ"] = DefinedName("グループ", attr_text="明細!$A$3:$C$5")
    wb.defined_names["別シート"] = DefinedName("別シート", attr_text="Sheet9!$A$6")
    wb.save(OUT / "nested.xlsx")


make_docx()
make_xlsx()
make_nested_docx()
make_nested_table_docx()
make_nested_xlsx()
make_loop_docx()
make_loop_xlsx()
make_shared_strings_xlsx()
print("generated:", sorted(p.name for p in OUT.iterdir()))
