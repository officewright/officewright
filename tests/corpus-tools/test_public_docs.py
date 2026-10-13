"""Checks scripts/import-public-docs.py and the documents it committed to tests/corpus/business/.

The originals are not committed, so CI cannot clean them again; it checks instead that every
committed document holds no person's data and no picture of the original, and that its
SOURCES.md row names its URL, download date, original SHA-256 and licence statement. The
cleaning itself is tested on a small package made here.
"""

import hashlib
import importlib.util
import io
import shutil
import tempfile
import struct
import sys
import unittest
import zipfile
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
BUSINESS = ROOT / "tests" / "corpus" / "business"


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


public = load("import-public-docs")


def png(width, height):
    """A real (not placeholder) grey PNG."""
    rows = b"".join(b"\x00" + bytes((x * 40) % 256 for x in range(width)) for _ in range(height))

    def chunk(kind, body):
        crc = struct.pack(">I", zlib.crc32(kind + body))
        return struct.pack(">I", len(body)) + kind + body + crc

    header = struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
PKG = "http://schemas.openxmlformats.org/package/2006/relationships"


def relationships(*items):
    body = "".join(
        f'<Relationship Id="rId{n}" Type="{kind}" Target="{target}"{extra}/>'
        for n, (kind, target, extra) in enumerate(items, 1)
    )
    return f'<?xml version="1.0"?><Relationships xmlns="{PKG}">{body}</Relationships>'


def package(**changes):
    """A package with everything the cleaning removes or rewrites."""
    parts = {
        "[Content_Types].xml": (
            '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/'
            'content-types"><Default Extension="xml" ContentType="application/xml"/>'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.'
            'relationships+xml"/><Default Extension="emf" ContentType="image/x-emf"/>'
            '<Default Extension="jpeg" ContentType="image/jpeg"/>'
            '<Override PartName="/docProps/custom.xml" ContentType="application/vnd.'
            'openxmlformats-officedocument.custom-properties+xml"/>'
            '<Override PartName="/customXml/itemProps1.xml" ContentType="application/vnd.'
            'openxmlformats-officedocument.customXmlProperties+xml"/>'
            '<Override PartName="/word/document.xml" ContentType="application/vnd.'
            'openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>'
        ),
        "_rels/.rels": relationships(
            (f"{REL}/officeDocument", "word/document.xml", ""),
            (f"{PKG}/metadata/thumbnail", "docProps/thumbnail.jpeg", ""),
            (f"{REL}/custom-properties", "docProps/custom.xml", ""),
            (f"{PKG}/metadata/core-properties", "docProps/core.xml", ""),
            (f"{REL}/extended-properties", "docProps/app.xml", ""),
        ),
        "docProps/core.xml": (
            '<cp:coreProperties xmlns:cp="c" xmlns:dc="d"><dc:title>Plan</dc:title>'
            "<dc:creator>Jane Roe</dc:creator><cp:lastModifiedBy>jroe</cp:lastModifiedBy>"
            "</cp:coreProperties>"
        ),
        "docProps/app.xml": "<Properties><Company>Roe Consulting</Company><Manager>R</Manager>"
        "<HyperlinkBase>http://intranet.example.com/roe/</HyperlinkBase>"
        "<Pages>1</Pages></Properties>",
        "docProps/custom.xml": "<Properties><property>jane.roe@agency.gov</property></Properties>",
        "docProps/thumbnail.jpeg": b"\xff\xd8\xff\xd9",
        "customXml/item1.xml": "<root><owner>Jane Roe</owner></root>",
        "customXml/itemProps1.xml": "<ds:datastoreItem xmlns:ds='x'/>",
        "customXml/_rels/item1.xml.rels": relationships(
            (f"{REL}/customXmlProps", "itemProps1.xml", "")
        ),
        "word/document.xml": "<w:document xmlns:w='w'><w:body><w:p><w:r><w:t>Call Jane Roe at "
        "jane.roe@agency.gov</w:t></w:r></w:p></w:body></w:document>",
        "word/_rels/document.xml.rels": relationships(
            (f"{REL}/image", "media/image1.emf", ""),
            (f"{REL}/image", "media/image2.png", ""),
            (f"{REL}/customXml", "../customXml/item1.xml", ""),
            (f"{REL}/settings", "settings.xml", ""),
            (f"{REL}/header", "header1.xml", ""),
            (f"{REL}/hyperlink", "mailto:jane.roe@agency.gov", ' TargetMode="External"'),
            (f"{REL}/hyperlink", "https://www.example.gov/", ' TargetMode="External"'),
        ),
        "word/settings.xml": "<w:settings xmlns:w='w'><w:attachedTemplate r:id='rId1'/>"
        "<w:docVars><w:docVar w:name='owner' w:val='Jane Roe'/></w:docVars><w:zoom/></w:settings>",
        "word/_rels/settings.xml.rels": relationships(
            (f"{REL}/attachedTemplate", "file:///C:/Users/jroe/Normal.dotm",
             ' TargetMode="External"')
        ),
        "word/header1.xml": "<w:hdr xmlns:w='w'/>",
        "word/_rels/header1.xml.rels": relationships((f"{REL}/image", "media/image2.png", "")),
        "word/media/image1.emf": b"\x01\x00\x00\x00 EMF seal",
        "word/media/image2.png": png(3, 2),
        "word/glossary/document.xml": "<w:glossaryDocument xmlns:w='w'/>",
        "word/glossary/_rels/document.xml.rels": relationships(
            (f"{REL}/image", "media/image9.gif", "")
        ),
        "word/glossary/media/image9.gif": b"GIF89a\x04\x00\x03\x00" + bytes(10),
    }
    parts.update(changes)
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for name, data in parts.items():
            if data is not None:
                archive.writestr(name, data.encode() if isinstance(data, str) else data)
    return buffer.getvalue()


DOC = public.PublicDoc(
    "test",
    "nara",
    "https://www.example.gov/test.docx",
    "2026-10-13",
    "0" * 64,
    replacements=(
        public.Replacement(
            "word/document.xml",
            r"[A-Z][a-z]+ [A-Z][a-z]+ at [a-z.]+@agency\.gov",
            hashlib.sha256(b"Jane Roe at jane.roe@agency.gov").hexdigest(),
            "the help desk at help@example.gov",
        ),
        public.Replacement(
            "word/_rels/document.xml.rels",
            r'mailto:[^"]+@agency\.gov',
            hashlib.sha256(b"mailto:jane.roe@agency.gov").hexdigest(),
            "mailto:help@example.gov",
        ),
    ),
    replaced="text: a name and address replaced",
)


def parts_of(data):
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        return {name: archive.read(name) for name in archive.namelist()}


class Cleaning(unittest.TestCase):
    def setUp(self):
        self.cleaned = public.clean(package(), DOC)
        self.parts = parts_of(self.cleaned.data)

    def test_private_parts_are_removed_and_unlinked(self):
        for name in ("docProps/custom.xml", "docProps/thumbnail.jpeg", "customXml/item1.xml",
                     "customXml/itemProps1.xml", "customXml/_rels/item1.xml.rels"):
            self.assertNotIn(name, self.parts)
        root = self.parts["_rels/.rels"].decode()
        self.assertNotIn("thumbnail", root)
        self.assertNotIn("custom.xml", root)
        self.assertNotIn("customXml", self.parts["word/_rels/document.xml.rels"].decode())
        types = self.parts["[Content_Types].xml"].decode()
        self.assertNotIn("/docProps/custom.xml", types)
        self.assertNotIn("/customXml/itemProps1.xml", types)
        self.assertIn('Extension="png"', types)

    def test_people_leave_the_properties(self):
        core = self.parts["docProps/core.xml"].decode()
        self.assertIn("<dc:creator></dc:creator>", core)
        self.assertIn("<cp:lastModifiedBy></cp:lastModifiedBy>", core)
        self.assertIn("<dc:title>Plan</dc:title>", core)
        app = self.parts["docProps/app.xml"].decode()
        self.assertIn("<Company></Company>", app)
        self.assertIn("<Manager></Manager>", app)
        self.assertIn("<HyperlinkBase></HyperlinkBase>", app)
        self.assertIn(
            "document properties emptied: creator, last modified by, company, manager, "
            "hyperlink base",
            self.cleaned.changes,
        )

    def test_properties_without_people_are_not_reported(self):
        quiet = package(**{
            "docProps/core.xml": "<cp:coreProperties xmlns:cp='c' xmlns:dc='d'><dc:creator>"
            "</dc:creator></cp:coreProperties>",
            "docProps/app.xml": "<Properties><Company></Company></Properties>",
        })
        changes = "; ".join(public.clean(quiet, DOC).changes)
        self.assertNotIn("document properties", changes)

    def test_every_picture_becomes_a_placeholder(self):
        self.assertNotIn("word/media/image1.emf", self.parts)
        for name, size in (("word/media/image1.png", (96, 96)), ("word/media/image2.png", (3, 2)),
                           ("word/glossary/media/image9.png", (4, 3))):
            with self.subTest(name):
                self.assertTrue(public.is_placeholder(self.parts[name]))
                self.assertEqual(public.image_size(self.parts[name]), size)
        self.assertNotIn("word/glossary/media/image9.gif", self.parts)
        glossary = self.parts["word/glossary/_rels/document.xml.rels"].decode()
        self.assertIn('Target="media/image9.png"', glossary)
        rels = self.parts["word/_rels/document.xml.rels"].decode()
        self.assertIn('Target="media/image1.png"', rels)
        self.assertIn('Target="media/image2.png"', rels)
        header = self.parts["word/_rels/header1.xml.rels"].decode()
        self.assertIn('Target="media/image2.png"', header)

    def test_the_placeholder_is_a_grey_frame_with_diagonals(self):
        data = public.placeholder(20, 10)
        position, idat = 8, b""
        while position < len(data):
            (length,) = struct.unpack(">I", data[position : position + 4])
            if data[position + 4 : position + 8] == b"IDAT":
                idat += data[position + 8 : position + 8 + length]
            position += 12 + length
        raw = zlib.decompress(idat)
        rows = [raw[y * 21 + 1 : (y + 1) * 21] for y in range(10)]
        self.assertEqual(rows[0], b"\x90" * 20)
        self.assertEqual(rows[9], b"\x90" * 20)
        self.assertEqual((rows[5][0], rows[5][19]), (0x90, 0x90))
        self.assertEqual(rows[5][10], 0xB0)  # where the diagonals cross
        self.assertEqual(rows[5][5], 0xE0)
        self.assertFalse(public.is_placeholder(png(20, 10)))

    def test_template_and_variables_leave_the_settings(self):
        settings = self.parts["word/settings.xml"].decode()
        self.assertNotIn("attachedTemplate", settings)
        self.assertNotIn("docVars", settings)
        self.assertIn("<w:zoom/>", settings)
        self.assertNotIn("Users", self.parts["word/_rels/settings.xml.rels"].decode())

    def test_replacements_are_applied(self):
        self.assertIn(b"Call the help desk at help@example.gov", self.parts["word/document.xml"])
        rels = self.parts["word/_rels/document.xml.rels"]
        self.assertIn(b"mailto:help@example.gov", rels)
        self.assertIn(b"https://www.example.gov/", rels)

    def test_the_result_has_no_problems_and_says_what_changed(self):
        self.assertEqual(public.problems(self.cleaned.data), [])
        changes = "; ".join(self.cleaned.changes)
        self.assertIn("3 pictures replaced by a grey placeholder", changes)
        self.assertIn("removed: custom XML parts, custom properties, thumbnail", changes)
        self.assertIn("attached template and document variables removed", changes)
        self.assertIn("text: a name and address replaced", changes)
        self.assertNotIn("Jane", changes)

    def test_problems_are_found(self):
        found = "; ".join(public.problems(package(**{
            "word/footnotes.xml": "<w:footnotes xmlns:w='w'><w:p><w:ins w:author='jroe'>"
            "<w:r><w:t>Call (202) 55</w:t></w:r><w:r><w:t>5-0199</w:t></w:r></w:ins></w:p>"
            "</w:footnotes>",
            "word/activeX/activeX1.bin": b"\x00TextBox",
        })))
        for expected in ("docProps/custom.xml is still there", "holds the address jane.roe",
                         "image1.emf is not our placeholder", "image2.png is not our placeholder",
                         "dc:creator is not empty", "Company is not empty",
                         "holds a user profile path", "footnotes.xml has tracked changes",
                         "holds the phone number (202) 555-0199",
                         "activeX1.bin is a binary part"):
            self.assertIn(expected, found)
        self.assertEqual(public.problems(self.cleaned.data, ()), [])
        listed = package(**{"word/document.xml": "<w:document xmlns:w='w'><w:p><w:r><w:t>"
                            "Call 1-800-318-2596</w:t></w:r></w:p></w:document>"})
        self.assertNotIn("phone", "; ".join(public.problems(listed, (), ("1-800-318-2596",))))

    def test_refusals(self):
        cases = {
            "embeds word/fonts/font1.odttf": package(**{"word/fonts/font1.odttf": b"x"}),
            "has comments": package(**{"word/comments.xml": "<w:comments xmlns:w='w'/>"}),
            "mail merge": package(**{"word/settings.xml": "<w:settings xmlns:w='w'>"
                                     "<w:mailMerge/></w:settings>"}),
            "links to a local file": package(**{"word/_rels/header1.xml.rels": relationships(
                (f"{REL}/hyperlink", "file:///J:/drafts/x.docx", ' TargetMode="External"'))}),
            "does not contain the text to replace": package(**{
                "word/document.xml": "<w:document xmlns:w='w'/>"}),
            # The pattern matches, but not the text whose hash is listed.
            "does not contain the text to replace ": package(**{
                "word/document.xml": "<w:document xmlns:w='w'><w:p><w:r><w:t>Call John Doe at "
                "john.doe@agency.gov</w:t></w:r></w:p></w:document>"}),
            "has tracked changes": package(**{
                "word/footnotes.xml": "<w:footnotes xmlns:w='w'><w:del w:author='x'/>"
                "</w:footnotes>"}),
            "embeds word/activeX/activeX1.xml": package(**{
                "word/activeX/activeX1.xml": "<ax:ocx xmlns:ax='a'/>"}),
            "links to a local file or another scheme": package(**{
                "word/_rels/header1.xml.rels": relationships(
                    (f"{REL}/hyperlink", "../drafts/x.docx", ' TargetMode="External"'))}),
            "is not UTF-8": package(**{"word/settings.xml": b"<w:settings>\xff</w:settings>"}),
        }
        for message, data in cases.items():
            with self.subTest(message), self.assertRaises(public.Refused) as caught:
                public.clean(data, DOC)
            self.assertIn(message.strip(), str(caught.exception))

    def test_an_original_must_be_the_listed_file(self):
        folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, folder)
        with self.assertRaises(public.Refused) as caught:
            public.original(DOC, folder, fetch=False)
        self.assertIn("is missing", str(caught.exception))
        (folder / "test.docx").write_bytes(package())
        with self.assertRaises(public.Refused) as caught:
            public.original(DOC, folder, fetch=False)
        self.assertIn("SHA-256 differs", str(caught.exception))

    def test_the_sources_table_is_inserted_then_replaced(self):
        sources = "# Corpus sources\n\n## Documents\n\nrows\n\n## Reference renders\n\nmore\n"
        once = public.with_rows(sources, ["| b |", "| a |"])
        self.assertIn(public.TABLE_TITLE, once)
        self.assertLess(once.index("| a |"), once.index("| b |"))
        self.assertLess(once.index(public.TABLE_TITLE), once.index("## Reference renders"))
        twice = public.with_rows(once, ["| c |"])
        self.assertEqual(twice.count(public.TABLE_TITLE), 1)
        self.assertNotIn("| a |", twice)
        self.assertIn("| c |", twice)
        self.assertIn("## Reference renders\n\nmore", twice)

    def test_jpeg_and_gif_sizes_are_read(self):
        jpeg = b"\xff\xd8\xff\xe0\x00\x10" + bytes(14) + b"\xff\xc0\x00\x11\x08\x00\x20\x00\x40"
        self.assertEqual(public.image_size(jpeg + bytes(20)), (64, 32))
        self.assertEqual(public.image_size(b"GIF89a\x05\x00\x07\x00" + bytes(10)), (5, 7))
        self.assertIsNone(public.image_size(b"\x01\x00\x00\x00 EMF"))


class Committed(unittest.TestCase):
    def setUp(self):
        self.sources = (ROOT / "tests" / "corpus" / "SOURCES.md").read_text(encoding="utf-8")

    def test_every_listed_document_is_committed_clean(self):
        self.assertTrue(public.DOCUMENTS)
        for doc in public.DOCUMENTS:
            with self.subTest(doc.name):
                data = (BUSINESS / f"{doc.name}.docx").read_bytes()
                self.assertEqual(public.problems(data, doc.emails, doc.phones), [])

    def test_every_listed_document_has_its_sources_row(self):
        for doc in public.DOCUMENTS:
            with self.subTest(doc.name):
                rows = [r for r in self.sources.splitlines()
                        if r.startswith(f"| business/{doc.name}.docx |")]
                self.assertEqual(len(rows), 1)
                source = public.SOURCES[doc.source]
                for text in (doc.url, f"downloaded {doc.downloaded}", doc.sha256,
                             source.quote, source.statement_url):
                    self.assertIn(text.replace("|", "\\|"), rows[0])

    def test_no_document_is_unlisted(self):
        listed = {f"{doc.name}.docx" for doc in public.DOCUMENTS}
        committed = {p.name for p in BUSINESS.glob("*.docx")}
        self.assertEqual(committed, listed)

    def test_names_are_unique_and_plain(self):
        names = [doc.name for doc in public.DOCUMENTS]
        self.assertEqual(len(names), len(set(names)))
        for name in names:
            self.assertRegex(name, r"^(contract|report|form|letter)-[a-z0-9-]+$")

    def test_every_source_has_a_statement(self):
        for key, source in public.SOURCES.items():
            with self.subTest(key):
                self.assertTrue(source.quote and source.statement_url.startswith("https://"))
        self.assertEqual({doc.source for doc in public.DOCUMENTS} - set(public.SOURCES), set())


if __name__ == "__main__":
    unittest.main()
