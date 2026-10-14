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
- the example embed_report, which embeds every font of a directory, must count a font
  the subsetter does not support apart from a damaged one.

OW_CLI must point at the officewright binary, and OW_EMBED_REPORT at the example's
(scripts/check.sh sets both).
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


def with_unsupported_notdef(font):
    """The CFF font with its .notdef glyph starting with "0 0 0 0 endchar": endchar with
    the four operands of an accented character (seac), which the subsetter does not
    implement. The glyph is the first of the CharStrings INDEX, which the Top DICT finds
    with operator 17."""
    font = bytearray(font)
    tables = int.from_bytes(font[4:6], "big")
    record = next(r for r in range(12, 12 + 16 * tables, 16) if font[r : r + 4] == b"CFF ")
    cff = int.from_bytes(font[record + 8 : record + 12], "big")

    def index(at):
        """The offset size and offsets of the INDEX at `at`, and where it ends."""
        count = int.from_bytes(font[at : at + 2], "big")
        size = font[at + 2]
        offsets = [int.from_bytes(font[at + 3 + i * size : at + 3 + (i + 1) * size], "big") for i in range(count + 1)]
        return offsets, at + 3 + (count + 1) * size - 1, offsets[-1] + at + 3 + (count + 1) * size - 1

    _, _, name_end = index(cff + font[cff + 2])
    offsets, data, _ = index(name_end)
    at, end, operands = data + offsets[0], data + offsets[1], []
    while at < end:
        b0 = font[at]
        if b0 in (28, 29):
            size = 2 if b0 == 28 else 4
            operands.append(int.from_bytes(font[at + 1 : at + 1 + size], "big", signed=True))
            at += 1 + size
        elif 32 <= b0 <= 246:
            operands.append(b0 - 139)
            at += 1
        elif 247 <= b0 <= 254:
            sign = 1 if b0 <= 250 else -1
            operands.append(sign * (((b0 - 247 if b0 <= 250 else b0 - 251) << 8) + font[at + 1] + 108))
            at += 2
        elif b0 == 30:
            while font[at] & 0x0F != 0x0F and font[at] >> 4 != 0x0F:
                at += 1
            at += 1
            operands.append(0)
        else:
            if b0 == 17:
                charstrings = cff + operands[-1]
            at += 2 if b0 == 12 else 1
            operands = []
    offsets, data, _ = index(charstrings)
    font[data + offsets[0] : data + offsets[0] + 5] = bytes([139, 139, 139, 139, 14])
    return bytes(font)


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

    def test_embed_report_counts_each_kind_of_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            fonts = Path(directory) / "fonts"
            fonts.mkdir()
            liberation, noto = (font for font, _ in CASES)
            (fonts / "regular.ttf").write_bytes(liberation.read_bytes())
            bold = FONTS / "liberation" / "LiberationSans-Bold.ttf"
            (fonts / "truncated.ttf").write_bytes(bold.read_bytes()[:5000])
            (fonts / "unsupported.otf").write_bytes(with_unsupported_notdef(noto.read_bytes()))
            result = run([os.environ["OW_EMBED_REPORT"], str(Path(directory) / "out"), str(fonts)])
            lines = result.stdout.decode("utf-8").splitlines()
            self.assertEqual(
                [line.strip() for line in lines[-4:]],
                ["3 files", "1 damaged", "1 embedded", "1 not supported by the subsetter yet"],
            )
            # Each failure is printed with its cause.
            self.assertIn(
                f"{fonts / 'unsupported.otf'}, face 0: cannot use the font data: cannot subset font face 0: "
                "it uses a feature the subsetter does not support yet",
                lines,
            )
            self.assertIn(
                f"{fonts / 'truncated.ttf'}, face 0: cannot use the font data: font face 0: "
                "required table 'hmtx' is missing",
                lines,
            )
