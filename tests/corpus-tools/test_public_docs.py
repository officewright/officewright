"""Checks scripts/import-public-docs.py and the documents it committed to tests/corpus/business/.

The originals are not committed, so CI cannot clean them again; it checks instead that every
committed document holds no person's data and no picture of the original, and that its
SOURCES.md row names its URL, download date, original SHA-256 and licence statement. The
cleaning itself is tested on a small package made here.
"""

import dataclasses
import hashlib
import importlib.util
import io
import shutil
import tempfile
import struct
import sys
import unittest
import unittest.mock
import warnings
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


def grey_png(width, height, rows, level=9):
    """An 8-bit grey PNG of the given filtered rows, compressed at the given level."""

    def chunk(kind, body):
        crc = struct.pack(">I", zlib.crc32(kind + body))
        return struct.pack(">I", len(body)) + kind + body + crc

    header = struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(rows, level)) + chunk(b"IEND", b""))


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
            (f"{REL}/attachedTemplate", "file:///C:/Users/&lt;name&gt;/Normal.dotm",
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


# What clean() and problems() say of a relationship to a local file, another scheme or a part
# the package does not hold.
LINK = "links to a local file, another scheme or a missing part"


def with_entry(data, name, body):
    """The package with one more entry, which zipfile writes even when its name is taken."""
    buffer = io.BytesIO(data)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", UserWarning)  # zipfile's "Duplicate name"
        with zipfile.ZipFile(buffer, "a") as archive:
            archive.writestr(name, body)
    return buffer.getvalue()


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

    def test_external_links_to_local_files_are_refused_however_they_are_written(self):
        # #331: relationships in single quotes, or with a character reference in TargetMode,
        # escaped the regular expressions of edit_relationships, which kept the link.
        spellings = {
            "single quotes": f"<Relationship Id='rId9' Type='{REL}/hyperlink' "
            "Target='file:///D:/private/report.docx' TargetMode='External'/>",
            "a character reference in TargetMode": f'<Relationship Id="rId9" Type="{REL}/'
            'hyperlink" Target="D:/private/report.docx" TargetMode="&#69;xternal"/>',
            "spaces around the equals signs": f"<Relationship Id = 'rId9' Type = '{REL}/"
            "hyperlink' Target = '\\\\server\\private\\x.docx' TargetMode = 'External'/>",
            # Not a hyperlink: a picture linked from a local file.
            "an image": f'<Relationship Id="rId9" Type="{REL}/image" '
            'Target="file:///D:/private/logo.png" TargetMode="External"/>',
            # Internal relationships name parts, so a target with a scheme, a drive or a
            # server names a local file however the mode is left out.
            "no TargetMode": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="file:///D:/private/report.docx"/>',
            "Internal and a drive": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="D:\\private\\report.docx" TargetMode="Internal"/>',
            "Internal and a server": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="\\\\server\\private\\x.docx" TargetMode="Internal"/>',
            "no TargetMode and two slashes": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="//server/private/x.docx"/>',
            # xsd:anyURI collapses whitespace, so a target is read without it at its ends.
            "a space before a drive": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target=" D:/private/report.docx"/>',
            "a tab before file:": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="&#9;file:///D:/private/report.docx"/>',
            "a line feed before a server": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="&#10;\\\\server\\private\\x.docx"/>',
            # No part name has a backslash.
            "backslashes after the start": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="..\\..\\private\\report.docx"/>',
            "one leading backslash": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="\\private\\report.docx"/>',
            "a slash and a backslash": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="/\\server\\private\\x.docx"/>',
            "a carriage return before a drive": f'<Relationship Id="rId9" Type="{REL}/'
            'hyperlink" Target="&#13;D:/private/report.docx"/>',
            "one leading backslash alone": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="\\private/report.docx"/>',
            # Percent-encoded, as urllib.parse.quote writes a Windows path: still no part.
            "a percent-encoded profile path": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="C%3A%5CUsers%5Cprivate%5Cmemo.docx"/>',
            "a percent-encoded server": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="%5C%5Cfs01%5Cprivate%5Cx.docx"/>',
            "percent-encoded slashes": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="%2F%2Fserver/private/x.docx"/>',
            "a percent-encoded drive letter": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="%44:/private/x.docx"/>',
            # Out of the package, with a query, or led by a no-break space: no part.
            "a way out of the package": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="../../private/report.docx"/>',
            "a query": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="document.xml?private"/>',
            "a no-break space": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="&#160;D:/private/report.docx"/>',
            # A local path that climbs back to a part the package holds is still a local path.
            "a drive climbing back": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="D:/private/../../settings.xml"/>',
            "file: climbing back": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="file:///D:/private/../../../settings.xml" TargetMode="Internal"/>',
            "a server climbing back": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="file://fs01/private/../../../settings.xml" TargetMode="Internal"/>',
            "backslashes climbing back": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="\\\\fs01\\private\\memo.docx/../settings.xml"/>',
            "a fragment climbing back": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="x#D:/private/report.docx/../../../settings.xml"/>',
            "a linked picture climbing back": f'<Relationship Id="rId9" Type="{REL}/image" '
            'Target="file:///D:/private/../../../media/image2.png"/>',
            "an encoded path climbing back": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="private%2F..%2F..%2Fsettings.xml"/>',
            # Each rule of a part reference alone: a folder climbed back from, which can be a
            # user's, a server named like a folder, an empty segment.
            "folders climbed back from": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="Users/private/../../settings.xml"/>',
            "two slashes before a part": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="//word/settings.xml"/>',
            "an empty segment": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="settings.xml/"/>',
            # A part the package does not hold.
            "a part that is not there": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="private/report.docx"/>',
            # One good relationship before it does not hide it.
            "after a web link": f'<Relationship Id="rId8" Type="{REL}/hyperlink" '
            'Target="https://example.gov/" TargetMode="External"/>'
            f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="file:///D:/private/report.docx" TargetMode="External"/>',
            # A web address further on does not make the target one.
            "a web address after a local file": f'<Relationship Id="rId9" Type="{REL}/hyperlink" '
            'Target="file:///D:/private/x.html?u=https://example.gov/" TargetMode="External"/>',
        }
        for label, element in spellings.items():
            rels = f'<?xml version="1.0"?><Relationships xmlns="{PKG}">{element}</Relationships>'
            data = package(**{"word/_rels/header1.xml.rels": rels})
            with self.subTest(label):
                with self.assertRaises(public.Refused) as caught:
                    public.clean(data, DOC)
                self.assertIn("header1.xml.rels", str(caught.exception))
                self.assertIn(LINK, str(caught.exception))
                self.assertNotIn("private", str(caught.exception))
                self.assertIn(f"word/_rels/header1.xml.rels {LINK}",
                              public.problems(data))
        # A target mode OPC does not allow is refused, whatever the target: officewright-opc
        # refuses the package.
        for mode in ("external", "EXTERNAL", " External ", "External&#9;"):
            rels = (f'<?xml version="1.0"?><Relationships xmlns="{PKG}"><Relationship Id="rId9" '
                    f'Type="{REL}/hyperlink" Target="file:///D:/private/x.docx" '
                    f'TargetMode="{mode}"/></Relationships>')
            data = package(**{"word/_rels/header1.xml.rels": rels})
            with self.subTest(mode=mode):
                with self.assertRaises(public.Refused) as caught:
                    public.clean(data, DOC)
                self.assertIn("header1.xml.rels has a target mode other than Internal or External",
                              str(caught.exception))
                self.assertNotIn("private", str(caught.exception))
                self.assertIn("word/_rels/header1.xml.rels has a target mode other than Internal "
                              "or External", public.problems(data))
        # What the cleaning leaves of an attached template is refused, whatever its address:
        # a personal site names the person. So is an attribute in a namespace, which OPC
        # does not allow.
        others = {
            "keeps an attached template": f'<Relationship Id="rId9" Type="{REL}/attached&#84;'
            'emplate" Target="https://agency-my.example.gov/personal/private/T.dotx" '
            'TargetMode="External"/>',
            "keeps an attached template ": f'<Relationship Id="rId9" Type="{REL}/'
            'attachedTemplate" Target="https://agency-my.example.gov/personal/private/T.dotx" '
            'TargetMode="External"></Relationship>',
            "keeps an attached template  ": f'<Relationship Id="rId9" Type="{REL}/'
            'attachedTemplate&#9;" Target="https://agency-my.example.gov/personal/private/T.dotx" '
            'TargetMode="External"/>',
            "has a relationship attribute in a namespace": f'<Relationship xmlns:p="{PKG}" '
            f'Id="rId9" Type="{REL}/hyperlink" Target="../private/x.docx" '
            'p:TargetMode="External"/>',
        }
        for message, element in others.items():
            rels = f'<?xml version="1.0"?><Relationships xmlns="{PKG}">{element}</Relationships>'
            data = package(**{"word/_rels/header1.xml.rels": rels})
            with self.subTest(message):
                with self.assertRaises(public.Refused) as caught:
                    public.clean(data, DOC)
                self.assertIn(f"header1.xml.rels {message.strip()}", str(caught.exception))
                self.assertNotIn("private", str(caught.exception))
                self.assertIn(f"word/_rels/header1.xml.rels {message.strip()}",
                              public.problems(data))
        # An embedded object at a local path is refused as a link, without its path.
        for target in ("D:/private/x.bin", "/D:/private/x.bin", "%5C%5Cserver%5Cprivate.bin",
                       "C:/../private/x.bin"):
            rels = (f'<?xml version="1.0"?><Relationships xmlns="{PKG}"><Relationship Id="rId9" '
                    f'Type="{REL}/oleObject" Target="{target}"/></Relationships>')
            with self.subTest(target), self.assertRaises(public.Refused) as caught:
                public.clean(package(**{"word/_rels/header1.xml.rels": rels}), DOC)
            self.assertIn(LINK, str(caught.exception))
            self.assertNotIn("private", str(caught.exception))
        # A relationships part named in capitals is read too.
        rels = (f'<?xml version="1.0"?><Relationships xmlns="{PKG}"><Relationship Id="rId9" '
                f'Type="{REL}/hyperlink" Target="file:///D:/private/x.docx" '
                'TargetMode="External"/></Relationships>')
        self.assertIn(f"word/_rels/footer1.xml.RELS {LINK}",
                      public.problems(package(**{"word/_rels/footer1.xml.RELS": rels})))
        # A web address in single quotes stays, and so does an internal target.
        rels = (f'<?xml version="1.0"?><Relationships xmlns="{PKG}">'
                f"<Relationship Id='rId8' Type='{REL}/hyperlink' Target='https://example.gov/'"
                " TargetMode='External'/></Relationships>")
        cleaned = public.clean(package(**{"word/_rels/header1.xml.rels": rels}), DOC)
        with zipfile.ZipFile(io.BytesIO(cleaned.data)) as archive:
            self.assertIn("Target='https://example.gov/'",
                          archive.read("word/_rels/header1.xml.rels").decode())
        # A part named as part names compare (percent-decoded, without regard to case), and
        # with the whitespace xsd:anyURI collapses, is a part.
        for target in ("Settings.XML", "setting%73.xml", "&#9;settings.xml ",
                       "&#13;&#10;settings.xml", "./settings.xml", "../word/settings.xml",
                       "/word/settings.xml"):
            rels = (f'<?xml version="1.0"?><Relationships xmlns="{PKG}"><Relationship Id="rId1" '
                    f'Type="{REL}/settings" Target="{target}"/></Relationships>')
            data = package(**{"word/_rels/header1.xml.rels": rels})
            with self.subTest(target):
                self.assertEqual(public.problems(public.clean(data, DOC).data), [])
        rels = (f'<?xml version="1.0"?><Relationships xmlns="{PKG}">'
                f"<Relationship Id='rId8' Type='{REL}/hyperlink' Target='HTTPS://example.gov/'"
                " TargetMode='External'/></Relationships>")
        # Each character a part reference may not have is refused even where the package
        # holds a part of that name, so the path is refused, not kept.
        for name in ("D:/private/report.xml", "private\\report.xml", "report.xml?private",
                     "report.xml#private"):
            target = name.replace("?", "%3F").replace("#", "%23")
            held = (f'<?xml version="1.0"?><Relationships xmlns="{PKG}"><Relationship Id="rId9" '
                    f'Type="{REL}/footer" Target="{target}"/></Relationships>')
            data = package(**{"word/_rels/header1.xml.rels": held,
                              f"word/{name}": "<w:ftr xmlns:w='w'/>"})
            with self.subTest(name):
                self.assertIn(f"word/_rels/header1.xml.rels {LINK}", public.problems(data))
        # A part whose name the package percent-encodes, named decoded.
        footer = (f'<?xml version="1.0"?><Relationships xmlns="{PKG}"><Relationship Id="rId1" '
                  f'Type="{REL}/footer" Target="my part.xml"/></Relationships>')
        data = package(**{"word/_rels/header1.xml.rels": footer,
                          "word/my%20part.xml": "<w:ftr xmlns:w='w'/>"})
        self.assertEqual(public.problems(public.clean(data, DOC).data), [])
        # An embedded object the package holds is refused by its relationship, whatever its
        # name or the whitespace around its type.
        for name, kind in (("ole%20object.bin", f"{REL}/oleObject"),
                           ("data.bin", f"{REL}/oleObject&#9;")):
            embedded = (f'<?xml version="1.0"?><Relationships xmlns="{PKG}"><Relationship '
                        f'Id="rId9" Type="{kind}" Target="{name}"/></Relationships>')
            data = package(**{"word/_rels/header1.xml.rels": embedded, f"word/{name}": b"\x00"})
            with self.subTest(name), self.assertRaises(public.Refused) as caught:
                public.clean(data, DOC)
            self.assertIn(f"embeds word/{name}", str(caught.exception))
        # The edit removes an attached template whose type ends in a space.
        settings = parts_of(package())["word/_rels/settings.xml.rels"].replace(
            b'/attachedTemplate"', b'/attachedTemplate "')
        cleaned = public.clean(package(**{"word/_rels/settings.xml.rels": settings}), DOC)
        self.assertNotIn(b"attachedTemplate",
                         parts_of(cleaned.data)["word/_rels/settings.xml.rels"])
        # The scheme in capitals is a web address too.
        cleaned = public.clean(package(**{"word/_rels/header1.xml.rels": rels}), DOC)
        self.assertEqual(public.problems(cleaned.data), [])
        # And with the whitespace xsd:anyURI collapses before it.
        rels = rels.replace("HTTPS://example.gov/", "&#9; https://example.gov/")
        cleaned = public.clean(package(**{"word/_rels/header1.xml.rels": rels}), DOC)
        self.assertEqual(public.problems(cleaned.data), [])

    def test_a_picture_target_in_single_quotes_is_renamed_too(self):
        rels = (f'<?xml version="1.0"?><Relationships xmlns="{PKG}">'
                f"<Relationship Id='rId1' Type='{REL}/image' Target='media/image1.emf'/>"
                "</Relationships>")
        cleaned = public.clean(package(**{"word/_rels/header1.xml.rels": rels}), DOC)
        with zipfile.ZipFile(io.BytesIO(cleaned.data)) as archive:
            header = archive.read("word/_rels/header1.xml.rels").decode()
        self.assertIn("Target='media/image1.png'", header)
        self.assertNotIn("image1.emf", header)

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

    def test_the_settings_note_names_what_was_removed(self):
        def note(settings):
            cleaned = public.clean(package(**{"word/settings.xml": settings}), DOC)
            return [c for c in cleaned.changes if c.startswith("settings:")]

        variables = '<w:settings xmlns:w="w"><w:docVars><w:docVar/></w:docVars></w:settings>'
        self.assertEqual(note(variables), ["settings: document variables removed"])
        self.assertEqual(note('<w:settings xmlns:w="w"><w:zoom/></w:settings>'), [])
        both = "settings: attached template and document variables removed"
        self.assertEqual(self.cleaned.changes.count(both), 1)

    def test_label_information_is_dropped(self):
        cleaned = public.clean(package(**{"docMetadata/LabelInfo.xml": "<labelList/>"}), DOC)
        self.assertNotIn("docMetadata/LabelInfo.xml", parts_of(cleaned.data))
        self.assertIn("removed: custom XML parts, custom properties, sensitivity label, thumbnail",
                      cleaned.changes)

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
            "word/activeX/activeX1.xml": "<ax:ocx xmlns:ax='ax'/>",
        })))
        for expected in ("docProps/custom.xml is still there", "holds the address jane.roe",
                         "image1.emf is not our placeholder", "image2.png is not our placeholder",
                         "dc:creator is not empty", "Company is not empty",
                         "holds a user profile path", "footnotes.xml has tracked changes",
                         "holds the phone number (202) 555-0199",
                         "activeX1.bin is a binary part", "activeX1.xml is embedded"):
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
            "links to a local file, another scheme or a missing part": package(**{
                "word/_rels/header1.xml.rels": relationships(
                    (f"{REL}/hyperlink", "../drafts/x.docx", ' TargetMode="External"'))}),
            "is not UTF-8": package(**{"word/settings.xml": b"<w:settings>\xff</w:settings>"}),
        }
        for message, data in cases.items():
            with self.subTest(message), self.assertRaises(public.Refused) as caught:
                public.clean(data, DOC)
            self.assertIn(message.strip(), str(caught.exception))

    def test_private_parts_are_found_by_what_they_are_not_by_their_usual_name(self):
        # Parts that hold people's data, named only by the relationships to them or by their
        # content type, as OPC finds them (#375).
        ms = "http://schemas.microsoft.com/office"
        custom_type = "application/vnd.openxmlformats-officedocument.custom-properties+xml"
        core = ("<cp:coreProperties xmlns:cp='c' xmlns:dc='d'><dc:title>Plan</dc:title>"
                "<dc:creator>Jane Roe</dc:creator></cp:coreProperties>")
        unusual = {
            "_rels/.rels": relationships(
                (f"{REL}/officeDocument", "word/document.xml", ""),
                (f"{PKG}/metadata/thumbnail", "look.jpeg", ""),
                (f"{REL}/custom-properties", "meta/more.xml", ""),
                (f"{PKG}/metadata/core-properties", "meta/props1.xml", ""),
                (f"{REL}/extended-properties", "meta/app2.xml", ""),
                # A part whose usual name says it is dropped stays dropped, whatever the
                # relationship to it says.
                (f"{PKG}/metadata/core-properties", "docProps/custom.xml", ""),
                (f"{ms}/2020/02/relationships/classificationlabels", "meta/labels.xml", ""),
            ),
            "[Content_Types].xml": (
                '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
                '<Default Extension="xml" ContentType="application/xml"/>'
                '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.'
                'relationships+xml"/><Default Extension="jpeg" ContentType="image/jpeg"/>'
                f'<Override PartName="/meta/more.xml" ContentType="{custom_type}"/>'
                # Found by its content type alone: no relationship names it.
                f'<Override PartName="/meta/%41ttributes.xml" ContentType="{custom_type.upper()}"/>'
                '<Override PartName="/meta/sens.xml" '
                'ContentType="application/vnd.ms-office.classificationlabels+xml"/></Types>'
            ),
            "docProps/core.xml": None, "docProps/app.xml": None, "docProps/thumbnail.jpeg": None,
            "customXml/item1.xml": None, "customXml/itemProps1.xml": None,
            "customXml/_rels/item1.xml.rels": None, "word/settings.xml": None,
            "word/_rels/settings.xml.rels": None,
            "meta/props1.xml": core,
            "meta/app2.xml": "<Properties><Company>Roe Consulting</Company></Properties>",
            "meta/more.xml": "<Properties><property>Jane Roe</property></Properties>",
            "meta/Attributes.xml": "<Properties><property>Jane Roe</property></Properties>",
            "meta/labels.xml": "<labelList/>",
            "meta/sens.xml": "<labelList/>",
            "look.jpeg": b"\xff\xd8\xff\xd9",
            "data/item1.xml": "<root><owner>Jane Roe</owner></root>",
            "data/props1.xml": "<ds:datastoreItem xmlns:ds='x'/>",
            "data/_rels/item1.xml.rels": relationships(
                (f"{REL}/customXmlProps", "props1.xml", "")),
            "word/persons.xml": "<w15:people xmlns:w15='w'><w15:person w15:author='Jane Roe'/>"
            "</w15:people>",
            "word/prefs.xml": "<w:settings xmlns:w='w'><w:docVars><w:docVar w:name='owner' "
            "w:val='Jane Roe'/></w:docVars><w:zoom/></w:settings>",
            # The usual names in another case.
            "DocProps/Thumbnail.png": b"\x89PNG",
            "CustomXml/item2.xml": "<root><owner>Jane Roe</owner></root>",
            "word/_rels/document.xml.rels": relationships(
                (f"{REL}/image", "media/image1.emf", ""),
                (f"{REL}/image", "media/image2.png", ""),
                # Removed in any spelling of the part's name.
                (f"{REL}/customXml", "../Data/%69tem1.xml", ""),
                (f"{REL}/settings", "prefs.xml", ""),
                (f"{REL}/header", "header1.xml", ""),
                (f"{ms}/2011/relationships/people", "persons.xml", ""),
                (f"{REL}/hyperlink", "mailto:jane.roe@agency.gov", ' TargetMode="External"'),
            ),
        }
        found = "; ".join(public.problems(package(**unusual)))
        for expected in ("meta/props1.xml: dc:creator is not empty",
                         "meta/app2.xml: Company is not empty", "meta/more.xml is still there",
                         "meta/Attributes.xml is still there", "meta/labels.xml is still there",
                         "meta/sens.xml is still there",
                         "look.jpeg is still there", "data/item1.xml is still there",
                         "data/props1.xml is still there", "data/_rels/item1.xml.rels is still "
                         "there", "word/persons.xml is still there", "DocProps/Thumbnail.png "
                         "is still there", "CustomXml/item2.xml is still there",
                         "docProps/custom.xml is still there"):
            self.assertIn(expected, found)
        cleaned = public.clean(package(**unusual), DOC)
        parts = parts_of(cleaned.data)
        for name in ("meta/more.xml", "meta/Attributes.xml", "meta/labels.xml", "meta/sens.xml",
                     "look.jpeg",
                     "data/item1.xml", "data/props1.xml", "data/_rels/item1.xml.rels",
                     "word/persons.xml", "DocProps/Thumbnail.png", "CustomXml/item2.xml",
                     "docProps/custom.xml"):
            with self.subTest(name):
                self.assertNotIn(name, parts)
        self.assertIn("<dc:creator></dc:creator>", parts["meta/props1.xml"].decode())
        self.assertIn("<Company></Company>", parts["meta/app2.xml"].decode())
        self.assertNotIn("docVars", parts["word/prefs.xml"].decode())
        rels = parts["_rels/.rels"].decode() + parts["word/_rels/document.xml.rels"].decode()
        for target in ("look.jpeg", "more.xml", "labels.xml", "tem1.xml", "persons.xml",
                       "docProps/custom.xml"):
            self.assertNotIn(target, rels)
        self.assertNotIn("ttributes.xml", parts["[Content_Types].xml"].decode())
        self.assertNotIn("more.xml", parts["[Content_Types].xml"].decode())
        self.assertIn("removed: comment authors, custom XML parts, custom properties, "
                      "sensitivity label, thumbnail", cleaned.changes)
        self.assertEqual(public.problems(cleaned.data), [])

        # Comments, by the relationship to them or by their content type alone.
        comments = "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"
        by_type = package(**{
            "word/remarks.xml": "<w:comments xmlns:w='w'/>",
            "[Content_Types].xml": unusual["[Content_Types].xml"].replace(
                "</Types>", f'<Override PartName="/word/remarks.xml" ContentType="{comments}"/>'
                "</Types>")})
        by_relationship = package(**{
            "word/remarks.xml": "<w:comments xmlns:w='w'/>",
            "word/_rels/header1.xml.rels": relationships((f"{REL}/comments", "remarks.xml", ""))})
        for name, data in (("content type", by_type), ("relationship", by_relationship)):
            with self.subTest(name):
                self.assertIn("word/remarks.xml holds comments", public.problems(data))
                with self.assertRaises(public.Refused) as caught:
                    public.clean(data, DOC)
                self.assertIn("has comments", str(caught.exception))

    def test_each_way_finds_each_part_with_people_s_data_alone(self):
        # Every way part_roles finds a part, each with a part that only it finds (#375).
        strict = "http://purl.oclc.org/ooxml/officeDocument/relationships"
        ms = "http://schemas.microsoft.com/office"
        ml = "application/vnd.openxmlformats-officedocument"
        core = ("<cp:coreProperties xmlns:cp='c' xmlns:dc='d'><dc:creator>Jane Roe</dc:creator>"
                "</cp:coreProperties>")
        app = "<Properties><Company>Roe Consulting</Company></Properties>"
        settings = ("<w:settings xmlns:w='w'><w:docVars><w:docVar w:name='owner' "
                    "w:val='Jane Roe'/></w:docVars></w:settings>")
        dropped = "<root><owner>Jane Roe</owner></root>"
        comments = "<w:comments xmlns:w='w'/>"
        roles = {"core": core, "app": app, "settings": settings, "custom": dropped,
                 "thumbnail": dropped, "customxml": dropped, "people": dropped,
                 "label": dropped, "comments": comments}
        relationship_types = [
            (f"{PKG}/metadata/core-properties", "core"),
            (f"{REL}/extended-properties", "app"), (f"{strict}/extendedProperties", "app"),
            (f"{REL}/custom-properties", "custom"), (f"{strict}/customProperties", "custom"),
            (f"{PKG}/metadata/thumbnail", "thumbnail"),
            (f"{REL}/customXml", "customxml"), (f"{REL}/customXmlProps", "customxml"),
            (f"{ms}/2011/relationships/people", "people"),
            (f"{ms}/2020/02/relationships/classificationlabels", "label"),
            (f"{REL}/comments", "comments"), (f"{strict}/comments", "comments"),
            (f"{REL}/settings", "settings"),
        ]
        content_types = [
            ("application/vnd.openxmlformats-package.core-properties+xml", "core"),
            (f"{ml}.extended-properties+xml", "app"),
            (f"{ml}.custom-properties+xml", "custom"),
            (f"{ml}.customXmlProperties+xml", "customxml"),
            (f"{ml}.wordprocessingml.people+xml", "people"),
            (f"{ml}.wordprocessingml.comments+xml", "comments"),
            (f"{ml}.wordprocessingml.settings+xml", "settings"),
            ("application/vnd.ms-office.classificationlabels+xml", "label"),
        ]
        names = [("DocProps/Core.xml", "core"), ("DOCPROPS/app.xml", "app"),
                 ("docProps/Custom.xml", "custom"), ("word/People.xml", "people"),
                 ("word/Settings.xml", "settings"), ("Word/comments.xml", "comments"),
                 ("docProps/Thumbnail.jpeg", "thumbnail"), ("CustomXML/item1.xml", "customxml"),
                 ("docMetaData/LabelInfo.xml", "label")]
        types_start = ('<Types xmlns="http://schemas.openxmlformats.org/package/2006/'
                       'content-types"><Default Extension="xml" ContentType="application/xml"/>'
                       '<Default Extension="rels" ContentType="application/vnd.openxmlformats-'
                       'package.relationships+xml"/>')
        plain = public.PublicDoc("plain", "nara", "https://www.example.gov/p.docx",
                                 "2026-10-19", "0" * 64)

        def minimal(types="", rels=(), **more):
            parts = {
                "[Content_Types].xml": types_start + types + "</Types>",
                "_rels/.rels": relationships((f"{REL}/officeDocument", "word/document.xml", ""),
                                             *rels),
                "word/document.xml": "<w:document xmlns:w='w'><w:body/></w:document>",
            }
            parts.update(more)
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w") as archive:
                for name, data in parts.items():
                    archive.writestr(name, data)
            return buffer.getvalue()

        cases = (
            [(f"relationship {kind}", role, "meta/p.xml",
              minimal(rels=[(kind, " meta/p.xml ", "")], **{"meta/p.xml": roles[role]}))
             for kind, role in relationship_types]
            + [(f"override {kind}", role, "meta/p.xml",
                minimal(f'<Override PartName=" /meta/%70.xml " ContentType="{kind.upper()}"/>',
                        **{"meta/p.xml": roles[role]}))
               for kind, role in content_types]
            + [(f"default {kind}", role, "meta/p.part",
                minimal(f'<Default Extension="PART" ContentType="{kind}"/>',
                        **{"meta/p.part": roles[role]}))
               for kind, role in content_types]
            + [(f"name {name}", role, name, minimal(**{name: roles[role]}))
               for name, role in names]
        )
        for label, role, name, data in cases:
            with self.subTest(label):
                found = public.problems(data)
                if role == "comments":
                    self.assertIn(f"{name} holds comments", found)
                    with self.assertRaises(public.Refused) as caught:
                        public.clean(data, plain)
                    self.assertIn("has comments", str(caught.exception))
                    continue
                cleaned = parts_of(public.clean(data, plain).data)
                if role in ("core", "app"):
                    if name.endswith(".xml"):
                        self.assertIn(f"{name}: "
                                      + ("dc:creator" if role == "core" else "Company")
                                      + " is not empty", found)
                    self.assertNotIn(b"Roe", cleaned[name])
                elif role == "settings":
                    self.assertNotIn(b"docVars", cleaned[name])
                else:
                    self.assertIn(f"{name} is still there", found)
                    self.assertNotIn(name, cleaned)
                self.assertEqual([p for p in public.problems(public.clean(data, plain).data)
                                  if "binary part" not in p], [])

    def test_a_part_with_several_roles_gets_the_cleaning_of_each(self):
        # The core properties named a second time as extended properties, settings or both
        # get the cleaning of each role (review of #378).
        extended = (f"{REL}/extended-properties", "docProps/core.xml", "")
        settings = (f"{REL}/settings", "../docProps/core.xml", "")
        for kind, more_root, more_body in (("extended", [extended], []),
                                           ("settings", [], [settings]),
                                           ("both", [extended], [settings])):
            root = [(f"{REL}/officeDocument", "word/document.xml", ""),
                    (f"{PKG}/metadata/core-properties", "docProps/core.xml", ""),
                    (f"{REL}/extended-properties", "docProps/app.xml", "")]
            body = [(f"{REL}/image", "media/image1.emf", ""),
                    (f"{REL}/image", "media/image2.png", ""),
                    (f"{REL}/settings", "settings.xml", ""),
                    (f"{REL}/header", "header1.xml", ""),
                    (f"{REL}/hyperlink", "mailto:jane.roe@agency.gov", ' TargetMode="External"')]
            root += more_root
            body += more_body
            data = package(**{"_rels/.rels": relationships(*root),
                              "word/_rels/document.xml.rels": relationships(*body),
                              # Read as extended properties too, so their people go as well.
                              "docProps/core.xml": "<cp:coreProperties xmlns:cp='c' "
                              "xmlns:dc='d'><dc:creator>Jane Roe</dc:creator><cp:lastModifiedBy>"
                              "jroe</cp:lastModifiedBy><Company>Roe Consulting</Company>"
                              "<w:docVars><w:docVar w:val='Jane Roe'/></w:docVars>"
                              "</cp:coreProperties>",
                              "docProps/custom.xml": None, "docProps/thumbnail.jpeg": None,
                              "customXml/item1.xml": None, "customXml/itemProps1.xml": None,
                              "customXml/_rels/item1.xml.rels": None})
            with self.subTest(kind):
                found = public.problems(data)
                self.assertIn("docProps/core.xml: dc:creator is not empty", found)
                self.assertIn("docProps/core.xml: cp:lastModifiedBy is not empty", found)
                if more_root:
                    self.assertIn("docProps/core.xml: Company is not empty", found)
                cleaned = public.clean(data, DOC)
                core = parts_of(cleaned.data)["docProps/core.xml"].decode()
                self.assertIn("<dc:creator></dc:creator>", core)
                self.assertIn("<cp:lastModifiedBy></cp:lastModifiedBy>", core)
                if more_root:
                    self.assertIn("<Company></Company>", core)
                if more_body:
                    self.assertNotIn("docVars", core)
                self.assertEqual(public.problems(cleaned.data), [])

    def test_relationships_to_dropped_and_renamed_parts_go_in_any_spelling(self):
        data = package(**{
            "word/_rels/document.xml.rels": relationships(
                (f"{REL}/image", " media/IMAGE1.emf ", ""),
                (f"{REL}/image", "media/%69mage2.png", ""),
                (f"{REL}/image", "media/image3.jpeg", ""),
                (f"{REL}/customXml", "../customXml/item1.xml", ""),
                (f"{REL}/settings", "settings.xml", ""),
                (f"{REL}/header", "header1.xml", ""),
                (f"{REL}/hyperlink", "mailto:jane.roe@agency.gov", ' TargetMode="External"')),
            # The .rels part of a dropped part, in another case.
            "customXml/_rels/item1.xml.rels": None,
            "word/media/Image3.jpeg": b"\xff\xd8\xff\xd9",
            "data/_rels/Item1.xml.rels": relationships((f"{REL}/customXmlProps", "p.xml", "")),
            "data/item1.xml": "<root>Jane Roe</root>",
            "data/p.xml": "<x/>",
            "_rels/.rels": relationships(
                (f"{REL}/officeDocument", "word/document.xml", ""),
                (f"{REL}/customXml", "data/item1.xml", ""),
                (f"{PKG}/metadata/core-properties", "docProps/core.xml", ""),
                (f"{REL}/extended-properties", "docProps/app.xml", ""))})
        parts = parts_of(public.clean(data, DOC).data)
        rels = parts["word/_rels/document.xml.rels"].decode()
        # Each target names the renamed picture, which is the only part of its name.
        self.assertIn('Target=" media/image1.png"', rels)
        self.assertIn('Target="media/image2.png"', rels)
        self.assertIn('Target="media/Image3.png"', rels)
        self.assertNotIn(".emf", rels)
        self.assertNotIn("%69", rels)
        self.assertNotIn("customXml", rels)
        for name in ("data/item1.xml", "data/p.xml", "data/_rels/Item1.xml.rels"):
            self.assertNotIn(name, parts)

    def test_a_default_content_type_gives_no_role_to_overridden_parts(self):
        # A Default types only the parts no Override types, and the content types stream is
        # no part (ECMA-376 Part 2, §10.1.2).
        custom = "application/vnd.openxmlformats-officedocument.custom-properties+xml"
        app = "application/vnd.openxmlformats-officedocument.extended-properties+xml"
        types = parts_of(package())["[Content_Types].xml"].decode().replace(
            '<Default Extension="xml" ContentType="application/xml"/>',
            f'<Default Extension="xml" ContentType="{custom}"/>'
            # An Override whose type is a role of its own keeps the Default off the part too.
            f'<Override PartName="/docProps/app.xml" ContentType="{app}"/>')
        parts = parts_of(public.clean(package(**{"[Content_Types].xml": types}), DOC).data)
        self.assertIn("word/document.xml", parts)
        self.assertIn("<Company></Company>", parts["docProps/app.xml"].decode())
        self.assertIn("[Content_Types].xml", parts)
        self.assertNotIn("docProps/core.xml", parts)

    def test_overrides_of_dropped_parts_go_in_any_quoting(self):
        types = parts_of(package())["[Content_Types].xml"].decode().replace(
            '<Override PartName="/docProps/custom.xml" ',
            "<Override PartName=' /docProps/custom.xml ' ")
        cleaned = parts_of(public.clean(package(**{"[Content_Types].xml": types}), DOC).data)
        self.assertNotIn("custom.xml", cleaned["[Content_Types].xml"].decode())

    def test_an_external_relationship_gives_no_role(self):
        data = package(**{"word/_rels/header1.xml.rels": relationships(
            (f"{REL}/custom-properties", "../word/document.xml", ' TargetMode="External"'))})
        self.assertNotIn("word/document.xml is still there", public.problems(data))

    def test_a_picture_that_is_also_a_part_to_drop_is_not_counted_as_replaced(self):
        data = package(**{"_rels/.rels": relationships(
            (f"{REL}/officeDocument", "word/document.xml", ""),
            (f"{PKG}/metadata/thumbnail", "word/media/image2.png", ""),
            (f"{REL}/custom-properties", "docProps/custom.xml", ""),
            (f"{PKG}/metadata/core-properties", "docProps/core.xml", ""),
            (f"{REL}/extended-properties", "docProps/app.xml", "")),
            "docProps/thumbnail.jpeg": None})
        cleaned = public.clean(data, DOC)
        self.assertNotIn("word/media/image2.png", parts_of(cleaned.data))
        self.assertIn("2 pictures replaced", "; ".join(cleaned.changes))

    def test_embedded_parts_are_found_by_what_they_are_not_by_their_usual_name(self):
        ms = "http://schemas.microsoft.com/office/2006/relationships"
        types = (
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Default Extension="ocx" ContentType="application/vnd.ms-office.activeX"/>'
            '<Override PartName="/word/controls/control1.xml" '
            'ContentType="application/vnd.ms-office.activeX+xml"/>'
            "<Override PartName='/word/Controls/Upper.xml' "
            "ContentType='APPLICATION/VND.MS-OFFICE.ACTIVEX+XML'/>"
            '<Override PartName="/word/styles.xml" ContentType="application/vnd.'
            'openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>'
        )
        document_rels = relationships(
            (f"{REL}/control", "controls/control2.xml", ""),
            (f"{REL}/oleObject", "objects/oleObject1.bin", ""),
            (f"{REL}/package", "objects/Sheet1.xlsx", ""),
            (f"{REL}/control", "https://example.gov/control.xml", ' TargetMode="External"'),
            (f"{REL}/styles", "styles.xml", ""),
            (f"{REL}/control", "controls/missing.xml", ""),
        ).replace("</Relationships>", f"<Relationship Id='rId9' Type='{REL}/control' "
                  "Target='/word/controls/control3.xml'/></Relationships>")
        parts = {
            "[Content_Types].xml": types.encode(),
            "word/_rels/document.xml.rels": document_rels.encode(),
            "word/_rels/fontTable.xml.rels": relationships(
                (f"{REL}/font", "extras/font1.odttf", "")).encode(),
            "word/controls/_rels/control2.xml.rels": relationships(
                (f"{ms}/activeXControlBinary", "control2.bin", "")).encode(),
            "word/ActiveX/activeX1.xml": b"<ax:ocx xmlns:ax='a'/>",  # the other case
            "word/Fonts/font2.odttf": b"x",
            "word/controls/control1.xml": b"<ax:ocx xmlns:ax='a'/>",  # by its content type
            "word/controls/upper.xml": b"<ax:ocx xmlns:ax='a'/>",  # named in another case
            "word/controls/control2.xml": b"<ax:ocx xmlns:ax='a'/>",  # by its relationship
            "word/controls/control2.bin": b"\x00",
            "word/controls/control3.xml": b"<ax:ocx xmlns:ax='a'/>",
            "word/controls/legacy.ocx": b"\x00",  # by the content type of its extension
            "word/extras/font1.odttf": b"x",
            "word/objects/oleObject1.bin": b"\x00",
            "word/objects/Sheet1.xlsx": b"PK",
            "word/styles.xml": b"<w:styles xmlns:w='w'/>",
            "word/document.xml": b"<w:document xmlns:w='w'/>",
        }
        found = [
            "word/ActiveX/activeX1.xml", "word/Fonts/font2.odttf",
            "word/controls/control1.xml", "word/controls/control2.bin",
            "word/controls/control2.xml", "word/controls/control3.xml",
            "word/controls/legacy.ocx", "word/controls/missing.xml", "word/controls/upper.xml",
            "word/extras/font1.odttf", "word/objects/Sheet1.xlsx", "word/objects/oleObject1.bin",
        ]
        self.assertEqual(public.embedded_parts(parts), found)
        # Both the import and the check of committed files go by it.
        data = package(**{name: data for name, data in parts.items()
                          if name != "word/document.xml"})
        problems = public.problems(data)
        for name in found:
            with self.subTest(name):
                self.assertIn(f"{name} is embedded", problems)
        with self.assertRaises(public.Refused) as caught:
            public.clean(package(**{"word/controls/control1.xml": "<ax:ocx xmlns:ax='a'/>",
                                    "[Content_Types].xml": types}), DOC)
        self.assertIn(
            "embeds word/Controls/Upper.xml, word/controls/control1.xml", str(caught.exception)
        )
        with self.assertRaises(public.Refused) as caught:
            public.clean(package(**{"word/VBAProject.bin": b"\x00"}), DOC)
        self.assertIn("has macros", str(caught.exception))

    def test_embedded_parts_are_found_whatever_the_case_of_their_names(self):
        # Part names compare without case (ECMA-376 Part 2, §9.1.1.1): a folder, a .rels
        # part and an extension the content types name in other cases than Word writes.
        ms = "http://schemas.microsoft.com/office/2006/relationships"
        types = (
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Default Extension="OCX" ContentType="application/vnd.ms-office.activeX"/></Types>'
        )
        parts = {
            "[Content_Types].xml": types.encode(),
            "word/_rels/document.xml.rels": relationships(
                (f"{REL}/control", "controls/control2.xml", "")
            ).encode(),
            "word/controls/_RELS/control2.xml.RELS": relationships(
                (f"{ms}/activeXControlBinary", "control2.bin", "")
            ).encode(),
            "word/controls/control2.xml": b"<ax:ocx xmlns:ax='a'/>",
            "word/controls/control2.bin": b"\x00",  # through the upper-case .RELS part
            "word/controls/Legacy.OCX": b"\x00",  # by the extension, both in upper case
            "word/Embeddings/oleObject2.bin": b"\x00",  # by its folder only
            "word/VBAProject.bin": b"\x00",  # macros, which problems() reports too
        }
        found = [
            "word/Embeddings/oleObject2.bin",
            "word/controls/Legacy.OCX",
            "word/controls/control2.bin",
            "word/controls/control2.xml",
        ]
        self.assertEqual(public.embedded_parts(parts), found)
        problems = public.problems(package(**parts))
        for name in found + ["word/VBAProject.bin"]:
            with self.subTest(name):
                self.assertIn(f"{name} is embedded", problems)

    def test_embedded_parts_are_found_whatever_the_xml_spelling(self):
        # #294: a control part stored as XML only, which no folder rule catches, named by a
        # content type or a relationship spelled as XML and OPC allow but Word does not
        # write. Each package has only one way to find it.
        ct_ns = "http://schemas.openxmlformats.org/package/2006/content-types"
        control = b"<ax:ocx xmlns:ax='a' ax:persistence='persistPropertyBag'/>"

        def types(overrides, name="[Content_Types].xml", encoding="utf-8"):
            xml = (f'<?xml version="1.0" encoding="{encoding}"?><Types xmlns="{ct_ns}">'
                   '<Default Extension="xml" ContentType="application/xml"/>'
                   f"{overrides}</Types>")
            return {name: xml.encode(encoding)}

        def rels(xml, encoding="utf-8"):
            text = f'<?xml version="1.0" encoding="{encoding}"?>{xml}'
            return {"word/_rels/document.xml.rels": text.encode(encoding)}

        override = ('<Override PartName="/word/controls/c1.xml" '
                    'ContentType="application/vnd.ms-office.activeX{}xml"/>')
        relationship = '<Relationship Id="rId1" Type="{}" Target="controls/c1.xml"/>'
        spellings = {
            "a character reference in the content type": types(override.format("&#43;")),
            "a prefixed content types element": {"[Content_Types].xml": (
                f'<ct:Types xmlns:ct="{ct_ns}"><ct:Override PartName="/word/controls/c1.xml" '
                'ContentType="application/vnd.ms-office.activeX+xml"/></ct:Types>').encode()},
            "content types in UTF-16": types(override.format("+"), encoding="utf-16"),
            "the content types stream in lower case": types(override.format("+"),
                                                            name="[content_types].xml"),
            "a content type with a parameter": types(
                override.format("+").replace('+xml"', '+xml;v=1"')),
            "a character reference in the relationship type": rels(
                f'<Relationships xmlns="{PKG}">'
                + relationship.format(f"{REL}/contr&#111;l") + "</Relationships>"),
            "prefixed relationship elements": rels(
                f'<r:Relationships xmlns:r="{PKG}">'
                + relationship.format(f"{REL}/control").replace("<Relationship", "<r:Relationship")
                + "</r:Relationships>"),
            "relationships in UTF-16": rels(
                f'<Relationships xmlns="{PKG}">' + relationship.format(f"{REL}/control")
                + "</Relationships>", encoding="utf-16"),
            "a Default content type with a parameter": types(
                '<Default Extension="ocx" ContentType="application/vnd.ms-office.activeX;v=1"/>'),
            "a percent-encoded Default extension": types(
                '<Default Extension="%6Fcx" ContentType="application/vnd.ms-office.activeX"/>'),
            "a Default extension that the part name percent-encodes": types(
                '<Default Extension="ocx" ContentType="application/vnd.ms-office.activeX"/>'),
        }
        for kind in ("control", "font", "oleObject", "package"):
            spellings[f"the Strict relationship type {kind}"] = rels(
                f'<Relationships xmlns="{PKG}">'
                + relationship.format(f"http://purl.oclc.org/ooxml/officeDocument/relationships"
                                      f"/{kind}") + "</Relationships>")
        for label, package_parts in spellings.items():
            with self.subTest(label):
                parts = {**package_parts, "word/controls/c1.xml": control,
                         "word/document.xml": b"<w:document xmlns:w='w'/>"}
                if "Default" in label:
                    # Found by its extension, not by a relationship or an Override.
                    name = ("word/controls/c1.%6Fcx" if "part name" in label
                            else "word/controls/c1.ocx")
                    parts = {**package_parts, name: b"\x00",
                             "word/document.xml": b"<w:document xmlns:w='w'/>"}
                    self.assertEqual(public.embedded_parts(parts), [name])
                    continue
                self.assertEqual(public.embedded_parts(parts), ["word/controls/c1.xml"])
        # Without any of them, the part is not found: the spellings above are what find it.
        plain = {**types(""), "word/controls/c1.xml": control}
        self.assertEqual(public.embedded_parts(plain), [])

    def test_package_parts_opc_does_not_allow_are_refused_and_reported(self):
        # Spellings officewright-opc refuses, which would otherwise hide a part from
        # embedded_parts: a root in no or another namespace, a document type that gives
        # TargetMode the default External, two content types streams, an unknown encoding.
        control_rel = f'<Relationship Id="rId1" Type="{REL}/control" Target="controls/c1.xml"/>'
        cases = {
            "a .rels root without a namespace": (
                {"word/_rels/document.xml.rels":
                 f"<Relationships>{control_rel}</Relationships>"},
                "word/_rels/document.xml.rels has the root element Relationships"),
            "a content types root in another namespace": (
                {"[Content_Types].xml": "<Types xmlns='urn:other'><Override "
                 "PartName='/word/controls/c1.xml' ContentType='application/vnd.ms-office."
                 "activeX+xml'/></Types>"},
                "[Content_Types].xml has the root element {urn:other}Types"),
            "a document type giving TargetMode a default": (
                {"word/_rels/document.xml.rels":
                 '<!DOCTYPE Relationships [<!ATTLIST Relationship TargetMode CDATA "External">]>'
                 f'<Relationships xmlns="{PKG}">{control_rel}</Relationships>'},
                "word/_rels/document.xml.rels declares a document type"),
            "an encoding the parser does not know": (
                {"word/_rels/document.xml.rels":
                 f'<?xml version="1.0" encoding="x-unknown"?><Relationships xmlns="{PKG}"/>'},
                "word/_rels/document.xml.rels is not well-formed XML"),
            "a multi-byte encoding the parser cannot read": (
                {"word/_rels/document.xml.rels":
                 f'<?xml version="1.0" encoding="UTF-32"?><Relationships xmlns="{PKG}"/>'},
                "word/_rels/document.xml.rels is not well-formed XML"),
        }
        for label, (parts, said) in cases.items():
            with self.subTest(label):
                with self.assertRaises(public.Refused) as caught:
                    public.clean(package(**parts), DOC)
                self.assertIn(said, str(caught.exception))
                self.assertTrue(any(p.startswith(said) for p in public.problems(package(**parts))),
                                public.problems(package(**parts)))
        # Two content types streams whose names differ only in case.
        twice = {"[Content_Types].xml": b"<Types/>", "[content_types].xml": b"<Types/>"}
        with self.assertRaises(public.Unreadable) as caught:
            public.embedded_parts(twice)
        self.assertIn("2 content types streams", str(caught.exception))

    def test_a_package_part_that_is_not_xml_is_refused_and_reported(self):
        for name in ("[Content_Types].xml", "word/_rels/document.xml.rels"):
            with self.subTest(name):
                broken = {name: "<Types><Override PartName='/word/x.xml'"}
                with self.assertRaises(public.Refused) as caught:
                    public.clean(package(**broken), DOC)
                self.assertIn(f"{name} is not well-formed XML", str(caught.exception))
                self.assertTrue(
                    any(p.startswith(f"{name} is not well-formed XML")
                        for p in public.problems(package(**broken))))

    def test_text_after_a_nested_text_box_and_split_addresses_are_read(self):
        nested = (
            "<w:document xmlns:w='w'><w:body><w:p><w:r><w:t>See </w:t></w:r><w:r><w:pict>"
            "<w:txbxContent><w:p><w:r><w:t>box</w:t></w:r></w:p></w:txbxContent></w:pict></w:r>"
            "<w:r><w:t>or call (202) 555-0142</w:t></w:r></w:p>"
            "<w:p><w:r><w:t>Write to jane.roe@ag</w:t></w:r><w:r><w:t>ency.gov</w:t></w:r></w:p>"
            "</w:body></w:document>"
        )
        found = "; ".join(public.problems(package(**{"word/document.xml": nested})))
        self.assertIn("document.xml holds the phone number (202) 555-0142", found)
        self.assertIn("document.xml holds the address jane.roe@agency.gov", found)

    def test_an_author_attribute_alone_is_a_problem(self):
        authored = "<w:document xmlns:w='w'><w:p w:author='jroe'/></w:document>"
        found = "; ".join(public.problems(package(**{"word/document.xml": authored})))
        self.assertIn("document.xml has tracked changes or an author", found)

    def test_fields_that_name_local_files_are_refused_and_reported(self):
        # A field instruction can name a file to open or load, in w:instrText split across
        # runs or in w:fldSimple (#372).
        w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"

        def field(*pieces, prefix="w", namespace=w):
            runs = "".join(f"<{prefix}:r><{prefix}:instrText xml:space='preserve'>{piece}"
                           f"</{prefix}:instrText></{prefix}:r>" for piece in pieces)
            return (f"<{prefix}:document xmlns:{prefix}='{namespace}'><{prefix}:body><{prefix}:p>"
                    f"<{prefix}:r><{prefix}:fldChar {prefix}:fldCharType='begin'/></{prefix}:r>"
                    f"{runs}<{prefix}:r><{prefix}:fldChar {prefix}:fldCharType='separate'/>"
                    f"</{prefix}:r><{prefix}:r><{prefix}:t>x</{prefix}:t></{prefix}:r>"
                    f"<{prefix}:r><{prefix}:fldChar {prefix}:fldCharType='end'/></{prefix}:r>"
                    f"</{prefix}:p></{prefix}:body></{prefix}:document>")

        def simple(instruction):
            return (f"<w:document xmlns:w='{w}'><w:body><w:p><w:fldSimple w:instr='{instruction}'>"
                    "<w:r><w:t>x</w:t></w:r></w:fldSimple></w:p></w:body></w:document>")

        def nested_result(result):
            return (f"<w:document xmlns:w='{w}'><w:body><w:p>"
                    "<w:r><w:fldChar w:fldCharType='begin'/></w:r>"
                    '<w:r><w:instrText>INCLUDEPICTURE "</w:instrText></w:r>'
                    "<w:r><w:fldChar w:fldCharType='begin'/></w:r>"
                    "<w:r><w:instrText>MERGEFIELD Photo</w:instrText></w:r>"
                    "<w:r><w:fldChar w:fldCharType='separate'/></w:r>"
                    f"<w:r><w:t>{result}</w:t></w:r>"
                    "<w:r><w:fldChar w:fldCharType='end'/></w:r>"
                    '<w:r><w:instrText>" \\d</w:instrText></w:r>'
                    "<w:r><w:fldChar w:fldCharType='separate'/></w:r>"
                    "<w:r><w:fldChar w:fldCharType='end'/></w:r></w:p></w:body></w:document>")

        nested = (f"<w:document xmlns:w='{w}'><w:body><w:p>"
                  "<w:r><w:fldChar w:fldCharType='begin'/></w:r>"
                  "<w:r><w:instrText>IF 1 = 1 </w:instrText></w:r>"
                  "<w:r><w:fldChar w:fldCharType='begin'/></w:r>"
                  '<w:r><w:instrText>INCLUDETEXT "D:\\\\drafts\\\\x.docx"</w:instrText></w:r>'
                  "<w:r><w:fldChar w:fldCharType='end'/></w:r>"
                  "<w:r><w:instrText> yes</w:instrText></w:r>"
                  "<w:r><w:fldChar w:fldCharType='end'/></w:r></w:p></w:body></w:document>")
        cases = {
            "a file URL": field('HYPERLINK "file:///D:/drafts/x.docx"'),
            # No piece names a place alone.
            "split across runs": field('HYPERLINK "fi', "le:/", "/D", ':/x.docx"'),
            "a drive with single backslashes": field('INCLUDETEXT "C:\\drafts\\x.docx"'),
            "an unquoted drive": field("INCLUDEPICTURE C:\\\\pics\\\\a.png \\d"),
            "a server by slashes": field('HYPERLINK "//server/share/x.docx"'),
            "a relative path": field('INCLUDEPICTURE "../pics/a.png"'),
            "another scheme": field('HYPERLINK "notes://server/db"'),
            "a scheme without slashes": field('HYPERLINK "callto:jroe"'),
            "a local path in a web address": field(
                'HYPERLINK "https://www.example.gov/?r=file:///C:/x"'),
            "a link to a server file": field('LINK Excel.Sheet.8 "\\\\\\\\srv\\\\x.xls" ""'),
            "a database": field('DATABASE \\d "orders.mdb" \\s "SELECT *"'),
            "a dde topic": field('DDEAUTO Excel "budget.xls"'),
            "fldSimple": simple('INCLUDEPICTURE "C:\\\\pics\\\\a.png"'),
            "a nested field": nested,
            "an unclosed field": field('HYPERLINK "file:///D:/x.docx"').replace(
                "<w:r><w:fldChar w:fldCharType='separate'/></w:r>", "").replace(
                "<w:r><w:fldChar w:fldCharType='end'/></w:r>", ""),
            "a deleted instruction": field('HYPERLINK "file:///D:/x.docx"').replace(
                "instrText", "delInstrText"),
            "another prefix": field('HYPERLINK "file:///D:/x.docx"', prefix="q"),
            "strict": field('HYPERLINK "file:///D:/x.docx"',
                            namespace="http://purl.oclc.org/ooxml/wordprocessingml/main"),
            "character references": field('HYPERLINK "&#x66;ile:///D:/x.docx"'),
            "cdata": field('HYPERLINK "<![CDATA[file:///D:/x.docx]]>"'),
            # A field that loads nothing still writes its instruction into the document.
            "a drive in any field": field('QUOTE "D:/drafts"'),
            "a server in any field": field('QUOTE "\\\\srv\\x"'),
            "a server by slashes in any field": field('QUOTE "//srv/x"'),
            "a file URL in any field": field('QUOTE "file:x"'),
            "another scheme in any field": field('QUOTE "smb://srv/x"'),
            # A place inside a web or mail address that does not begin an argument.
            "a file URL inside an address": field(
                'QUOTE "(https://www.example.gov/?r=file:///D:/private/x.docx)"'),
            "a server inside an address": field(
                'QUOTE "see https://www.example.gov/?r=\\\\srv\\private"'),
            "a drive inside a mail address": field(
                'QUOTE "a mailto:x@example.gov?body=C:\\private"'),
            # Each field that loads or opens a file, with a relative path.
            "a relative path with backslashes": field('INCLUDETEXT "..\\drafts\\x.docx"'),
            "a relative link": field('LINK Excel.Sheet.8 "drafts/x.xls" ""'),
            "a relative rd": field('RD "drafts/x.docx"'),
            "a relative include": field('INCLUDE "drafts/x.docx"'),
            "a relative import": field('IMPORT "drafts/x.wmf"'),
            "a relative hyperlink": field('HYPERLINK "drafts/x.docx"'),
            "a field name in lower case": field('includepicture "../pics/a.png"'),
            "a name ended by a switch": field('INCLUDEPICTURE\\d "..\\pics\\a.png"'),
            "a name ended by a quote": field('INCLUDETEXT"..\\drafts\\x.docx"'),
            "dde": field('DDE Excel "budget.xls"'),
            # Fields whose result shows a path or a person.
            "filename": field("FILENAME \\p"), "template": field("TEMPLATE \\p"),
            "author": field("AUTHOR"), "last saved by": field("LASTSAVEDBY"),
            "user name": field("USERNAME"), "user initials": field("USERINITIALS"),
            "user address": field("USERADDRESS"),
            **{f"docproperty {p}": field(f'DOCPROPERTY "{p}"') for p in (
                "Author", "Creator", "LastSavedBy", "LastModifiedBy", "Manager")},
            "docproperty company": field("DOCPROPERTY Company"),
            # Instruction text outside a field's instruction is read as one of its own.
            "an instruction outside a field": (
                f"<w:document xmlns:w='{w}'><w:body><w:p><w:r><w:instrText>"
                'HYPERLINK "file:///D:/x.docx"</w:instrText></w:r></w:p></w:body></w:document>'),
            "an instruction that begins with the path": simple('"D:\\drafts\\x.docx"'),
            "an unquoted path first": simple("D:\\drafts\\x"),
            # INFO and DOCPROPERTY show a property: only an allowed one is kept.
            **{f"info {p}": field(f"INFO {p}") for p in (
                "Author", "LastSavedBy", "FileName", "Template")},
            "docproperty hyperlink base": field("DOCPROPERTY HyperlinkBase"),
            "a custom docproperty": field('DOCPROPERTY "Owner"'),
            "docvariable": field("DOCVARIABLE owner"),
            # Two separators in any mix, and "//" after a scheme that does not begin a word.
            "a backslash and a slash": field('QUOTE "\\/srv\\x"'),
            "a slash and a backslash": field('QUOTE "/\\srv\\x"'),
            "a scheme after an underscore": field(
                'QUOTE "https://www.example.gov/?r=_smb://srv/x"'),
            "a scheme after a digit": field('QUOTE "1smb://srv/x"'),
            "a web scheme that does not begin a word": field('QUOTE "x_https://srv/x"'),
            # The result of a field nested in an instruction is part of that instruction.
            "a nested result with a drive": nested_result("D:\\photos\\x.jpg"),
            "a nested relative result": nested_result("photos/x.jpg"),
            "a nested fldSimple result": (
                f"<w:document xmlns:w='{w}'><w:body><w:p>"
                "<w:r><w:fldChar w:fldCharType='begin'/></w:r>"
                '<w:r><w:instrText>INCLUDETEXT "</w:instrText></w:r>'
                "<w:fldSimple w:instr='MERGEFIELD f'><w:r><w:t>drafts/x.docx</w:t></w:r>"
                '</w:fldSimple><w:r><w:instrText>"</w:instrText></w:r>'
                "<w:r><w:fldChar w:fldCharType='end'/></w:r></w:p></w:body></w:document>"),
        }
        plain = public.PublicDoc("plain", "nara", "https://www.example.gov/p.docx",
                                 "2026-10-19", "0" * 64)
        for label, document in cases.items():
            for part in ("word/document.xml", "word/header1.xml"):
                data = package(**{part: document})
                with self.subTest(label, part=part):
                    expected = f"{part} has a field"
                    self.assertTrue(any(p.startswith(expected) for p in public.problems(data)),
                                    public.problems(data))
                    with self.assertRaises(public.Refused) as caught:
                        public.clean(data, plain)
                    self.assertIn(expected, str(caught.exception))
                    if label.startswith(("an instruction that begins", "an unquoted path")):
                        self.assertIn(f"{part} has a field that names", str(caught.exception))
                    # The part is named, never the path.
                    for path in ("drafts", "D:", "C:", "server", "srv", "pics", "jroe", "orders",
                                 "private", "DRAFTS"):
                        self.assertNotIn(path, str(caught.exception))
        utf16 = field('HYPERLINK "file:///D:/x.docx"').replace(
            "<w:document", "<?xml version='1.0' encoding='UTF-16'?><w:document", 1)
        # UTF-16 with a byte order mark, and without one in either byte order.
        for encoding in ("utf-16", "utf-16-le", "utf-16-be"):
            with self.subTest(encoding):
                self.assertIn("word/footnotes.xml has a field (HYPERLINK)",
                              "; ".join(public.problems(package(**{
                                  "word/footnotes.xml": utf16.encode(encoding)}))))
        # A part name in upper case is read too, and each part is named once.
        twice = field('HYPERLINK "file:///D:/x.docx"').replace(
            "</w:p>", "<w:fldSimple w:instr='INCLUDETEXT \"D:/x\"'/></w:p>")
        found = public.field_problems({"word/header1.XML": twice.encode()})
        self.assertEqual(found, ["word/header1.XML has a field (HYPERLINK) that names a local "
                                 "file, another scheme or another program, or people's data"])

    def test_fields_across_markup_compatibility_or_foreign_markup_are_refused(self):
        # Word takes one branch of mc:AlternateContent and drops ignorable markup, so a field
        # mark there could end a field this check reads while Word reads on (review of #381).
        w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
        mc = "http://schemas.openxmlformats.org/markup-compatibility/2006"

        def document(middle, instruction='INCLUDETEXT "x.docx"'):
            return (f"<w:document xmlns:w='{w}' xmlns:mc='{mc}' xmlns:zz='urn:zz' "
                    "mc:Ignorable='zz'><w:body><w:p>"
                    "<w:r><w:fldChar w:fldCharType='begin'/></w:r>" + middle +
                    f"<w:r><w:instrText>{instruction}</w:instrText></w:r>"
                    "<w:r><w:fldChar w:fldCharType='separate'/></w:r>"
                    "<w:r><w:fldChar w:fldCharType='end'/></w:r></w:p></w:body></w:document>")

        def branch(choice, fallback):
            return (f"<w:r><mc:AlternateContent><mc:Choice Requires='zz'>{choice}</mc:Choice>"
                    f"<mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent></w:r>")

        separate = "<w:fldChar w:fldCharType='separate'/>"
        end = "<w:fldChar w:fldCharType='end'/>"

        def piece(middle):
            return (f"<w:document xmlns:w='{w}' xmlns:mc='{mc}' xmlns:zz='urn:zz' "
                    "xmlns:w14='urn:w14' mc:Ignorable='zz'><w:body><w:p>"
                    "<w:r><w:fldChar w:fldCharType='begin'/></w:r>"
                    '<w:r><w:instrText>INCLUDETEXT "D</w:instrText></w:r>' + middle
                    + '<w:r><w:instrText>"</w:instrText></w:r>'
                    "<w:r><w:fldChar w:fldCharType='separate'/></w:r>"
                    "<w:r><w:fldChar w:fldCharType='end'/></w:r></w:p></w:body></w:document>")
        cases = {
            "a separate in a Choice": document(branch(separate, "")),
            "an end in a Choice": document(branch(end, "")),
            "a separate in a Fallback": document(branch("", separate)),
            "a separate in ignorable markup": document(f"<zz:x><w:r>{separate}</w:r></zz:x>"),
            "markup inside an instruction": document(
                "", 'HYPERLINK "fi<zz:x/>le:///D:/x.docx"'),
            "custom field data": document("<w:r><w:fldData>AAAA</w:fldData></w:r>"),
            "a begin in a Choice": document(branch("<w:fldChar w:fldCharType='begin'/>", "")),
            # Instruction text in a branch goes on the instruction begun outside it, in the
            # branch Word takes (review of #381).
            "an instruction piece in a Fallback": piece(branch(
                "", "<w:instrText>:\\private\\x.docx</w:instrText>")),
            "an instruction piece in a w14 Choice": piece(
                "<w:r><mc:AlternateContent><mc:Choice Requires='w14'><w:instrText>"
                ":\\private\\x.docx</w:instrText></mc:Choice><mc:Fallback/>"
                "</mc:AlternateContent></w:r>"),
            "an instruction piece in foreign markup": piece(
                "<w:r><zz:x><w:instrText>:\\private\\x.docx</w:instrText></zz:x></w:r>"),
        }
        # Custom field data in a field without an instruction.
        cases["custom field data alone"] = (
            f"<w:document xmlns:w='{w}'><w:body><w:p><w:r><w:fldChar w:fldCharType='begin'>"
            "<w:fldData>AAAA</w:fldData></w:fldChar></w:r><w:r>"
            "<w:fldChar w:fldCharType='end'/></w:r></w:p></w:body></w:document>")
        for label, xml in cases.items():
            data = package(**{"word/header1.xml": xml})
            with self.subTest(label):
                message = "word/header1.xml has a field across Markup Compatibility branches"
                self.assertIn(message, "; ".join(public.problems(data)))
                with self.assertRaises(public.Refused) as caught:
                    public.clean(data, DOC)
                self.assertIn(message, str(caught.exception))
        # A whole field in each branch, as Word writes text boxes, is read and kept.
        whole = ("<w:r><w:fldChar w:fldCharType='begin'/></w:r><w:r><w:instrText>PAGE"
                 "</w:instrText></w:r><w:r><w:fldChar w:fldCharType='end'/></w:r>")
        kept = (f"<w:document xmlns:w='{w}' xmlns:mc='{mc}'><w:body><w:p><w:r>"
                f"<mc:AlternateContent><mc:Choice Requires='w14'>{whole}</mc:Choice>"
                f"<mc:Fallback>{whole}</mc:Fallback></mc:AlternateContent></w:r></w:p></w:body>"
                "</w:document>")
        self.assertEqual(public.field_problems({"word/header1.xml": kept.encode()}), [])

    def test_fields_that_name_no_local_file_are_kept(self):
        w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
        for instruction in ("PAGE  \\* MERGEFORMAT", 'TOC \\o "1-3" \\h \\z \\u',
                            "PAGEREF _Toc198565444 \\h", "SEQ Table \\* ARABIC",
                            'DATE \\@ "M/d/yyyy"', 'HYPERLINK "https://www.example.gov/a/b"',
                            'HYPERLINK \\l "_Toc1"', 'HYPERLINK "mailto:help@example.gov"',
                            'INCLUDEPICTURE "https://www.example.gov/a.png" \\d',
                            'HYPERLINK "x.docx"', 'HYPERLINK "HTTPS://WWW.EXAMPLE.GOV/A"',
                            'DOCPROPERTY "Title"', "INFO NumPages", "DOCPROPERTY Pages"):
            document = (f"<w:document xmlns:w='{w}'><w:body><w:p><w:fldSimple "
                        f"w:instr='{instruction}'/><w:r><w:fldChar w:fldCharType='begin'/></w:r>"
                        f"<w:r><w:instrText>{instruction}</w:instrText></w:r><w:r>"
                        "<w:fldChar w:fldCharType='end'/></w:r></w:p></w:body></w:document>")
            with self.subTest(instruction):
                self.assertEqual(public.field_problems({"word/header1.xml":
                                                        document.encode()}), [])

    def test_field_instructions_are_read_as_field_syntax_reads_them(self):
        # ECMA-376 Part 1 §17.16.1: in quotes, a backslash escapes a quote or a backslash, so
        # "\\" before a quote ends the argument; the name ends at anything but a letter.
        self.assertEqual(public.field_arguments('QUOTE "x\\\\" "y z"'),
                         ("QUOTE", ["x\\\\", "y z"]))
        self.assertEqual(public.field_arguments('QUOTE "say \\"hi\\""'), ("QUOTE", ['say "hi"']))
        self.assertEqual(public.field_arguments('includetext"a b"\\c x'),
                         ("INCLUDETEXT", ["a b", "x"]))
        self.assertEqual(public.field_arguments('"D:\\x"'), ("", ["D:\\x"]))

    def test_a_part_with_fields_that_cannot_be_read_hides_no_other_problem(self):
        # The field check stands apart from the others in problems() (#381's merge of #378).
        data = package(**{"word/header1.xml": "<w:hdr xmlns:w='w'><w:instrText>",
                          "word/comments.xml": "<w:comments xmlns:w='w'/>"})
        found = "; ".join(public.problems(data))
        self.assertIn("word/header1.xml is not well-formed XML", found)
        self.assertIn("word/comments.xml holds comments", found)
        self.assertIn("docProps/custom.xml is still there", found)

    def test_a_part_nested_too_deeply_is_refused_with_a_message(self):
        w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
        deep = (f"<w:document xmlns:w='{w}'>" + "<w:sdt>" * 1200
                + "<w:instrText>PAGE</w:instrText>" + "</w:sdt>" * 1200 + "</w:document>")
        with self.assertRaises(public.Unreadable) as caught:
            public.field_problems({"word/document.xml": deep.encode()})
        self.assertIn("word/document.xml nests its elements too deeply", str(caught.exception))

    def test_a_part_with_fields_must_be_well_formed(self):
        broken = package(**{"word/header1.xml": "<w:hdr xmlns:w='w'><w:instrText>"})
        typed = package(**{"word/header1.xml": "<!DOCTYPE hdr><w:hdr xmlns:w='w'>"
                           "<w:instrText>PAGE</w:instrText></w:hdr>"})
        for data, message in ((broken, "word/header1.xml is not well-formed XML"),
                              (typed, "word/header1.xml declares a document type")):
            with self.subTest(message):
                self.assertIn(message, "; ".join(public.problems(data)))
                with self.assertRaises(public.Refused) as caught:
                    public.clean(data, DOC)
                self.assertIn(message, str(caught.exception))

    def test_more_refusals(self):
        with self.assertRaises(public.Refused) as caught:
            public.clean(package(**{"word/vbaProject.bin": b"macro"}), DOC)
        self.assertIn("has macros", str(caught.exception))
        lost = public.PublicDoc(
            "lost", "nara", "https://www.example.gov/x.docx", "2026-10-13", "0" * 64,
            replacements=(public.Replacement("word/missing.xml", "x", "0" * 64, "y"),),
        )
        with self.assertRaises(public.Refused) as caught:
            public.clean(package(), lost)
        self.assertIn("no part word/missing.xml", str(caught.exception))

    def test_entries_that_name_one_part_are_refused_and_reported(self):
        # The checks read parts by name, which gives the last of two entries: here the first
        # links to a local file and the second, read in its place, does not.
        local = relationships(
            (f"{REL}/hyperlink", "file:///J:/drafts/x.docx", ' TargetMode="External"'))
        hidden = with_entry(package(**{"word/_rels/header1.xml.rels": local}),
                            "word/_rels/header1.xml.rels",
                            relationships((f"{REL}/image", "media/image2.png", "")))
        # Part names compare without regard to case.
        cased = with_entry(package(), "word/Document.xml", "<w:document xmlns:w='w'/>")
        for data, names in ((hidden, "word/_rels/header1.xml.rels and "
                                     "word/_rels/header1.xml.rels"),
                            (cased, "word/document.xml and word/Document.xml")):
            with self.subTest(names):
                with self.assertRaises(public.Refused) as caught:
                    public.clean(data, DOC)
                self.assertIn(f"entries {names} name one part", str(caught.exception))
        # And a percent-encoded spelling, which OPC reads as the same part: a twin of the core
        # properties would keep a person's name. (The backslash spelling is checked on names in
        # test_entry_names_compare_as_part_names: zipfile on Windows rewrites it.)
        core = "<cp:coreProperties xmlns:cp='c' xmlns:dc='d'><dc:creator>Jane Roe</dc:creator>"
        for name, twin_name in (("docProps/core.xml", "docProps/%63ore.xml"),):
            data = with_entry(package(), twin_name, core + "</cp:coreProperties>")
            with self.subTest(twin_name):
                with self.assertRaises(public.Refused) as caught:
                    public.clean(data, DOC)
                self.assertIn(f"entries {name} and {twin_name} name one part",
                              str(caught.exception))
        # problems() reports exact, case and percent twins, each group of them.
        twin = with_entry(self.cleaned.data, "WORD/document.xml", "<w:document xmlns:w='w'/>")
        self.assertIn("entries word/document.xml and WORD/document.xml name one part",
                      public.problems(twin))
        twins = with_entry(with_entry(self.cleaned.data, "word/document.xml", "<w:document/>"),
                           "docProps/%61pp.xml", "<Properties/>")
        self.assertEqual(
            [found for found in public.problems(twins) if "name one part" in found],
            ["entries docProps/app.xml and docProps/%61pp.xml name one part",
             "entries word/document.xml and word/document.xml name one part"],
        )
        self.assertEqual(public.problems(self.cleaned.data), [])
        # The cleaning itself must not write two entries that name one part.
        for first, second in (("word/media/a.emf", "word/media/A.png"),
                              ("word/media/a.jpeg", "word/Media/a.png"),
                              ("word/media/%61.jpeg", "word/media/a.png")):
            pictures = package(**{first: b"\x01\x00\x00\x00 EMF",
                                  second: public.placeholder(3, 2)})
            with self.subTest(first):
                with self.assertRaises(public.Refused) as caught:
                    public.clean(pictures, DOC)
                self.assertIn("a picture's new name would name one part with another entry",
                              str(caught.exception))
        # clean() names every group of twins.
        both = with_entry(with_entry(package(), "word/Document.xml", "<w:document/>"),
                          "docProps/%61pp.xml", "<Properties/>")
        with self.assertRaises(public.Refused) as caught:
            public.clean(both, DOC)
        self.assertIn("entries word/document.xml and word/Document.xml name one part",
                      str(caught.exception))
        self.assertIn("entries docProps/app.xml and docProps/%61pp.xml name one part",
                      str(caught.exception))

    def test_entry_names_compare_as_part_names(self):
        # Written as names, not into an archive: zipfile on Windows reads a backslash as a
        # slash already, which would hide the rule this checks.
        self.assertEqual(public.repeated_parts(["word/document.xml", "word\\document.xml"]),
                         ["word/document.xml and word\\document.xml"])
        self.assertEqual(public.repeated_parts(["a/%41.xml", "a/a.xml", "a/A.XML"]),
                         ["a/%41.xml and a/a.xml and a/A.XML"])
        # Only ASCII letters fold, and percent-decoding is not form decoding: these are all
        # different parts.
        for pair in (["Ä.xml", "ä.xml"], ["ß.xml", "ss.xml"], ["a+b.xml", "a b.xml"],
                     ["%2541.xml", "%41.xml"]):
            with self.subTest(pair):
                self.assertEqual(public.repeated_parts(pair), [])

    def test_a_placeholder_must_be_8_bit_grey_and_not_oversized(self):
        good = public.placeholder(4, 3)
        deep = good[:24] + b"\x10" + good[25:]  # bit depth 16 in IHDR
        self.assertFalse(public.is_placeholder(deep))
        self.assertTrue(public.is_placeholder(good))
        self.assertEqual(public.image_size(public.placeholder(5000, 10)), (public.MAX_SIDE, 10))
        side = public.MAX_SIDE + 1
        self.assertFalse(public.is_placeholder(grey_png(side, 1, public.placeholder_rows(side, 1))))

    def test_packages_compare_pictures_by_pixels(self):
        cleaned = public.clean(package(), DOC).data
        parts = parts_of(cleaned)
        recompressed = grey_png(96, 96, public.placeholder_rows(96, 96), level=1)
        self.assertNotEqual(recompressed, parts["word/media/image1.png"])

        def repacked(**changes):
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w") as archive:
                for name, data in {**parts, **changes}.items():
                    archive.writestr(name, data)
            return buffer.getvalue()

        same = repacked(**{"word/media/image1.png": recompressed})
        self.assertTrue(public.same_package(same, cleaned))
        other = repacked(**{"word/media/image1.png": png(96, 96)})
        self.assertFalse(public.same_package(other, cleaned))
        self.assertFalse(public.same_package(repacked(**{"word/settings.xml": b"<x/>"}), cleaned))
        smaller = grey_png(48, 48, public.placeholder_rows(48, 48))
        self.assertTrue(public.is_placeholder(smaller))
        self.assertFalse(public.same_package(repacked(**{"word/media/image1.png": smaller}), cleaned))

    def test_the_sources_row_names_everything(self):
        doc = public.PublicDoc("letter-x", "nara", "https://www.example.gov/a|b.docx",
                               "2026-10-13", "f" * 64)
        row = public.sources_row(doc, ["one change", "another | change"])
        source = public.SOURCES["nara"]
        self.assertTrue(row.startswith("| business/letter-x.docx | "))
        for text in ("https://www.example.gov/a\\|b.docx", "downloaded 2026-10-13", "f" * 64,
                     source.licence, f'"{source.quote}"', source.statement_url,
                     "one change; another \\| change"):
            self.assertIn(text, row)
        self.assertEqual(row.count(" | "), 3)

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

    def test_a_document_is_taken_out_of_its_archive(self):
        folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, folder)
        member = package()
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("forms/Part_I_de.docx", member)
            archive.writestr("forms/Part_I_bg.docx", b"other")
        doc = public.PublicDoc("test", "eu-commission", "https://www.example.eu/a.zip",
                               "2026-10-13", hashlib.sha256(member).hexdigest(),
                               member="forms/Part_I_de.docx")

        def serve(request, timeout):
            self.assertEqual(request.full_url, doc.url)
            return io.BytesIO(buffer.getvalue())

        with unittest.mock.patch.object(public.urllib.request, "urlopen", serve):
            self.assertEqual(public.original(doc, folder, fetch=True), member)
            self.assertEqual((folder / "test.docx").read_bytes(), member)
            (folder / "test.docx").unlink()
            missing = dataclasses.replace(doc, member="forms/Part_I_el.docx")
            with self.assertRaises(public.Refused) as caught:
                public.original(missing, folder, fetch=True)
            self.assertIn("forms/Part_I_el.docx is not in the archive", str(caught.exception))
            self.assertFalse((folder / "test.docx").exists())

    def test_check_mode_finds_a_stale_document(self):
        folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, folder)
        original = package()
        (folder / "originals").mkdir()
        (folder / "originals" / "test.docx").write_bytes(original)
        doc = dataclasses.replace(DOC, sha256=hashlib.sha256(original).hexdigest())
        sources = folder / "SOURCES.md"
        sources.write_text("# Sources\n\n## Reference renders\n", encoding="utf-8")

        def run(*flags):
            argv = ["import-public-docs.py", str(folder / "originals"), *flags]
            with unittest.mock.patch.multiple(public, DOCUMENTS=[doc], OUT_DIR=folder / "out",
                                              SOURCES_MD=sources), \
                    unittest.mock.patch.object(sys, "argv", argv), \
                    unittest.mock.patch("sys.stdout", io.StringIO()), \
                    unittest.mock.patch("sys.stderr", io.StringIO()) as stderr:
                return public.main(), stderr.getvalue()

        self.assertEqual(run("--check")[0], 1)
        self.assertEqual(run(), (0, ""))
        self.assertEqual(run("--check"), (0, ""))
        (folder / "out" / "test.docx").write_bytes(original)
        code, message = run("--check")
        self.assertEqual(code, 1)
        self.assertIn("differs: test", message)
        run()
        table = sources.read_text(encoding="utf-8")
        self.assertIn("| business/test.docx |", table)
        sources.write_text(table.replace("| business/test.docx |", "| business/x.docx |"),
                           encoding="utf-8")
        code, message = run("--check")
        self.assertEqual(code, 1)
        self.assertIn("differs: SOURCES.md", message)

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

    def test_documents_under_attribution_licences_are_credited(self):
        notices = (ROOT / "THIRD-PARTY-NOTICES.md").read_text(encoding="utf-8")
        attributed = [d for d in public.DOCUMENTS if "CC BY" in public.SOURCES[d.source].licence]
        self.assertTrue(attributed)
        for doc in attributed:
            with self.subTest(doc.name):
                self.assertIn(f"`{doc.name}.docx`", notices)

    def test_a_member_of_an_archive_is_named_in_its_row(self):
        doc = next(d for d in public.DOCUMENTS if d.member)
        row = public.sources_row(doc, ["x"])
        self.assertIn(f", file {doc.member} in the archive, downloaded {doc.downloaded}", row)

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
