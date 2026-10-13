"""Checks scripts/make-business-docs.py and the documents it wrote to tests/corpus/generated/."""

import importlib.util
import posixpath
import re
import shutil
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from xml.etree import ElementTree

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
GENERATED = ROOT / "tests" / "corpus" / "generated"


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


gen = load("make-business-docs")
OFFICE = set(gen.OPEN_FONTS)
LANGUAGES = {"uk": "uk-UA", "ru": "ru-RU", "de": "de-DE", "zh": "zh-CN"}


def xml_parts(path):
    with zipfile.ZipFile(path) as archive:
        return {
            n: archive.read(n).decode("utf-8")
            for n in archive.namelist()
            if n.endswith((".xml", ".rels"))
        }


class Committed(unittest.TestCase):
    def test_the_committed_documents_are_what_the_script_writes(self):
        self.assertEqual(gen.check(gen.documents()), [])

    def test_every_document_but_the_chinese_has_an_open_font_twin(self):
        names = {name for name, _, _ in gen.variants(gen.documents())}
        for doc in gen.documents():
            with self.subTest(doc.name):
                self.assertEqual(f"{doc.name}-open.docx" in names, not doc.name.endswith("-zh"))

    def test_twins_name_only_open_fonts_and_originals_name_office_fonts(self):
        pattern = r'<w:(?:rFonts|font)\b[^>]*\bw:(?:ascii|hAnsi|cs|eastAsia|name)="([^"]+)"'
        for name, doc, twin in gen.variants(gen.documents()):
            fonts = set(re.findall(pattern, "".join(xml_parts(GENERATED / name).values())))
            with self.subTest(name):
                if twin:
                    self.assertEqual(fonts & OFFICE, set())
                    self.assertTrue(fonts & set(gen.OPEN_FONTS.values()))
                elif not name.endswith("-zh.docx"):
                    self.assertTrue(fonts & OFFICE)

    def test_contact_details_are_fictional(self):
        for path in sorted(GENERATED.glob("*.docx")):
            text = "".join(xml_parts(path).values())
            with self.subTest(path.name):
                for address in re.findall(r"[\w.+-]+@[\w.-]+\.\w+", text):
                    self.assertRegex(address, r"@([\w-]+\.)*example\.(com|org|gov)$")
                for url in re.findall(r'Target="(https?://[^"]+)"', text):
                    self.assertRegex(url, r"^https?://([\w-]+\.)*example\.(com|org)/")
                for phone in re.findall(r"\(555\) \d{3}-\d{4}", text):
                    self.assertRegex(phone, r"\(555\) 01\d-\d{4}")
                for iban in re.findall(r"\b(?:UA|DE)\d{2}(?: ?\d{4})+", text):
                    self.assertEqual(
                        set(re.sub(r"\D", "", iban)), {"0"} | (set(iban[-1]) & set("12"))
                    )

    def test_each_language_is_declared(self):
        for name, doc, _ in gen.variants(gen.documents()):
            match = re.search(r"-(uk|ru|de|zh)(-open)?\.docx$", name)
            expected = LANGUAGES[match.group(1)] if match else None
            with self.subTest(name):
                if expected:
                    self.assertEqual(doc.lang, expected)
                    styles = xml_parts(GENERATED / name)["word/styles.xml"]
                    # Chinese is the East Asian language; Latin text in it is English.
                    if expected == "zh-CN":
                        self.assertIn('w:lang w:val="en-US" w:eastAsia="zh-CN"', styles)
                    else:
                        self.assertIn(f'w:lang w:val="{expected}" w:eastAsia="en-US"', styles)

    def test_fonts_in_the_font_table_are_fonts(self):
        for path in sorted(GENERATED.glob("*.docx")):
            table = xml_parts(path)["word/fontTable.xml"]
            names = set(re.findall(r'w:name="([^"]+)"', table))
            with self.subTest(path.name):
                self.assertFalse({n for n in names if re.fullmatch(r"[a-z]{2}-[A-Z]{2}", n)})
                self.assertTrue(names)

    def test_no_story_ends_with_a_table(self):
        for path in sorted(GENERATED.glob("*.docx")):
            for name, xml in xml_parts(path).items():
                if re.match(r"word/(document|header\d+|footer\d+)\.xml$", name):
                    with self.subTest(f"{path.name} {name}"):
                        self.assertNotRegex(xml, r"</w:tbl>(<w:sectPr\b.*)?</w:(body|hdr|ftr)>")

    def test_packages_are_well_formed(self):
        for path in sorted(GENERATED.glob("*.docx")):
            with self.subTest(path.name), zipfile.ZipFile(path) as archive:
                names = set(archive.namelist())
                types = archive.read("[Content_Types].xml").decode()
                defaults = set(re.findall(r'Extension="([^"]+)"', types))
                overrides = set(re.findall(r'PartName="/([^"]+)"', types))
                kinds = dict(re.findall(r'PartName="/([^"]+)" ContentType="([^"]+)"', types))
                for name in names:
                    if name.endswith((".xml", ".rels")):
                        ElementTree.fromstring(archive.read(name))
                    if name != "[Content_Types].xml":
                        self.assertTrue(
                            name in overrides or name.rsplit(".", 1)[-1] in defaults, name
                        )
                    # The main parts need their own content types; the xml default is not one.
                    kind = re.match(
                        r"word/(document|styles|settings|fontTable|numbering|footnotes"
                        r"|header|footer)\d*\.xml$",
                        name,
                    )
                    if kind:
                        expected = {"document": "document.main"}.get(kind.group(1), kind.group(1))
                        self.assertEqual(kinds.get(name), f"{gen.CT_W}.{expected}+xml", name)
                    if name.endswith(".rels"):
                        base = posixpath.dirname(posixpath.dirname(name))
                        for rel in re.findall(
                            r"<Relationship\b[^>]*/>", archive.read(name).decode()
                        ):
                            if 'TargetMode="External"' not in rel:
                                target = re.search(r'Target="([^"]*)"', rel).group(1)
                                self.assertIn(
                                    posixpath.normpath(posixpath.join(base, target)), names
                                )

    def test_every_file_is_small(self):
        for path in GENERATED.glob("*.docx"):
            self.assertLess(path.stat().st_size, 1_000_000, path.name)


class Check(unittest.TestCase):
    def setUp(self):
        self.temp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.temp)
        shutil.copytree(GENERATED, self.temp / "generated")
        self.original = gen.OUT_DIR
        gen.OUT_DIR = self.temp / "generated"
        self.addCleanup(setattr, gen, "OUT_DIR", self.original)

    def test_a_changed_missing_or_extra_file_is_reported(self):
        docs = gen.documents()
        changed = self.temp / "generated" / "invoice-de.docx"
        changed.write_bytes(gen.package(docs[0]))
        (self.temp / "generated" / "letter-uk.docx").unlink()
        (self.temp / "generated" / "stray.docx").write_bytes(b"")
        problems = "\n".join(gen.check(docs))
        self.assertIn("invoice-de.docx", problems)
        self.assertIn("letter-uk.docx is missing", problems)
        self.assertIn("stray.docx is not written by this script", problems)

    def test_packages_are_deterministic(self):
        doc = gen.documents()[0]
        self.assertEqual(gen.package(doc), gen.package(doc))
        self.assertNotEqual(gen.package(doc), gen.package(doc, twin=True))


if __name__ == "__main__":
    unittest.main()
