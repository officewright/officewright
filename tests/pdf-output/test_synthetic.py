"""Checks the page that officewright-pdf's example `synthetic` draws: "HHHHHHHH" at 24 pt
in Noto Sans resolved regular, italic, bold and bold italic from tests/fonts, where Noto
Sans has a regular face only, so every other style is drawn on top of it. With tools that
share no code with it:

- qpdf validates the file structure;
- pdftotext reads every line back as written;
- pdftoppm renders the page, where italic lines lean right by about a third of their
  height and upright ones do not, and bold lines carry more ink than regular ones.

OW_SYNTHETIC must point at the example's binary (scripts/check.sh sets it).
"""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from test_embedded_fonts import run

FONTS = Path(__file__).resolve().parents[2] / "tests" / "fonts"

# The page is A4. Rendered at 144 dpi a point is two pixels, and row 0 is the top.
PAGE_HEIGHT = 841.8898
SCALE = 2

# Each line's baseline, from the bottom of the page, in the example's order.
BASELINES = {"regular": 700, "italic": 640, "bold": 580, "bold italic": 520}


class Page:
    """The page rendered in grey at 144 dpi."""

    def __init__(self, pdf):
        prefix = pdf.with_suffix("")
        run(["pdftoppm", "-r", "144", "-gray", "-singlefile", str(pdf), str(prefix)])
        data = prefix.with_suffix(".pgm").read_bytes()
        # A binary PGM: "P5", width, height and 255, then a byte per pixel.
        magic, width, height, maximum, pixels = data.split(maxsplit=4)
        assert (magic, maximum) == (b"P5", b"255")
        self.width, self.height, self.pixels = int(width), int(height), pixels

    def row(self, y):
        """The pixels of the row that holds the height y (points from the bottom), from
        x = 60 to 400 pt."""
        top = int((PAGE_HEIGHT - y) * SCALE)
        return self.pixels[top * self.width + 60 * SCALE : top * self.width + 400 * SCALE]

    def leftmost_ink(self, y):
        """Where, in points, the first dark pixel of the row at height y is."""
        row = self.row(y)
        for x, value in enumerate(row):
            if value < 128:
                return 60 + x / SCALE
        return None

    def ink(self, baseline):
        """How many dark pixels the line of `baseline` has, from its baseline to 20 pt up."""
        return sum(
            sum(1 for value in self.row(baseline + dy / SCALE) if value < 128)
            for dy in range(0, 20 * SCALE)
        )


class SyntheticStylesTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory()
        cls.pdf = Path(cls.directory.name) / "synthetic.pdf"
        result = run([os.environ["OW_SYNTHETIC"], str(FONTS), "Noto Sans", str(cls.pdf)])
        cls.report = result.stdout.decode("utf-8")
        cls.page = Page(cls.pdf)

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def test_the_resolver_reports_every_synthetic_style(self):
        self.assertEqual(
            self.report.splitlines(),
            [
                '"Noto Sans" italic → "Noto Sans" with synthetic italic',
                '"Noto Sans" bold → "Noto Sans" with synthetic bold',
                '"Noto Sans" bold italic → "Noto Sans" with synthetic bold and italic',
            ],
        )

    def test_qpdf_finds_no_errors_or_warnings(self):
        result = subprocess.run(["qpdf", "--check", str(self.pdf)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("WARNING", result.stdout + result.stderr)

    def test_pdftotext_reads_every_line_back(self):
        result = run(["pdftotext", "-layout", str(self.pdf), "-"])
        lines = [line.strip() for line in result.stdout.decode("utf-8").splitlines() if line.strip()]
        self.assertEqual(lines, ["HHHHHHHH"] * 4)

    def test_italic_leans_right_by_a_third(self):
        # The left stem of the first H, 2 pt above the baseline and 14 pt above it, below
        # the H's cap height of about 17 pt.
        for style, baseline in BASELINES.items():
            low = self.page.leftmost_ink(baseline + 2)
            high = self.page.leftmost_ink(baseline + 14)
            self.assertIsNotNone(low, style)
            self.assertIsNotNone(high, style)
            slant = (high - low) / 12
            if "italic" in style:
                self.assertTrue(0.25 < slant < 0.42, f"{style}: {slant}")
            else:
                self.assertTrue(abs(slant) < 0.05, f"{style}: {slant}")

    def test_bold_carries_more_ink(self):
        regular = self.page.ink(BASELINES["regular"])
        italic = self.page.ink(BASELINES["italic"])
        bold = self.page.ink(BASELINES["bold"])
        bold_italic = self.page.ink(BASELINES["bold italic"])
        self.assertGreater(regular, 0)
        # The stroke, a thirtieth of the size wide, widens each stem by 0.8 pt.
        self.assertGreater(bold / regular, 1.15, (bold, regular))
        self.assertGreater(bold_italic / italic, 1.15, (bold_italic, italic))
        self.assertLess(italic / regular, 1.1, (italic, regular))


if __name__ == "__main__":
    unittest.main()
