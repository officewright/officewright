"""Checks how officewright-pdf puts JPEG and PNG files into a PDF, with tools that share no
code with it. The images are made by images.py and drawn by the example `images`:

- qpdf validates the file structure;
- pdfimages lists each image with its size, colour space, bit depth and encoding: JPEG
  files and plain PNG files passed through, other PNG files decoded with a soft mask;
- pdftoppm renders every image with its colours where the page puts it, and its
  transparent parts show the green square behind it.

OW_IMAGES must point at the example's binary (scripts/check.sh sets it).
"""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import images
from test_primitives import pixel, render

REPOSITORY = Path(__file__).resolve().parent.parent.parent
SEEDS = REPOSITORY / "fuzz" / "seeds"

BACKGROUND = (0, 128, 0)


def cyan(rgb):
    """poppler converts CMYK to RGB as printing inks look, not by inverting: process cyan
    comes out about (0, 172, 239). Inverted samples left uninverted would be black."""
    r, g, b = rgb
    return r < 40 and g > 120 and b > 200


def magenta(rgb):
    """Process magenta, about (236, 0, 139)."""
    r, g, b = rgb
    return r > 200 and g < 40 and b > 100


# For each image, points as fractions of its width and height from its top left, and
# their colour.
COLOURS = {
    "rgb.png": [((0.25, 0.25), images.RED), ((0.75, 0.25), images.GREEN), ((0.25, 0.75), images.BLUE), ((0.75, 0.75), images.WHITE)],
    "palette.png": [((0.25, 0.5), images.RED), ((0.75, 0.5), BACKGROUND)],
    "rgba.png": [((0.25, 0.5), images.BLUE), ((0.75, 0.5), BACKGROUND)],
    "gray16.png": [((0.25, 0.5), images.BLACK), ((0.75, 0.5), images.WHITE)],
    "adam7.png": [((0.25, 0.25), images.RED), ((0.75, 0.25), images.GREEN), ((0.25, 0.75), images.BLUE), ((0.75, 0.75), images.WHITE)],
    "gray-key.png": [((0.25, 0.5), BACKGROUND), ((0.75, 0.5), images.BLACK)],
    "gray.jpg": [((0.25, 0.5), images.BLACK), ((0.75, 0.5), images.WHITE)],
    "ycbcr.jpg": [((0.25, 0.5), images.RED), ((0.75, 0.5), images.BLUE)],
    "cmyk.jpg": [((0.25, 0.5), cyan), ((0.75, 0.5), magenta)],
}


def draw(directory):
    """The PDF of every test image, and the example's stderr."""
    paths = []
    for name, data, _ in images.samples():
        path = Path(directory) / name
        path.write_bytes(data)
        paths.append(str(path))
    pdf = Path(directory) / "images.pdf"
    result = subprocess.run([os.environ["OW_IMAGES"], str(pdf), *paths], capture_output=True, check=True)
    return pdf, result.stderr.decode("utf-8")


class ImagesTest(unittest.TestCase):
    def test_qpdf_finds_no_errors_or_warnings(self):
        with tempfile.TemporaryDirectory() as directory:
            pdf, stderr = draw(directory)
            self.assertEqual(stderr, "")
            # 16-bit samples make the file PDF 1.5.
            self.assertTrue(pdf.read_bytes().startswith(b"%PDF-1.5\n"))
            result = subprocess.run(["qpdf", "--check", str(pdf)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_pdfimages_lists_each_image_as_it_went_in(self):
        with tempfile.TemporaryDirectory() as directory:
            pdf, _ = draw(directory)
            lines = subprocess.run(["pdfimages", "-list", str(pdf)], capture_output=True, text=True, check=True)
            listed = []
            for line in lines.stdout.splitlines()[2:]:
                # page num type width height color comp bpc enc interp object ID ...
                fields = line.split()
                listed.append((fields[2], int(fields[3]), int(fields[4]), fields[5], int(fields[6]), int(fields[7]), fields[8]))
            expected = [entry for _, _, entries in images.samples() for entry in entries]
            self.assertEqual(listed, expected)

    def test_images_render_with_their_colours_and_transparency(self):
        with tempfile.TemporaryDirectory() as directory:
            pdf, _ = draw(directory)
            image, stderr = render(pdf)
            self.assertEqual(stderr, "")
            for i, (name, _, _) in enumerate(images.samples()):
                x = 40 + 130 * (i % 4)
                y = 680 - 130 * (i // 4)
                for (u, v), colour in COLOURS[name]:
                    with self.subTest(image=name, at=(u, v)):
                        actual = pixel(image, int(x + 100 * u), int(y + 100 * (1 - v)))
                        if callable(colour):
                            self.assertTrue(colour(actual), f"{actual} is not {colour.__name__}")
                            continue
                        # JPEG and the conversion of YCbCr are not exact.
                        tolerance = 12 if name.endswith(".jpg") else 0
                        self.assertTrue(
                            all(abs(a - e) <= tolerance for a, e in zip(actual, colour)),
                            f"{actual} is not {colour}",
                        )

    def test_the_fuzz_seeds_are_what_images_py_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            images.write_seeds(directory)
            for kind in ("png", "jpeg"):
                written = sorted(p.name for p in (Path(directory) / kind).iterdir())
                committed = sorted(p.name for p in (SEEDS / kind).iterdir())
                self.assertEqual(committed, written, f"run: python3 tests/pdf-output/images.py fuzz/seeds")
                for name in written:
                    self.assertEqual(
                        (SEEDS / kind / name).read_bytes(),
                        (Path(directory) / kind / name).read_bytes(),
                        f"fuzz/seeds/{kind}/{name} differs; run: python3 tests/pdf-output/images.py fuzz/seeds",
                    )
