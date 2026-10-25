"""Small real documents for offline tests (L39): a DOCX with paragraphs, a table and document properties, a one-page
PDF with a text layer, and a PDF copy with boxes drawn over it as annotations (PDFKit), as the live case did."""

import zipfile
from pathlib import Path

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
_TYPES = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
          '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
          '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
          '<Default Extension="xml" ContentType="application/xml"/>'
          '<Override PartName="/word/document.xml" '
          'ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
          '<Override PartName="/docProps/core.xml" '
          'ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>')
_RELS = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
         '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
         '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/'
         'officeDocument" Target="word/document.xml"/></Relationships>')


def _p(text: str) -> str:
    return f'<w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>'


def _table(rows: list[list[str]]) -> str:
    return "<w:tbl>" + "".join("<w:tr>" + "".join(f"<w:tc>{_p(cell)}</w:tc>" for cell in row) + "</w:tr>"
                               for row in rows) + "</w:tbl>"


def docx(path: Path, body: list[str | list[list[str]]], creator: str = "Student Name") -> Path:
    """A DOCX whose body is paragraphs (str) and tables (rows of cells), with dc:creator set."""
    xml = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}"><w:body>' +
           "".join(_p(b) if isinstance(b, str) else _table(b) for b in body) + "</w:body></w:document>")
    core = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties '
            'xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
            f'xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:creator>{creator}</dc:creator></cp:coreProperties>')
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", _TYPES)
        z.writestr("_rels/.rels", _RELS)
        z.writestr("word/document.xml", xml)
        z.writestr("docProps/core.xml", core)
    return path


def pdf(path: Path, lines: list[str]) -> Path:
    """A one-page PDF whose text layer holds `lines` (Helvetica, one per line)."""
    text = " ".join(f"({line}) Tj 0 -20 Td" for line in lines)
    stream = f"BT /F1 12 Tf 72 720 Td {text} ET".encode()
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
               b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
               b"/Resources << /Font << /F1 5 0 R >> >> >>",
               b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream",
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    out, offsets = bytearray(b"%PDF-1.4\n"), []
    for i, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    out += b"".join(b"%010d 00000 n \n" % o for o in offsets)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
    path.write_bytes(bytes(out))
    return path


def overlay(src: Path, dst: Path, text: str) -> Path:
    """`src` with a white box and a free-text box saying `text` drawn over its first line, written to `dst`: the text
    layer stays as it was (the live case's "correction")."""
    from Foundation import NSURL, NSMakeRect
    from Quartz import PDFAnnotation, PDFDocument
    doc = PDFDocument.alloc().initWithURL_(NSURL.fileURLWithPath_(str(src)))
    page = doc.pageAtIndex_(0)
    rect = NSMakeRect(72, 712, 200, 20)
    page.addAnnotation_(PDFAnnotation.alloc().initWithBounds_forType_withProperties_(rect, "Square", None))
    note = PDFAnnotation.alloc().initWithBounds_forType_withProperties_(rect, "FreeText", None)
    note.setContents_(text)
    page.addAnnotation_(note)
    assert doc.writeToURL_(NSURL.fileURLWithPath_(str(dst)))
    return dst
