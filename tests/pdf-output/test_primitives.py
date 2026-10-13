"""Checks the page of glyph runs and vector graphics that officewright-pdf's example
`primitives` draws, for a TrueType font (Liberation Sans) and a CFF font (Noto Sans) from
tests/fonts, with tools that share no code with it:

- qpdf validates the file structure;
- pdffonts must see the font embedded, subset and with a ToUnicode map;
- pdftotext must read back every text as written: ligatures as their letters, small
  capitals as lowercase letters, an ellipsis of three glyphs as one character;
- pdftoppm must render the page without complaint, the subset exactly as the whole font,
  which proves that every glyph the runs draw kept its glyph in the subset; and the
  shapes, colours and clips where the example puts them.

OW_PRIMITIVES must point at the example's binary (scripts/check.sh sets it).
"""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from test_embedded_fonts import CASES, run, with_fs_type

# The page is A4; rendered at 72 dpi a pixel is a point, and row 0 is the top of the page.
PAGE_HEIGHT = 841.8898

# The lines of text the example draws, as pdftotext reads them.
TEXT = [
    "Glyph runs and vector primitives",
    "Ligatures: fine office flow",
    "Letter spacing",
    "Scaled 150 %",
    "Slanted",
    "Stroked",
    "Small Caps",
    "Café au lait…",
    "Highlighted text",
    "E = mc2",
]

WHITE = (255, 255, 255)
BLACK = (0, 0, 0)
RED = (255, 0, 0)
YELLOW = (255, 255, 0)
BLUE = (0, 0, 255)
GREY = (192, 192, 192)

# Points of the page in PDF coordinates (x, y from the bottom left) and their colour.
PIXELS = (
    # The highlight, right of its text.
    ((265, 575), YELLOW),
    # The dashed frame's bottom edge, 2 pt wide around y = 540: its first dash runs from
    # x = 300 to 306, its first gap to 309.
    ((303, 540), RED),
    ((307, 540), WHITE),
    # Inside the triangle.
    ((370, 460), GREY),
    # The circle around (172, 350), clipped to x < 172, and the square drawn after the
    # clip was removed.
    ((150, 350), RED),
    ((190, 350), WHITE),
    ((210, 310), BLACK),
    # Two clips: the blue fill shows only where both overlap.
    ((375, 325), BLUE),
    ((325, 375), WHITE),
    ((425, 275), WHITE),
)


def draw(directory, font, name="page.pdf"):
    pdf = Path(directory) / name
    result = run([os.environ["OW_PRIMITIVES"], str(font), str(pdf)])
    return pdf, result.stderr.decode("utf-8")


def render(pdf):
    """The page rendered at 72 dpi as (width, height, RGB bytes), and pdftoppm's stderr."""
    prefix = pdf.with_suffix("")
    result = run(["pdftoppm", "-r", "72", "-singlefile", str(pdf), str(prefix)])
    data = prefix.with_suffix(".ppm").read_bytes()
    # A binary PPM: "P6", width, height and 255, then three bytes per pixel.
    magic, width, height, maximum, pixels = data.split(maxsplit=4)
    assert (magic, maximum) == (b"P6", b"255")
    return (int(width), int(height), pixels), result.stderr.decode("utf-8")


def pixel(image, x, y):
    """The colour of the pixel that holds the point (x, y) of the page."""
    width, _, pixels = image
    row = int(PAGE_HEIGHT - y)
    at = 3 * (row * width + x)
    return tuple(pixels[at : at + 3])


class PrimitivesTest(unittest.TestCase):
    def test_qpdf_finds_no_errors_or_warnings(self):
        for font, _ in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                pdf, stderr = draw(directory, font)
                self.assertEqual(stderr, "")
                # Replacement text in marked content makes the file PDF 1.5.
                self.assertTrue(pdf.read_bytes().startswith(b"%PDF-1.5\n"))
                result = subprocess.run(["qpdf", "--check", str(pdf)], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_pdffonts_sees_an_embedded_subset_with_a_tounicode_map(self):
        for font, kind in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                pdf, _ = draw(directory, font)
                lines = run(["pdffonts", str(pdf)]).stdout.decode("utf-8").splitlines()
                self.assertEqual(len(lines), 3, lines)
                fields = lines[2].split()
                self.assertEqual(
                    (" ".join(fields[1:-6]), fields[-5], fields[-4], fields[-3]),
                    (kind, "yes", "yes", "yes"),
                )

    def test_pdftotext_reads_back_every_text_as_written(self):
        for font, _ in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                pdf, _ = draw(directory, font)
                text = run(["pdftotext", "-enc", "UTF-8", str(pdf), "-"]).stdout.decode("utf-8")
                lines = [line for line in text.splitlines() if line.strip()]
                self.assertEqual(sorted(lines), sorted(TEXT))

    def test_a_subset_renders_exactly_as_the_whole_font(self):
        for font, _ in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                # A copy whose license forbids subsetting, so every glyph is embedded under
                # its own glyph ID.
                whole_font = Path(directory) / f"whole{font.suffix}"
                whole_font.write_bytes(with_fs_type(font.read_bytes(), 0x0100))
                subset, _ = draw(directory, font, "subset.pdf")
                whole, stderr = draw(directory, whole_font, "whole.pdf")
                self.assertIn("forbids subsetting", stderr)
                subset_image, subset_stderr = render(subset)
                whole_image, whole_stderr = render(whole)
                # poppler reports font problems on stderr.
                self.assertEqual(subset_stderr, "")
                self.assertEqual(whole_stderr, "")
                self.assertEqual(subset_image, whole_image)

    def test_shapes_colours_and_clips_are_where_the_page_puts_them(self):
        for font, _ in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                pdf, _ = draw(directory, font)
                image, stderr = render(pdf)
                self.assertEqual(stderr, "")
                for (x, y), colour in PIXELS:
                    self.assertEqual(pixel(image, x, y), colour, f"at ({x}, {y})")

    def test_a_run_colour_stays_with_its_run(self):
        """The blue run is drawn in blue, and the next run, black, has no blue in it."""

        def colours(image, y):
            # The band of a line of 12 pt text with its baseline at y.
            return {pixel(image, x, row) for x in range(72, 250) for row in range(y - 3, y + 9)}

        for font, _ in CASES:
            with self.subTest(font=font.name), tempfile.TemporaryDirectory() as directory:
                pdf, _ = draw(directory, font)
                image, _ = render(pdf)
                self.assertIn((0, 0, 192), colours(image, 690))
                slanted = colours(image, 665)
                self.assertLess(min(slanted), (64, 64, 64))
                self.assertTrue(all(r == g == b for r, g, b in slanted), slanted)
