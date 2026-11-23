"""Checks that corpus-import.py reads links and named destinations from the real qpdf's JSON.

The unit tests in ../test_corpus_import.py give the importer qpdf's JSON written by hand; this
one runs qpdf on a small PDF with links of the kinds the importer reads, among them the
one Word writes for the references of notes (a /Dest array on the link itself), so that the
forms the importer reads (strings, references, name trees) are the ones qpdf gives. scripts/check.sh
runs it on Linux and macOS, where qpdf is installed.
"""

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location(
    "corpus_import", ROOT / "scripts" / "corpus-import.py"
)
importer = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = importer
spec.loader.exec_module(importer)


def pdf(objects):
    """A PDF of the given object dictionaries, numbered from 1, the first the catalog."""
    out = b"%PDF-1.7\n"
    offsets = []
    for number, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += f"{number} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    out += b"".join(f"{offset:010d} 00000 n \n".encode() for offset in offsets)
    out += (f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n"
            "%%EOF\n").encode()
    return out


# Two pages of 300 by 400 pt. Page 1 holds a URI link, a GoTo link to page 2 with its
# quadrilateral, a link to a named destination, a text annotation, which is no link, and a
# link with its destination on page 2 as Word writes one for a note's reference.
# The name tree, its keys in order, has a destination named in UTF-16 and one wrapped in a
# /D dictionary.
LINKS_PDF = pdf([
    b"<< /Type /Catalog /Pages 2 0 R /Names << /Dests 7 0 R >> >>",
    b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>",
    b"<< /Type /Page /Parent 2 0 R /Resources << >> /Annots [5 0 R 6 0 R 8 0 R 9 0 R 10 0 R] >>",
    b"<< /Type /Page /Parent 2 0 R /Resources << >> >>",
    b"<< /Type /Annot /Subtype /Link /Rect [72 300 200 314] /Border [0 0 0] "
    b"/A << /S /URI /URI (https://example.org/a) >> >>",
    b"<< /Type /Annot /Subtype /Link /Rect [72 280 200 294] "
    b"/QuadPoints [72 294 200 294 72 280 200 280] /A << /S /GoTo /D [4 0 R /XYZ 72 370 0] >> >>",
    b"<< /Names [<FEFF004E0061006D0065> [3 0 R /XYZ 10 390 0] "
    b"(_Toc1) << /D [4 0 R /XYZ 90 200 null] >>] >>",
    b"<< /Type /Annot /Subtype /Link /Rect [72 260 200 274] /Dest (_Toc1) >>",
    b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /Contents (a note) >>",
    b"<< /Type /Annot /Subtype /Link /Rect [72 240 200 254] /Border [0 0 0] "
    b"/Dest [4 0 R /XYZ 72 370 0] >>",
])


class Links(unittest.TestCase):
    def test_the_links_of_qpdf_s_json_are_read(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "links.pdf"
            path.write_bytes(LINKS_PDF)
            text = importer.run_tool("qpdf", "--json=2", str(path))
        links, destinations = importer.pdf_links(text, 2)
        self.assertEqual(links, [
            {"page": 1, "box": [72.0, 86.0, 200.0, 100.0], "uri": "https://example.org/a"},
            {"page": 1, "box": [72.0, 106.0, 200.0, 120.0],
             "quad_points": [[72.0, 106.0], [200.0, 106.0], [72.0, 120.0], [200.0, 120.0]],
             "dest": {"page": 2, "fit": "XYZ", "x": 72.0, "y": 30.0}},
            {"page": 1, "box": [72.0, 126.0, 200.0, 140.0], "dest": {"name": "_Toc1"}},
            {"page": 1, "box": [72.0, 146.0, 200.0, 160.0],
             "dest": {"page": 2, "fit": "XYZ", "x": 72.0, "y": 30.0}},
        ])
        self.assertEqual(destinations, [
            {"name": "Name", "page": 1, "fit": "XYZ", "x": 10.0, "y": 10.0},
            {"name": "_Toc1", "page": 2, "fit": "XYZ", "x": 90.0, "y": 200.0},
        ])


if __name__ == "__main__":
    unittest.main()
