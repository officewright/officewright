"""Checks that the committed reference renders agree with each other and with SOURCES.md.

For every folder tests/corpus/reference/<group>/<name>/: geometry.json describes the
committed document, there is one valid grey PNG per page at 110 dpi, the re-saved copy
exists, and the SOURCES.md row names the folder, the page count and the PDF's SHA-256.
"""

import json
import math
import re
import unittest
from pathlib import Path

from pngcheck import png_pixels

CORPUS = Path(__file__).resolve().parents[2] / "tests" / "corpus"
REFERENCE = CORPUS / "reference"
DPI = 110
PER_METRE = round(DPI / 0.0254)


def folders():
    return sorted(path for path in REFERENCE.glob("*/*") if path.is_dir())


class References(unittest.TestCase):
    def setUp(self):
        self.sources = (CORPUS / "SOURCES.md").read_text(encoding="utf-8")

    def test_there_are_references(self):
        self.assertTrue(folders(), "tests/corpus/reference/ holds no rendered documents")

    def test_each_folder_holds_its_document_pages_and_geometry(self):
        for folder in folders():
            group, name = folder.parent.name, folder.name
            with self.subTest(document=f"{group}/{name}"):
                geometry = json.loads((folder / "geometry.json").read_text(encoding="utf-8"))
                self.assertEqual(geometry["format"], 1)
                self.assertEqual(geometry["document"], f"{group}/{name}.docx")
                self.assertTrue((CORPUS / group / f"{name}.docx").is_file())
                self.assertTrue((CORPUS / "resaved" / group / f"{name}.docx").is_file())
                count = geometry["page_count"]
                self.assertEqual(len(geometry["pages"]), count)
                pngs = sorted(p.name for p in folder.glob("*.png"))
                self.assertEqual(pngs, [f"page-{n:03}.png" for n in range(1, count + 1)])
                self.assertEqual(
                    sorted(p.name for p in folder.iterdir()), sorted(pngs + ["geometry.json"])
                )
                for page in geometry["pages"]:
                    png = (folder / f"page-{page['number']:03}.png").read_bytes()
                    width, height, per_metre, _ = png_pixels(png)
                    # pdftoppm rounds the page size in pixels up.
                    self.assertEqual(width, math.ceil(page["width"] * DPI / 72))
                    self.assertEqual(height, math.ceil(page["height"] * DPI / 72))
                    self.assertEqual(per_metre, (PER_METRE, PER_METRE, 1))

    def test_every_font_word_did_not_embed_is_drawn_with_a_named_file(self):
        for folder in folders():
            with self.subTest(document=f"{folder.parent.name}/{folder.name}"):
                geometry = json.loads((folder / "geometry.json").read_text(encoding="utf-8"))
                drawn = geometry["fonts_rasterized_with"]
                self.assertEqual([font["font"] for font in drawn], geometry["fonts_not_embedded"])
                for font in drawn:
                    self.assertRegex(font["sha256"], r"^[0-9a-f]{64}$")
                    self.assertTrue(font["full_name"])

    def test_each_folder_has_its_sources_row(self):
        for folder in folders():
            group, name = folder.parent.name, folder.name
            with self.subTest(document=f"{group}/{name}"):
                geometry = json.loads((folder / "geometry.json").read_text(encoding="utf-8"))
                key = f"| reference/{group}/{name}/ |"
                rows = [r for r in self.sources.splitlines() if r.startswith(key)]
                self.assertEqual(len(rows), 1)
                for font in geometry["fonts_rasterized_with"]:
                    self.assertIn(f"{font['font']} ({font['full_name']}, ", rows[0])
                    self.assertIn(f"SHA-256 {font['sha256']})", rows[0])
                count = geometry["page_count"]
                self.assertIn(f"; {count} page{'s' if count != 1 else ''};", rows[0])
                self.assertIn(f"SHA-256 {geometry['pdf_sha256']};", rows[0])
                self.assertRegex(geometry["pdf_sha256"], r"^[0-9a-f]{64}$")

    def test_every_sources_row_has_its_folder(self):
        for path in re.findall(r"^\| reference/([^ |]+)/ \|", self.sources, re.M):
            with self.subTest(row=path):
                self.assertTrue((REFERENCE / path / "geometry.json").is_file())


if __name__ == "__main__":
    unittest.main()
