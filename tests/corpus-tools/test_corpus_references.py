"""Checks that the committed reference renders agree with each other and with SOURCES.md.

For every folder tests/corpus/reference/<group>/<name>/: geometry.json describes the
committed document, there is one valid grey PNG per page at 110 dpi, the re-saved copy
exists, and the SOURCES.md row names the folder, the page count and the PDF's SHA-256.

The checks scripts/corpus-import.py makes at import time are made again over what is
committed, so a later change cannot slip past them: no person's name, profile path or
embedded font in a re-saved copy, no language or number format Word added that SOURCES.md
does not name as accepted, no profile path in geometry.json or formatting.json, no PNG chunk
the importer does not write, Word's page breaks as the re-saved copy has them, and the
corpus limits of CLAUDE.md (each file under 1 MB, all under 50 MB).
"""

import importlib.util
import json
import math
import re
import struct
import sys
import unittest
from pathlib import Path

from pngcheck import png_pixels

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "tests" / "corpus"
REFERENCE = CORPUS / "reference"
DPI = 110
PER_METRE = round(DPI / 0.0254)


def load_importer():
    path = ROOT / "scripts" / "corpus-import.py"
    spec = importlib.util.spec_from_file_location("corpus_import", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


importer = load_importer()


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
                # formatting.json comes with references made by make-reference.ps1 format 3.
                files = sorted(p.name for p in folder.iterdir() if p.name != "formatting.json")
                self.assertEqual(files, sorted(pngs + ["geometry.json"]))
                if (folder / "formatting.json").exists():
                    text = (folder / "formatting.json").read_text(encoding="utf-8")
                    formatting = json.loads(text)
                    self.assertEqual(formatting["format"], 1)
                    self.assertEqual(formatting["document"], geometry["document"])
                    self.assertTrue(formatting["stories"])
                    fonts, formats = len(formatting["fonts"]), len(formatting["paragraph_formats"])
                    for story in formatting["stories"]:
                        for paragraph in story["paragraphs"]:
                            self.assertLess(paragraph["font"], fonts)
                            self.assertLess(paragraph["format"], formats)
                    self.assertLess((folder / "formatting.json").stat().st_size, 1_000_000)
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
                    substitute = ""
                    if "windows_substitute" in font:
                        substitute = (
                            f"drawn with {font['windows_substitute']} as Windows substitutes it, "
                        )
                    self.assertIn(f"{font['font']} ({substitute}{font['full_name']}, ", rows[0])
                    self.assertIn(f"SHA-256 {font['sha256']})", rows[0])
                count = geometry["page_count"]
                self.assertIn(f"; {count} page{'s' if count != 1 else ''};", rows[0])
                self.assertIn(f"SHA-256 {geometry['pdf_sha256']};", rows[0])
                self.assertRegex(geometry["pdf_sha256"], r"^[0-9a-f]{64}$")

    def resaved(self, folder):
        group, name = folder.parent.name, folder.name
        return CORPUS / "resaved" / group / f"{name}.docx", CORPUS / group / f"{name}.docx"

    def test_re_saved_copies_carry_no_personal_data_or_embedded_fonts(self):
        for folder in folders():
            resaved, _ = self.resaved(folder)
            with self.subTest(document=f"{folder.parent.name}/{folder.name}"):
                problems = importer.docx_privacy_problems(resaved)
                problems += importer.embedded_font_problems(importer.read_parts(resaved))
                self.assertEqual(problems, [])

    def row(self, folder):
        key = f"| reference/{folder.parent.name}/{folder.name}/ |"
        (row,) = [r for r in self.sources.splitlines() if r.startswith(key)]
        return row

    def notes(self, folder):
        """The Notes cell of the folder's SOURCES.md row, as its separate notes."""
        return self.row(folder).rstrip(" |").rsplit(" | ", 1)[-1].split("; ")

    def test_locale_traces_are_only_those_sources_md_names_as_accepted(self):
        for folder in folders():
            resaved, original = self.resaved(folder)
            with self.subTest(document=f"{folder.parent.name}/{folder.name}"):
                added = importer.locale_problems(
                    importer.read_parts(original), importer.read_parts(resaved)
                )
                accepted = [
                    note for note in self.notes(folder)
                    if note.startswith("accepted with --allow-locale")
                ]
                self.assertEqual(accepted, [importer.locale_note(added)] if added else [])

    def test_sources_md_describes_each_formatting_export(self):
        for folder in folders():
            with self.subTest(document=f"{folder.parent.name}/{folder.name}"):
                said = [
                    note for note in self.notes(folder)
                    if note.startswith("formatting as Word computes it")
                ]
                path = folder / "formatting.json"
                if path.exists():
                    formatting = json.loads(path.read_text(encoding="utf-8"))
                    self.assertEqual(said, [importer.formatting_note(formatting)])
                else:
                    self.assertEqual(said, [], "the row describes a formatting.json")

    def test_word_s_page_breaks_are_those_of_the_re_saved_copy(self):
        for folder in folders():
            resaved, _ = self.resaved(folder)
            with self.subTest(document=f"{folder.parent.name}/{folder.name}"):
                geometry = json.loads((folder / "geometry.json").read_text(encoding="utf-8"))
                expected = importer.rendered_page_breaks(importer.read_parts(resaved))
                self.assertEqual(geometry["page_breaks"], expected)

    def test_every_re_saved_copy_belongs_to_a_reference(self):
        # Every file at any depth: resaved/<group>/<name>.docx with its reference folder.
        resaved = CORPUS / "resaved"
        for path in sorted(p for p in resaved.rglob("*") if p.is_file()):
            relative = path.relative_to(resaved)
            with self.subTest(file=str(path.relative_to(CORPUS))):
                self.assertEqual(len(relative.parts), 2, "not resaved/<group>/<name>.docx")
                self.assertEqual(path.suffix, ".docx")
                self.assertTrue((REFERENCE / relative.parts[0] / path.stem).is_dir())

    def test_reference_files_name_no_profile_path(self):
        for folder in folders():
            for path in sorted(folder.glob("*.json")):
                with self.subTest(file=str(path.relative_to(CORPUS))):
                    self.assertIsNone(importer.PROFILE_PATH.search(path.read_bytes()))

    def test_pngs_hold_only_the_chunks_the_importer_writes(self):
        for folder in folders():
            for path in sorted(folder.glob("*.png")):
                with self.subTest(file=str(path.relative_to(CORPUS))):
                    data = path.read_bytes()
                    kinds, position = [], 8
                    while position < len(data):
                        (length,) = struct.unpack(">I", data[position : position + 4])
                        kinds.append(data[position + 4 : position + 8])
                        position += 12 + length
                    self.assertTrue(set(kinds) <= {b"IHDR", b"pHYs", b"IDAT", b"IEND"}, kinds)

    def test_the_corpus_keeps_within_its_limits(self):
        files = [path for path in CORPUS.rglob("*") if path.is_file()]
        for path in files:
            with self.subTest(file=str(path.relative_to(CORPUS))):
                self.assertLess(path.stat().st_size, 1_000_000)
        self.assertLess(sum(path.stat().st_size for path in files), 50_000_000)

    def test_every_sources_row_has_its_folder(self):
        for path in re.findall(r"^\| reference/([^ |]+)/ \|", self.sources, re.M):
            with self.subTest(row=path):
                self.assertTrue((REFERENCE / path / "geometry.json").is_file())


if __name__ == "__main__":
    unittest.main()
