"""Checks corpus-import.py's font embedding with the real qpdf and poppler.

The unit tests in ../test_corpus_import.py replace qpdf and poppler; these run them. A small
PDF names a font without embedding it, as Word's PDFs do with Times New Roman. The importer
must write a copy that embeds the font file, and poppler must draw that copy exactly as it
draws the same PDF with the font embedded by hand, and not as it draws the original, for
which it picks a substitute. scripts/check.sh runs this on Linux and macOS, where qpdf and
poppler are installed (Windows has no official poppler build).
"""

import importlib.util
import shutil
import subprocess
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

# officewright-font's test font: TrueType outlines, a glyph for A. No system has it, so
# poppler draws a substitute when it is not embedded.
MINIMAL = ROOT / "crates" / "officewright-font" / "tests" / "data" / "minimal.ttf"
# CFF outlines, which the importer embeds as /FontFile3 /Subtype /OpenType.
NOTO = ROOT / "tests" / "fonts" / "noto" / "NotoSans-Regular.otf"


def pdf(name, subtype, text, embedded=None):
    """A one-page PDF that sets text in the font name; embedded is (key, stream dict, data)
    to embed a font file into the descriptor by hand."""
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 120] "
        b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
        f"<< /Type /Font /Subtype /{subtype} /BaseFont /{name} /FirstChar 32 /LastChar 126 "
        f"/Widths [{' '.join(['600'] * 95)}] /Encoding /WinAnsiEncoding "
        f"/FontDescriptor 6 0 R >>".encode(),
    ]
    content = f"BT /F1 48 Tf 10 40 Td ({text}) Tj ET".encode()
    objects.append(b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream")
    descriptor = (
        f"/Type /FontDescriptor /FontName /{name} /Flags 32 /FontBBox [0 -250 1000 900] "
        "/ItalicAngle 0 /Ascent 900 /Descent -250 /CapHeight 700 /StemV 80"
    )
    if embedded:
        key, stream, data = embedded
        objects.append(f"<< {descriptor} /{key} 7 0 R >>".encode())
        objects.append(
            f"<< {stream} /Length {len(data)} >>\nstream\n".encode() + data + b"\nendstream"
        )
    else:
        objects.append(f"<< {descriptor} >>".encode())
    out = b"%PDF-1.6\n"
    offsets = []
    for number, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % number + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    out += b"".join(b"%010d 00000 n \n" % offset for offset in offsets)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (
        len(objects) + 1,
        xref,
    )
    return out


def raster(path):
    with tempfile.TemporaryDirectory() as directory:
        pages = importer.poppler_raster(path, Path(directory))
        return [page.read_bytes() for page in pages]


class Embedding(unittest.TestCase):
    def setUp(self):
        self.temp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.temp)

    def embed(self, font, subtype, text, name=None):
        """Renders of the PDF naming the font (by name, else its PostScript name): as it
        is, as the importer embeds the font, and with the font embedded by hand."""
        index = importer.font_index([font.parent])
        (face,) = [f for faces in index.values() for f in faces if f.path == font]
        drawn = importer.drawn_font(face.postscript, index)
        if name:
            drawn = importer.DrawnFont(name, face, drawn.data)
        name = drawn.name
        original = self.temp / "original.pdf"
        original.write_bytes(pdf(name, subtype, text))
        work = self.temp / "work"
        work.mkdir()
        copy = importer.with_fonts_embedded(original, [drawn], work, importer.TOOLS)
        check = subprocess.run(["qpdf", "--check", str(copy)], capture_output=True, text=True)
        self.assertEqual(check.returncode, 0, check.stdout + check.stderr)
        self.assertNotIn("WARNING", check.stdout + check.stderr)
        data = font.read_bytes()
        if face.cff:
            key, stream = "FontFile3", "/Subtype /OpenType"
        else:
            key, stream = "FontFile2", f"/Length1 {len(data)}"
        by_hand = self.temp / "by-hand.pdf"
        by_hand.write_bytes(pdf(name, subtype, text, (key, stream, data)))
        return raster(original), raster(copy), raster(by_hand)

    def test_a_truetype_font_is_drawn_from_the_embedded_file(self):
        original, copy, by_hand = self.embed(MINIMAL, "TrueType", "AAA")
        self.assertEqual(copy, by_hand)
        self.assertNotEqual(copy, original, "poppler drew the same without the font")

    def test_a_cff_font_is_drawn_from_the_embedded_file(self):
        # A name no system has, so that poppler cannot find Noto Sans installed and draw the
        # original with it.
        original, copy, by_hand = self.embed(NOTO, "Type1", "Hamburg", "OfficewrightCffTest")
        self.assertEqual(copy, by_hand)
        self.assertNotEqual(copy, original, "poppler drew the same without the font")


if __name__ == "__main__":
    unittest.main()
