"""Checks PDFs made by `officewright text-to-pdf --font` with tools that share no code with
it, for a TrueType font (Liberation Sans) and a CFF font (Noto Sans) from tests/fonts:

- qpdf validates the file structure;
- poppler's pdffonts must see each font embedded, subset and with a ToUnicode map;
- pdftotext must read back every character of unicode-sample.txt, which has every
  character of the Latin, Greek and Cyrillic scripts that both fonts have;
- pdftoppm must render the pages without complaint, and a subset must render exactly as
  the whole font does, which proves the subset kept the right glyph for every character
  (text extraction cannot see that: it reads the ToUnicode map, not the glyphs);
- where two characters share a glyph, pdftotext must read each through the replacement
  text (ActualText) of the second, in a file that declares PDF 1.5;
- a variable font is embedded with a warning, and the tools accept the PDF (that its
  program keeps only the default instance, none of these tools can see: the Rust test
  embedded_fonts.rs checks its tables).

OW_CLI must point at the officewright binary (scripts/check.sh sets it).
"""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SAMPLE = HERE / "unicode-sample.txt"
FONTS = HERE.parent / "fonts"
# officewright-font's test font, with glyphs for the space, A, the acute accent and Á.
MINIMAL = HERE.parent.parent / "crates" / "officewright-font" / "tests" / "data" / "minimal.ttf"

# Each font with the type pdffonts gives an embedded font of its kind.
CASES = (
    (FONTS / "liberation" / "LiberationSans-Regular.ttf", "CID TrueType"),
    (FONTS / "noto" / "NotoSans-Regular.otf", "CID Type 0C"),
)


def with_fs_type(font, fs_type):
    """The font with fsType in its OS/2 table, what its license allows, set to fs_type."""
    font = bytearray(font)
    tables = int.from_bytes(font[4:6], "big")
    for record in range(12, 12 + 16 * tables, 16):
        if font[record : record + 4] == b"OS/2":
            os2 = int.from_bytes(font[record + 8 : record + 12], "big")
            font[os2 + 8 : os2 + 10] = fs_type.to_bytes(2, "big")
            return bytes(font)
    raise ValueError("no OS/2 table")


def with_shared_glyph(font):
    """The minimal font with Á mapped to the glyph of A, as fonts do for look-alike
    characters. Its cmap has one segment per character, whose idDelta is the glyph minus
    the character."""

    def deltas(acute):
        pairs = ((0x20, 1), (0x41, 2), (0xB4, 3), (0xC1, acute))
        return b"".join(((glyph - code) & 0xFFFF).to_bytes(2, "big") for code, glyph in pairs)

    assert font.count(deltas(4)) == 1
    return font.replace(deltas(4), deltas(2))


def with_table(font, tag, data):
    """The font with table `tag` added: its table directory, sorted by tag as it must be,
    gets a record more, and every table keeps its data."""
    count = int.from_bytes(font[4:6], "big")
    tables = {}
    for record in range(12, 12 + 16 * count, 16):
        offset = int.from_bytes(font[record + 8 : record + 12], "big")
        length = int.from_bytes(font[record + 12 : record + 16], "big")
        tables[font[record : record + 4]] = font[offset : offset + length]
    tables[tag] = data
    selector = len(tables).bit_length() - 1
    search = 16 << selector
    fields = (len(tables), search, selector, 16 * len(tables) - search)
    directory = bytearray(font[:4] + b"".join(field.to_bytes(2, "big") for field in fields))
    body = bytearray()
    for tag in sorted(tables):
        table = tables[tag] + bytes(-len(tables[tag]) % 4)
        checksum = sum(int.from_bytes(table[at : at + 4], "big") for at in range(0, len(table), 4))
        offset = 12 + 16 * len(tables) + len(body)
        directory += tag + b"".join(v.to_bytes(4, "big") for v in (checksum & 0xFFFFFFFF, offset, len(tables[tag])))
        body += table
    return bytes(directory + body)


# The fvar table of a variable font with one axis, weight from 100 to 900, its default
# 400, named by name ID 2; it has no named instances.
WEIGHT_AXIS = (
    b"".join(field.to_bytes(2, "big") for field in (1, 0, 16, 2, 1, 20, 0, 8))
    + b"wght"
    + b"".join((value << 16).to_bytes(4, "big") for value in (100, 400, 900))
    + b"\0\0\0\2"
)


def characters(text):
    """The text without whitespace: pdftotext's guesses about spaces in the character
    tables differ from the input; the prose test checks the spaces between words."""
    return "".join(text.split())


def prose(text):
    """The sample up to its first character table."""
    return text.split("\nEvery character of")[0]


def run(command):
    return subprocess.run(command, capture_output=True, check=True)


class EmbeddedFontTest(unittest.TestCase):
    def convert(self, directory, font, name="sample.pdf", source=SAMPLE):
        pdf = Path(directory) / name
        result = run([os.environ["OW_CLI"], "text-to-pdf", "--font", str(font), str(source), str(pdf)])
        return pdf, result.stderr.decode("utf-8")

    def fonts(self, pdf):
        """The fonts pdffonts lists: (type, embedded, subset, has a ToUnicode map)."""
        lines = run(["pdffonts", str(pdf)]).stdout.decode("utf-8").splitlines()
        fonts = []
        for line in lines[2:]:
            # name type... encoding emb sub uni object generation
            fields = line.split()
            fonts.append((" ".join(fields[1:-6]), fields[-5], fields[-4], fields[-3]))
        return fonts

    def render(self, pdf, directory):
        """The pages rendered as grayscale images, and what pdftoppm wrote to stderr."""
        prefix = Path(directory) / pdf.stem
        result = run(["pdftoppm", "-r", "50", "-gray", str(pdf), str(prefix)])
        pages = sorted(Path(directory).glob(f"{pdf.stem}-*.pgm"))
        return [page.read_bytes() for page in pages], result.stderr.decode("utf-8")

    def test_qpdf_finds_no_errors_or_warnings(self):
        for font, _ in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                pdf, _ = self.convert(directory, font)
                # Exit status 0 means no errors and no warnings (2 and 3 otherwise).
                result = subprocess.run(["qpdf", "--check", str(pdf)], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_pdffonts_sees_an_embedded_subset_with_a_tounicode_map(self):
        for font, kind in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                pdf, stderr = self.convert(directory, font)
                self.assertEqual(stderr, "")
                self.assertEqual(self.fonts(pdf), [(kind, "yes", "yes", "yes")])

    def test_pdftotext_reads_back_every_character(self):
        expected = SAMPLE.read_text(encoding="utf-8")
        for font, _ in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                pdf, _ = self.convert(directory, font)
                text = run(["pdftotext", "-enc", "UTF-8", str(pdf), "-"]).stdout.decode("utf-8")
                self.assertEqual(characters(text), characters(expected))

    def test_pdftotext_keeps_the_words_of_prose(self):
        expected = prose(SAMPLE.read_text(encoding="utf-8"))
        for font, _ in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                source = Path(directory) / "prose.txt"
                source.write_bytes(expected.encode("utf-8"))
                pdf, _ = self.convert(directory, font, source=source)
                text = run(["pdftotext", "-enc", "UTF-8", str(pdf), "-"]).stdout.decode("utf-8")
                self.assertEqual(text.split(), expected.split())

    def test_a_subset_renders_exactly_as_the_whole_font(self):
        for font, kind in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                # A copy whose license forbids subsetting, so all its glyphs are embedded
                # under their own glyph IDs.
                whole_font = Path(directory) / f"whole{font.suffix}"
                whole_font.write_bytes(with_fs_type(font.read_bytes(), 0x0100))
                subset, _ = self.convert(directory, font, "subset.pdf")
                whole, stderr = self.convert(directory, whole_font, "whole.pdf")
                self.assertIn("its license forbids subsetting, so the whole font is embedded", stderr)
                self.assertEqual(self.fonts(whole), [(kind, "yes", "no", "yes")])

                subset_pages, subset_stderr = self.render(subset, directory)
                whole_pages, whole_stderr = self.render(whole, directory)
                # poppler reports font problems on stderr.
                self.assertEqual(subset_stderr, "")
                self.assertEqual(whole_stderr, "")
                self.assertGreater(len(subset_pages), 1)
                self.assertEqual(len(subset_pages), len(whole_pages))
                for number, (page, reference) in enumerate(zip(subset_pages, whole_pages), 1):
                    # A binary PGM: "P5", width, height and 255, then a byte per pixel.
                    pixels = page.split(b"\n", 3)[3]
                    self.assertLess(min(pixels), 128, f"page {number} is blank")
                    self.assertEqual(page, reference, f"page {number} differs from the whole font")

    def test_text_extraction_reads_characters_that_share_a_glyph(self):
        with tempfile.TemporaryDirectory() as directory:
            font = Path(directory) / "shared.ttf"
            font.write_bytes(with_shared_glyph(MINIMAL.read_bytes()))
            source = Path(directory) / "shared.txt"
            source.write_text("AÁA ÁA", encoding="utf-8")
            pdf, _ = self.convert(directory, font, source=source)
            # The second character is shown with replacement text, a PDF 1.5 feature.
            self.assertTrue(pdf.read_bytes().startswith(b"%PDF-1.5\n"))
            result = subprocess.run(["qpdf", "--check", str(pdf)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            text = run(["pdftotext", "-enc", "UTF-8", str(pdf), "-"]).stdout.decode("utf-8")
            self.assertEqual(text.split(), ["AÁA", "ÁA"])

    def test_a_variable_font_is_embedded_with_a_warning(self):
        with tempfile.TemporaryDirectory() as directory:
            font = Path(directory) / "variable.ttf"
            font.write_bytes(with_table(MINIMAL.read_bytes(), b"fvar", WEIGHT_AXIS))
            source = Path(directory) / "variable.txt"
            source.write_text("AÁ A", encoding="utf-8")
            pdf, stderr = self.convert(directory, font, source=source)
            warning = "it is a variable font; only its default instance is embedded"
            self.assertEqual(stderr, f"warning: font OfficewrightTest-Regular: {warning}\n")
            result = subprocess.run(["qpdf", "--check", str(pdf)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(self.fonts(pdf), [("CID TrueType", "yes", "yes", "yes")])
            text = run(["pdftotext", "-enc", "UTF-8", str(pdf), "-"]).stdout.decode("utf-8")
            self.assertEqual(text.split(), ["AÁ", "A"])
            pages, poppler = self.render(pdf, directory)
            self.assertEqual(poppler, "")
            self.assertLess(min(pages[0].split(b"\n", 3)[3]), 128, "the page is blank")
