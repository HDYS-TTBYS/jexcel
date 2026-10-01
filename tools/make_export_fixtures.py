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


make_docx()
make_xlsx()
make_shared_strings_xlsx()
print("generated:", sorted(p.name for p in OUT.iterdir()))
