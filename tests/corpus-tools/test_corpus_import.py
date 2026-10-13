"""Checks scripts/corpus-import.py with fake make-reference.ps1 results.

The poppler tools are replaced by fixed output, so the checks run where poppler is not
installed (Windows) and do not depend on its version.
"""

import hashlib
import importlib.util
import json
import shutil
import struct
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

from pngcheck import png_pixels

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


importer = load("corpus-import")
rules = load("make-rule-docs")

# pdffonts 24.02 output for a document with one embedded subset and one referenced font.
PDFFONTS = """\
name                                 type              encoding         emb sub uni object ID
------------------------------------ ----------------- ---------------- --- --- --- ---------
ABCDEF+Calibri                       CID TrueType      Identity-H       yes yes yes      9  0
Garamond                             TrueType          WinAnsi          no  no  no      12  0
"""

# The same after the importer embedded Garamond into its copy.
PDFFONTS_EMBEDDED = PDFFONTS.replace(
    "Garamond                             TrueType          WinAnsi          no  no  no ",
    "Garamond                             TrueType          WinAnsi          yes no  no ",
)
assert PDFFONTS_EMBEDDED != PDFFONTS

# qpdf --json=2 output (abridged) for that document: Calibri's descriptor embeds a font file,
# Garamond's does not; a second Garamond descriptor (another encoding) embeds one already.
OBJECTS = {
    "qpdf": [
        {"jsonversion": 2, "maxobjectid": 12},
        {
            "obj:7 0 R": {"value": {"/Type": "/FontDescriptor", "/FontName": "/Garamond",
                                    "/FontFile2": "8 0 R"}},
            "obj:9 0 R": {"value": {"/Type": "/FontDescriptor", "/FontName": "/ABCDEF+Calibri",
                                    "/FontFile2": "10 0 R"}},
            "obj:10 0 R": {"stream": {"dict": {"/Length1": 1000}}},
            "obj:11 0 R": {"value": {"/Type": "/Font", "/BaseFont": "/Garamond",
                                     "/FontDescriptor": "12 0 R"}},
            "obj:12 0 R": {"value": {"/Type": "/FontDescriptor", "/FontName": "/Garamond",
                                     "/Flags": 32, "/Ascent": 891}},
        },
    ]
}


def name_table(names):
    """A name table with Windows Unicode records, US English, for the given IDs."""
    records, storage = b"", b""
    for name_id, text in sorted(names.items()):
        data = text.encode("utf-16-be")
        records += struct.pack(">HHHHHH", 3, 1, 0x409, name_id, len(data), len(storage))
        storage += data
    return struct.pack(">HHH", 0, len(names), 6 + len(records)) + records + storage


def face_tables(postscript, full_name, version, cff):
    # head with a stale checkSumAdjustment (bytes 8 to 12), which a cut-out face must fix.
    head = struct.pack(">IIII", 0x00010000, 0x00010000, 0x12345678, 0x5F0F3CF5) + bytes(38)
    tables = {b"name": name_table({4: full_name, 5: version, 6: postscript}), b"head": head}
    tables[b"CFF " if cff else b"glyf"] = b"\x00" * 8
    return sorted(tables.items())


def directory(version, tables, start):
    """An offset table and its records for tables laid out from start, and the tables."""
    head = version + struct.pack(">HHHH", len(tables), 16, 0, 0)
    records, body = b"", b""
    for tag, data in tables:
        records += struct.pack(">4sIII", tag, 0, start + len(body), len(data))
        body += data + b"\x00" * (-len(data) % 4)
    return head + records, body


def sfnt(postscript, full_name="Test Font", version="Version 1.0", cff=False):
    """A font file with a name table and one outline table, enough for the importer."""
    tables = face_tables(postscript, full_name, version, cff)
    head, body = directory(b"OTTO" if cff else b"\x00\x01\x00\x00", tables, 12 + 16 * len(tables))
    return head + body


def collection(*faces):
    """A TrueType collection of faces given as (postscript, full_name, version)."""
    header_size = 12 + 4 * len(faces)
    tables = [face_tables(*face, False) for face in faces]
    directories_size = sum(12 + 16 * len(face) for face in tables)
    heads, bodies, offsets = b"", b"", []
    for face in tables:
        offsets.append(header_size + len(heads))
        head, body = directory(
            b"\x00\x01\x00\x00", face, header_size + directories_size + len(bodies)
        )
        heads += head
        bodies += body
    header = b"ttcf" + struct.pack(">HHI", 1, 0, len(faces))
    header += struct.pack(f">{len(faces)}I", *offsets)
    return header + heads + bodies


# pdftotext -bbox-layout output for three pages, one line on the first.
BBOX = """\
<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title></title></head>
<body>
<doc>
  <page width="595.320000" height="841.920000">
    <flow><block xMin="72.024000" yMin="79.820000" xMax="135.000000" yMax="90.860000">
      <line xMin="72.024000" yMin="79.820000" xMax="135.000000" yMax="90.860000">
        <word xMin="72.024000" yMin="79.820000" xMax="93.011000" yMax="90.860000">Case</word>
        <word xMin="95.429000" yMin="79.820000" xMax="135.000000" yMax="90.860000">one</word>
      </line>
    </block></flow>
  </page>
  <page width="595.320000" height="841.920000"></page>
  <page width="595.320000" height="841.920000"></page>
</doc>
</body>
</html>
"""


def pixels_of(page, width=4, height=2):
    """Distinct grey values per pixel, so a test sees any reordering."""
    return bytes((10 * page + index) % 256 for index in range(width * height))


def pgm(width, height, pixels):
    return f"P5\n{width} {height}\n255\n".encode() + pixels


SOURCES = """\
# Corpus sources

## Reference renders (`reference/`)

| File | Rendered from | Application and version | Date | Notes |
|------|---------------|-------------------------|------|-------|

## External corpora
"""


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def rewrite(path, changes):
    """Rewrite a package, replacing bytes in the named parts."""
    with zipfile.ZipFile(path) as archive:
        parts = [(info.filename, archive.read(info)) for info in archive.infolist()]
    with zipfile.ZipFile(path, "w") as archive:
        for name, data in parts:
            for old, new in changes.get(name, []):
                data = data.replace(old, new)
            archive.writestr(name, data)


class Import(unittest.TestCase):
    def setUp(self):
        self.temp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.temp)
        self.corpus = self.temp / "corpus"
        self.results = self.temp / "out"
        (self.corpus / "word-rules").mkdir(parents=True)
        self.results.mkdir()
        (self.corpus / "SOURCES.md").write_text(SOURCES, encoding="utf-8")
        self.info = "Producer:        Microsoft Word\nAuthor:          \nPages:           3\n"
        self.xmp = ""
        self.bbox = BBOX
        self.raster_pages = 3
        self.font_dir = self.temp / "fonts"
        self.font_dir.mkdir()
        self.garamond = sfnt("Garamond", "Garamond", "Version 2.40")
        (self.font_dir / "GARA.TTF").write_bytes(self.garamond)
        self.pdffonts = PDFFONTS
        self.copy_fonts = PDFFONTS_EMBEDDED
        self.updates = []
        self.rasterized = []
        self.tools = importer.Tools(
            info=lambda path: self.info,
            xmp=lambda path: self.xmp,
            fonts=self.fonts,
            raster=self.raster,
            bbox=lambda path: self.bbox,
            version=lambda: "24.02.0",
            objects=lambda path: json.dumps(OBJECTS),
            update=self.update,
        )
        for doc in rules.documents()[:2]:
            self.add_result(doc)

    def add_result(self, doc, **changes):
        original = rules.package(doc)
        (self.corpus / "word-rules" / f"{doc.name}.docx").write_bytes(original)
        resaved = self.results / f"{doc.name}.resaved.docx"
        resaved.write_bytes(original)
        pdf = self.results / f"{doc.name}.pdf"
        pdf.write_bytes(b"%PDF-1.7\n%%EOF\n")
        record = {
            "script_version": 2,
            "file": f"{doc.name}.docx",
            "sha256": sha256(original),
            "resaved_file": resaved.name,
            "resaved_sha256": sha256(resaved.read_bytes()),
            "pdf_file": pdf.name,
            "pdf_sha256": sha256(pdf.read_bytes()),
            "status": "rendered",
            "error": "",
            "pages": 3,
            "compatibility_mode": 15,
            "fonts": ["Calibri"],
            "fonts_missing": [],
            "privacy_findings": [],
            "word_version": "16.0",
            "word_build": "16.0.1",
            "printer": "Microsoft Print to PDF",
            "default_printer": "Microsoft Print to PDF",
            "windows_version": "10.0.26100.0",
            "windows_display_version": "24H2",
            "rendered": "2026-10-13",
            "export_seconds": 1.5,
        }
        record.update(changes)
        (self.results / f"{doc.name}.json").write_text(json.dumps(record), encoding="utf-8")
        return record

    def fonts(self, pdf):
        return self.copy_fonts if pdf.name == "embedded.pdf" else self.pdffonts

    def update(self, pdf, update, copy):
        """qpdf --update-from-json: records the update with the data of each new stream."""
        update = json.loads(update.read_text(encoding="utf-8"))
        data = {
            key: Path(value["stream"]["datafile"]).read_bytes()
            for key, value in update["qpdf"][1].items()
            if "stream" in value
        }
        self.updates.append((pdf.name, update, data))
        copy.write_bytes(pdf.read_bytes())

    def raster(self, pdf, directory):
        self.rasterized.append(pdf.name)
        pages = []
        for number in range(1, self.raster_pages + 1):
            page = directory / f"page-{number}.pgm"
            page.write_bytes(pgm(4, 2, pixels_of(number)))
            pages.append(page)
        return pages

    def save_record(self, record):
        name = record["file"][: -len(".docx")]
        (self.results / f"{name}.json").write_text(json.dumps(record), encoding="utf-8")

    def change_resaved(self, record, changes):
        """Edit the re-saved copy and record its new hash, as if Word had written it."""
        resaved = self.results / record["resaved_file"]
        rewrite(resaved, changes)
        record["resaved_sha256"] = sha256(resaved.read_bytes())
        self.save_record(record)

    def run_import(self, dry_run=False, allow_locale=False, font_dirs=None):
        return importer.import_results(
            self.results,
            "word-rules",
            self.corpus,
            self.tools,
            dry_run,
            allow_locale,
            [self.font_dir] if font_dirs is None else font_dirs,
        )

    def refused(self, **arguments):
        with self.assertRaises(importer.Refused) as caught:
            self.run_import(**arguments)
        self.assertFalse((self.corpus / "reference").exists(), "nothing may be copied")
        return str(caught.exception)

    def test_imports_rasters_geometry_and_sources_rows_but_no_pdf(self):
        checked = self.run_import()
        self.assertEqual(len(checked), 2)
        reference = self.corpus / "reference" / "word-rules"
        resaved = self.corpus / "resaved" / "word-rules"
        self.assertEqual(
            sorted(p.name for p in reference.iterdir()), ["E-line-exact", "E-line-fonts"]
        )
        self.assertEqual(
            sorted(p.name for p in (reference / "E-line-fonts").iterdir()),
            ["geometry.json", "page-001.png", "page-002.png", "page-003.png"],
        )
        self.assertEqual(list(self.corpus.rglob("*.pdf")), [])
        self.assertEqual(sorted(p.name for p in resaved.iterdir()),
                         ["E-line-exact.docx", "E-line-fonts.docx"])
        sources = (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        pdf_sha256 = sha256(b"%PDF-1.7\n%%EOF\n")
        self.assertIn(
            "| reference/word-rules/E-line-fonts/ | word-rules/E-line-fonts.docx, re-saved by "
            "Word as resaved/word-rules/E-line-fonts.docx | Microsoft Word 16.0, build 16.0.1, "
            "Windows 10.0.26100.0 (24H2) | 2026-10-13 | compatibility mode 15; 3 pages; printer "
            "Microsoft Print to PDF; fonts Word substituted: none; w:compat settings Word "
            "dropped from the re-saved copy: none; fields as saved; PDF not "
            f"committed, SHA-256 {pdf_sha256}; rasters and geometry by poppler 24.02.0 at 110 dpi, "
            "grey; fonts the PDF does not embed, rasterized with the maintainer's installed "
            f"copies: Garamond (Garamond, Version 2.40, SHA-256 {sha256(self.garamond)}) |",
            sources,
        )

    def test_rasters_are_grey_pngs_of_each_page(self):
        self.run_import()
        pages = self.corpus / "reference" / "word-rules" / "E-line-fonts"
        for number in (1, 2, 3):
            width, height, per_metre, pixels = png_pixels(
                (pages / f"page-{number:03}.png").read_bytes()
            )
            self.assertEqual((width, height, pixels), (4, 2, pixels_of(number)))
            self.assertEqual(per_metre, (4331, 4331, 1))  # 110 dpi

    def test_geometry_holds_pages_lines_words_and_word_page_breaks(self):
        doc = rules.documents()[0]
        record = self.add_result(doc)
        # Word marks a page break before the first line of the first paragraph, and one in
        # the middle of the third paragraph, after "Cambria 11 pt, line 01: ...gjpqy".
        first = b'<w:t xml:space="preserve">Calibri 11 pt, line 01'
        third = b'<w:t xml:space="preserve">Cambria 11 pt, line 02'
        self.change_resaved(record, {"word/document.xml": [
            (first, b"<w:lastRenderedPageBreak/>" + first),
            (third, b"<w:lastRenderedPageBreak/>" + third),
        ]})
        self.run_import()
        geometry = json.loads(
            (self.corpus / "reference" / "word-rules" / doc.name / "geometry.json")
            .read_text(encoding="utf-8")
        )
        self.assertEqual(geometry["page_count"], 3)
        self.assertEqual(geometry["document"], f"word-rules/{doc.name}.docx")
        self.assertEqual(geometry["fonts_not_embedded"], ["Garamond"])
        page = geometry["pages"][0]
        self.assertEqual((page["number"], page["width"], page["height"]), (1, 595.32, 841.92))
        self.assertEqual(page["lines"][0]["text"], "Case one")
        self.assertEqual(page["lines"][0]["box"], [72.024, 79.82, 135.0, 90.86])
        self.assertEqual(
            page["lines"][0]["words"][1], {"text": "one", "box": [95.429, 79.82, 135.0, 90.86]}
        )
        first, third = geometry["page_breaks"]
        self.assertEqual((first["paragraph"], first["offset"], first["text_before"]), (0, 0, ""))
        self.assertTrue(first["text_after"].startswith("Calibri 11 pt, line 01"))
        line = "Cambria 11 pt, line 01: Hamburgefonts gjpqy"
        self.assertEqual((third["paragraph"], third["offset"]), (2, len(line)))
        self.assertEqual(third["text_before"], line[-30:])
        self.assertTrue(third["text_after"].startswith("Cambria 11 pt, line 02"))

    def test_importing_again_replaces_rows_and_old_pages(self):
        self.run_import()
        stale = self.corpus / "reference" / "word-rules" / "E-line-fonts" / "page-009.png"
        stale.write_bytes(b"stale")
        self.run_import()
        sources = (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        self.assertEqual(sources.count("| reference/word-rules/E-line-fonts/ |"), 1)
        self.assertEqual(sources.count("| reference/word-rules/E-line-exact/ |"), 1)
        self.assertFalse(stale.exists())

    def test_records_compat_flags_word_dropped(self):
        doc = next(d for d in rules.documents() if d.name == "E-tab-hanging-noindenttab")
        record = self.add_result(doc)
        self.change_resaved(record, {"word/settings.xml": [
            (b"<w:doNotUseIndentAsNumberingTabStop/>", b""),
        ]})
        self.run_import()
        folder = self.corpus / "reference" / "word-rules" / doc.name
        geometry = json.loads((folder / "geometry.json").read_text(encoding="utf-8"))
        self.assertEqual(geometry["compat_flags_dropped"], ["doNotUseIndentAsNumberingTabStop"])
        self.assertEqual(geometry["compat_flags_added"], [])
        sources = (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        self.assertIn(
            "w:compat settings Word dropped from the re-saved copy: "
            "doNotUseIndentAsNumberingTabStop;",
            sources,
        )

    def test_records_compat_settings_word_added_or_dropped(self):
        record = self.add_result(rules.documents()[0])
        hyphenation = (
            b'<w:compatSetting w:name="useWord2013TrackBottomHyphenation" '
            b'w:uri="http://schemas.microsoft.com/office/word" w:val="1"/>'
        )
        self.change_resaved(record, {"word/settings.xml": [
            (b"</w:compat>", hyphenation + b"</w:compat>"),
            (b'<w:compatSetting w:name="enableOpenTypeFeatures" '
             b'w:uri="http://schemas.microsoft.com/office/word" w:val="1"/>', b""),
        ]})
        self.run_import()
        folder = self.corpus / "reference" / "word-rules" / rules.documents()[0].name
        geometry = json.loads((folder / "geometry.json").read_text(encoding="utf-8"))
        self.assertEqual(geometry["compat_flags_added"], ["useWord2013TrackBottomHyphenation=1"])
        self.assertEqual(geometry["compat_flags_dropped"], ["enableOpenTypeFeatures=1"])

    def test_refuses_a_resaved_copy_without_a_document_part(self):
        record = self.add_result(rules.documents()[0])
        resaved = self.results / record["resaved_file"]
        with zipfile.ZipFile(resaved) as archive:
            parts = [(i.filename, archive.read(i)) for i in archive.infolist()]
        with zipfile.ZipFile(resaved, "w") as archive:
            for name, data in parts:
                if name != "word/document.xml":
                    archive.writestr(name, data)
        record["resaved_sha256"] = sha256(resaved.read_bytes())
        self.save_record(record)
        self.assertIn("cannot read a package", self.refused())

    def test_refuses_an_image_pdftoppm_should_not_write(self):
        original = self.raster
        for image in (b"P6\n1 1\n255\n\x00\x00\x00", b"P5\n1 1\n65535\n\x00\x00"):
            with self.subTest(image=image[:2]):

                def unexpected(pdf, directory, image=image):
                    pages = original(pdf, directory)
                    pages[0].write_bytes(image)
                    return pages

                self.tools = importer.Tools(**{**self.tools.__dict__, "raster": unexpected})
                self.assertIn("pdftoppm wrote an unexpected image", self.refused())

    def test_rasterizes_a_copy_with_the_fonts_the_pdf_does_not_embed(self):
        self.run_import()
        self.assertEqual(self.rasterized, ["embedded.pdf", "embedded.pdf"])
        source, update, data = self.updates[0]
        self.assertTrue(source.endswith(".pdf") and source != "embedded.pdf")
        header, objects = update["qpdf"]
        self.assertEqual((header["jsonversion"], header["maxobjectid"]), (2, 13))
        # Garamond's descriptor gains the file, keeps everything else; Calibri's, which
        # embeds a font already, is not touched.
        self.assertEqual(
            objects["obj:12 0 R"]["value"],
            {"/Type": "/FontDescriptor", "/FontName": "/Garamond", "/Flags": 32,
             "/Ascent": 891, "/FontFile2": "13 0 R"},
        )
        self.assertEqual(objects["obj:13 0 R"]["stream"]["dict"], {"/Length1": len(self.garamond)})
        self.assertEqual(data, {"obj:13 0 R": self.garamond})
        self.assertEqual(sorted(objects), ["obj:12 0 R", "obj:13 0 R"])

    def test_a_cff_font_is_embedded_as_opentype(self):
        noto = sfnt("Garamond", "Garamond", "Version 3", cff=True)
        (self.font_dir / "GARA.TTF").unlink()
        (self.font_dir / "Garamond.otf").write_bytes(noto)
        self.run_import()
        objects = self.updates[0][1]["qpdf"][1]
        self.assertEqual(objects["obj:12 0 R"]["value"]["/FontFile3"], "13 0 R")
        self.assertNotIn("/FontFile2", objects["obj:12 0 R"]["value"])
        self.assertEqual(objects["obj:13 0 R"]["stream"]["dict"], {"/Subtype": "/OpenType"})
        self.assertEqual(self.updates[0][2], {"obj:13 0 R": noto})

    def test_geometry_names_the_font_file_by_version_and_hash_not_by_path(self):
        self.run_import()
        folder = self.corpus / "reference" / "word-rules" / "E-line-fonts"
        geometry = json.loads((folder / "geometry.json").read_text(encoding="utf-8"))
        self.assertEqual(geometry["fonts_not_embedded"], ["Garamond"])
        self.assertEqual(
            geometry["fonts_rasterized_with"],
            [{"font": "Garamond", "full_name": "Garamond", "version": "Version 2.40",
              "sha256": sha256(self.garamond)}],
        )
        written = (folder / "geometry.json").read_text(encoding="utf-8")
        written += (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        for secret in (str(self.font_dir), str(self.temp), "GARA.TTF"):
            self.assertNotIn(secret, written)

    def test_refuses_a_font_no_font_folder_has(self):
        message = self.refused(font_dirs=[])
        self.assertIn("Garamond is not embedded in the PDF", message)
        self.assertIn("--font-dir", message)
        (self.font_dir / "GARA.TTF").write_bytes(sfnt("Garamond-Bold"))
        self.assertIn("Garamond is not embedded in the PDF", self.refused())

    def test_a_dry_run_checks_the_fonts_but_embeds_nothing(self):
        self.assertIn("Garamond is not embedded", self.refused(dry_run=True, font_dirs=[]))
        self.run_import(dry_run=True)
        self.assertEqual((self.updates, self.rasterized), ([], []))

    def test_refuses_different_fonts_of_the_same_name(self):
        (self.font_dir / "gara2.ttf").write_bytes(self.garamond)  # the same font twice is fine
        self.run_import()
        shutil.rmtree(self.corpus / "reference")
        (self.font_dir / "gara3.ttf").write_bytes(sfnt("Garamond", "Garamond", "Version 2.41"))
        message = self.refused()
        self.assertIn("different fonts of that name: GARA.TTF, gara2.ttf, gara3.ttf", message)

    def test_refuses_a_style_word_synthesizes(self):
        self.pdffonts = PDFFONTS.replace("Garamond          ", "Garamond,Bold     ")
        self.assertIn("Garamond,Bold: Word names a style it synthesizes", self.refused())

    def test_refuses_a_copy_that_still_does_not_embed_the_font(self):
        self.copy_fonts = PDFFONTS
        self.assertIn("still not embedded after embedding: Garamond", self.refused())

    def test_refuses_a_font_name_that_would_break_the_row(self):
        (self.font_dir / "GARA.TTF").write_bytes(sfnt("Garamond", "Gara|mond"))
        self.assertIn("unexpected name or version in GARA.TTF", self.refused())

    def test_a_face_of_a_collection_is_cut_out(self):
        (self.font_dir / "GARA.TTF").unlink()
        (self.font_dir / "fonts.ttc").write_bytes(
            collection(("Other", "Other", "Version 1"), ("Garamond", "Garamond", "Version 2.40"))
        )
        self.run_import()
        data = self.updates[0][2]["obj:13 0 R"]
        # The cut-out face is a font of its own with the face's tables.
        (self.temp / "cut.ttf").write_bytes(data)
        (face,) = importer.font_faces(self.temp / "cut.ttf")
        self.assertEqual((face.postscript, face.collection, face.cff), ("Garamond", False, False))
        folder = self.corpus / "reference" / "word-rules" / "E-line-fonts"
        geometry = json.loads((folder / "geometry.json").read_text(encoding="utf-8"))
        self.assertEqual(geometry["fonts_rasterized_with"][0]["face_in_collection"], 1)

    def test_refuses_fonts_word_embedded(self):
        record = self.add_result(rules.documents()[0])
        resaved = self.results / record["resaved_file"]
        with zipfile.ZipFile(resaved, "a") as archive:
            archive.writestr("word/fonts/font1.odttf", b"font")
        self.change_resaved(record, {"word/fontTable.xml": [
            (b'<w:font w:name="Calibri"/>',
             b'<w:font w:name="Calibri"><w:embedRegular w:fontKey="{0}" r:id="rId1" '
             b'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"/>'
             b"</w:font>"),
        ]})
        message = self.refused()
        self.assertIn("word/fonts/font1.odttf is an embedded font", message)
        self.assertIn("word/fontTable.xml has embedRegular", message)

    def test_refuses_a_page_count_pdftotext_does_not_find(self):
        self.bbox = BBOX.replace('<page width="595.320000" height="841.920000"></page>', "", 1)
        self.assertIn("pdftotext found 2 pages, not 3", self.refused())

    def test_refuses_rasters_of_another_page_count_and_writes_nothing(self):
        self.raster_pages = 2
        message = self.refused()
        self.assertIn("pdftoppm wrote 2 pages, not 3", message)
        self.assertEqual((self.corpus / "SOURCES.md").read_text(encoding="utf-8"), SOURCES)

    def test_dry_run_copies_nothing(self):
        self.assertEqual(len(self.run_import(dry_run=True)), 2)
        self.assertFalse((self.corpus / "reference").exists())
        self.assertEqual((self.corpus / "SOURCES.md").read_text(encoding="utf-8"), SOURCES)

    def test_accepts_a_single_element_array_written_as_its_element(self):
        doc = rules.documents()[0]
        self.add_result(doc, fonts_missing="Aptos")
        self.run_import()
        sources = (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        self.assertIn("fonts Word substituted: Aptos;", sources)

    def test_refuses_a_failed_render(self):
        self.add_result(rules.documents()[0], status="failed", error="Word exited")
        self.assertIn("status failed: Word exited", self.refused())

    def test_refuses_personal_data_found_on_windows(self):
        self.add_result(
            rules.documents()[0],
            status="privacy-failed",
            privacy_findings=["docProps/core.xml: the Office user name"],
        )
        self.assertIn("docProps/core.xml: the Office user name", self.refused())

    def test_refuses_other_bytes_than_the_committed_document(self):
        doc = rules.documents()[0]
        self.add_result(doc)
        (self.corpus / "word-rules" / f"{doc.name}.docx").write_bytes(b"changed")
        self.assertIn("other bytes than the committed file", self.refused())

    def test_refuses_an_output_changed_after_word_wrote_it(self):
        doc = rules.documents()[0]
        self.add_result(doc)
        (self.results / f"{doc.name}.pdf").write_bytes(b"%PDF-1.7\nchanged\n%%EOF\n")
        self.assertIn(f"{doc.name}.pdf changed after Word wrote it", self.refused())

    def test_refuses_names_in_the_resaved_document(self):
        doc = rules.documents()[0]
        record = self.add_result(doc)
        resaved = self.results / record["resaved_file"]
        with zipfile.ZipFile(resaved, "a") as archive:
            app = "<Properties><Company>Example Corp</Company><Manager>Joe</Manager></Properties>"
            archive.writestr("docProps/app.xml", app)
        self.change_resaved(record, {
            "docProps/core.xml": [
                (b"<dc:creator>Officewright", b"<dc:creator>John Doe"),
                (b"<cp:lastModifiedBy>Officewright", b"<cp:lastModifiedBy>Jane Roe"),
            ],
            "word/settings.xml": [
                (b"<w:zoom", b"<!-- C:\\Users\\<name>\\Normal.dotm --><w:zoom"),
            ],
        })
        message = self.refused()
        self.assertIn("docProps/core.xml: creator is 'John Doe'", message)
        self.assertIn("docProps/core.xml: lastModifiedBy is 'Jane Roe'", message)
        self.assertIn("docProps/app.xml: Company is 'Example Corp'", message)
        self.assertIn("docProps/app.xml: Manager is 'Joe'", message)
        self.assertIn("word/settings.xml contains a user profile path", message)

    def test_refuses_a_profile_path_in_the_pdf(self):
        doc = rules.documents()[0]
        record = self.add_result(doc)
        pdf = self.results / record["pdf_file"]
        pdf.write_bytes(b"%PDF-1.7\n% C:\\Users\\<name>\\file.docx\n%%EOF\n")
        record["pdf_sha256"] = sha256(pdf.read_bytes())
        self.save_record(record)
        self.assertIn("PDF contains a user profile path", self.refused())

    def test_refuses_findings_even_when_the_status_says_rendered(self):
        self.add_result(rules.documents()[0], privacy_findings=["pdf: the Windows user name"])
        self.assertIn("personal data: pdf: the Windows user name", self.refused())

    def test_refuses_a_missing_output(self):
        record = self.add_result(rules.documents()[0])
        (self.results / record["resaved_file"]).unlink()
        self.assertIn(f"{record['resaved_file']} is missing", self.refused())

    def test_refuses_a_document_that_is_not_committed(self):
        doc = rules.documents()[0]
        self.add_result(doc)
        (self.corpus / "word-rules" / f"{doc.name}.docx").unlink()
        self.assertIn("tests/corpus/word-rules/ has no such document", self.refused())

    def test_refuses_outputs_of_another_document(self):
        for key, value in (("pdf_file", "E-line-exact.pdf"),
                           ("resaved_file", "E-line-exact.resaved.docx")):
            with self.subTest(key=key):
                self.add_result(rules.documents()[0], **{key: value})
                self.assertIn(f"{key} must be 'E-line-fonts", self.refused())

    def test_refuses_a_compatibility_mode_word_did_not_keep(self):
        mode_12 = next(d for d in rules.documents() if d.name.endswith("-12"))
        self.add_result(mode_12)
        self.assertIn("Word reported compatibility mode 15, the input asks for 12", self.refused())

    def test_refuses_a_resaved_copy_in_another_mode(self):
        record = self.add_result(rules.documents()[0])
        self.change_resaved(record, {
            "word/settings.xml": [(b'w:name="compatibilityMode" w:uri="http://schemas.'
                                   b'microsoft.com/office/word" w:val="15"',
                                   b'w:name="compatibilityMode" w:uri="http://schemas.'
                                   b'microsoft.com/office/word" w:val="14"')],
        })
        self.assertIn("the re-saved copy has compatibility mode 14", self.refused())

    def test_refuses_a_resaved_copy_it_cannot_read(self):
        for part, old, new in (
            ("word/settings.xml", b'w:val="15"', b'w:val="fifteen"'),
            ("word/document.xml", b"</w:body>", b"</w:bod>"),
        ):
            with self.subTest(part=part):
                record = self.add_result(rules.documents()[0])
                self.change_resaved(record, {part: [(old, new)]})
                self.assertIn("cannot read a package", self.refused())

    def test_refuses_languages_and_number_formats_word_added(self):
        record = self.add_result(rules.documents()[0])
        self.change_resaved(record, {
            "word/settings.xml": [
                (b'<w:themeFontLang w:val="en-US"', b'<w:themeFontLang w:val="xx-YY"'),
                (b'<w:decimalSymbol w:val="."/>', b'<w:decimalSymbol w:val=","/>'),
            ],
        })
        message = self.refused()
        self.assertIn("the re-saved copy names the language xx-YY", message)
        self.assertIn("the re-saved copy has decimalSymbol ','", message)

    def test_allow_locale_accepts_them(self):
        record = self.add_result(rules.documents()[0])
        self.change_resaved(record, {
            "word/settings.xml": [
                (b'<w:listSeparator w:val=","/>', b'<w:listSeparator w:val=";"/>'),
            ],
        })
        self.assertEqual(len(self.run_import(allow_locale=True)), 2)

    def test_refuses_values_that_would_break_the_table(self):
        for key, value in (("word_build", "16.0 | x"), ("rendered", "today"),
                           ("windows_display_version", "24H2|"), ("fonts_missing", ["A|B"]),
                           ("printer", "PDF|x"), ("default_printer", "a\nb")):
            with self.subTest(key=key):
                self.add_result(rules.documents()[0], **{key: value})
                self.assertIn("E-line-fonts.json", self.refused())

    def test_refuses_an_author_in_the_pdf(self):
        self.info = "Author:          Jane Roe\nPages:           3\n"
        self.assertIn("PDF Author is 'Jane Roe'", self.refused())

    def test_refuses_a_creator_in_the_xmp_metadata(self):
        self.xmp = (
            "<x:xmpmeta><rdf:RDF><rdf:Description><dc:creator><rdf:Seq>"
            "<rdf:li>Jane Roe</rdf:li></rdf:Seq></dc:creator>"
            "</rdf:Description></rdf:RDF></x:xmpmeta>"
        )
        self.assertIn("PDF XMP dc:creator is 'Jane Roe'", self.refused())

    def test_refuses_a_page_count_word_did_not_report(self):
        self.info = "Author:          \nPages:           4\n"
        self.assertIn("Word counted 3 pages, the PDF has 4", self.refused())

    def test_refuses_another_format_version(self):
        self.add_result(rules.documents()[0], script_version=1)
        self.assertIn("format 1", self.refused())

    def test_notes_another_printer_without_its_name(self):
        self.add_result(rules.documents()[0], printer="\\\\print01\\Floor 3")
        self.run_import()
        sources = (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        self.assertIn("printer other than Microsoft Print to PDF, name not recorded;", sources)
        self.assertNotIn("print01", sources)

    def test_refuses_a_path_instead_of_a_file_name(self):
        for path in ("..\\elsewhere.docx", "../elsewhere.docx", "sub/E-line-fonts.docx", ".docx"):
            with self.subTest(path=path):
                self.add_result(rules.documents()[0], file=path)
                self.assertIn("file must be the name of a .docx file", self.refused())


class Pdffonts(unittest.TestCase):
    def test_lists_fonts_that_are_not_embedded_without_subset_tags(self):
        self.assertEqual(importer.not_embedded(PDFFONTS), ["Garamond"])

    def test_a_name_longer_than_its_column_is_read(self):
        name = "SomeFoundryExtraLongFontFamilyName-SemiBoldItalic"
        row = f"{name} TrueType          WinAnsi          no  no  no      14  0"
        self.assertEqual(importer.not_embedded(PDFFONTS + row + "\n"), sorted(["Garamond", name]))


    def test_emb_is_told_from_sub_and_uni(self):
        columns = "TrueType          WinAnsi          "
        rows = (
            "NotEmbedded" + " " * 26 + columns + "no  no  yes     21  0\n"
            "EmbeddedWhole" + " " * 24 + columns + "yes no  no      22  0\n"
        )
        self.assertEqual(importer.not_embedded(PDFFONTS + rows), ["Garamond", "NotEmbedded"])

    def test_a_font_that_is_not_an_object_of_its_own_is_read(self):
        row = "Direct" + " " * 31 + "TrueType          WinAnsi          no  no  no  [none]"
        self.assertEqual(importer.not_embedded(PDFFONTS + row + "\n"), ["Direct", "Garamond"])

    def test_a_name_with_spaces_is_read_whole(self):
        row = "Segoe UI" + " " * 29 + "TrueType          WinAnsi          no  no  no      15  0"
        self.assertEqual(importer.not_embedded(PDFFONTS + row + "\n"), ["Garamond", "Segoe UI"])

    def test_a_row_it_cannot_read_is_refused(self):
        with self.assertRaises(importer.Refused):
            importer.not_embedded(PDFFONTS + "Odd                Mystery  WinAnsi no no no 16 0\n")

    def test_a_type_with_spaces_is_read(self):
        row = "Helvetica" + " " * 28 + "Type 1            WinAnsi          no  no  no       3  0"
        self.assertEqual(importer.not_embedded(PDFFONTS + row + "\n"), ["Garamond", "Helvetica"])


def word_export(parts, runs=True, texts=None):
    """make-reference.ps1's formatting export for a package, as Word would write it: the
    body paragraphs with their texts, a built-in style, and a header story."""
    body = importer.body_paragraph_styles(parts)
    font = {"name": "Calibri", "size": 11, "bold": False, "italic": False, "color": "auto"}
    bold = dict(font, bold=True)
    paragraphs = []
    for index, (text, _, _) in enumerate(body):
        paragraph = {
            "index": index, "text": (texts[index] if texts else text)[:60], "style": None,
            "style_builtin": True, "left_indent": 0, "right_indent": 0,
            "first_line_indent": 0, "space_before": 0, "space_after": 0,
            "space_before_auto": False, "space_after_auto": False, "line_spacing": 12,
            "line_spacing_rule": "single", "alignment": "left", "list_string": "",
            "list_level": None, "font": font,
        }
        if runs:
            paragraph["runs"] = [{"length": 3, "font": bold}, {"length": 4, "font": font}]
        paragraphs.append(paragraph)
    header = dict(paragraphs[0], text="Header", style="Letterhead", style_builtin=False,
                  list_string="1.", list_level=1, alignment="other-6")
    return {
        "format": 1, "complete": True, "runs": runs, "runs_note": "",
        "paragraph_seconds": 0.4, "run_seconds": 0.3,
        "stories": [
            {"story": "main", "instance": 0, "paragraphs": paragraphs},
            # PowerShell 5.1 writes a one-element array as its element.
            {"story": "primary-header", "instance": 0, "paragraphs": header},
        ],
    }


class Formatting(Import):
    def add_v3(self, export=None, **changes):
        doc = rules.documents()[0]
        record = self.add_result(doc)
        parts = importer.read_parts(self.results / record["resaved_file"])
        export = export if export is not None else word_export(parts)
        path = self.results / f"{doc.name}.formatting.json"
        path.write_text(json.dumps(export), encoding="utf-8")
        record.update({
            "script_version": 3,
            "formatting_file": path.name,
            "formatting_sha256": sha256(path.read_bytes()),
            "formatting_paragraphs": sum(
                len(s["paragraphs"]) if isinstance(s["paragraphs"], list) else 1
                for s in export["stories"]
            ),
            "formatting_runs": export["runs"],
            "formatting_complete": export["complete"],
        })
        record.update(changes)
        self.save_record(record)
        return doc, parts

    def committed(self, doc):
        folder = self.corpus / "reference" / "word-rules" / doc.name
        return json.loads((folder / "formatting.json").read_text(encoding="utf-8"))

    def test_the_export_is_committed_compact_with_styles_from_the_resaved_copy(self):
        doc, parts = self.add_v3()
        self.run_import()
        formatting = self.committed(doc)
        body = importer.body_paragraph_styles(parts)
        self.assertEqual(formatting["format"], 1)
        self.assertEqual(formatting["document"], f"word-rules/{doc.name}.docx")
        self.assertTrue(formatting["styles_from_resaved_copy"])
        main, header = formatting["stories"]
        self.assertEqual(len(main["paragraphs"]), len(body))
        first = main["paragraphs"][0]
        self.assertEqual((first["style_id"], first["style_name"]), body[0][1:])
        self.assertNotIn("style", first)
        # Fonts and paragraph formats are tables; runs are [length, font] pairs.
        self.assertEqual(len(formatting["fonts"]), 2)
        self.assertEqual(len(formatting["paragraph_formats"]), 2)
        self.assertEqual(formatting["fonts"][first["font"]]["name"], "Calibri")
        self.assertEqual([length for length, _ in first["runs"]], [3, 4])
        self.assertTrue(formatting["fonts"][first["runs"][0][1]]["bold"])
        self.assertEqual(header["paragraphs"][0]["style"], "Letterhead")
        self.assertEqual(header["paragraphs"][0]["list_string"], "1.")
        self.assertNotIn("style_id", header["paragraphs"][0])
        sources = (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        count = len(body) + 1
        self.assertIn(f"formatting as Word computes it: {count} paragraphs, with runs", sources)

    def test_a_style_name_may_have_253_characters(self):
        doc = rules.documents()[0]
        record = self.add_result(doc)
        export = word_export(importer.read_parts(self.results / record["resaved_file"]))
        export["stories"][0]["paragraphs"][0]["style"] = "S" * 253
        self.add_v3(export)
        self.run_import()
        self.assertEqual(self.committed(doc)["stories"][0]["paragraphs"][0]["style"], "S" * 253)

    def test_misaligned_paragraphs_get_no_style_ids(self):
        doc = rules.documents()[0]
        record = self.add_result(doc)
        parts = importer.read_parts(self.results / record["resaved_file"])
        texts = ["Something else entirely"] * len(importer.body_paragraph_styles(parts))
        self.add_v3(word_export(parts, texts=texts))
        self.run_import()
        formatting = self.committed(doc)
        self.assertFalse(formatting["styles_from_resaved_copy"])
        self.assertNotIn("style_id", formatting["stories"][0]["paragraphs"][0])

    def test_refusals(self):
        first = lambda export: export["stories"][0]["paragraphs"][0]
        cases = {
            "changed after Word wrote it": (None, {"formatting_sha256": "0" * 64}),
            "does not match its record": (None, {"formatting_paragraphs": 1}),
            "unexpected alignment": (lambda e: first(e).update(alignment="sideways"), {}),
            "unexpected colour": (lambda e: first(e)["font"].update(color="red"), {}),
            "text is not an excerpt": (lambda e: first(e).update(text="x" * 61), {}),
            "is not a known story": (lambda e: e["stories"][0].update(story="glossary"), {}),
            "length is not a count": (lambda e: first(e)["runs"][0].update(length=0), {}),
            "names a user profile path": (
                lambda e: first(e).update(text="C:\\Users\\<name>\\x"), {}
            ),
            "unexpected runs_note": (lambda e: e.update(runs_note="left out: Ausnahme"), {}),
            "unexpected theme font": (lambda e: first(e)["font"].update(theme="+Textkoerper"), {}),
            "unexpected font name": (lambda e: first(e)["font"].update(name="+Body"), {}),
            "an unresolved theme font has a name": (
                lambda e: first(e)["font"].update(theme="unresolved"), {}
            ),
            "does not match its record (complete)": (None, {"formatting_complete": False}),
            "unexpected style name": (lambda e: first(e).update(style="S" * 254), {}),
            "unexpected runs_note (not text)": (lambda e: e.update(runs_note=["x"]), {}),
            "unexpected alignment (not text)": (lambda e: first(e).update(alignment=["left"]), {}),
            "unexpected theme font (not text)": (lambda e: first(e)["font"].update(theme={}), {}),
        }
        doc = rules.documents()[0]
        for message, (edit, changes) in cases.items():
            with self.subTest(message):
                record = self.add_result(doc)
                export = word_export(importer.read_parts(self.results / record["resaved_file"]))
                if edit:
                    edit(export)
                self.add_v3(export, **changes)
                self.assertIn(message.split(" (")[0], self.refused())

    def test_runs_are_dropped_to_stay_under_the_size_limit(self):
        doc, _ = self.add_v3()
        limit = importer.MAX_FORMATTING_BYTES
        self.addCleanup(setattr, importer, "MAX_FORMATTING_BYTES", limit)
        with_runs = importer.formatting_bytes(self.check_compact(doc))
        importer.MAX_FORMATTING_BYTES = len(with_runs) - 1
        self.run_import()
        formatting = self.committed(doc)
        self.assertFalse(formatting["runs"])
        self.assertEqual(formatting["runs_note"], "left out to keep the file under 1 MB")
        self.assertNotIn("runs", formatting["stories"][0]["paragraphs"][0])
        # The bold font only runs used goes with them.
        self.assertEqual([font["bold"] for font in formatting["fonts"]], [False])
        self.assertEqual({p["font"] for s in formatting["stories"] for p in s["paragraphs"]}, {0})
        importer.MAX_FORMATTING_BYTES = 10
        shutil.rmtree(self.corpus / "reference")
        self.assertIn("formatting.json would be", self.refused())

    def check_compact(self, doc):
        record = json.loads((self.results / f"{doc.name}.json").read_text(encoding="utf-8"))
        parts = importer.read_parts(self.results / record["resaved_file"])
        export = importer.read_formatting((self.results / record["formatting_file"]).read_bytes())
        result, _ = importer.compact_formatting(export, parts, "x", "y")
        return result

    def test_a_format_2_result_still_imports_without_formatting(self):
        self.run_import()
        folder = self.corpus / "reference" / "word-rules" / rules.documents()[0].name
        self.assertFalse((folder / "formatting.json").exists())


class BodyParagraphs(unittest.TestCase):
    def test_text_boxes_are_not_body_paragraphs(self):
        w = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
        mc = 'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"'
        box = "<w:txbxContent><w:p><w:r><w:t>In a box</w:t></w:r></w:p></w:txbxContent>"
        document = (
            f"<w:document {w} {mc}><w:body>"
            '<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>'
            f"<w:p><w:r><mc:AlternateContent><mc:Choice>{box}</mc:Choice>"
            f"<mc:Fallback>{box}</mc:Fallback></mc:AlternateContent></w:r>"
            "<w:r><w:t>After the box</w:t></w:r></w:p>"
            f"<w:p><w:r><w:pict>{box}</w:pict></w:r></w:p>"
            "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"
            "</w:body></w:document>"
        )
        styles = (
            f"<w:styles {w}>"
            '<w:style w:type="paragraph" w:default="1" w:styleId="Normal">'
            '<w:name w:val="Normal"/></w:style>'
            '<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style>'
            "</w:styles>"
        )
        found = importer.body_paragraph_styles(
            {"word/document.xml": document.encode(), "word/styles.xml": styles.encode()}
        )
        self.assertEqual(found, [
            ("Title", "Heading1", "heading 1"),
            ("After the box", "Normal", "Normal"),
            ("", "Normal", "Normal"),
            ("Cell", "Normal", "Normal"),
            ("", None, None),
        ])

    def test_table_row_ends_line_up_with_words_paragraphs(self):
        w = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'

        def cell(text):
            return f"<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>"

        document = (
            f"<w:document {w}><w:body><w:p><w:r><w:t>Before</w:t></w:r></w:p><w:tbl>"
            f"<w:tr>{cell('A')}{cell('B')}</w:tr><w:tr>{cell('C')}{cell('D')}</w:tr></w:tbl>"
            "<w:p><w:r><w:t>After</w:t></w:r></w:p></w:body></w:document>"
        )
        parts = {"word/document.xml": document.encode()}

        def export(texts):
            font = {"name": "Calibri", "size": 11, "bold": False, "italic": False,
                    "color": "auto", "theme": "minorHAnsi"}
            paragraphs = [
                {"index": i, "text": text, "style": None, "style_builtin": True,
                 "left_indent": 0, "right_indent": 0, "first_line_indent": 0,
                 "space_before": 0, "space_after": 0, "space_before_auto": False,
                 "space_after_auto": False, "line_spacing": 12, "line_spacing_rule": "single",
                 "alignment": "left", "list_string": "", "list_level": None, "font": font}
                for i, text in enumerate(texts)
            ]
            data = {"format": 1, "complete": True, "runs": False, "runs_note": "",
                    "stories": [{"story": "main", "instance": 0, "paragraphs": paragraphs}]}
            return importer.read_formatting(json.dumps(data).encode())

        # Word's main story of this document: each row ends with a paragraph of its own.
        words = ["Before", "A", "B", "", "C", "D", "", "After"]
        result, note = importer.compact_formatting(export(words), parts, "x", "y")
        self.assertTrue(result["styles_from_resaved_copy"])
        paragraphs = result["stories"][0]["paragraphs"]
        self.assertNotIn("style_id", paragraphs[3])
        self.assertEqual(paragraphs[4]["style_id"], "")
        self.assertNotIn("not matched", note)
        result, note = importer.compact_formatting(
            export(["Before", "A", "B", "C", "D", "After"]), parts, "x", "y"
        )
        self.assertFalse(result["styles_from_resaved_copy"])
        self.assertIn("body styles not matched to the re-saved copy", note)


class FontFiles(unittest.TestCase):
    def setUp(self):
        self.temp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.temp)

    def test_an_unreadable_face_is_refused_when_it_is_needed(self):
        good = collection(("Other", "Other", "1"), ("Garamond", "Garamond", "2"))
        # The name table is fine, but the face's glyf record (the first of glyf, head and
        # name) points past the end of the file.
        (second,) = struct.unpack(">I", good[16:20])
        record = second + 12
        broken = bytearray(good)
        broken[record + 8 : record + 12] = struct.pack(">I", len(good) + 100)
        (self.temp / "broken.ttc").write_bytes(bytes(broken))
        index = importer.font_index([self.temp])
        self.assertIn("Garamond", index)
        with self.assertRaises(importer.Refused) as caught:
            importer.drawn_font("Garamond", index)
        self.assertIn("Garamond: the font file cannot be read: broken.ttc", str(caught.exception))

    def test_a_folder_that_cannot_be_listed_is_refused(self):
        with mock.patch.object(Path, "iterdir", side_effect=PermissionError("denied")):
            with self.assertRaises(importer.Refused) as caught:
                importer.font_index([self.temp])
        self.assertIn("cannot be listed", str(caught.exception))

    def test_files_that_are_not_fonts_are_skipped(self):
        (self.temp / "broken.ttf").write_bytes(b"\x00\x01\x00\x00" + b"\xff" * 3)
        (self.temp / "empty.otf").write_bytes(b"")
        (self.temp / "notes.txt").write_bytes(sfnt("Hidden"))
        (self.temp / "font.ttf").write_bytes(sfnt("Found"))
        self.assertEqual(sorted(importer.font_index([self.temp])), ["Found"])

    def test_a_missing_folder_is_refused(self):
        with self.assertRaises(importer.Refused):
            importer.font_index([self.temp / "missing"])

    def test_us_english_windows_names_come_first(self):
        mac = "Mac Name".encode("mac_roman")
        german = "Deutscher Name".encode("utf-16-be")
        english = "English Name".encode("utf-16-be")
        records = [(1, 0, 0, 4, mac), (3, 1, 0x407, 4, german), (3, 1, 0x409, 4, english),
                   (1, 0, 0, 6, b"OnlyMac")]
        storage, packed = b"", b""
        for platform, encoding, language, name_id, data in records:
            packed += struct.pack(">HHHHHH", platform, encoding, language, name_id, len(data),
                                  len(storage))
            storage += data
        table = struct.pack(">HHH", 0, len(records), 6 + len(packed)) + packed + storage
        self.assertEqual(importer.name_strings(table), {4: "English Name", 6: "OnlyMac"})

    def test_the_cut_out_face_keeps_its_tables(self):
        data = collection(("First", "First", "1"), ("Second", "Second Font", "2"))
        (self.temp / "pair.ttc").write_bytes(data)
        faces = importer.font_faces(self.temp / "pair.ttc")
        self.assertEqual([(f.index, f.postscript) for f in faces], [(0, "First"), (1, "Second")])
        cut = importer.face_bytes(faces[1])
        (self.temp / "cut.ttf").write_bytes(cut)
        read = lambda offset, size: cut[offset : offset + size]
        tables = importer.table_directory(read, 0)
        (second,) = struct.unpack(">I", data[16:20])
        original = importer.table_directory(lambda o, n: data[o : o + n], second)
        self.assertEqual(sorted(tables), sorted(original))
        for tag, (offset, length) in tables.items():
            start, size = original[tag]
            if tag != b"head":
                self.assertEqual(cut[offset : offset + length], data[start : start + size])
            self.assertEqual(offset % 4, 0)
        # Each table's checksum and head's checkSumAdjustment are right for the new file.
        (count,) = struct.unpack(">H", cut[4:6])
        for record in range(12, 12 + 16 * count, 16):
            tag, checksum, offset, length = struct.unpack(">4sIII", cut[record : record + 16])
            table = bytearray(cut[offset : offset + length])
            if tag == b"head":
                table[8:12] = bytes(4)
            self.assertEqual(checksum, importer.checksum32(table), tag)
        self.assertEqual(importer.checksum32(cut), 0xB1B0AFBA)


class PageOrder(unittest.TestCase):
    def test_pages_are_sorted_by_number(self):
        names = [Path(f"page-{n}.pgm") for n in (10, 9, 1, 2)]
        self.assertEqual(
            [p.name for p in importer.in_page_order(names)],
            ["page-1.pgm", "page-2.pgm", "page-9.pgm", "page-10.pgm"],
        )

    def test_no_fonts(self):
        self.assertEqual(importer.not_embedded(PDFFONTS.split("-")[0]), [])


if __name__ == "__main__":
    unittest.main()
