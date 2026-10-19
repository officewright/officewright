"""Checks scripts/corpus-import.py with fake make-reference.ps1 results.

The poppler tools are replaced by fixed output, so the checks run where poppler is not
installed (Windows) and do not depend on its version.
"""

import dataclasses
import hashlib
import importlib.util
import json
import shutil
import struct
import sys
import tempfile
import unittest
import unittest.mock
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


def load_checks():
    """test_corpus_references, the checks of committed references, as a module of its own, so
    that a test can point them at the corpus an import wrote."""
    path = Path(__file__).resolve().parent / "test_corpus_references.py"
    spec = importlib.util.spec_from_file_location("reference_checks", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

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
            "dropped from the re-saved copy: none; style and numbering properties of the "
            "re-saved copy: 0 dropped, 0 added, 0 changed (listed in geometry.json); paragraph "
            "styles of the re-saved copy changed: 0 (listed in geometry.json); fields as saved; "
            "PDF not "
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

    def test_records_style_numbering_and_paragraph_style_changes(self):
        # #297: what Word drops, adds and changes in the styles and the numbering of the copy
        # it re-saves, and the paragraph styles it writes, go into geometry.json, the
        # SOURCES.md row and the import's summary.
        doc = next(d for d in rules.documents() if d.name == "E-list-label-format")
        record = self.add_result(doc)
        self.change_resaved(record, {
            "word/styles.xml": [
                # The name is the user interface's: losing it changes nothing. The size is new.
                (b'<w:style w:type="paragraph" w:customStyle="1" w:styleId="BoldStyle">'
                 b'<w:name w:val="BoldStyle"/><w:basedOn w:val="Normal"/><w:rPr><w:b/><w:bCs/>'
                 b'</w:rPr></w:style>',
                 b'<w:style w:type="paragraph" w:customStyle="1" w:styleId="BoldStyle">'
                 b'<w:basedOn w:val="Normal"/><w:rPr><w:b/><w:bCs/><w:sz w:val="24"/>'
                 b'</w:rPr></w:style>'),
                (b'<w:style w:type="paragraph" w:default="1" w:styleId="Normal">',
                 b'<w:style w:type="paragraph" w:styleId="Normal">'),
            ],
            "word/numbering.xml": [
                (b'<w:pStyle w:val="LabelParaOneWay"/>', b""),
                # Word numbers definitions and instances again; the nsid stays.
                (b'w:abstractNumId="0"><w:nsid w:val="5C3A0101"/>',
                 b'w:abstractNumId="9"><w:nsid w:val="5C3A0101"/>'),
                (b'<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>',
                 b'<w:num w:numId="11"><w:abstractNumId w:val="9"/></w:num>'),
                (b'w:abstractNumId="1"><w:nsid w:val="5C3A0102"/><w:multiLevelType '
                 b'w:val="singleLevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt '
                 b'w:val="decimal"/><w:lvlText w:val="%1."/>',
                 b'w:abstractNumId="1"><w:nsid w:val="5C3A0102"/><w:multiLevelType '
                 b'w:val="singleLevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt '
                 b'w:val="decimal"/><w:lvlText w:val="%1)"/>'),
            ],
            "word/document.xml": [
                (b'<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/>',
                 b'<w:p><w:pPr><w:pStyle w:val="BoldStyle"/><w:numPr><w:ilvl w:val="0"/>'
                 b'<w:numId w:val="1"/>'),
            ],
        })
        checked = self.run_import()
        folder = self.corpus / "reference" / "word-rules" / doc.name
        geometry = json.loads((folder / "geometry.json").read_text(encoding="utf-8"))
        self.assertEqual(geometry["styles_changed"],
                         ["+ BoldStyle: rPr/sz val=24", "- Normal: default"])
        self.assertEqual(geometry["numbering_changed"], [
            "~ abstractNum 1 level 0: lvlText val=%1. -> %1)",
            "- abstractNum 7 level 0: pStyle val=LabelParaOneWay",
        ])
        self.assertEqual(geometry["paragraph_styles_changed"], ["paragraph 1: none -> BoldStyle"])
        sources = (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        self.assertIn(
            "; style and numbering properties of the re-saved copy: 2 dropped, 1 added, 1 changed "
            "(listed in geometry.json); paragraph styles of the re-saved copy changed: 1 (listed "
            "in geometry.json); ",
            sources,
        )
        lines = importer.summary(checked, "imported")
        for line in (
            "  note: styles of the re-saved copy: - Normal: default",
            "  note: numbering of the re-saved copy: ~ abstractNum 1 level 0: lvlText val=%1. -> %1)",
            "  note: Word changed the paragraph style of paragraph 1: none -> BoldStyle",
        ):
            self.assertIn(line, lines)

    def test_the_checks_of_committed_references_read_what_the_import_writes(self):
        # A row or geometry.json the importer writes must pass test_corpus_references, or
        # the next import fails check.sh. A document with changed styles and an extra
        # paragraph in the copy gives every note its numbers. Two checks need what this
        # harness does not make: rasters of the page's size, and the business documents.
        doc = next(d for d in rules.documents() if d.name == "E-list-label-format")
        record = self.add_result(doc)
        self.change_resaved(record, {
            "word/styles.xml": [(b'<w:style w:type="paragraph" w:default="1" w:styleId="Normal">',
                                 b'<w:style w:type="paragraph" w:styleId="Normal">')],
            "word/document.xml": [(b"</w:body>", b"<w:p/></w:body>")],
        })
        self.run_import()
        checks = load_checks()
        unfit = {"test_each_folder_holds_its_document_pages_and_geometry",
                 "test_every_business_document_has_a_reference_or_says_why_not"}
        names = [name for name in unittest.TestLoader().getTestCaseNames(checks.References)
                 if name not in unfit]
        self.assertGreater(len(names), 10)
        result = unittest.TestResult()
        with unittest.mock.patch.object(checks, "CORPUS", self.corpus), \
                unittest.mock.patch.object(checks, "REFERENCE", self.corpus / "reference"):
            unittest.TestSuite(checks.References(name) for name in names).run(result)
        self.assertEqual([f"{test}: {trace}" for test, trace in result.failures + result.errors],
                         [])
        geometry = json.loads((self.corpus / "reference" / "word-rules" / doc.name /
                               "geometry.json").read_text(encoding="utf-8"))
        self.assertEqual(geometry["styles_changed"], ["- Normal: default"])
        self.assertEqual(geometry["paragraph_styles_changed"][0][:22], "the re-saved copy has ")

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
                (b'<w:decimalSymbol w:val="."/>', b'<w:decimalSymbol w:val=","/>'),
            ],
        })
        self.assertEqual(len(self.run_import(allow_locale=True)), 2)
        # SOURCES.md names what was accepted, and only for that document.
        rows = [
            line for line in (self.corpus / "SOURCES.md").read_text(encoding="utf-8").splitlines()
            if "accepted with --allow-locale" in line
        ]
        self.assertEqual(len(rows), 1)
        self.assertIn(
            f"| reference/word-rules/{rules.documents()[0].name}/ ", rows[0]
        )
        self.assertIn(
            "accepted with --allow-locale: the re-saved copy has decimalSymbol ',', "
            "the re-saved copy has listSeparator ';'",
            rows[0],
        )

    def test_a_locale_trace_that_would_break_the_table_is_refused(self):
        record = self.add_result(rules.documents()[0])
        self.change_resaved(record, {
            "word/settings.xml": [
                (b'<w:listSeparator w:val=","/>', b'<w:listSeparator w:val="|"/>'),
            ],
        })
        self.assertIn("which would break the SOURCES.md table", self.refused(allow_locale=True))

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

    def use_helvetica(self, arial=True):
        """A PDF that names Helvetica and Helvetica,Bold without embedding them."""
        columns = "TrueType          WinAnsi          no  no  no "
        listing = PDFFONTS.replace(
            f"{'Garamond':<37}{columns}     12  0\n",
            f"{'Helvetica':<37}{columns}     12  0\n{'Helvetica,Bold':<37}{columns}     14  0\n",
        )
        self.assertNotEqual(listing, PDFFONTS)
        self.pdffonts = listing
        self.copy_fonts = listing.replace("no  no  no ", "yes no  no ")
        objects = {"qpdf": [
            {"jsonversion": 2, "maxobjectid": 14},
            {
                "obj:12 0 R": {"value": {"/Type": "/FontDescriptor", "/FontName": "/Helvetica"}},
                "obj:14 0 R": {"value": {"/Type": "/FontDescriptor",
                                         "/FontName": "/Helvetica,Bold"}},
            },
        ]}
        self.tools = dataclasses.replace(self.tools, objects=lambda path: json.dumps(objects))
        self.arial = sfnt("ArialMT", "Arial", "Version 7.06")
        self.arial_bold = sfnt("Arial-BoldMT", "Arial Bold", "Version 7.06")
        if arial:
            (self.font_dir / "arial.ttf").write_bytes(self.arial)
            (self.font_dir / "arialbd.ttf").write_bytes(self.arial_bold)

    def test_helvetica_is_drawn_with_arial_as_windows_substitutes_it(self):
        self.use_helvetica()
        self.run_import()
        embedded = {}
        for _, update, data in self.updates:
            for key, value in update["qpdf"][1].items():
                if "value" in value:
                    name = value["value"]["/FontName"]
                    stream = value["value"]["/FontFile2"].replace(" 0 R", "")
                    embedded[name] = data[f"obj:{stream} 0 R"]
        self.assertEqual(embedded["/Helvetica"], self.arial)
        self.assertEqual(embedded["/Helvetica,Bold"], self.arial_bold)
        doc = rules.documents()[0]
        geometry = json.loads(
            (self.corpus / "reference" / "word-rules" / doc.name / "geometry.json")
            .read_text(encoding="utf-8")
        )
        drawn = {font["font"]: font for font in geometry["fonts_rasterized_with"]}
        self.assertEqual(
            (drawn["Helvetica"]["windows_substitute"], drawn["Helvetica"]["full_name"]),
            ("ArialMT", "Arial"),
        )
        self.assertEqual(drawn["Helvetica,Bold"]["windows_substitute"], "Arial-BoldMT")
        sources = (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        self.assertIn(
            "Helvetica,Bold (drawn with Arial-BoldMT as Windows substitutes it, Arial Bold, "
            f"Version 7.06, SHA-256 {sha256(self.arial_bold)})",
            sources,
        )

    def test_helvetica_without_arial_is_refused(self):
        self.use_helvetica(arial=False)
        self.assertIn(
            "Helvetica is not embedded in the PDF, and no --font-dir folder has a font with the "
            "PostScript name ArialMT, which Windows draws it with",
            self.refused(),
        )

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


class Changes(unittest.TestCase):
    """What Word changed in the styles, the numbering and the paragraph styles of the copy it
    re-saved: the functions alone, on packages of a part or two."""

    def test_style_changes_name_each_attribute_word_dropped_added_or_changed(self):
        def styles(body, defaults=""):
            return {"word/styles.xml": (
                f'<w:styles xmlns:w="{importer.W[1:-1]}" xmlns:x="urn:x">{defaults}{body}'
                "</w:styles>").encode()}

        def style(style_id, inner, default=""):
            return (f'<w:style w:type="paragraph"{default} w:styleId="{style_id}">'
                    f"{inner}</w:style>")

        before = styles(
            style("Title", '<w:name w:val="Title"/><w:uiPriority w:val="10"/><w:rsid w:val="1"/>'
                  '<w:pPr><w:spacing w:after="120" w:before="0"/><w:ind w:leftChars="200"/>'
                  '<w:tabs><w:tab w:val="left" w:pos="720"/><w:tab w:val="right" w:pos="9000"/>'
                  '</w:tabs></w:pPr><w:rPr><w:rFonts w:ascii="A" w:hAnsi="A" x:y="1"/>'
                  '<w:sz w:val="22"/></w:rPr>'
                  '<w:tblStylePr w:type="firstRow"><w:rPr><w:b/></w:rPr></w:tblStylePr>'
                  '<w:tblStylePr w:type="lastRow"><w:rPr><w:b/></w:rPr></w:tblStylePr>',
                  ' w:default="1"'),
            '<w:docDefaults><w:pPrDefault><w:pPr><w:spacing w:after="0"/></w:pPr>'
            "</w:pPrDefault></w:docDefaults>",
        )
        after = styles(
            # The interface's children and other namespaces' attributes go; the attributes
            # come in another order; "true" says default as "1" does.
            style("Title", '<w:pPr><w:spacing w:after="120"/><w:ind w:left="200" '
                  'w:leftChars="200"/><w:tabs><w:tab w:val="right" w:pos="9000"/></w:tabs>'
                  '</w:pPr><w:rPr><w:rFonts w:hAnsi="A" w:ascii="A"/><w:sz w:val="24"/></w:rPr>'
                  '<w:tblStylePr w:type="firstRow"><w:rPr><w:b/></w:rPr><w:tblPr/></w:tblStylePr>'
                  '<w:tblStylePr w:type="lastRow"/>', ' w:default="true"')
            # A style Word adds whole is not one the document used.
            + style("Added", '<w:rPr><w:i/></w:rPr>'),
            # Defaults every document has: what Word fills in there counts.
            '<w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault>'
            "<w:pPrDefault/></w:docDefaults>",
        )
        self.assertEqual(importer.style_changes(before, after), [
            "+ Title: pPr/ind left=200",
            "- Title: pPr/spacing before=0",
            "- Title: pPr/tabs/tab[pos=720] val=left",
            "~ Title: rPr/sz val=22 -> 24",
            "- Title: tblStylePr[type=lastRow]/rPr/b",
            "- docDefaults: pPrDefault/pPr/spacing after=0",
            "+ docDefaults: rPrDefault/rPr/sz val=22",
        ])
        self.assertEqual(importer.style_changes(before, before), [])
        self.assertEqual(importer.style_changes({}, {}), [])
        # Elements that repeat with nothing to tell them apart are numbered, not folded.
        twice = styles(style("T", '<w:pPr><w:x w:val="1"/><w:x w:val="2"/></w:pPr>'))
        once = styles(style("T", '<w:pPr><w:x w:val="1"/></w:pPr>'))
        self.assertEqual(importer.style_changes(twice, once),
                         ["+ T: pPr/x val=1", "- T: pPr/x#1 val=1", "- T: pPr/x#2 val=2"])
        without_default = styles(style("Title", "", ""))
        self.assertIn("- Title: default",
                      importer.style_changes(styles(style("Title", "", ' w:default="on"')),
                                             without_default))

    def test_numbering_changes_follow_definitions_word_numbers_again(self):
        def numbering(*items):
            return {"word/numbering.xml": (
                f'<w:numbering xmlns:w="{importer.W[1:-1]}">{"".join(items)}</w:numbering>'
            ).encode()}

        def definition(number, nsid, levels, extra="", template="ABCD"):
            body = "".join(
                f'<w:lvl w:ilvl="{i}" w:tplc="0409000{i}"><w:start w:val="1"/>{inner}</w:lvl>'
                for i, inner in levels
            )
            return (f'<w:abstractNum w:abstractNumId="{number}"><w:nsid w:val="{nsid}"/>'
                    f'<w:tmpl w:val="{template}"/>{extra}{body}</w:abstractNum>')

        def instance(number, uses, overrides=""):
            return (f'<w:num w:numId="{number}"><w:abstractNumId w:val="{uses}"/>{overrides}'
                    "</w:num>")

        def override_level(code):
            return (f'<w:lvlOverride w:ilvl="2"><w:lvl w:ilvl="2" w:tplc="{code}">'
                    '<w:start w:val="3"/></w:lvl></w:lvlOverride>')

        restart = ('<w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride>'
                   '<w:lvlOverride w:ilvl="1"><w:startOverride w:val="1"/></w:lvlOverride>'
                   + override_level("1"))
        before = numbering(
            definition(1, "00AA", [(0, '<w:lvlText w:val="%1."/>')],
                       '<w:multiLevelType w:val="hybridMultilevel"/>'),
            definition(4, "00BB", [(0, '<w:pStyle w:val="P"/>'), (1, "")]),
            instance(6, 1, restart), instance(2, 4),
        )
        # Word numbers the definitions from 0 and the instance 6 as 5, fills in levels the
        # input did not give, drops one override and a level's pStyle, changes the type, and
        # writes another template and level template codes, which only its interface reads.
        after = numbering(
            definition(0, "00AA", [(0, '<w:lvlText w:val="%1."/>'),
                                   (1, '<w:lvlText w:val="%2."/>')],
                       '<w:multiLevelType w:val="multilevel"/>', "1234"),
            definition(1, "00BB", [(0, ""), (1, '<w:lvlJc w:val="left"/>')]),
            instance(5, 0, '<w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/>'
                     "</w:lvlOverride>" + override_level("2")),
            instance(2, 1),
        )
        self.assertEqual(importer.numbering_changes(before, after), [
            "~ abstractNum 1: multiLevelType val=hybridMultilevel -> multilevel",
            "- abstractNum 4 level 0: pStyle val=P",
            "+ abstractNum 4 level 1: lvlJc val=left",
            "- num 6: lvlOverride[ilvl=1]/startOverride val=1",
        ])
        self.assertEqual(importer.numbering_changes(before, before), [])
        self.assertEqual(importer.numbering_changes({}, after), [])
        # A list instance a style names is mapped too, so its renumbering is no change.
        styles_before = {"word/styles.xml": (
            f'<w:styles xmlns:w="{importer.W[1:-1]}"><w:style w:type="paragraph" w:styleId="L">'
            '<w:pPr><w:numPr><w:numId w:val="6"/></w:numPr></w:pPr></w:style></w:styles>'
        ).encode()}
        styles_after = {"word/styles.xml": styles_before["word/styles.xml"].replace(
            b'w:val="6"', b'w:val="5"')}
        self.assertEqual(importer.style_changes({**before, **styles_before},
                                                {**after, **styles_after}), [])
        # An id no instance of the copy has names nothing, and stays as written.
        self.assertEqual(importer.style_changes({**before, **styles_before},
                                                {**before, **styles_after}),
                         ["~ L: pPr/numPr/numId val=6 -> 5"])
        self.assertEqual(importer.style_changes({**before, **styles_after},
                                                {**before, **styles_after}), [])

    def test_paragraph_style_changes_compare_paragraphs_in_order(self):
        def document(*paragraphs, wrap=lambda body: body):
            body = "".join(
                "<w:p>" + (f'<w:pPr><w:pStyle w:val="{s}"/></w:pPr>' if s else "")
                + (f"<w:r><w:t>{t}</w:t></w:r>" if t else "") + "</w:p>"
                for s, t in (p if isinstance(p, tuple) else (p, "") for p in paragraphs)
            )
            xml = (f'<w:document xmlns:w="{importer.W[1:-1]}" xmlns:mc="http://schemas.'
                   'openxmlformats.org/markup-compatibility/2006"><w:body>'
                   f"{wrap(body)}</w:body></w:document>")
            return {"word/document.xml": xml.encode()}

        self.assertEqual(
            importer.paragraph_style_changes(document(None, "A", "B"), document("D", "A", None)),
            ["paragraph 1: none -> D", "paragraph 3: B -> none"],
        )
        self.assertEqual(importer.paragraph_style_changes(document("A"), document("A")), [])
        # Paragraphs in tables and content controls are the body's.
        for wrap in (lambda body: f"<w:tbl><w:tr><w:tc>{body}</w:tc></w:tr></w:tbl>",
                     lambda body: f"<w:sdt><w:sdtContent>{body}</w:sdtContent></w:sdt>"):
            self.assertEqual(
                importer.paragraph_style_changes(document("A", wrap=wrap),
                                                 document("B", wrap=wrap)),
                ["paragraph 1: A -> B"],
            )
        # A text box is a story of its own, and alternative content is not the main story.
        boxed = document("A")
        boxed["word/document.xml"] = boxed["word/document.xml"].replace(
            b"</w:p>",
            b"<w:r><w:pict><w:txbxContent><w:p><w:pPr><w:pStyle w:val=\"Box\"/></w:pPr></w:p>"
            b"</w:txbxContent></w:pict></w:r></w:p>",
        )
        self.assertEqual(importer.paragraph_style_changes(document("A"), boxed), [])
        alternative = document("A", wrap=lambda body: body + (
            '<mc:AlternateContent><mc:Choice Requires="w14"><w:p><w:pPr><w:pStyle w:val="C"/>'
            "</w:pPr></w:p></mc:Choice></mc:AlternateContent>"))
        self.assertEqual(importer.paragraph_style_changes(document("A"), alternative), [])
        # Another number of paragraphs: said first, and the rest paired by their text.
        self.assertEqual(
            importer.paragraph_style_changes(
                document(("A", "one"), ("B", "two")),
                document((None, "new"), ("A", "one"), ("C", "two"))),
            ["the re-saved copy has 3 paragraphs, the input 2", "paragraph 2: B -> C"],
        )
        # The main part is the one _rels/.rels names, by the transitional relationship; a
        # strict one is not followed, since its part would be in the strict namespace.
        def moved(kind):
            return {"word/main.xml": document("A")["word/document.xml"], "_rels/.rels": (
                '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/'
                f'relationships"><Relationship Id="rId1" Type="{kind}/officeDocument" '
                'Target="/word/main.xml"/></Relationships>').encode()}

        transitional = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
        strict = "http://purl.oclc.org/ooxml/officeDocument/relationships"
        self.assertEqual(importer.paragraph_style_changes(moved(transitional), document("B")),
                         ["paragraph 1: A -> B"])
        self.assertEqual(importer.paragraph_style_changes(moved(strict), document("B")),
                         ["the re-saved copy has 1 paragraph, the input 0"])

    def test_styles_and_numbering_are_the_parts_the_main_part_names(self):
        rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"

        def package(bold):
            styles = (f'<w:styles xmlns:w="{importer.W[1:-1]}"><w:style w:type="paragraph" '
                      f'w:styleId="A"><w:rPr>{"<w:b/>" if bold else ""}</w:rPr></w:style>'
                      "</w:styles>")
            relationships = (
                '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/'
                'relationships">{}</Relationships>')
            return {
                "_rels/.rels": relationships.format(
                    f'<Relationship Id="rId1" Type="{rel}/officeDocument" '
                    'Target="word/main.xml"/>').encode(),
                "word/main.xml": f'<w:document xmlns:w="{importer.W[1:-1]}"/>'.encode(),
                "word/_rels/main.xml.rels": relationships.format(
                    f'<Relationship Id="rId1" Type="{rel}/styles" Target="look.xml"/>').encode(),
                "word/look.xml": styles.encode(),
                # Not the styles part, although it has the usual name.
                "word/styles.xml": styles.replace("A", "Other").encode(),
            }

        self.assertEqual(importer.style_changes(package(True), package(False)), ["- A: rPr/b"])
        # An external relationship names no part of the package: that copy has no styles.
        external = {name: data.replace(b'Target="look.xml"', b'Target="look.xml" '
                                       b'TargetMode="External"')
                    for name, data in package(True).items()}
        self.assertEqual(importer.style_changes(package(True), external), ["- A: rPr/b"])

    def test_no_list_is_no_instance(self):
        # w:numId 0 takes a style out of lists; it maps onto nothing.
        d, i = self.definition, self.instance
        numbering = self.numbering(d(0, "00AA"), i(1, 0))
        self.assertEqual(importer.style_changes({**numbering, **self.style_naming(0)},
                                                {**numbering, **self.style_naming(0)}), [])


    @staticmethod
    def numbering(*items):
        return {"word/numbering.xml": (
            f'<w:numbering xmlns:w="{importer.W[1:-1]}">{"".join(items)}</w:numbering>'
        ).encode()}

    @staticmethod
    def definition(number, nsid, text="%1.", level=""):
        nsid_xml = f'<w:nsid w:val="{nsid}"/>' if nsid else ""
        return (f'<w:abstractNum w:abstractNumId="{number}">{nsid_xml}<w:lvl w:ilvl="0"{level}>'
                f'<w:start w:val="1"/><w:lvlText w:val="{text}"/></w:lvl></w:abstractNum>')

    @staticmethod
    def instance(number, uses, start=None):
        override = ""
        if start is not None:
            override = (f'<w:lvlOverride w:ilvl="0"><w:startOverride w:val="{start}"/>'
                        "</w:lvlOverride>")
        return f'<w:num w:numId="{number}"><w:abstractNumId w:val="{uses}"/>{override}</w:num>'

    @staticmethod
    def style_naming(number):
        return {"word/styles.xml": (
            f'<w:styles xmlns:w="{importer.W[1:-1]}"><w:style w:type="paragraph" w:styleId="L">'
            f'<w:pPr><w:numPr><w:numId w:val="{number}"/></w:numPr></w:pPr></w:style></w:styles>'
        ).encode()}

    def test_instances_word_numbers_again_are_paired_in_their_order(self):
        # Word closes the gaps in the numIds and keeps their order (contract-zh: 1, 2, 3, 4, 6
        # became 1 to 5). Instances 3 and 4 of one definition, restarted at 5 and 9, become 2
        # and 3: nothing changed, although instance 3 of the input and of the copy differ.
        d, i = self.definition, self.instance
        before = self.numbering(d(0, "00AA"), i(1, 0), i(3, 0, 5), i(4, 0, 9))
        after = self.numbering(d(0, "00AA"), i(1, 0), i(2, 0, 5), i(3, 0, 9))
        self.assertEqual(importer.numbering_changes(before, after), [])
        self.assertEqual(importer.style_changes({**before, **self.style_naming(4)},
                                                {**after, **self.style_naming(3)}), [])
        # The same with three definitions, which Word numbers again too.
        before = self.numbering(d(1, "00AA"), d(2, "00BB"), d(3, "00CC"),
                                i(1, 1), i(3, 2), i(4, 3))
        after = self.numbering(d(0, "00AA"), d(1, "00BB"), d(2, "00CC"), i(1, 0), i(2, 1), i(3, 2))
        self.assertEqual(importer.numbering_changes(before, after), [])
        # Instances in another order of definitions are paired by their definitions.
        before = self.numbering(d(0, "00AA"), d(1, "00BB"), i(1, 0), i(2, 1))
        after = self.numbering(d(0, "00AA"), d(1, "00BB"), i(1, 1), i(2, 0))
        self.assertEqual(importer.numbering_changes(before, after), [])
        # A restart Word did change is still one.
        before = self.numbering(d(0, "00AA"), i(1, 0), i(3, 0, 5))
        after = self.numbering(d(0, "00AA"), i(1, 0), i(2, 0, 6))
        self.assertEqual(importer.numbering_changes(before, after),
                         ["~ num 3: lvlOverride[ilvl=0]/startOverride val=5 -> 6"])

    def test_an_instance_word_dropped_leaves_the_others_paired_by_their_definition(self):
        # Word drops instance 2 and numbers 3 as 2: the copy's 2 is the input's 3, which uses
        # the same definition, not the input's 2, which uses another.
        d, i = self.definition, self.instance
        before = self.numbering(d(0, "00AA"), d(1, "00BB"), i(1, 0), i(2, 1), i(3, 0, 4))
        after = self.numbering(d(0, "00AA"), d(1, "00BB"), i(1, 0), i(2, 0, 4))
        self.assertEqual(importer.numbering_changes(before, after),
                         ["- num 2: abstractNumId val=1"])

    def test_another_number_of_instances_is_recorded_not_guessed(self):
        # When the copy has another number of instances of a definition, nothing tells a
        # dropped instance from a changed one or an added one, so only the count is recorded.
        d, i = self.definition, self.instance
        # Word drops instance 1 and numbers 2 and 3 as 1 and 2.
        before = self.numbering(d(0, "00AA"), i(1, 0), i(2, 0, 5), i(3, 0, 9))
        after = self.numbering(d(0, "00AA"), i(1, 0, 5), i(2, 0, 9))
        self.assertEqual(importer.numbering_changes(before, after),
                         ["~ abstractNum 0: num count=3 -> 2"])
        # Word adds an instance after the others; a style naming instance 1 is no change.
        before = {**self.numbering(d(0, "00AA"), i(1, 0, 5), i(2, 0)), **self.style_naming(1)}
        after = {**self.numbering(d(0, "00AA"), i(1, 0, 9), i(2, 0), i(3, 0, 5)),
                 **self.style_naming(1)}
        self.assertEqual(importer.numbering_changes(before, after),
                         ["~ abstractNum 0: num count=2 -> 3"])
        self.assertEqual(importer.style_changes(before, after), [])
        # The definition numbered again too: the count is the input's definition's.
        before = self.numbering(d(1, "00AA"), i(1, 1), i(2, 1, 5), i(3, 1, 9))
        after = self.numbering(d(0, "00AA"), i(1, 0, 5), i(2, 0, 9))
        self.assertEqual(importer.numbering_changes(before, after),
                         ["~ abstractNum 1: num count=3 -> 2"])
        # The other definitions keep their pairs; an instance of a definition the input does
        # not have takes no instance of an unpaired one.
        before = self.numbering(d(0, "00AA"), d(1, "00BB"), i(1, 0), i(2, 0, 5), i(3, 1, 7))
        after = self.numbering(d(0, "00AA"), d(1, "00BB"), d(2, "00CC"), i(1, 0), i(2, 2),
                               i(3, 1, 8))
        self.assertEqual(importer.numbering_changes(before, after), [
            "~ abstractNum 0: num count=2 -> 1",
            "~ num 3: lvlOverride[ilvl=0]/startOverride val=7 -> 8",
        ])

    def test_kept_ids_and_kept_counts_pair_instances_before_their_overrides(self):
        d, i = self.definition, self.instance

        def styles():
            return {"word/styles.xml": (
                f'<w:styles xmlns:w="{importer.W[1:-1]}">' + "".join(
                    f'<w:style w:type="paragraph" w:styleId="List{n}"><w:pPr><w:numPr>'
                    f'<w:numId w:val="{n}"/></w:numPr></w:pPr></w:style>' for n in (1, 2))
                + "</w:styles>").encode()}

        # Word keeps every id and restarts instance 1 at 9, like instance 2: still 1 and 2.
        before = {**self.numbering(d(0, "00AA"), i(1, 0, 5), i(2, 0, 9)), **styles()}
        after = {**self.numbering(d(0, "00AA"), i(1, 0, 9), i(2, 0, 9)), **styles()}
        self.assertEqual(importer.numbering_changes(before, after),
                         ["~ num 1: lvlOverride[ilvl=0]/startOverride val=5 -> 9"])
        self.assertEqual(importer.style_changes(before, after), [])
        # Word numbers 1, 3 and 4 as 1, 2 and 3 and drops the restart of 3: in order.
        before = self.numbering(d(0, "00AA"), i(1, 0), i(3, 0, 5), i(4, 0, 9))
        after = self.numbering(d(0, "00AA"), i(1, 0), i(2, 0), i(3, 0, 9))
        self.assertEqual(importer.numbering_changes(before, after),
                         ["- num 3: lvlOverride[ilvl=0]/startOverride val=5"])

    def test_pairing_by_id_and_in_order(self):
        d, i = self.definition, self.instance
        # The same ids, written in another order: by id.
        before = self.numbering(d(0, "00AA"), i(2, 0, 9), i(1, 0, 5))
        after = self.numbering(d(0, "00AA"), i(1, 0, 5), i(2, 0, 9))
        self.assertEqual(importer.numbering_changes(before, after), [])
        # Numbered again and the first restart changed to the second's: in order, so the
        # style that names the first still names it.
        before = {**self.numbering(d(0, "00AA"), i(2, 0, 5), i(3, 0, 9)), **self.style_naming(2)}
        after = {**self.numbering(d(0, "00AA"), i(1, 0, 9), i(2, 0, 9)), **self.style_naming(1)}
        self.assertEqual(importer.numbering_changes(before, after),
                         ["~ num 2: lvlOverride[ilvl=0]/startOverride val=5 -> 9"])
        self.assertEqual(importer.style_changes(before, after), [])

    def test_pairing_follows_the_order_of_the_ids(self):
        d, i = self.definition, self.instance
        # numbering.xml written out of id order: in order means in the order of the ids.
        before = self.numbering(d(0, "00AA"), i(3, 0, 9), i(2, 0, 5))
        after = self.numbering(d(0, "00AA"), i(1, 0, 5), i(2, 0, 9))
        self.assertEqual(importer.numbering_changes(before, after), [])
        # And the copy's: written 2 then 1.
        before = self.numbering(d(0, "00AA"), i(1, 0, 5), i(2, 0, 9))
        after = self.numbering(d(0, "00AA"), i(2, 0, 9), i(1, 0, 5))
        self.assertEqual(importer.numbering_changes(before, after), [])
        # The order of the ids is that of their numbers: 9 comes before 10.
        before = self.numbering(d(0, "00AA"), i(9, 0, 5), i(10, 0, 9))
        after = self.numbering(d(0, "00AA"), i(1, 0, 5), i(2, 0, 9))
        self.assertEqual(importer.numbering_changes(before, after), [])
        # An id that is not a number, from a damaged input, is ordered too.
        self.assertEqual(importer.id_order("²"), (1, "²"))
        # A style that names an instance only the input has now names none.
        before = {**self.numbering(d(0, "00AA"), i(1, 0), i(6, 0, 5)), **self.style_naming(6)}
        after = {**self.numbering(d(0, "00AA"), i(1, 0)), **self.style_naming(6)}
        self.assertEqual(importer.style_changes(before, after),
                         ["~ L: pPr/numPr/numId val=6 -> copy 6"])

    def test_list_ids_of_styles_stay_without_numbering_on_both_sides(self):
        numbering = self.numbering(self.definition(0, "00AA"), self.instance(4, 0))
        self.assertEqual(importer.style_changes(self.style_naming(4), self.style_naming(4)), [])
        self.assertEqual(importer.style_changes(self.style_naming(4),
                                                {**numbering, **self.style_naming(4)}), [])

    def test_definitions_without_their_nsid_are_matched_through_instances_or_ids(self):
        d, i = self.definition, self.instance
        # No nsid: the copy's definition 0 is the input's 3, which instance 1 uses on both sides.
        before = self.numbering(d(3, None), i(1, 3))
        after = self.numbering(d(0, None, "%1)"), i(1, 0))
        self.assertEqual(importer.numbering_changes(before, after),
                         ["~ abstractNum 3 level 0: lvlText val=%1. -> %1)"])
        # No nsid and no instance: the same id.
        before = self.numbering(d(2, None))
        after = self.numbering(d(2, None, "%1)"))
        self.assertEqual(importer.numbering_changes(before, after),
                         ["~ abstractNum 2 level 0: lvlText val=%1. -> %1)"])
        # Word may write an nsid in capitals or not.
        before = self.numbering(d(1, "00ab12cd"), i(6, 1))
        after = self.numbering(d(0, "00AB12CD", "%1)"), i(5, 0))
        self.assertEqual(importer.numbering_changes(before, after),
                         ["~ abstractNum 1 level 0: lvlText val=%1. -> %1)"])

    def test_a_definition_of_the_copy_alone_does_not_fall_together_with_the_input_s(self):
        # The copy's 0 and 2 are the input's 1 and 2 by their nsid; its 1, used by no
        # instance, is the copy's own and must not read as a change of the input's 1.
        d = self.definition
        before = self.numbering(d(1, "00AA"), d(2, "00BB"))
        after = self.numbering(d(0, "00AA"), d(1, "00CC", "x"), d(2, "00BB"))
        self.assertEqual(importer.numbering_changes(before, after), [])

    def test_what_word_adds_to_an_instance_and_what_it_leaves_out(self):
        d, i = self.definition, self.instance
        # An override Word adds to an instance the input had is an addition.
        before = self.numbering(d(0, "00AA"), i(1, 0))
        after = self.numbering(d(0, "00AA"), i(1, 0, 1))
        self.assertEqual(importer.numbering_changes(before, after),
                         ["+ num 1: lvlOverride[ilvl=0]/startOverride val=1"])
        # w:tentative only says whether Word's interface shows a level as used.
        def overridden(tentative):
            return self.numbering(d(0, "00AA"), (
                '<w:num w:numId="1"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0">'
                f'<w:lvl w:ilvl="0"{tentative}><w:start w:val="3"/></w:lvl></w:lvlOverride>'
                "</w:num>"))

        self.assertEqual(importer.numbering_changes(overridden(""),
                                                    overridden(' w:tentative="1"')), [])

    def test_a_paragraph_whose_text_changed_is_still_paired(self):
        def document(*paragraphs):
            body = "".join(f'<w:p><w:pPr><w:pStyle w:val="{s}"/></w:pPr><w:r><w:t>{t}</w:t>'
                           "</w:r></w:p>" for s, t in paragraphs)
            return {"word/document.xml": (f'<w:document xmlns:w="{importer.W[1:-1]}"><w:body>'
                                          f"{body}</w:body></w:document>").encode()}

        # Word adds a paragraph at the end, and the text of the second differs (a field it
        # updated): the second is still the second.
        self.assertEqual(
            importer.paragraph_style_changes(
                document(("A", "one"), ("B", "page 1"), ("A", "three")),
                document(("A", "one"), ("C", "page 2"), ("A", "three"), ("A", "end"))),
            ["the re-saved copy has 4 paragraphs, the input 3", "paragraph 2: B -> C"],
        )

    def test_the_note_and_the_summary_tell_the_count_of_paragraphs_from_a_changed_style(self):
        geometry = {"styles_changed": ["- Normal: default", "+ A: rPr/b"],
                    "numbering_changed": ["~ num 1: abstractNumId val=0 -> 1"],
                    "paragraph_styles_changed": [
                        "the re-saved copy has 2 paragraphs, the input 1"]}
        self.assertEqual(importer.style_notes(geometry), [
            "style and numbering properties of the re-saved copy: 1 dropped, 1 added, 1 changed "
            "(listed in geometry.json)",
            "paragraph styles of the re-saved copy changed: 0 (listed in geometry.json)",
        ])
        item = importer.Checked(
            record={"file": "x.docx"}, pdf=Path("x.pdf"), resaved=Path("x.resaved.docx"),
            row="", geometry={**geometry, "page_count": 1, "compat_flags_dropped": []}, drawn=[],
        )
        lines = importer.summary([item], "imported")
        self.assertIn("  note: the re-saved copy has 2 paragraphs, the input 1", lines)
        self.assertFalse([line for line in lines if "paragraph style of the re-saved" in line])

    def test_a_note_that_would_read_as_two_is_refused(self):
        record = {"file": "x.docx", "windows_version": "10", "printer": "Microsoft Print to PDF",
                  "compatibility_mode": 15, "pages": 1, "fonts_missing": [],
                  "pdf_sha256": "0" * 64, "word_version": "16.0", "word_build": "16.0.1",
                  "rendered": "2026-10-19"}
        importer.reference_row("word-rules", record, [], "24.02.0", [])
        with self.assertRaisesRegex(importer.Refused, "x.docx: the SOURCES.md note 'a; b' would "
                                    "read as two notes"):
            importer.reference_row("word-rules", record, [], "24.02.0", [],
                                   styles=["a; b"])
        # Any note: the formatting note too.
        with self.assertRaisesRegex(importer.Refused, "would read as two notes"):
            importer.reference_row("word-rules", record, [], "24.02.0", [], formatting_note="a; b")

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

    def test_an_opentype_suffix_is_read(self):
        # The suffix is built from parts: spelled out, the spelling check reads it as a word.
        suffix = "(" + "O" + "T)"
        row = f"{'Minion':<37}{'Type 1C ' + suffix:<18}Custom           no  no  no      17  0"
        self.assertEqual(importer.not_embedded(PDFFONTS + row + "\n"), ["Garamond", "Minion"])

    def test_an_embedded_opentype_font_is_read_as_embedded(self):
        # As pdffonts lists a CFF face embedded as /FontFile3 /OpenType: emb yes.
        suffix = "(" + "O" + "T)"
        name = "ABCDEF+Minion"
        row = f"{name:<37}{'Type 1C ' + suffix:<18}Custom           yes yes no      17  0"
        self.assertEqual(importer.not_embedded(PDFFONTS + row + "\n"), ["Garamond"])


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

    def test_whether_a_style_is_built_in_is_committed(self):
        # Outside the body there is no style id from the re-saved copy, so the flag is what
        # tells a built-in style from a missing one (#126).
        doc = rules.documents()[0]
        record = self.add_result(doc)
        export = word_export(importer.read_parts(self.results / record["resaved_file"]))
        export["stories"][0]["paragraphs"][1]["style_builtin"] = None  # Word could not say
        self.add_v3(export)
        self.run_import()
        main, header = self.committed(doc)["stories"]
        self.assertIs(main["paragraphs"][0]["style_builtin"], True)
        self.assertNotIn("style_builtin", main["paragraphs"][1])
        self.assertIs(header["paragraphs"][0]["style_builtin"], False)

    def test_a_theme_colour_is_committed_with_its_tint_or_shade(self):
        doc = rules.documents()[0]
        record = self.add_result(doc)
        export = word_export(importer.read_parts(self.results / record["resaved_file"]))
        font = export["stories"][0]["paragraphs"][0]["font"]
        font.update(color=None, theme_color="accent1", tint_shade=-0.25)
        self.add_v3(export)
        self.run_import()
        formatting = self.committed(doc)
        first = formatting["stories"][0]["paragraphs"][0]
        committed = formatting["fonts"][first["font"]]
        self.assertEqual((committed["theme_color"], committed["tint_shade"]), ("accent1", -0.25))
        self.assertIsNone(committed["color"])

    def test_tint_and_shade_may_reach_their_bounds(self):
        doc = rules.documents()[0]
        for tint_shade in (-1, 1):
            with self.subTest(tint_shade):
                record = self.add_result(doc)
                export = word_export(importer.read_parts(self.results / record["resaved_file"]))
                font = export["stories"][0]["paragraphs"][0]["font"]
                font.update(color=None, theme_color="text2", tint_shade=tint_shade)
                self.add_v3(export)
                self.run_import()
                formatting = self.committed(doc)
                first = formatting["stories"][0]["paragraphs"][0]
                self.assertEqual(formatting["fonts"][first["font"]]["tint_shade"], tint_shade)

    def test_the_run_reader_check_note_is_accepted(self):
        doc = rules.documents()[0]
        record = self.add_result(doc)
        export = word_export(importer.read_parts(self.results / record["resaved_file"]), runs=False)
        export.update(runs_note="left out: Word failed the check of the run reader")
        self.add_v3(export, formatting_runs=False)
        self.run_import()
        self.assertFalse(self.committed(doc)["runs"])

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
            "unexpected theme colour": (
                lambda e: first(e)["font"].update(color=None, theme_color="accent9", tint_shade=0), {}
            ),
            "unexpected tint or shade": (
                lambda e: first(e)["font"].update(color=None, theme_color="accent1", tint_shade=2), {}
            ),
            "unexpected tint or shade (true)": (
                lambda e: first(e)["font"].update(color=None, theme_color="accent1", tint_shade=True), {}
            ),
            "unexpected theme colour (tint or shade alone)": (
                lambda e: first(e)["font"].update(color=None, theme_color=None, tint_shade=0), {}
            ),
            "unexpected tint or shade (missing)": (
                lambda e: first(e)["font"].update(color=None, theme_color="accent1"), {}
            ),
            "a theme colour next to an RGB colour": (
                lambda e: first(e)["font"].update(color="FF0000", theme_color="accent1", tint_shade=0), {}
            ),
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

    def test_a_paragraph_without_a_style_has_none_unless_one_style_is_the_default(self):
        w = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
        document = (
            f"<w:document {w}><w:body>"
            "<w:p><w:r><w:t>No style</w:t></w:r></w:p>"
            '<w:p><w:pPr><w:pStyle w:val="AlsoDefault"/></w:pPr><w:r><w:t>Named</w:t></w:r></w:p>'
            "</w:body></w:document>"
        )

        def parts(*defaults):
            styles = ""
            for style_id, name in (("Normal", "Normal"), ("AlsoDefault", "Also Default")):
                default = ' w:default="1"' if style_id in defaults else ""
                styles += (
                    f'<w:style w:type="paragraph"{default} w:styleId="{style_id}">'
                    f'<w:name w:val="{name}"/></w:style>'
                )
            return {"word/document.xml": document.encode(),
                    "word/styles.xml": f"<w:styles {w}>{styles}</w:styles>".encode()}

        def found(*defaults):
            return importer.body_paragraph_styles(parts(*defaults))

        named = ("Named", "AlsoDefault", "Also Default")
        self.assertEqual(found("Normal"), [("No style", "Normal", "Normal"), named])
        self.assertEqual(found("AlsoDefault"), [("No style", "AlsoDefault", "Also Default"), named])
        # Two styles marked default, or none: which one Word applied is not known (#149).
        self.assertEqual(found("Normal", "AlsoDefault"), [("No style", None, None), named])
        self.assertEqual(found(), [("No style", None, None), named])
        # formatting.json then leaves the style out for that paragraph only.
        font = {"name": "Calibri", "size": 11, "bold": False, "italic": False,
                "color": "auto", "theme": "minorHAnsi"}
        export = {"format": 1, "complete": True, "runs": False, "runs_note": "", "stories": [
            {"story": "main", "instance": 0, "paragraphs": [
                {"index": i, "text": text, "style": None, "style_builtin": True,
                 "left_indent": 0, "right_indent": 0, "first_line_indent": 0,
                 "space_before": 0, "space_after": 0, "space_before_auto": False,
                 "space_after_auto": False, "line_spacing": 12, "line_spacing_rule": "single",
                 "alignment": "left", "list_string": "", "list_level": None, "font": font}
                for i, text in enumerate(["No style", "Named"])
            ]},
        ]}
        export = importer.read_formatting(json.dumps(export).encode())
        result, _ = importer.compact_formatting(export, parts("Normal", "AlsoDefault"), "x", "y")
        first, second = result["stories"][0]["paragraphs"]
        self.assertNotIn("style_id", first)
        self.assertNotIn("style_name", first)
        self.assertEqual(
            (second["style_id"], second["style_name"]), ("AlsoDefault", "Also Default")
        )

    def test_an_empty_paragraph_with_a_hidden_mark_is_folded_into_the_next(self):
        w = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'

        def paragraph(text, mark="", run="", style=None):
            p_style = f'<w:pStyle w:val="{style}"/>' if style else ""
            mark_xml = f"<w:rPr>{mark}</w:rPr>" if mark else ""
            p_pr = f"<w:pPr>{p_style}{mark_xml}</w:pPr>" if style or mark else ""
            r = f"<w:r><w:rPr>{run}</w:rPr><w:t>{text}</w:t></w:r>" if text or run else ""
            return f"<w:p>{p_pr}{r}</w:p>"

        hidden = "<w:vanish/>"
        document = (
            f"<w:document {w}><w:body>"
            + paragraph("Before")
            + paragraph("", hidden)  # folded into "Impact", as Word folds it
            + paragraph("", '<w:vanish w:val="true"/>')  # folded too
            + paragraph("Impact", style="Heading3")
            + paragraph("Hidden mark", hidden)  # text: not folded
            + paragraph("", '<w:vanish w:val="0"/>')
            + paragraph("", '<w:vanish w:val="false"/>')
            + paragraph("", '<w:vanish w:val="off"/>')
            + paragraph("")
            + paragraph("", "<w:b/>")
            + paragraph("", run=hidden)  # a hidden run, not a hidden mark
            + paragraph("After", style="Heading3")
            + paragraph("", hidden)
            + paragraph("", hidden)
            + paragraph("")  # an empty paragraph after them takes the fold as well
            + "</w:body></w:document>"
        )
        styles = (
            f"<w:styles {w}>"
            '<w:style w:type="paragraph" w:default="1" w:styleId="Normal">'
            '<w:name w:val="Normal"/></w:style>'
            '<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/>'
            "</w:style></w:styles>"
        )
        parts = {"word/document.xml": document.encode(), "word/styles.xml": styles.encode()}
        normal = ("Normal", "Normal")
        texts = ["Before", "Impact", "Hidden mark", "", "", "", "", "", "", "After", ""]
        self.assertEqual(
            importer.body_paragraph_styles(parts),
            [
                ("Before", *normal),
                # Word gives the entry the formatting of the first paragraph folded into it,
                # so the style of "Impact" is not the one Word applied.
                ("Impact", None, None),
                ("Hidden mark", *normal),
                *[("", *normal)] * 6,
                ("After", "Heading3", "heading 3"),
                ("", None, None),
            ],
        )
        # So the export of such a document lines up with the copy, and the folded entry
        # gets no style id.
        font = {"name": "Calibri", "size": 11, "bold": False, "italic": False,
                "color": "auto", "theme": "minorHAnsi"}
        export = {"format": 1, "complete": True, "runs": False, "runs_note": "", "stories": [
            {"story": "main", "instance": 0, "paragraphs": [
                {"index": i, "text": text, "style": None, "style_builtin": True,
                 "left_indent": 0, "right_indent": 0, "first_line_indent": 0,
                 "space_before": 0, "space_after": 0, "space_before_auto": False,
                 "space_after_auto": False, "line_spacing": 12, "line_spacing_rule": "single",
                 "alignment": "left", "list_string": "", "list_level": None, "font": font}
                for i, text in enumerate(texts)
            ]},
        ]}
        export = importer.read_formatting(json.dumps(export).encode())
        result, note = importer.compact_formatting(export, parts, "x", "y")
        self.assertTrue(result["styles_from_resaved_copy"])
        self.assertNotIn("not matched", note)
        paragraphs = result["stories"][0]["paragraphs"]
        self.assertEqual(
            [p.get("style_id") for p in paragraphs],
            ["Normal", None] + ["Normal"] * 7 + ["Heading3", None],
        )

    def test_every_spelling_of_true_marks_the_default(self):
        w = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
        document = (
            f"<w:document {w}><w:body><w:p><w:r><w:t>Plain</w:t></w:r></w:p></w:body></w:document>"
        )
        for value, expected in (("1", "Body"), ("true", "Body"), ("on", "Body"),
                                ("0", None), ("false", None), ("off", None)):
            styles = (
                f"<w:styles {w}>"
                f'<w:style w:type="paragraph" w:default="{value}" w:styleId="Body">'
                '<w:name w:val="Body"/></w:style></w:styles>'
            )
            with self.subTest(value=value):
                found = importer.body_paragraph_styles(
                    {"word/document.xml": document.encode(), "word/styles.xml": styles.encode()}
                )
                self.assertEqual(found, [("Plain", expected, expected)])

    def test_style_ids_land_on_their_paragraphs_around_a_table(self):
        w = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'

        def paragraph(text, style=None):
            p_pr = f'<w:pPr><w:pStyle w:val="{style}"/></w:pPr>' if style else ""
            return f"<w:p>{p_pr}<w:r><w:t>{text}</w:t></w:r></w:p>"

        def cell(text):
            return f"<w:tc>{paragraph(text)}</w:tc>"

        document = (
            f"<w:document {w}><w:body>{paragraph('Before', 'Title')}<w:tbl>"
            f"<w:tr>{cell('A')}{cell('B')}</w:tr><w:tr>{cell('C')}{cell('D')}</w:tr></w:tbl>"
            f"{paragraph('After', 'Heading1')}</w:body></w:document>"
        )
        styles = f"<w:styles {w}>" + "".join(
            f'<w:style w:type="paragraph"{default} w:styleId="{sid}"><w:name w:val="{sid}"/>'
            "</w:style>"
            for sid, default in (("Normal", ' w:default="1"'), ("Title", ""), ("Heading1", ""))
        ) + "</w:styles>"
        parts = {"word/document.xml": document.encode(), "word/styles.xml": styles.encode()}
        font = {"name": "Calibri", "size": 11, "bold": False, "italic": False,
                "color": "auto", "theme": "minorHAnsi"}
        texts = ["Before", "A", "B", "", "C", "D", "", "After"]
        export = {"format": 1, "complete": True, "runs": False, "runs_note": "", "stories": [
            {"story": "main", "instance": 0, "paragraphs": [
                {"index": i, "text": text, "style": None, "style_builtin": True,
                 "left_indent": 0, "right_indent": 0, "first_line_indent": 0,
                 "space_before": 0, "space_after": 0, "space_before_auto": False,
                 "space_after_auto": False, "line_spacing": 12, "line_spacing_rule": "single",
                 "alignment": "left", "list_string": "", "list_level": None, "font": font}
                for i, text in enumerate(texts)
            ]},
        ]}
        export = importer.read_formatting(json.dumps(export).encode())
        result, _ = importer.compact_formatting(export, parts, "x", "y")
        found = [p.get("style_id") for p in result["stories"][0]["paragraphs"]]
        self.assertEqual(
            found, ["Title", "Normal", "Normal", None, "Normal", "Normal", None, "Heading1"]
        )

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
        # No styles part, so no default style: the copy does not say which style Word applied.
        self.assertNotIn("style_id", paragraphs[4])
        self.assertTrue(all("style_id" not in p for p in paragraphs))
        self.assertNotIn("not matched", note)
        result, note = importer.compact_formatting(
            export(["Before", "A", "B", "C", "D", "After"]), parts, "x", "y"
        )
        self.assertFalse(result["styles_from_resaved_copy"])
        self.assertIn("body styles not matched to the re-saved copy", note)


class Summary(unittest.TestCase):
    """What the import prints about each document."""

    def item(self, word_pages, pages, drawn=()):
        return importer.Checked(
            record={"file": "report.docx", "pages": word_pages},
            pdf=Path("report.pdf"),
            resaved=Path("report.resaved.docx"),
            row="",
            geometry={"page_count": pages, "compat_flags_dropped": []},
            drawn=list(drawn),
        )

    def test_each_document_and_the_count(self):
        lines = importer.summary([self.item(35, 35), self.item(1, 1)], "would import")
        self.assertEqual(
            lines,
            [
                "would import report.docx: 35 pages",
                "would import report.docx: 1 page",
                "corpus-import.py: would import 2 reference renders",
            ],
        )

    def test_a_substituted_font_is_named_with_its_substitute(self):
        face = importer.FontFace(
            path=Path("arial.ttf"), index=0, collection=False, postscript="ArialMT",
            full_name="Arial", version="Version 7.06", cff=False,
        )
        drawn = importer.DrawnFont("Helvetica", face, b"", "ArialMT")
        lines = importer.summary([self.item(1, 1, [drawn])], "imported")
        self.assertEqual(
            lines[1],
            "  note: Helvetica is not embedded; rasterized with Arial Version 7.06, as Windows "
            "substitutes it for Helvetica",
        )


class FormattingNote(unittest.TestCase):
    """SOURCES.md's note on a formatting export, made from the committed file."""

    def result(self, **changes):
        result = {"runs": True, "runs_note": "", "complete": True,
                  "styles_from_resaved_copy": True,
                  "stories": [{"paragraphs": [{}, {}]}, {"paragraphs": [{}]}]}
        result.update(changes)
        return result

    def test_each_part_of_the_note(self):
        note = importer.formatting_note
        self.assertEqual(
            note(self.result()), "formatting as Word computes it: 3 paragraphs, with runs"
        )
        self.assertEqual(
            note(self.result(runs=False, runs_note=importer.SIZE_RUNS_NOTE)),
            "formatting as Word computes it: 3 paragraphs, runs left out (size)",
        )
        self.assertEqual(
            note(self.result(runs=False, complete=False, styles_from_resaved_copy=False)),
            "formatting as Word computes it: 3 paragraphs, without runs, incomplete (time budget), "
            "body styles not matched to the re-saved copy",
        )

    def test_a_file_over_the_limit_loses_its_runs_and_says_so(self):
        font = {"name": "Calibri"}
        paragraphs = [{"index": i, "text": "x", "font": 0, "runs": [[1, 1]] * 2000}
                      for i in range(150)]
        result = self.result(fonts=[font, dict(font, bold=True)], document="x.docx",
                             stories=[{"story": "main", "instance": 0, "paragraphs": paragraphs}])
        self.assertGreater(len(importer.formatting_bytes(result)), importer.MAX_FORMATTING_BYTES)
        data, note = importer.within_size(result, importer.formatting_note(result))
        self.assertLess(len(data), importer.MAX_FORMATTING_BYTES)
        self.assertEqual(
            note, "formatting as Word computes it: 150 paragraphs, runs left out (size)"
        )


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

    def test_only_the_substituted_styles_of_helvetica_may_have_a_comma(self):
        (self.temp / "arial.ttf").write_bytes(sfnt("ArialMT", "Arial", "Version 7.06"))
        (self.temp / "arialbd.ttf").write_bytes(sfnt("Arial-BoldMT", "Arial Bold", "7.06"))
        (self.temp / "ariali.ttf").write_bytes(sfnt("Arial-ItalicMT", "Arial Italic", "7.06"))
        index = importer.font_index([self.temp])
        self.assertEqual(importer.drawn_font("Helvetica", index).substitute, "ArialMT")
        self.assertEqual(importer.drawn_font("Helvetica,Bold", index).face.full_name, "Arial Bold")
        self.assertEqual(importer.drawn_font("ArialMT", index).substitute, "")
        # Styles Word's PDFs have not shown are not guessed.
        refused = ("Helvetica,Italic", "Helvetica,BoldItalic", "Helvetica,Oblique", "ArialMT,Bold")
        for name in refused:
            with self.assertRaises(importer.Refused) as caught:
                importer.drawn_font(name, index)
            self.assertIn(f"{name}: Word names a style it synthesizes", str(caught.exception))

    def test_a_pdf_font_name_that_would_break_the_row_is_refused(self):
        (self.temp / "font.ttf").write_bytes(sfnt("Gara|mond", "Garamond", "Version 1.0"))
        index = importer.font_index([self.temp])
        for name in ("Gara|mond", "Gara\x07mond"):
            with self.assertRaises(importer.Refused) as caught:
                importer.drawn_font(name, index)
            self.assertIn("unexpected font name in the PDF", str(caught.exception))

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
