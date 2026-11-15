//! A DOCX's text (changes.py `_docx`, `_paragraphs`): one line per paragraph (each table cell's own), headers,
//! footers, footnotes and endnotes after the body, then its document properties. ElementTree's walk over the parts,
//! with `zip` reading the package and `roxmltree` the XML.

use std::io::{Cursor, Read};

/// A part declaring more than this uncompressed is not inflated (a zip bomb).
pub const XML_MAX: u64 = 50_000_000;
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const PROPS: [(&str, &str, &str); 6] = [
    ("http://purl.org/dc/elements/1.1/", "title", "title"),
    ("http://purl.org/dc/elements/1.1/", "subject", "subject"),
    ("http://purl.org/dc/elements/1.1/", "creator", "creator"),
    (
        "http://purl.org/dc/elements/1.1/",
        "description",
        "description",
    ),
    (
        "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
        "lastModifiedBy",
        "last modified by",
    ),
    (
        "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
        "keywords",
        "keywords",
    ),
];

/// `_PARTS.fullmatch`: word/(header|footer|footnotes|endnotes)\d*\.xml (Python's `\d` is any Unicode digit).
fn is_part(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("word/") else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(".xml") else {
        return false;
    };
    for kind in ["header", "footer", "footnotes", "endnotes"] {
        if let Some(digits) = rest.strip_prefix(kind) {
            if digits.chars().all(ferb_py::unicode::is_decimal) {
                return true;
            }
        }
    }
    false
}

fn parse(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    Some(text.strip_prefix('\u{feff}').unwrap_or(text).to_string())
}

fn is_w(node: &roxmltree::Node, local: &str) -> bool {
    node.tag_name().namespace() == Some(W) && node.tag_name().name() == local
}

/// The DOCX's text, or None for anything that isn't one FERB can read.
pub fn docx(data: &[u8]) -> Option<String> {
    let mut z = zip::ZipArchive::new(Cursor::new(data)).ok()?;
    let names: Vec<String> = z.file_names().map(str::to_string).collect();
    if !names.iter().any(|n| n == "word/document.xml") {
        return None;
    }
    let mut parts: Vec<String> = names.iter().filter(|n| is_part(n)).cloned().collect();
    parts.sort();
    let mut lines: Vec<String> = Vec::new();
    let read =
        |z: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str| -> Option<(u64, Option<Vec<u8>>)> {
            let mut f = z.by_name(name).ok()?;
            let size = f.size();
            if size > XML_MAX {
                return Some((size, None));
            }
            let mut out = Vec::new();
            f.read_to_end(&mut out).ok()?;
            Some((size, Some(out)))
        };
    for name in std::iter::once("word/document.xml".to_string()).chain(parts) {
        let (_, bytes) = read(&mut z, &name)?;
        let Some(bytes) = bytes else {
            lines.push(format!("[{name}: too large to read]"));
            continue;
        };
        if name != "word/document.xml" {
            let base = name.rsplit('/').next().unwrap_or(&name);
            lines.push(format!("[{}]", base.strip_suffix(".xml").unwrap_or(base)));
        }
        let text = parse(&bytes)?;
        let doc = roxmltree::Document::parse(&text).ok()?;
        lines.extend(paragraphs(doc.root_element()));
    }
    if names.iter().any(|n| n == "docProps/core.xml") {
        let (_, core) = read(&mut z, "docProps/core.xml")?;
        if let Some(bytes) = core {
            let text = parse(&bytes)?;
            let doc = roxmltree::Document::parse(&text).ok()?;
            for node in doc.root_element().children().filter(|n| n.is_element()) {
                let tag = node.tag_name();
                let Some((_, _, label)) = PROPS
                    .iter()
                    .find(|(ns, local, _)| tag.namespace() == Some(*ns) && tag.name() == *local)
                else {
                    continue;
                };
                let own: String = node
                    .children()
                    .take_while(|c| !c.is_element())
                    .filter(|c| c.is_text())
                    .map(|c| c.text().unwrap_or(""))
                    .collect();
                let value = ferb_py::value::strip(&own);
                if !value.is_empty() {
                    lines.push(format!("[document property] {label}: {value}"));
                }
            }
        }
    }
    Some(lines.join("\n"))
}

/// An element's own text before its first child element (ElementTree's `.text`).
fn own_text(node: &roxmltree::Node) -> String {
    node.children()
        .take_while(|c| !c.is_element())
        .filter(|c| c.is_text())
        .map(|c| c.text().unwrap_or(""))
        .collect()
}

fn paragraphs(root: roxmltree::Node) -> Vec<String> {
    fn walk(node: roxmltree::Node, buf: &mut Vec<String>, lines: &mut Vec<String>) {
        for child in node.children().filter(|c| c.is_element()) {
            if is_w(&child, "p") {
                let mut inner = Vec::new();
                walk(child, &mut inner, lines);
                lines.push(inner.concat());
            } else if is_w(&child, "t") {
                buf.push(own_text(&child));
            } else if is_w(&child, "tab") {
                buf.push("\t".into());
            } else if is_w(&child, "br") || is_w(&child, "cr") {
                buf.push("\n".into());
            } else if !is_w(&child, "delText") {
                walk(child, buf, lines);
            }
        }
    }
    let mut lines = Vec::new();
    walk(root, &mut Vec::new(), &mut lines);
    lines
}
