//! OOXML（docx / xlsx の中身）を読み書きするための、最小限の XML ツリー。
//!
//! 目的は「差し込み欄を置換して、それ以外は元のまま書き戻す」こと。名前空間は解釈せず、
//! 要素名・属性名は元の文字列（`w:p` など）のまま保持する。属性の順序、空白だけのテキスト、
//! コメントや処理命令も保つ（Word の `xml:space="preserve"` の文字列が壊れないように）。

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Element(Element),
    Text(String),
    /// コメント・処理命令・DOCTYPE など。解釈せず、そのまま書き戻す。
    Raw(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// 先頭の `<?xml ... ?>` の中身（`xml version="1.0" ...`）。
    pub decl: Option<String>,
    /// ルート要素の前に現れたコメントなど。
    pub before: Vec<Node>,
    pub root: Element,
}

impl Element {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            attrs: vec![],
            children: vec![],
        }
    }

    /// 名前空間の接頭辞を除いた名前（`w:p` → `p`）。
    pub fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_attr(&mut self, name: &str, value: &str) {
        match self.attrs.iter_mut().find(|(k, _)| k == name) {
            Some((_, v)) => *v = value.to_string(),
            None => self.attrs.push((name.to_string(), value.to_string())),
        }
    }

    pub fn remove_attr(&mut self, name: &str) {
        self.attrs.retain(|(k, _)| k != name);
    }

    /// 直下のテキストをつなげたもの。
    pub fn text(&self) -> String {
        self.children
            .iter()
            .filter_map(|n| {
                if let Node::Text(t) = n {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn set_text(&mut self, text: &str) {
        self.children = vec![Node::Text(text.to_string())];
    }
}

fn xml_err(e: impl std::fmt::Display) -> Error {
    Error::Xml(e.to_string())
}

fn start_to_element(e: &BytesStart) -> Result<Element> {
    let name = String::from_utf8(e.name().as_ref().to_vec()).map_err(xml_err)?;
    let mut attrs = vec![];
    for a in e.attributes() {
        let a = a.map_err(xml_err)?;
        let key = String::from_utf8(a.key.as_ref().to_vec()).map_err(xml_err)?;
        attrs.push((key, a.unescape_value().map_err(xml_err)?.into_owned()));
    }
    Ok(Element {
        name,
        attrs,
        children: vec![],
    })
}

pub fn parse(bytes: &[u8]) -> Result<Document> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut decl = None;
    let mut before = vec![];
    let mut stack: Vec<Element> = vec![];
    let mut root: Option<Element> = None;

    loop {
        let event = reader.read_event_into(&mut buf).map_err(xml_err)?;
        match event {
            Event::Decl(d) => decl = Some(String::from_utf8_lossy(&d).into_owned()),
            Event::Start(e) => stack.push(start_to_element(&e)?),
            Event::Empty(e) => {
                let el = start_to_element(&e)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Element(el)),
                    None => root = Some(el),
                }
            }
            Event::End(_) => {
                let el = stack
                    .pop()
                    .ok_or_else(|| Error::Xml("閉じタグが多すぎます".into()))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Element(el)),
                    None => root = Some(el),
                }
            }
            Event::Text(t) => {
                let text = t.unescape().map_err(xml_err)?.into_owned();
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Text(text));
                }
                // ルートの外の空白は捨てる
            }
            Event::CData(c) => {
                if let Some(parent) = stack.last_mut() {
                    parent
                        .children
                        .push(Node::Text(String::from_utf8_lossy(&c).into_owned()));
                }
            }
            Event::Comment(c) => push_raw(
                &mut stack,
                &mut before,
                format!("<!--{}-->", String::from_utf8_lossy(&c)),
            ),
            Event::PI(p) => push_raw(
                &mut stack,
                &mut before,
                format!("<?{}?>", String::from_utf8_lossy(&p)),
            ),
            Event::DocType(d) => push_raw(
                &mut stack,
                &mut before,
                format!("<!DOCTYPE{}>", String::from_utf8_lossy(&d)),
            ),
            Event::Eof => break,
        }
        buf.clear();
    }
    if !stack.is_empty() {
        return Err(Error::Xml("閉じられていない要素があります".into()));
    }
    Ok(Document {
        decl,
        before,
        root: root.ok_or_else(|| Error::Xml("ルート要素がありません".into()))?,
    })
}

fn push_raw(stack: &mut [Element], before: &mut Vec<Node>, raw: String) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(Node::Raw(raw)),
        None => before.push(Node::Raw(raw)),
    }
}

pub fn write(doc: &Document) -> Result<Vec<u8>> {
    let mut w = Writer::new(Vec::new());
    if let Some(d) = &doc.decl {
        // 宣言は元の文字列のまま（standalone など）。後ろの改行は Word の出力に合わせる
        w.get_mut()
            .extend_from_slice(format!("<?{d}?>\r\n").as_bytes());
    }
    for n in &doc.before {
        write_node(&mut w, n)?;
    }
    write_element(&mut w, &doc.root)?;
    Ok(w.into_inner())
}

fn write_node(w: &mut Writer<Vec<u8>>, n: &Node) -> Result<()> {
    match n {
        Node::Element(e) => write_element(w, e),
        Node::Text(t) => w
            .write_event(Event::Text(BytesText::new(t)))
            .map_err(xml_err),
        Node::Raw(r) => {
            w.get_mut().extend_from_slice(r.as_bytes());
            Ok(())
        }
    }
}

fn write_element(w: &mut Writer<Vec<u8>>, e: &Element) -> Result<()> {
    let mut start = BytesStart::new(e.name.as_str());
    for (k, v) in &e.attrs {
        start.push_attribute((k.as_str(), v.as_str()));
    }
    if e.children.is_empty() {
        return w.write_event(Event::Empty(start)).map_err(xml_err);
    }
    w.write_event(Event::Start(start)).map_err(xml_err)?;
    for c in &e.children {
        write_node(w, c)?;
    }
    w.write_event(Event::End(BytesEnd::new(e.name.as_str())))
        .map_err(xml_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_structure_text_and_escapes() {
        let src = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:document xmlns:w=\"urn:x\"><w:p><!-- c --><w:r><w:t xml:space=\"preserve\"> a &amp; b &lt;c&gt; \"q\" </w:t></w:r><w:br/></w:p></w:document>";
        let doc = parse(src.as_bytes()).unwrap();
        assert_eq!(
            doc.decl.as_deref(),
            Some("xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"")
        );
        let out = String::from_utf8(write(&doc).unwrap()).unwrap();
        // 宣言・構造・コメント・エスケープが保たれる（引用符は &quot; と書かれるが XML としては同じ意味）
        assert!(out.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:document xmlns:w=\"urn:x\"><w:p><!-- c -->"));
        assert!(out.contains("&amp; b &lt;c&gt;") && out.contains("<w:br/>"));
        // 再度読んでも同じ（テキストの中身は一字も変わらない）
        assert_eq!(parse(out.as_bytes()).unwrap(), doc);
        assert!(out.contains("xml:space=\"preserve\""));
    }

    #[test]
    fn whitespace_only_text_is_preserved() {
        let doc = parse(b"<a><t xml:space=\"preserve\"> </t><t>  x  </t></a>").unwrap();
        let t: Vec<String> = doc
            .root
            .children
            .iter()
            .filter_map(|n| {
                if let Node::Element(e) = n {
                    Some(e.text())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(t, [" ", "  x  "]);
    }

    #[test]
    fn rejects_broken_xml() {
        assert!(parse(b"<a><b></a>").is_err());
        assert!(parse(b"<a>").is_err());
        assert!(parse(b"").is_err());
    }
}
