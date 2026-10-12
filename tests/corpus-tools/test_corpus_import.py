"""Checks scripts/corpus-import.py with fake make-reference.ps1 results.

pdfinfo and pdffonts are replaced by fixed output, so the checks run where poppler is not
installed (Windows) and do not depend on its version.
"""

import hashlib
import importlib.util
import json
import shutil
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

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
        self.tools = importer.Tools(
            info=lambda path: self.info, xmp=lambda path: self.xmp, fonts=lambda path: PDFFONTS
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
            "script_version": 1,
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
            "windows_version": "10.0.26100.0",
            "windows_display_version": "24H2",
            "rendered": "2026-10-13",
            "export_seconds": 1.5,
        }
        record.update(changes)
        (self.results / f"{doc.name}.json").write_text(json.dumps(record), encoding="utf-8")
        return record

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

    def test_imports_files_and_sources_rows(self):
        checked = self.run_import()
        self.assertEqual(len(checked), 2)
        reference = self.corpus / "reference" / "word-rules"
        resaved = self.corpus / "resaved" / "word-rules"
        self.assertEqual(sorted(p.name for p in reference.iterdir()),
                         ["E-line-exact.pdf", "E-line-fonts.pdf"])
        self.assertEqual(sorted(p.name for p in resaved.iterdir()),
                         ["E-line-exact.docx", "E-line-fonts.docx"])
        sources = (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        self.assertIn(
            "| reference/word-rules/E-line-fonts.pdf | word-rules/E-line-fonts.docx, re-saved by "
            "Word as resaved/word-rules/E-line-fonts.docx | Microsoft Word 16.0, build 16.0.1, "
            "Windows 10.0.26100.0 (24H2) | 2026-10-13 | compatibility mode 15; 3 pages; fonts "
            "Word substituted: none; fonts not embedded: Garamond; fields as saved |",
            sources,
        )

    def test_importing_again_replaces_the_rows(self):
        self.run_import()
        self.run_import()
        sources = (self.corpus / "SOURCES.md").read_text(encoding="utf-8")
        self.assertEqual(sources.count("| reference/word-rules/E-line-fonts.pdf |"), 1)
        self.assertEqual(sources.count("| reference/word-rules/E-line-exact.pdf |"), 1)

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
                           ("windows_display_version", "24H2|"), ("fonts_missing", ["A|B"])):
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
        self.add_result(rules.documents()[0], script_version=2)
        self.assertIn("format 2", self.refused())

    def test_refuses_a_path_instead_of_a_file_name(self):
        for path in ("..\\elsewhere.docx", "../elsewhere.docx", "sub/E-line-fonts.docx", ".docx"):
            with self.subTest(path=path):
                self.add_result(rules.documents()[0], file=path)
                self.assertIn("file must be the name of a .docx file", self.refused())


class Pdffonts(unittest.TestCase):
    def test_lists_fonts_that_are_not_embedded_without_subset_tags(self):
        self.assertEqual(importer.not_embedded(PDFFONTS), ["Garamond"])

    def test_no_fonts(self):
        self.assertEqual(importer.not_embedded(PDFFONTS.split("-")[0]), [])


if __name__ == "__main__":
    unittest.main()
