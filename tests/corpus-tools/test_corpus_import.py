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
        self.tools = importer.Tools(
            info=lambda path: self.info,
            xmp=lambda path: self.xmp,
            fonts=lambda path: PDFFONTS,
            raster=self.raster,
            bbox=lambda path: self.bbox,
            version=lambda: "24.02.0",
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

    def raster(self, pdf, directory):
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

    def run_import(self, dry_run=False, allow_locale=False):
        return importer.import_results(
            self.results, "word-rules", self.corpus, self.tools, dry_run, allow_locale
        )

    def refused(self):
        with self.assertRaises(importer.Refused) as caught:
            self.run_import()
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
            "grey; fonts the PDF does not embed, rasterized with poppler's substitutes: Garamond |",
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
                (b"<w:zoom", b"<!-- C:\\Users\\jroe\\Normal.dotm --><w:zoom"),
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
        pdf.write_bytes(b"%PDF-1.7\n% C:\\Users\\jroe\\file.docx\n%%EOF\n")
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

    def test_a_type_with_spaces_is_read(self):
        row = "Helvetica" + " " * 28 + "Type 1            WinAnsi          no  no  no       3  0"
        self.assertEqual(importer.not_embedded(PDFFONTS + row + "\n"), ["Garamond", "Helvetica"])


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
