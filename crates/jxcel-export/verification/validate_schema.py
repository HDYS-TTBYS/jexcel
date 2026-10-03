#!/usr/bin/env python3
"""書き出した docx / xlsx が、OOXML（ECMA-376）のスキーマに違反していないかを検査する。

使い方:
  cargo run -p jxcel-export --example make_verification_set -- ./verify
  python3 verification/validate_schema.py ./verify [スキーマの置き場 (既定: ~/.cache/jxcel-ooxml-xsd)]

スキーマ（XSD）は Apache POI の公開 jar（Maven Central）に同梱されているものを、初回に取得して置き場に展開する。
lxml が要る（pip install lxml）。

「書き出し後にだけ出たエラー」を不具合として数える。テンプレートに最初からあるエラー（Word 自身が書く
互換性の宣言 mc:Ignorable など）は比べて除く。`<t xml:space="preserve">` は Excel 自身も書く形（XSD の記述漏れ）なので除く。
Word・Excel の実機が読むときの厳しさ（修復の確認が出るか）に近い検査だが、実機そのものではない。
"""
import collections
import glob
import io
import os
import sys
import urllib.request
import zipfile

from lxml import etree

JAR = "https://repo1.maven.org/maven2/org/apache/poi/poi-ooxml-full/5.2.5/poi-ooxml-full-5.2.5.jar"
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)  # crates/jxcel-export

XML_XSD = """<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="http://www.w3.org/XML/1998/namespace">
 <xs:attribute name="lang" type="xs:language"/>
 <xs:attribute name="space"><xs:simpleType><xs:restriction base="xs:NCName"><xs:enumeration value="default"/><xs:enumeration value="preserve"/></xs:restriction></xs:simpleType></xs:attribute>
 <xs:attribute name="base" type="xs:anyURI"/>
 <xs:attribute name="id" type="xs:ID"/>
</xs:schema>
"""

NAMESPACES = {
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main": "wml.xsd",
    "http://schemas.openxmlformats.org/spreadsheetml/2006/main": "sml.xsd",
    "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing": "dml-spreadsheetDrawing.xsd",
    "http://schemas.openxmlformats.org/drawingml/2006/chart": "dml-chart.xsd",
}


def fetch_schemas(cache):
    if os.path.exists(os.path.join(cache, "wml.xsd")):
        return
    os.makedirs(cache, exist_ok=True)
    print("スキーマを取得しています:", JAR)
    data = urllib.request.urlopen(JAR, timeout=120).read()
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        for n in z.namelist():
            if n.startswith("org/apache/poi/schemas/ooxml/src/") and n.endswith(".xsd"):
                with open(os.path.join(cache, os.path.basename(n)), "wb") as f:
                    f.write(z.read(n))
    with open(os.path.join(cache, "xml.xsd"), "w") as f:
        f.write(XML_XSD)
    wml = os.path.join(cache, "wml.xsd")
    s = open(wml, encoding="utf-8").read()
    s = s.replace(
        '<xsd:import namespace="http://www.w3.org/XML/1998/namespace"/>',
        '<xsd:import namespace="http://www.w3.org/XML/1998/namespace" schemaLocation="xml.xsd"/>',
    )
    open(wml, "w", encoding="utf-8").write(s)


class Validator:
    def __init__(self, cache):
        self.cache = cache
        self.schemas = {}

    def schema(self, name):
        if name not in self.schemas:
            self.schemas[name] = etree.XMLSchema(etree.parse(os.path.join(self.cache, name)))
        return self.schemas[name]

    def errors(self, path):
        out = collections.Counter()
        parts = 0
        z = zipfile.ZipFile(path)
        for n in z.namelist():
            if not n.endswith(".xml") or n.startswith(("docProps/", "customXml/")) or "theme" in n:
                continue
            try:
                root = etree.fromstring(z.read(n))
            except Exception as e:  # noqa: BLE001
                out[(n, "整形式でない: " + str(e)[:80])] += 1
                continue
            xsd = NAMESPACES.get(etree.QName(root).namespace)
            if not xsd:
                continue
            parts += 1
            sc = self.schema(xsd)
            sc.validate(root.getroottree())
            kind = os.path.basename(n).rstrip("0123456789.xml") or n
            for e in sc.error_log:
                if "attribute '{http://www.w3.org/XML/1998/namespace}space'" in e.message:
                    continue
                out[(kind, e.message[:160])] += 1
        return out, parts


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    outdir = sys.argv[1]
    cache = sys.argv[2] if len(sys.argv) > 2 else os.path.expanduser("~/.cache/jxcel-ooxml-xsd")
    fetch_schemas(cache)
    v = Validator(cache)
    bad = 0
    files = sorted(glob.glob(os.path.join(outdir, "*.out*.docx")) + glob.glob(os.path.join(outdir, "*.out*.xlsx")))
    if not files:
        print("検査するファイルがありません:", outdir)
        return 2
    for f in files:
        base = os.path.basename(f)
        stem, ext = base.split(".out")[0], base.rsplit(".", 1)[1]
        tpl = next(
            (c for c in (f"{ROOT}/tests/fixtures/{stem}.{ext}", f"{ROOT}/verification/templates/{stem}.{ext}") if os.path.exists(c)),
            None,
        )
        out, parts = v.errors(f)
        known = v.errors(tpl)[0] if tpl else collections.Counter()
        new = {k: c for k, c in out.items() if k not in known}
        bad += len(new)
        print(f"{'OK ' if not new else 'NG '} {base}: 検査した部品 {parts}, 書き出しで増えたエラー {len(new)}")
        for (n, m), c in list(new.items())[:8]:
            print(f"      {n}: {m} (×{c})")
    print("\n書き出しで増えたスキーマ違反:", bad, "種類")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
