"""Checks scripts/make-rule-docs.py and the documents it committed to tests/corpus/word-rules/.

The documents' markup is checked against the ECMA-376 schemas by hand (see the script's
pull request); these tests keep the committed files, the generator and SOURCES.md in step
and check the package structure that Word needs to open a file at all.
"""

import dataclasses
import importlib.util
import re
import shutil
import struct
import sys
import tempfile
import unittest
import zipfile
import zlib
from pathlib import Path
from xml.etree import ElementTree

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"
CARLITO = Path(__file__).resolve().parents[2] / "tests/fonts/carlito/Carlito-Regular.ttf"
W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
W14 = "{http://schemas.microsoft.com/office/word/2010/wordml}"
CT = "{http://schemas.openxmlformats.org/package/2006/content-types}"
PR = "{http://schemas.openxmlformats.org/package/2006/relationships}"


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


rules = load("make-rule-docs")


def part(parts, name):
    return ElementTree.fromstring(dict(parts)[name])


def note_parts(parts):
    """The names of a document's footnotes and endnotes parts, if it has them."""
    return [name for name, _ in parts if name in ("word/footnotes.xml", "word/endnotes.xml")]


def carlito_width(text, size=11):
    """The width in points of text in Carlito, Calibri's metric twin, at size points: the
    advances of hmtx for the glyphs that its (3, 1) cmap, of format 4, maps the text to."""
    data = CARLITO.read_bytes()
    (count,) = struct.unpack(">H", data[4:6])
    tables = {}
    for i in range(count):
        tag, _, offset, length = struct.unpack(">4sIII", data[12 + 16 * i : 28 + 16 * i])
        tables[tag] = data[offset : offset + length]
    (units,) = struct.unpack(">H", tables[b"head"][18:20])
    (metrics,) = struct.unpack(">H", tables[b"hhea"][34:36])
    cmap = tables[b"cmap"]
    (subtables,) = struct.unpack(">H", cmap[2:4])
    for i in range(subtables):
        platform, encoding, offset = struct.unpack(">HHI", cmap[4 + 8 * i : 12 + 8 * i])
        if (platform, encoding) == (3, 1):
            sub = cmap[offset:]
    assert struct.unpack(">H", sub[0:2]) == (4,)
    segments = struct.unpack(">H", sub[6:8])[0] // 2
    ends = struct.unpack(f">{segments}H", sub[14 : 14 + 2 * segments])
    at = 16 + 2 * segments
    starts = struct.unpack(f">{segments}H", sub[at : at + 2 * segments])
    deltas = struct.unpack(f">{segments}h", sub[at + 2 * segments : at + 4 * segments])
    ranges_at = at + 4 * segments
    ranges = struct.unpack(f">{segments}H", sub[ranges_at : ranges_at + 2 * segments])

    def glyph(code):
        for k in range(segments):
            if starts[k] <= code <= ends[k]:
                if ranges[k] == 0:
                    return (code + deltas[k]) & 0xFFFF
                place = ranges_at + 2 * k + ranges[k] + 2 * (code - starts[k])
                (found,) = struct.unpack(">H", sub[place : place + 2])
                return (found + deltas[k]) & 0xFFFF if found else 0
        return 0

    total = 0
    for character in text:
        index = min(glyph(ord(character)), metrics - 1)
        total += struct.unpack(">H", tables[b"hmtx"][4 * index : 4 * index + 2])[0]
    return total * size / units


def carlito_line_height(size):
    """The height in points of a single line of Carlito at size points, as Word sets one:
    the ascent and descent of the OS/2 table's Windows metrics (Calibri's are the same)."""
    data = CARLITO.read_bytes()
    (count,) = struct.unpack(">H", data[4:6])
    tables = {}
    for i in range(count):
        tag, _, offset, length = struct.unpack(">4sIII", data[12 + 16 * i : 28 + 16 * i])
        tables[tag] = data[offset : offset + length]
    (units,) = struct.unpack(">H", tables[b"head"][18:20])
    ascent, descent = struct.unpack(">HH", tables[b"OS/2"][74:78])
    return (ascent + descent) * size / units


class CommittedDocuments(unittest.TestCase):
    def test_match_the_generator_and_sources(self):
        self.assertEqual(rules.check(rules.documents()), [])


class Check(unittest.TestCase):
    """check() itself: it must notice every way the committed files can drift."""

    def setUp(self):
        self.temp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.temp)
        self.out = self.temp / "word-rules"
        shutil.copytree(rules.OUT_DIR, self.out)
        self.sources = self.temp / "SOURCES.md"
        shutil.copyfile(rules.SOURCES, self.sources)
        for name, value in (("OUT_DIR", self.out), ("SOURCES", self.sources)):
            original = getattr(rules, name)
            setattr(rules, name, value)
            self.addCleanup(setattr, rules, name, original)
        self.docs = rules.documents()

    def sources_row(self, name: str) -> str:
        return next(line for line in self.sources.read_text(encoding="utf-8").splitlines()
                    if line.startswith(f"| word-rules/{name} |"))

    def replace_row(self, row: str, new: str) -> int:
        """Replaces the row in SOURCES.md; returns the number of its line, counted from 1."""
        lines = self.sources.read_text(encoding="utf-8").splitlines()
        number = lines.index(row) + 1
        self.sources.write_text(
            self.sources.read_text(encoding="utf-8").replace(row, new), "utf-8"
        )
        return number

    def test_clean_copy_passes(self):
        self.assertEqual(rules.check(self.docs), [])

    def test_changed_part_is_reported(self):
        path = self.out / "E-line-exact.docx"
        parts = rules.stored_parts(path)
        with zipfile.ZipFile(path, "w") as archive:
            for name, data in parts:
                if name == "word/document.xml":
                    data = data.replace(b"exact 8 pt", b"exact 9 pt")
                archive.writestr(name, data)
        self.assertEqual(
            rules.check(self.docs),
            ["tests/corpus/word-rules/E-line-exact.docx differs from what this script writes"],
        )

    def test_other_zip_bytes_with_the_same_parts_are_reported(self):
        # The bytes are what Word renders and what corpus-import.py checks by hash.
        path = self.out / "E-line-exact.docx"
        parts = rules.stored_parts(path)
        with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as archive:
            for name, data in parts:
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.external_attr = 0o644 << 16
                archive.writestr(info, data)
            archive.comment = b"rezipped"
        self.assertEqual(
            rules.check(self.docs),
            [
                "tests/corpus/word-rules/E-line-exact.docx has the parts this script writes "
                "in other ZIP bytes"
            ],
        )

    def test_missing_and_unknown_files_are_reported(self):
        (self.out / "E-line-exact.docx").unlink()
        (self.out / "stray.docx").write_bytes(b"")
        self.assertEqual(
            rules.check(self.docs),
            [
                "tests/corpus/word-rules/stray.docx is not written by this script",
                "tests/corpus/word-rules/E-line-exact.docx is missing",
            ],
        )

    def test_a_sources_row_outside_the_table_is_reported(self):
        text = self.sources.read_text(encoding="utf-8")
        row = "| word-rules/E-line-exact.docx |"
        # A blank line before the row ends the table there.
        self.sources.write_text(text.replace(row, "\n" + row), "utf-8")
        self.assertEqual(
            rules.check(self.docs),
            ["the word-rules rows of tests/corpus/SOURCES.md are not one block of its table"],
        )
        # Nor may the block begin anywhere but right under the table's header.
        first = next(line for line in text.splitlines() if line.startswith("| word-rules/"))
        self.sources.write_text(text.replace(first, "Some text\n" + first), "utf-8")
        self.assertEqual(
            rules.check(self.docs),
            ["the word-rules rows of tests/corpus/SOURCES.md are not one block of its table"],
        )
        # A row text further in a line is no row.
        self.sources.write_text(text.replace(row, "| note " + row), "utf-8")
        self.assertEqual(
            rules.check(self.docs),
            ["tests/corpus/SOURCES.md has no row for word-rules/E-line-exact.docx"],
        )

    def test_missing_sources_row_is_reported(self):
        text = self.sources.read_text(encoding="utf-8")
        self.sources.write_text(text.replace("| word-rules/E-line-exact.docx |", "|"), "utf-8")
        self.assertEqual(
            rules.check(self.docs),
            ["tests/corpus/SOURCES.md has no row for word-rules/E-line-exact.docx"],
        )

    def test_a_stale_sources_row_is_reported(self):
        # The description of a document changed in the script, its row in SOURCES.md did not.
        row = self.sources_row("E-line-exact.docx")
        self.assertTrue(row.endswith(" |"))
        stale = row[:-2] + " (an older description) |"
        number = self.replace_row(row, stale)
        self.assertEqual(
            rules.check(self.docs),
            [f"line {number} of tests/corpus/SOURCES.md, a row of word-rules/E-line-exact.docx, "
             "is not the one --list prints"],
        )

    def test_a_document_listed_twice_is_reported(self):
        # As a merge that keeps both sides of SOURCES.md can leave it, also with the row
        # repeated exactly.
        row = self.sources_row("E-line-exact.docx")
        number = self.replace_row(row, row + "\n" + row)
        self.assertEqual(
            rules.check(self.docs),
            ["tests/corpus/SOURCES.md lists word-rules/E-line-exact.docx 2 times "
             f"(lines {number}, {number + 1})"],
        )

    def test_a_stale_copy_beside_the_right_row_is_reported(self):
        # Before the right row or after it.
        row = self.sources_row("E-line-exact.docx")
        stale = row[:-2] + " (an older description) |"
        for pair, stale_line in ((stale + "\n" + row, 0), (row + "\n" + stale, 1)):
            with self.subTest(stale_line=stale_line):
                text = self.sources.read_text(encoding="utf-8")
                number = self.replace_row(row, pair)
                self.assertEqual(
                    rules.check(self.docs),
                    [f"line {number + stale_line} of tests/corpus/SOURCES.md, a row of "
                     "word-rules/E-line-exact.docx, is not the one --list prints",
                     "tests/corpus/SOURCES.md lists word-rules/E-line-exact.docx 2 times "
                     f"(lines {number}, {number + 1})"],
                )
                self.sources.write_text(text, "utf-8")

    def test_a_second_row_in_another_form_is_reported(self):
        # A row whose first cell names the document in another spelling, next to the right
        # one: without the space, padded, with the path from the repository's root, as code.
        row = self.sources_row("E-line-exact.docx")
        rest = row.removeprefix("| word-rules/E-line-exact.docx |")
        for first in ("|word-rules/E-line-exact.docx |", "|  word-rules/E-line-exact.docx  |",
                      "| tests/corpus/word-rules/E-line-exact.docx |",
                      "| `word-rules/E-line-exact.docx` |"):
            with self.subTest(first=first):
                text = self.sources.read_text(encoding="utf-8")
                number = self.replace_row(row, row + "\n" + first + rest)
                self.assertEqual(
                    rules.check(self.docs),
                    [f"line {number + 1} of tests/corpus/SOURCES.md, a row of "
                     "word-rules/E-line-exact.docx, is not the one --list prints",
                     "tests/corpus/SOURCES.md lists word-rules/E-line-exact.docx 2 times "
                     f"(lines {number}, {number + 1})"],
                )
                self.sources.write_text(text, "utf-8")

    def test_a_row_for_no_document_is_reported(self):
        row = self.sources_row("E-line-exact.docx")
        gone = row.replace("E-line-exact.docx", "E-gone.docx")
        number = self.replace_row(row, row + "\n" + gone)
        self.assertEqual(
            rules.check(self.docs),
            ["tests/corpus/SOURCES.md has a row for word-rules/E-gone.docx, which this script "
             f"does not write (line {number + 1})"],
        )


class Packages(unittest.TestCase):
    def setUp(self):
        self.docs = rules.documents()

    def test_names_are_unique(self):
        names = [doc.name for doc in self.docs]
        self.assertEqual(len(names), len(set(names)))

    def test_bytes_do_not_depend_on_the_run(self):
        for doc in self.docs:
            self.assertEqual(rules.package(doc), rules.package(doc), doc.name)

    def test_entries_are_stored_with_a_fixed_date(self):
        for doc in self.docs:
            with zipfile.ZipFile(rules.OUT_DIR / f"{doc.name}.docx") as archive:
                for info in archive.infolist():
                    self.assertEqual(info.compress_type, zipfile.ZIP_STORED, info.filename)
                    self.assertEqual(info.date_time, (1980, 1, 1, 0, 0, 0), info.filename)

    def test_every_part_is_typed_and_every_relationship_resolves(self):
        for doc in self.docs:
            parts = rules.parts(doc)
            names = {name for name, _ in parts}
            types = part(parts, "[Content_Types].xml")
            overrides = {e.get("PartName") for e in types.iter(f"{CT}Override")}
            defaults = {e.get("Extension") for e in types.iter(f"{CT}Default")}
            for name in names - {"[Content_Types].xml"}:
                typed = f"/{name}" in overrides or name.rsplit(".", 1)[-1] in defaults
                self.assertTrue(typed, f"{doc.name}: {name} has no content type")
            for override in overrides:
                self.assertIn(override[1:], names, f"{doc.name}: {override} does not exist")
            for rels, base in (("_rels/.rels", ""), ("word/_rels/document.xml.rels", "word/")):
                for rel in part(parts, rels).iter(f"{PR}Relationship"):
                    self.assertIn(base + rel.get("Target"), names, f"{doc.name}: {rels}")

    def test_header_and_footer_parts_are_typed_and_referenced_by_their_kind(self):
        # ECMA-376 §17.10.1 and §17.10.4: a w:headerReference names a header part through a
        # header relationship, with the header content type and a w:hdr root; likewise for
        # footers. The Default for .xml would type such a part as plain XML.
        R = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
        relationship = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/"
        content = "application/vnd.openxmlformats-officedocument.wordprocessingml."
        for doc in self.docs:
            parts = dict(rules.parts(doc))
            types = ElementTree.fromstring(parts["[Content_Types].xml"])
            overrides = {e.get("PartName"): e.get("ContentType")
                         for e in types.iter(f"{CT}Override")}
            rels = ElementTree.fromstring(parts["word/_rels/document.xml.rels"])
            by_id = {r.get("Id"): r for r in rels.iter(f"{PR}Relationship")}
            body = ElementTree.fromstring(parts["word/document.xml"])
            referenced = set()
            for kind, root in (("header", "hdr"), ("footer", "ftr")):
                for reference in body.iter(f"{W}{kind}Reference"):
                    rel = by_id.get(reference.get(f"{R}id"))
                    self.assertIsNotNone(rel, doc.name)
                    self.assertEqual(rel.get("Type"), relationship + kind, doc.name)
                    name = "word/" + rel.get("Target")
                    self.assertEqual(overrides.get("/" + name), f"{content}{kind}+xml", name)
                    self.assertEqual(ElementTree.fromstring(parts[name]).tag, f"{W}{root}", name)
                    referenced.add(rel.get("Id"))
            # No header or footer part that nothing references.
            for rel in by_id.values():
                if rel.get("Type") in (relationship + "header", relationship + "footer"):
                    self.assertIn(rel.get("Id"), referenced, doc.name)

    def test_font_table_lists_every_font_the_document_names(self):
        for doc in self.docs:
            parts = rules.parts(doc)
            table = {f.get(f"{W}name") for f in part(parts, "word/fontTable.xml")}
            named = set()
            for name in ("word/document.xml", "word/styles.xml", *note_parts(parts)):
                for fonts in part(parts, name).iter(f"{W}rFonts"):
                    for attribute, value in fonts.attrib.items():
                        # w:hint names a slot, not a font.
                        if attribute == f"{W}hint":
                            continue
                        # A theme attribute names the theme's font for its slot.
                        if attribute.endswith("Theme"):
                            value = rules.THEME_FONTS[rules.THEME_SLOTS[value]]
                        named.add(value)
            self.assertEqual(table, named, doc.name)

    def test_compatibility_settings_match_the_name(self):
        for doc in self.docs:
            compat = part(rules.parts(doc), "word/settings.xml").find(f"{W}compat")
            modes = [
                int(s.get(f"{W}val"))
                for s in compat.iter(f"{W}compatSetting")
                if s.get(f"{W}name") == "compatibilityMode"
            ]
            if doc.name.endswith("-12"):
                self.assertEqual(modes, [], doc.name)
            elif doc.name.endswith("-14"):
                self.assertEqual(modes, [14], doc.name)
            else:
                self.assertEqual(modes, [15], doc.name)
            # The settings Word writes only into documents in mode 15.
            others = {
                s.get(f"{W}name")
                for s in compat.iter(f"{W}compatSetting")
                if s.get(f"{W}name") != "compatibilityMode"
            }
            self.assertEqual(bool(others), modes == [15], doc.name)
            flags = {
                child.tag.replace(W, "") for child in compat if child.tag != f"{W}compatSetting"
            }
            expected = {
                "E-autospacing-nohtml": {"doNotUseHTMLParagraphAutoSpacing"},
                "E-tab-hanging-noindenttab": {"doNotUseIndentAsNumberingTabStop"},
                "E-tab-hanging-noindenttab-14": {"doNotUseIndentAsNumberingTabStop"},
                "E-underline-trailspace": {"ulTrailSpace"},
                "E-underline-trailspace-margin": {"ulTrailSpace"},
                "E-table-autofit-constrained-14": {"doNotAutofitConstrainedTables"},
                "E-table-autofit-grow-14": {"growAutofit"},
                "E-table-autofit-beside-constrained-14": {"doNotAutofitConstrainedTables"},
                "E-table-autofit-grow-15": {"growAutofit"},
                "E-underline-trailspace-indent": {"ulTrailSpace"},
            }.get(doc.name, set())
            self.assertEqual(flags, expected, doc.name)
            # The documents about a setting name its value; every other document in mode 15
            # turns each of the four on, as Word does in a new document.
            named = {
                "E-opentype-on": ("enableOpenTypeFeatures", ["1"]),
                "E-opentype-off": ("enableOpenTypeFeatures", ["0"]),
                "E-opentype-none": ("enableOpenTypeFeatures", []),
                "E-opentype-12": ("enableOpenTypeFeatures", []),
                "E-table-style-header-rows-on": ("differentiateMultirowTableHeaders", ["1"]),
                "E-table-style-header-rows-off": ("differentiateMultirowTableHeaders", ["0"]),
                "E-table-style-header-rows-3-on": ("differentiateMultirowTableHeaders", ["1"]),
                "E-table-style-header-rows-3-off": ("differentiateMultirowTableHeaders", ["0"]),
            }
            for size in (11, 12, 13):
                named[f"E-table-style-size-{size}pt"] = (
                    "overrideTableStyleFontSizeAndJustification", ["1"])
                named[f"E-table-style-size-{size}pt-none"] = (
                    "overrideTableStyleFontSizeAndJustification", [])
                named[f"E-table-style-size-{size}pt-14"] = (
                    "overrideTableStyleFontSizeAndJustification", [])
            for setting in ("overrideTableStyleFontSizeAndJustification",
                            "enableOpenTypeFeatures", "doNotFlipMirrorIndents",
                            "differentiateMultirowTableHeaders"):
                values = [
                    s.get(f"{W}val")
                    for s in compat.iter(f"{W}compatSetting")
                    if s.get(f"{W}name") == setting
                ]
                expected = ["1"] if modes == [15] else []
                if doc.name in named and named[doc.name][0] == setting:
                    expected = named[doc.name][1]
                self.assertEqual(values, expected, f"{doc.name}: {setting}")

    def test_lists_only_use_defined_numbering(self):
        for doc in self.docs:
            parts = rules.parts(doc)
            # Instance 0 takes a paragraph out of a list; it never names one (ECMA-376
            # §17.9.18).
            used = {
                n.get(f"{W}val")
                for name in ("word/document.xml", "word/styles.xml")
                for n in part(parts, name).iter(f"{W}numId")
            } - {"0"}
            if not used:
                continue
            numbering = part(parts, "word/numbering.xml")
            defined = {n.get(f"{W}numId") for n in numbering.iter(f"{W}num")}
            self.assertLessEqual(used, defined, doc.name)

    def test_styles_named_are_defined(self):
        # Word falls back to Normal for a style it does not find, so a mistyped name would
        # only show in the render.
        for doc in self.docs:
            parts = rules.parts(doc)
            styles = part(parts, "word/styles.xml")
            defined = {s.get(f"{W}styleId") for s in styles.iter(f"{W}style")}
            # A list level names a paragraph style too (w:pStyle), and a definition a
            # numbering style (w:numStyleLink, w:styleLink).
            roots = [part(parts, "word/document.xml"), styles]
            if any(name == "word/numbering.xml" for name, _ in parts):
                roots.append(part(parts, "word/numbering.xml"))
            roots += [part(parts, name) for name in note_parts(parts)]
            named = {
                n.get(f"{W}val")
                for root in roots
                for tag in ("pStyle", "rStyle", "basedOn", "numStyleLink", "styleLink", "tblStyle")
                for n in root.iter(f"{W}{tag}")
            }
            self.assertLessEqual(named, defined, doc.name)
            # And of the right type: a paragraph or a list level names a paragraph style, a
            # run or a paragraph mark a character style, and a style is based on its own type.
            kind = {s.get(f"{W}styleId"): s.get(f"{W}type") for s in styles.iter(f"{W}style")}
            for root in roots:
                for tag, expected in (
                    ("pStyle", "paragraph"), ("rStyle", "character"), ("tblStyle", "table")
                ):
                    for n in root.iter(f"{W}{tag}"):
                        self.assertEqual(kind[n.get(f"{W}val")], expected, doc.name)
            for style in styles.iter(f"{W}style"):
                base = style.find(f"{W}basedOn")
                if base is not None:
                    self.assertEqual(kind[base.get(f"{W}val")], style.get(f"{W}type"), doc.name)

    def test_refuses_a_colour_mapping_value_that_is_not_a_theme_colour(self):
        doc = next(doc for doc in self.docs if doc.color_mapping)
        # A value that is no colour, and one that is the name of a mapping attribute.
        for value in ("accent7", "t1"):
            mistyped = dataclasses.replace(doc, color_mapping=(("t2", value),))
            with self.assertRaisesRegex(ValueError, f"{value} is not a theme colour"):
                rules.settings_xml(mistyped)

    def test_paragraph_properties_lists_and_styles_follow_the_schema_order(self):
        # Like CT_RPr, these are sequences (ECMA-376 Part 1: CT_PPr 17.3.1.26, CT_NumPr
        # 17.9.18, CT_Lvl 17.9.6, CT_Style 17.7.4.17); an element the list does not name
        # fails too.
        orders = {
            "pPr": [
                "pStyle", "keepNext", "keepLines", "pageBreakBefore", "framePr",
                "widowControl", "numPr", "suppressLineNumbers", "pBdr", "shd", "tabs",
                "suppressAutoHyphens", "kinsoku", "wordWrap", "overflowPunct",
                "topLinePunct", "autoSpaceDE", "autoSpaceDN", "bidi", "adjustRightInd",
                "snapToGrid", "spacing", "ind", "contextualSpacing", "mirrorIndents",
                "suppressOverlap", "jc", "textDirection", "textAlignment",
                "textboxTightWrap", "outlineLvl", "divId", "cnfStyle", "rPr", "sectPr",
                "pPrChange",
            ],
            "numPr": ["ilvl", "numId", "numberingChange", "ins"],
            "lvl": [
                "start", "numFmt", "lvlRestart", "pStyle", "isLgl", "suff", "lvlText",
                "lvlPicBulletId", "legacy", "lvlJc", "pPr", "rPr",
            ],
            "style": [
                "name", "aliases", "basedOn", "next", "link", "autoRedefine", "hidden",
                "uiPriority", "semiHidden", "unhideWhenUsed", "qFormat", "locked",
                "personal", "personalCompose", "personalReply", "rsid", "pPr", "rPr",
                "tblPr", "trPr", "tcPr", "tblStylePr",
            ],
            # CT_TblPr 17.4.60 (with CT_TblPrBase), CT_TcPr 17.4.70, CT_TblStylePr 17.7.6.6.
            "tblPr": [
                "tblStyle", "tblpPr", "tblOverlap", "bidiVisual", "tblStyleRowBandSize",
                "tblStyleColBandSize", "tblW", "jc", "tblCellSpacing", "tblInd",
                "tblBorders", "shd", "tblLayout", "tblCellMar", "tblLook", "tblCaption",
                "tblDescription", "tblPrChange",
            ],
            "tcPr": [
                "cnfStyle", "tcW", "gridSpan", "hMerge", "vMerge", "tcBorders", "shd",
                "noWrap", "tcMar", "textDirection", "tcFitText", "vAlign", "hideMark",
            ],
            "tblStylePr": ["pPr", "rPr", "tblPr", "trPr", "tcPr"],
            # CT_TblBorders, CT_TcBorders, CT_TblCellMar and CT_TcMar; left and right are the
            # transitional names of start and end, in their places.
            "tblBorders": [
                "top", "start", "left", "bottom", "end", "right", "insideH", "insideV",
            ],
            "tcBorders": [
                "top", "start", "left", "bottom", "end", "right", "insideH", "insideV",
                "tl2br", "tr2bl",
            ],
            "tblCellMar": ["top", "start", "left", "bottom", "end", "right"],
            "tcMar": ["top", "start", "left", "bottom", "end", "right"],
            # CT_Tbl 17.4.38 and CT_Row 17.4.79.
            "tbl": ["bookmarkStart", "bookmarkEnd", "tblPr", "tblGrid", "tr"],
            "tr": ["tblPrEx", "trPr", "tc"],
            # CT_SectPr 17.6.17; the header and footer references come first, in any order.
            "sectPr": [
                "headerReference", "footnotePr", "endnotePr", "type", "pgSz", "pgMar",
                "paperSrc", "pgBorders", "lnNumType", "pgNumType", "cols", "formProt",
                "vAlign", "noEndnote", "titlePg", "textDirection", "bidi", "rtlGutter",
                "docGrid", "printerSettings", "sectPrChange",
            ],
            # A section's footnote and endnote properties, 17.11.11 and 17.11.5.
            "footnotePr": ["pos", "numFmt", "numStart", "numRestart"],
            "endnotePr": ["pos", "numFmt", "numStart", "numRestart"],
        }
        for doc in self.docs:
            parts = rules.parts(doc)
            for name, _ in parts:
                stories = ("word/document.xml", "word/styles.xml", "word/numbering.xml",
                           "word/footnotes.xml", "word/endnotes.xml")
                if name not in stories and not name.startswith(("word/header", "word/footer")):
                    continue
                root = part(parts, name)
                for kind, order in orders.items():
                    for element in root.iter(f"{W}{kind}"):
                        tags = [child.tag.replace(W, "") for child in element]
                        tags = ["headerReference" if t == "footerReference" else t for t in tags]
                        for tag in tags:
                            self.assertIn(tag, order, f"{doc.name}: {kind} {tags}")
                        places = [order.index(tag) for tag in tags]
                        self.assertEqual(places, sorted(places), f"{doc.name}: {tags}")
                        # Only a style's conditional formats, a section's header and footer
                        # references, a table's rows and a row's cells may come more than once.
                        repeated = {tag for tag in tags if tags.count(tag) > 1}
                        self.assertLessEqual(
                            repeated, {"tblStylePr", "headerReference", "tr", "tc"},
                            f"{doc.name}: {tags}"
                        )

    def test_properties_come_first_in_their_element(self):
        # w:pPr, w:rPr, w:tcPr and w:trPr are the first child of their paragraph, run, cell
        # or row (CT_P 17.3.1.22, CT_R 17.3.2.25, CT_Tc 17.4.66, CT_Row 17.4.79), after a
        # row's w:tblPrEx at most.
        for doc in self.docs:
            for name, _ in rules.parts(doc):
                if not name.startswith(("word/document.xml", "word/header", "word/footer",
                                        "word/footnotes", "word/endnotes")):
                    continue
                root = part(rules.parts(doc), name)
                for parent, props in (("p", "pPr"), ("r", "rPr"), ("tc", "tcPr"), ("tr", "trPr")):
                    for element in root.iter(f"{W}{parent}"):
                        tags = [c.tag.replace(W, "") for c in element if c.tag != f"{W}tblPrEx"]
                        if props in tags:
                            self.assertEqual(tags.index(props), 0, f"{doc.name}: {tags}")

    def test_run_properties_follow_the_schema_order(self):
        # CT_RPr is a sequence (ECMA-376 Part 1, 17.3.2.28); Word refuses a document whose
        # run properties are out of order.
        order = [
            "rStyle",
            "rFonts",
            "b",
            "bCs",
            "i",
            "iCs",
            "caps",
            "smallCaps",
            "strike",
            "dstrike",
            "outline",
            "shadow",
            "emboss",
            "imprint",
            "noProof",
            "snapToGrid",
            "vanish",
            "webHidden",
            "color",
            "spacing",
            "w",
            "kern",
            "position",
            "sz",
            "szCs",
            "highlight",
            "u",
            "effect",
            "bdr",
            "shd",
            "fitText",
            "vertAlign",
            "rtl",
            "cs",
            "em",
            "lang",
            "eastAsianLayout",
            "specVanish",
            "oMath",
        ]
        # Word 2010's run properties ([MS-DOCX] 2.6) follow every one of ECMA-376, in this
        # order.
        order += [
            f"{W14}{tag}"
            for tag in (
                "glow", "shadow", "reflection", "textOutline", "textFill", "scene3d",
                "props3d", "ligatures", "numForm", "numSpacing", "stylisticSets", "cntxtAlts",
            )
        ]
        for doc in self.docs:
            parts = rules.parts(doc)
            names = {name for name, _ in parts}
            stories = [n for n in sorted(names) if n.startswith(("word/header", "word/footer"))]
            for name in ["word/document.xml", "word/styles.xml", "word/numbering.xml"] + stories:
                if name not in names:
                    continue
                for properties in part(parts, name).iter(f"{W}rPr"):
                    tags = [child.tag.replace(W, "") for child in properties]
                    for tag in tags:
                        self.assertIn(tag, order, f"{doc.name}: {name} {tags}")
                    places = [order.index(tag) for tag in tags]
                    self.assertEqual(places, sorted(set(places)), f"{doc.name}: {tags}")

    def test_cases_have_the_formatting_their_labels_name(self):
        def paragraphs(name):
            """Per paragraph: its runs as (text, font, size in pt, toggles), and its breaks as
            (number of runs before the break, w:type of the break)."""
            doc = next(doc for doc in self.docs if doc.name == name)
            body = part(rules.parts(doc), "word/document.xml")
            for paragraph in body.iter(f"{W}p"):
                runs, breaks = [], []
                for r in paragraph.iter(f"{W}r"):
                    brk = r.find(f"{W}br")
                    if brk is not None and r.find(f"{W}t") is None:
                        breaks.append((len(runs), brk.get(f"{W}type")))
                        continue
                    props = r.find(f"{W}rPr")
                    fonts = props.find(f"{W}rFonts")
                    size = props.find(f"{W}sz")
                    toggles = {c.tag.replace(W, "") for c in props} - {"rFonts", "sz", "szCs"}
                    text = "\n".join(t.text or "" for t in r.iter(f"{W}t"))
                    runs.append(
                        (text, fonts.get(f"{W}ascii"), int(size.get(f"{W}val")) / 2, toggles)
                    )
                yield runs, breaks

        def label_of(text):
            match = re.match(r"(.+?) (\d+(?:\.\d+)?) pt[, ]", text)
            self.assertIsNotNone(match, text)
            return match.group(1), float(match.group(2))

        italic = list(paragraphs("E-italic-missing"))
        styles = {
            "regular": set(),
            "italic": {"i", "iCs"},
            "bold": {"b", "bCs"},
            "bold italic": {"b", "bCs", "i", "iCs"},
        }
        # Every planned case exactly once: a duplicate cannot stand in for a missing one.
        self.assertEqual(
            sorted((runs[0][1], runs[0][2], sorted(runs[0][3])) for runs, _ in italic),
            sorted(
                (font, size, sorted(toggles))
                for font in ("Noto Sans", "Liberation Sans")
                for size in (11, 20)
                for toggles in styles.values()
            ),
        )
        for runs, _ in italic:
            ((text, font, size, toggles),) = runs
            words = text.split(":")[0]
            self.assertEqual(label_of(text), (font, size), text)
            self.assertEqual("italic" in words, {"i", "iCs"} <= toggles, text)
            self.assertEqual("bold" in words, {"b", "bCs"} <= toggles, text)

        for mode in (12, 14, 15):
            name = f"E-smallcaps-{mode}"
            self.assertEqual(next(d for d in self.docs if d.name == name).mode, mode)
            cases = list(paragraphs(name))
            self.assertEqual(
                sorted((runs[1][1], runs[1][2], runs[1][0]) for runs, _ in cases),
                sorted(
                    (font, size, word)
                    for font in ("Liberation Sans", "Carlito", "Noto Sans")
                    for size in (9, 10.5, 11, 13, 21, 40, 72)
                    for word in ("hamburg", "HAMBURG", "Hamburg")
                ),
            )
            for runs, breaks in cases:
                (label, label_font, label_size, label_toggles), (word, font, size, toggles) = runs
                # One plain line break, between the label and the word.
                self.assertEqual(breaks, [(1, None)], label)
                self.assertEqual(label_toggles, set(), label)
                self.assertEqual(label_font, font, label)
                self.assertEqual(label_of(label), (font, size), label)
                kind = label.split(", ")[-1].rstrip(":")
                expected = {
                    "small caps": ("hamburg", {"smallCaps"}),
                    "capitals": ("HAMBURG", set()),
                    "mixed small caps": ("Hamburg", {"smallCaps"}),
                }[kind]
                self.assertEqual((word, toggles), expected, label)

        pages = list(paragraphs("E-line-fonts-truetype"))
        self.assertEqual(
            [runs[0][1:3] for runs, _ in pages],
            [
                (font, size)
                for font in ("Noto Sans", "Aptos", "Liberation Sans")
                for size in (11, 12)
            ],
        )
        for runs, _ in pages:
            ((text, font, size, _),) = runs
            self.assertEqual(len(text.split("\n")), 10, text)
            for line in text.split("\n"):
                self.assertEqual(label_of(line), (font, size), line)

    def test_paragraph_rule_cases_have_the_markup_their_labels_name(self):
        def val(element, tag):
            child = None if element is None else element.find(f"{W}{tag}")
            return None if child is None else child.get(f"{W}val")

        def load(name):
            """The document's styles by id, and per body paragraph its label without the
            case number, its w:pStyle and its w:pPr, checking the case numbers."""
            parts = rules.parts(next(doc for doc in self.docs if doc.name == name))
            styles = {
                style.get(f"{W}styleId"): style
                for style in part(parts, "word/styles.xml").iter(f"{W}style")
            }
            cases = []
            body = part(parts, "word/document.xml").find(f"{W}body")
            for n, paragraph in enumerate(body.findall(f"{W}p"), 1):
                text = "".join(t.text for t in paragraph.iter(f"{W}t"))
                prefix = f"Case {n:02d}: "
                self.assertTrue(text.startswith(prefix), text)
                p_pr = paragraph.find(f"{W}pPr")
                cases.append((text.removeprefix(prefix), val(p_pr, "pStyle"), p_pr))
            return parts, styles, cases

        # E-list-level-inherit: what each label says the style, the list definition and
        # the paragraph give.
        parts, styles, cases = load("E-list-level-inherit")
        numbering = part(parts, "word/numbering.xml")
        definition_of = {
            n.get(f"{W}numId"): val(n, "abstractNumId") for n in numbering.iter(f"{W}num")
        }
        self.assertEqual(len(cases), 8)
        for label, style, p_pr in cases:
            styled, *rest = label.split("; ")
            if style is None:
                self.assertEqual(styled, "no style", label)
            else:
                said = re.fullmatch(
                    r"the style gives (?:instance (\d+))?(?: and )?(?:level (\d+)|(no level))?",
                    styled,
                )
                self.assertIsNotNone(said, label)
                style_num_pr = styles[style].find(f"{W}pPr/{W}numPr")
                instance = val(style_num_pr, "numId")
                if said.group(1):
                    self.assertEqual(instance, said.group(1), label)
                if said.group(2) or said.group(3):
                    self.assertEqual(val(style_num_pr, "ilvl"), said.group(2), label)
            direct = ""
            for clause in rest:
                linked = re.fullmatch(r"level (\d+) names the style", clause)
                if linked:
                    definition = numbering.find(
                        f"{W}abstractNum[@{W}abstractNumId='{definition_of[instance]}']"
                    )
                    # That level names the style, and no other level does.
                    naming = [
                        level.get(f"{W}ilvl")
                        for level in definition.findall(f"{W}lvl")
                        if val(level, "pStyle") == style
                    ]
                    self.assertEqual(naming, [linked.group(1)], label)
                    # Nor does a level of any other definition.
                    elsewhere = [
                        number.get(f"{W}abstractNumId")
                        for number in numbering.findall(f"{W}abstractNum")
                        if number is not definition
                        and any(
                            val(level, "pStyle") == style
                            for level in number.findall(f"{W}lvl")
                        )
                    ]
                    self.assertEqual(elsewhere, [], label)
                else:
                    direct += clause
            said = dict(re.findall(r"direct (numId|ilvl) (\d+)", direct))
            for tag in ("numId", "ilvl"):
                self.assertEqual(val(p_pr.find(f"{W}numPr"), tag), said.get(tag), label)
        # Instances 1 and 2 use definitions of different number formats, so a render
        # tells which one a paragraph took.
        formats = {
            d.get(f"{W}abstractNumId"): {val(lvl, "numFmt") for lvl in d.iter(f"{W}lvl")}
            for d in numbering.iter(f"{W}abstractNum")
        }
        self.assertTrue(formats[definition_of["1"]].isdisjoint(formats[definition_of["2"]]))

        # E-paragraph-units: every value the paragraph or its style writes in w:ind and
        # w:spacing is in the label, and nothing else is.
        def written(p_pr):
            values = {}
            for tag in ("ind", "spacing"):
                element = None if p_pr is None else p_pr.find(f"{W}{tag}")
                if element is not None:
                    values.update((k.replace(W, ""), v) for k, v in element.items())
            return {
                (name, f"{int(value) / 20:g}" if name in ("left", "before") else value)
                for name, value in values.items()
            }

        def said(text):
            found = set(re.findall(r"(leftChars|beforeLines) (\d+)", text))
            found |= {("left", n) for n in re.findall(r"left(?: indent)? (\d+) pt", text)}
            before = r"(?<!left )(?<!indent )\b(\d+) pt"
            found |= {("before", n) for n in re.findall(before, text)}
            return found

        _, styles, cases = load("E-paragraph-units")
        self.assertEqual(len(cases), 8)
        previous = None
        for label, style, p_pr in cases:
            styled, _, direct = label.partition("; ")
            if style is None:
                self.assertEqual(styled, "no style", label)
            elif styled == "the same style":
                self.assertEqual(style, previous, label)
            else:
                self.assertEqual(said(styled), written(styles[style].find(f"{W}pPr")), label)
            self.assertEqual(said(direct), written(p_pr), label)
            previous = style

        # E-default-paragraph-style: each label names the paragraph's style, and the
        # styles are what the purpose says, in that order.
        doc = next(doc for doc in self.docs if doc.name == "E-default-paragraph-style")
        _, styles, cases = load(doc.name)
        self.assertEqual(len(cases), 5)
        for label, style, _ in cases:
            pattern = r"a paragraph that names (no style|\w+)(, after the others)?"
            named = re.fullmatch(pattern, label)
            self.assertIsNotNone(named, label)
            name = named.group(1)
            self.assertEqual(style, None if name == "no style" else name, label)
        paragraph_styles = [
            (style_id, element.get(f"{W}default"))
            for style_id, element in styles.items()
            if element.get(f"{W}type") == "paragraph"
        ]
        self.assertEqual(
            paragraph_styles, [("FirstDefault", "1"), ("Normal", None), ("LastDefault", "1")]
        )
        described = re.findall(
            r"(\w+) \((right-aligned|centred|left-aligned), (?:(\d+) pt after|no space after)\)",
            doc.purpose,
        )
        self.assertEqual(
            [style_id for style_id, _, _ in described], ["FirstDefault", "LastDefault", "Normal"]
        )
        alignment_of = {"right-aligned": "right", "centred": "center", "left-aligned": None}
        for style_id, alignment, after in described:
            p_pr = styles[style_id].find(f"{W}pPr")
            self.assertEqual(val(p_pr, "jc"), alignment_of[alignment], style_id)
            spacing = None if p_pr is None else p_pr.find(f"{W}spacing")
            space = None if spacing is None else spacing.get(f"{W}after")
            self.assertEqual(space, str(int(after) * 20) if after else None, style_id)

    def test_list_label_cases_have_the_markup_their_labels_name(self):
        def described(properties):
            """The run properties as a label says them."""
            words = []
            for tag, on, off in (("b", "bold", "not bold"), ("i", "italic", "not italic")):
                element = None if properties is None else properties.find(f"{W}{tag}")
                if element is not None:
                    words.append(off if element.get(f"{W}val") == "0" else on)
            if properties is not None:
                color = properties.find(f"{W}color")
                if color is not None:
                    self.assertEqual(color.get(f"{W}val"), "FF0000")
                    words.append("red")
                size = properties.find(f"{W}sz")
                if size is not None:
                    words.append(f"{int(size.get(f'{W}val')) / 2:g} pt")
                if properties.find(f"{W}u") is not None:
                    words.append("underlined")
                highlight = properties.find(f"{W}highlight")
                if highlight is not None:
                    words.append(f"highlight {highlight.get(f'{W}val')}")
                # Every property has its complex-script twin, and nothing else is there.
                for tag in ("b", "i", "sz"):
                    twin = properties.find(f"{W}{tag}Cs")
                    element = properties.find(f"{W}{tag}")
                    self.assertEqual(
                        None if element is None else element.attrib,
                        None if twin is None else twin.attrib,
                    )
                known = {"rStyle", "b", "bCs", "i", "iCs", "color", "sz", "szCs", "u",
                         "highlight"}
                self.assertLessEqual({c.tag.replace(W, "") for c in properties}, known)
            return ", ".join(words)

        def check_level(level, text):
            """Level 0 of a decimal list whose label reads "1."."""
            self.assertEqual(level.get(f"{W}ilvl"), "0", text)
            self.assertEqual(level.find(f"{W}start").get(f"{W}val"), "1", text)
            self.assertEqual(level.find(f"{W}numFmt").get(f"{W}val"), "decimal", text)
            self.assertEqual(level.find(f"{W}lvlText").get(f"{W}val"), "%1.", text)

        for name, count in (("E-list-label-format", 8), ("E-list-label-rstyle", 2)):
            parts = rules.parts(next(doc for doc in self.docs if doc.name == name))
            styles = {
                s.get(f"{W}styleId"): s for s in part(parts, "word/styles.xml").iter(f"{W}style")
            }
            numbering = part(parts, "word/numbering.xml")
            instances = {n.get(f"{W}numId"): n for n in numbering.iter(f"{W}num")}
            definitions = {
                d.get(f"{W}abstractNumId"): d for d in numbering.iter(f"{W}abstractNum")
            }
            body = part(parts, "word/document.xml").find(f"{W}body")
            paragraphs = body.findall(f"{W}p")
            self.assertEqual(len(paragraphs), count, name)
            used = []
            for n, paragraph in enumerate(paragraphs, 1):
                text = "".join(t.text for t in paragraph.iter(f"{W}t"))
                p_pr = paragraph.find(f"{W}pPr")
                clauses = []
                style = p_pr.find(f"{W}pStyle")
                if style is not None:
                    style_id = style.get(f"{W}val")
                    clauses.append(
                        f"style {style_id} ({described(styles[style_id].find(f'{W}rPr'))})"
                    )
                mark = p_pr.find(f"{W}rPr")
                if mark is not None:
                    char_style = mark.find(f"{W}rStyle")
                    if char_style is not None:
                        style_id = char_style.get(f"{W}val")
                        props = styles[style_id].find(f"{W}rPr")
                        clauses.append(f"mark style {style_id} ({described(props)})")
                    else:
                        clauses.append(f"mark {described(mark)}")
                num_pr = p_pr.find(f"{W}numPr")
                self.assertEqual(num_pr.find(f"{W}ilvl").get(f"{W}val"), "0", text)
                instance = instances[num_pr.find(f"{W}numId").get(f"{W}val")]
                definition_id = instance.find(f"{W}abstractNumId").get(f"{W}val")
                used.append((instance.get(f"{W}numId"), definition_id))
                (level,) = definitions[definition_id].findall(f"{W}lvl")
                check_level(level, text)
                override = instance.find(f"{W}lvlOverride/{W}lvl")
                if override is not None:
                    self.assertEqual(instance.find(f"{W}lvlOverride").get(f"{W}ilvl"), "0")
                    check_level(override, text)
                    self.assertIsNone(override.find(f"{W}pStyle"), text)
                    clauses.append(f"definition's level {described(level.find(f'{W}rPr'))}")
                    clauses.append(f"override level {described(override.find(f'{W}rPr'))}")
                else:
                    linked = level.find(f"{W}pStyle")
                    if linked is not None:
                        style_id = linked.get(f"{W}val")
                        props = styles[style_id].find(f"{W}rPr")
                        # Whether the style names this list back, as Word writes it.
                        back = styles[style_id].find(f"{W}pPr/{W}numPr")
                        if back is not None:
                            self.assertIsNone(back.find(f"{W}ilvl"), text)
                            self.assertEqual(back.find(f"{W}numId").get(f"{W}val"),
                                             instance.get(f"{W}numId"), text)
                        way = "both ways" if back is not None else "one way"
                        clauses.append(
                            f"level style {style_id} ({described(props)}), linked {way}"
                        )
                    label = level.find(f"{W}rPr")
                    char_style = None if label is None else label.find(f"{W}rStyle")
                    if char_style is not None:
                        style_id = char_style.get(f"{W}val")
                        props = styles[style_id].find(f"{W}rPr")
                        clauses.append(f"level character style {style_id} ({described(props)})")
                        self.assertEqual(len(label), 1, text)
                    elif label is not None:
                        clauses.append(f"level {described(label)}")
                self.assertEqual(text, f"Case {n:02d}: " + "; ".join(clauses))
            # A list and a definition of its own for every case, so each label reads "1.".
            self.assertEqual(len({i for i, _ in used}), count, name)
            self.assertEqual(len({d for _, d in used}), count, name)

    def test_open_type_cases_have_the_markup_their_labels_name(self):
        expected = {
            "plain": set(),
            "standard ligatures": {f"{W14}ligatures"},
            "kerning from 1 pt": {f"{W}kern"},
            "standard ligatures and kerning from 1 pt": {f"{W}kern", f"{W14}ligatures"},
        }
        names = {
            "E-opentype-on": (15, 1), "E-opentype-off": (15, 0), "E-opentype-none": (15, None),
            "E-opentype-12": (12, None),
        }
        for name, (mode, setting) in names.items():
            doc = next(doc for doc in self.docs if doc.name == name)
            values = dict(doc.compat_values)
            self.assertEqual(doc.mode, mode, name)
            self.assertEqual(values.get("enableOpenTypeFeatures", 1 if mode == 15 else None),
                             setting, name)
            said = {1: "on", 0: "off", None: "not written"}[setting]
            self.assertIn(f"mode {mode} with the setting enableOpenTypeFeatures {said}",
                          doc.purpose)
            paragraphs = part(rules.parts(doc), "word/document.xml").find(f"{W}body")
            paragraphs = paragraphs.findall(f"{W}p")
            labels, samples = paragraphs[0::2], paragraphs[1::2]
            self.assertEqual(len(labels), len(expected), name)
            seen = []
            for n, (label, sample) in enumerate(zip(labels, samples), 1):
                text = "".join(t.text for t in label.iter(f"{W}t"))
                self.assertTrue(text.startswith(f"Case {n:02d}: "), text)
                case = text.removeprefix(f"Case {n:02d}: ")
                seen.append(case)
                (r,) = sample.findall(f"{W}r")
                # Standard ligatures (fi, ff, ffl) and kerning pairs (AV, VA, Ty).
                self.assertEqual(r.find(f"{W}t").text, "office affine fluffy AVATAR Type", name)
                props = r.find(f"{W}rPr")
                found = set() if props is None else {c.tag for c in props}
                # No font or size of its own: the document defaults, Calibri 11 pt.
                self.assertFalse(found & {f"{W}rFonts", f"{W}sz", f"{W}szCs"}, text)
                self.assertEqual(found, expected[case], text)
                if props is not None and props.find(f"{W}kern") is not None:
                    self.assertEqual(props.find(f"{W}kern").get(f"{W}val"), "2", text)
                if props is not None and props.find(f"{W14}ligatures") is not None:
                    value = props.find(f"{W14}ligatures").get(f"{W14}val")
                    self.assertEqual(value, "standard", text)
            self.assertEqual(sorted(seen), sorted(expected), name)
            styles = part(rules.parts(doc), "word/styles.xml")
            defaults = styles.find(f"{W}docDefaults/{W}rPrDefault/{W}rPr")
            self.assertEqual(defaults.find(f"{W}rFonts").get(f"{W}ascii"), "Calibri", name)
            self.assertEqual(defaults.find(f"{W}sz").get(f"{W}val"), "22", name)
            normal = next(s for s in styles.iter(f"{W}style") if s.get(f"{W}styleId") == "Normal")
            self.assertIsNone(normal.find(f"{W}rPr"), name)
            # The w14 namespace is declared, and ignorable, where it is used.
            document = dict(rules.parts(doc))["word/document.xml"].decode()
            self.assertIn('mc:Ignorable="w14"', document)

    def test_only_documents_with_w14_markup_declare_its_namespace(self):
        for doc in self.docs:
            document = dict(rules.parts(doc))["word/document.xml"].decode()
            self.assertEqual("xmlns:w14=" in document, "<w14:" in document, doc.name)

    def test_default_after_normal_cases_name_their_styles(self):
        doc = next(d for d in self.docs if d.name == "E-default-paragraph-style-after-normal")
        parts = rules.parts(doc)
        styles = part(parts, "word/styles.xml")
        paragraph_styles = [
            (s.get(f"{W}styleId"), s.get(f"{W}default"))
            for s in styles.iter(f"{W}style")
            if s.get(f"{W}type") == "paragraph"
        ]
        self.assertEqual(paragraph_styles, [("Normal", "1"), ("LaterDefault", "1")])
        later = next(s for s in styles.iter(f"{W}style") if s.get(f"{W}styleId") == "LaterDefault")
        self.assertEqual(later.find(f"{W}pPr/{W}jc").get(f"{W}val"), "center")
        self.assertEqual(later.find(f"{W}pPr/{W}spacing").get(f"{W}after"), "480")
        self.assertIn("Normal (left-aligned, no space after)", doc.purpose)
        self.assertIn("LaterDefault (centred, 24 pt after), written after it", doc.purpose)
        body = part(parts, "word/document.xml").find(f"{W}body")
        for n, paragraph in enumerate(body.findall(f"{W}p"), 1):
            text = "".join(t.text for t in paragraph.iter(f"{W}t"))
            named = re.fullmatch(
                rf"Case {n:02d}: a paragraph that names (no style|\w+)(, after the others)?", text
            )
            self.assertIsNotNone(named, text)
            style = paragraph.find(f"{W}pPr/{W}pStyle")
            self.assertEqual(
                None if style is None else style.get(f"{W}val"),
                None if named.group(1) == "no style" else named.group(1),
                text,
            )

    def test_table_style_cases_have_the_markup_their_labels_name(self):
        def doc_parts(name):
            parts = rules.parts(next(doc for doc in self.docs if doc.name == name))
            styles = {
                s.get(f"{W}styleId"): s for s in part(parts, "word/styles.xml").iter(f"{W}style")
            }
            body = part(parts, "word/document.xml").find(f"{W}body")
            return styles, list(body)

        def cases(children):
            """(label without the case number, table) of each labelled table, checking the
            numbers and that an empty paragraph follows every table."""
            found = []
            for i, child in enumerate(children):
                if child.tag != f"{W}tbl":
                    continue
                label = "".join(t.text for t in children[i - 1].iter(f"{W}t"))
                n = len(found) + 1
                self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
                if i + 1 < len(children):
                    after = children[i + 1]
                    self.assertTrue(after.tag == f"{W}p" and not list(after.iter(f"{W}t")))
                found.append((label.removeprefix(f"Case {n:02d}: "), child))
            return found

        def cell_texts(table):
            return [
                ["".join(t.text for t in tc.iter(f"{W}t")) for tc in tr.findall(f"{W}tc")]
                for tr in table.findall(f"{W}tr")
            ]

        def conditions(style):
            return {
                c.get(f"{W}type"): (
                    c.find(f"{W}rPr/{W}color").get(f"{W}val"),
                    c.find(f"{W}tcPr/{W}shd").get(f"{W}fill"),
                )
                for c in style.findall(f"{W}tblStylePr")
            }

        def look_of(table):
            element = table.find(f"{W}tblPr/{W}tblLook")
            return None if element is None else {
                k.replace(W, ""): v for k, v in element.attrib.items()
            }

        def named(rows, cols):
            return [[f"r{r}c{c}" for c in range(1, cols + 1)] for r in range(1, rows + 1)]

        # E-table-style-look.
        styles, children = doc_parts("E-table-style-look")
        found = conditions(styles["Conditions"])
        # Every conditional format of ST_TblStyleOverrideType but the whole table.
        self.assertEqual(sorted(found), sorted([
            "firstRow", "lastRow", "firstCol", "lastCol", "band1Vert", "band2Vert", "band1Horz",
            "band2Horz", "neCell", "nwCell", "seCell", "swCell",
        ]))
        # Band sizes of 1 written: without them Word's default of 0 turns banding off.
        for kind in ("Row", "Col"):
            size = styles["Conditions"].find(f"{W}tblPr/{W}tblStyle{kind}BandSize")
            self.assertEqual(size.get(f"{W}val"), "1", kind)
        colours = [colour for colour, _ in found.values()]
        fills = [fill for _, fill in found.values()]
        self.assertEqual(len(set(colours)), len(colours))
        self.assertEqual(len(set(fills)), len(fills))
        self.assertNotIn("000000", colours)
        on = {"firstRow": "1", "lastRow": "1", "firstColumn": "1", "lastColumn": "1",
              "noHBand": "0", "noVBand": "0"}
        expected = {
            "every condition on": on,
            "no w:tblLook": None,
            "first row and last column on, the rest off": {
                "firstRow": "1", "lastRow": "0", "firstColumn": "0", "lastColumn": "1",
                "noHBand": "1", "noVBand": "1"},
            "only the old bitmask, w:val 04A0": {"val": "04A0"},
            "w:val 04A0 and w:firstRow 0": {"val": "04A0", "firstRow": "0"},
            "only w:firstRow 1": {"firstRow": "1"},
        }
        tables = cases(children)
        self.assertEqual([label for label, _ in tables], list(expected))
        for label, table in tables:
            self.assertEqual(look_of(table), expected[label], label)
            self.assertEqual(table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), "Conditions")
            self.assertEqual(cell_texts(table), named(5, 5), label)

        # E-table-style-bands.
        styles, children = doc_parts("E-table-style-bands")
        tables = cases(children)
        self.assertEqual(len(tables), 5)
        sizes = []
        for label, table in tables:
            style_id = table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val")
            style = styles[style_id]
            said = re.fullmatch(
                r"(row|column) band(?:s, no band size written| size (\d)"
                r"(, which Word restricts to 0 to 3)?)", label)
            self.assertIsNotNone(said, label)
            kind = "Row" if said.group(1) == "row" else "Col"
            size = style.find(f"{W}tblPr/{W}tblStyle{kind}BandSize")
            other = style.find(f"{W}tblPr/{W}tblStyle{'Col' if kind == 'Row' else 'Row'}BandSize")
            self.assertIsNone(other, label)
            self.assertEqual(None if size is None else size.get(f"{W}val"), said.group(2), label)
            self.assertEqual(bool(said.group(3)), said.group(2) is not None
                             and int(said.group(2)) > 3, label)
            sizes.append((said.group(1), said.group(2)))
            bands = {"Row": ("band1Horz", "band2Horz"), "Col": ("band1Vert", "band2Vert")}[kind]
            self.assertEqual(sorted(conditions(style)), sorted(bands), label)
            rows, cols = (6, 3) if kind == "Row" else (3, 6)
            self.assertEqual(cell_texts(table), named(rows, cols), label)
            self.assertEqual(look_of(table), {
                "firstRow": "0", "lastRow": "0", "firstColumn": "0", "lastColumn": "0",
                "noHBand": "0" if kind == "Row" else "1",
                "noVBand": "1" if kind == "Row" else "0"}, label)
        self.assertEqual(sizes, [("row", None), ("row", "0"), ("row", "1"), ("row", "2"),
                                 ("column", "2")])

        # E-table-style-bands-5: the size above Word's limit, alone.
        styles, children = doc_parts("E-table-style-bands-5")
        (label, table), = cases(children)
        self.assertEqual(label, "row band size 5, which Word restricts to 0 to 3")
        self.assertEqual(table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), "RowBands5")
        size = styles["RowBands5"].find(f"{W}tblPr/{W}tblStyleRowBandSize")
        self.assertEqual(size.get(f"{W}val"), "5")
        self.assertEqual(sorted(conditions(styles["RowBands5"])), ["band1Horz", "band2Horz"])
        self.assertEqual(cell_texts(table), named(8, 3))
        self.assertEqual(look_of(table), {
            "firstRow": "0", "lastRow": "0", "firstColumn": "0", "lastColumn": "0",
            "noHBand": "0", "noVBand": "1"})

        # E-table-style-size-*: Normal's size and the setting are the name's.
        sized = [doc for doc in self.docs if doc.name.startswith("E-table-style-size-")]
        self.assertEqual(
            sorted(doc.name for doc in sized),
            sorted(f"E-table-style-size-{n}pt{s}" for n in (11, 12, 13)
                   for s in ("", "-none", "-14")),
        )
        for doc in sized:
            size = int(re.search(r"-(\d+)pt", doc.name).group(1))
            self.assertEqual(doc.mode, 14 if doc.name.endswith("-14") else 15, doc.name)
            said = "on" if doc.name.endswith("pt") else "not written"
            if doc.mode == 14:
                said = "not written, in compatibility mode 14"
            self.assertTrue(doc.purpose.endswith(
                f"overrideTableStyleFontSizeAndJustification {said}"), doc.name)
            styles, children = doc_parts(doc.name)
            normal = styles["Normal"]
            self.assertEqual(normal.find(f"{W}rPr/{W}sz").get(f"{W}val"), str(size * 2))
            self.assertEqual(normal.find(f"{W}pPr/{W}jc").get(f"{W}val"), "left")
            big = styles["Big"]
            self.assertEqual(big.find(f"{W}rPr/{W}sz").get(f"{W}val"), "28")
            self.assertEqual(big.find(f"{W}pPr/{W}jc").get(f"{W}val"), "center")
            (label, table), = cases(children)
            self.assertEqual(
                label, f"a table in the style Big (14 pt, centred); Normal {size} pt, left")
            self.assertEqual(table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), "Big")
            # Nothing in the table's own paragraphs or runs competes with the style.
            for cell in table.iter(f"{W}tc"):
                self.assertIsNone(cell.find(f"{W}p/{W}pPr"))
                self.assertIsNone(cell.find(f"{W}p/{W}r/{W}rPr"))

        # E-table-style-not-allowed.
        styles, children = doc_parts("E-table-style-not-allowed")
        (first, with_width), (second, without), (third, wide), (fourth, fixed) = cases(children)
        self.assertEqual(third, "the style Width5000 gives only w:tblW 250 pt, otherwise the same")
        self.assertEqual(
            fourth, "the style FixedOnly gives only a fixed layout, otherwise the same")
        wide_pr = styles["Width5000"].find(f"{W}tblPr")
        self.assertEqual(wide_pr.find(f"{W}tblW").attrib, {f"{W}w": "5000", f"{W}type": "dxa"})
        self.assertIsNone(wide_pr.find(f"{W}tblLayout"))
        fixed_pr = styles["FixedOnly"].find(f"{W}tblPr")
        self.assertIsNone(fixed_pr.find(f"{W}tblW"))
        self.assertEqual(fixed_pr.find(f"{W}tblLayout").get(f"{W}type"), "fixed")
        width = styles["Width2000"].find(f"{W}tblPr")
        self.assertEqual(width.find(f"{W}tblW").attrib, {f"{W}w": "2000", f"{W}type": "dxa"})
        self.assertEqual(width.find(f"{W}tblLayout").get(f"{W}type"), "fixed")
        self.assertIn("w:tblW 100 pt and a fixed layout", first)
        # NoWidth is the autofit table the other three are measured against.
        self.assertIsNone(styles["NoWidth"].find(f"{W}tblPr/{W}tblW"))
        self.assertIsNone(styles["NoWidth"].find(f"{W}tblPr/{W}tblLayout"))
        self.assertIn("a grid of two 40 pt columns, each holding one word wider than 40 pt", first)
        self.assertEqual(second, "the style NoWidth, otherwise the same")
        for table, style_id in ((with_width, "Width2000"), (without, "NoWidth"),
                                (wide, "Width5000"), (fixed, "FixedOnly")):
            self.assertEqual(table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), style_id)
            self.assertIsNone(table.find(f"{W}tblPr/{W}tblW"))
            self.assertIsNone(table.find(f"{W}tblPr/{W}tblLayout"))
            self.assertIsNone(table.find(f".//{W}tcW"))
            # 80 pt in all, so a table that takes the style's 100 pt is wider than its grid.
            self.assertEqual([g.get(f"{W}w") for g in table.iter(f"{W}gridCol")],
                             ["800", "800"])
            # One word, wider than 40 pt in Calibri 11 pt, per cell.
            self.assertEqual(cell_texts(table), [["Hamburgefontsiv", "Hamburgefontsiv"]])

        def bold_of(styles, style_id):
            """The w:b of a style: True, False, or None when it has none."""
            element = styles[style_id].find(f"{W}rPr/{W}b")
            return None if element is None else element.get(f"{W}val") != "0"

        def cell_styles(table):
            """Per cell: (text, paragraph style, character style of its one run)."""
            out = []
            for paragraph in table.iter(f"{W}p"):
                p_style = paragraph.find(f"{W}pPr/{W}pStyle")
                runs = paragraph.findall(f"{W}r")
                self.assertEqual(len(runs), 1)
                r_style = runs[0].find(f"{W}rPr/{W}rStyle")
                self.assertTrue(runs[0].find(f"{W}rPr") is None or r_style is not None)
                if r_style is not None:
                    self.assertEqual(len(runs[0].find(f"{W}rPr")), 1)
                out.append((
                    "".join(t.text for t in paragraph.iter(f"{W}t")),
                    None if p_style is None else p_style.get(f"{W}val"),
                    None if r_style is None else r_style.get(f"{W}val"),
                ))
            return out

        # E-table-style-toggles: defaults not bold; a bold table style under a bold paragraph
        # style, Normal, and a bold character style. Each cell's text names its style.
        doc = next(d for d in self.docs if d.name == "E-table-style-toggles")
        self.assertEqual(doc.run_defaults_extra, "")
        styles, children = doc_parts(doc.name)
        (label, table), = cases(children)
        self.assertEqual(table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), "BoldTable")
        self.assertEqual(
            [bold_of(styles, s) for s in ("BoldTable", "BoldPara", "CharBold")],
            [True, True, True])
        self.assertEqual(cell_styles(table), [("in BoldPara", "BoldPara", None),
                                              ("in Normal", None, None),
                                              ("in CharBold", None, "CharBold")])
        self.assertIn("BoldPara (bold)", label)
        self.assertIn("CharBold (bold)", label)
        # Normal is not a style that flips bold: the predictions count on it.
        self.assertIsNone(styles["Normal"].find(f"{W}rPr"))

        # E-table-style-toggles-defaults: bold defaults; BoldOn over Normal, BoldOff over
        # Normal, CharBoldOff and CharBold.
        doc = next(d for d in self.docs if d.name == "E-table-style-toggles-defaults")
        self.assertEqual(doc.run_defaults_extra, "<w:b/><w:bCs/>")
        # Over bold defaults, w:b on shows only whether it toggles them off.
        self.assertIn("whether its w:b on toggles the defaults off", doc.purpose)
        styles, children = doc_parts(doc.name)
        (on_label, on), (off_label, off) = cases(children)
        self.assertEqual(on_label, "the table style BoldOn (w:b on); a paragraph in Normal")
        self.assertIn("the table style BoldOff (w:b off)", off_label)
        self.assertIn("CharBoldOff (w:b off)", off_label)
        self.assertIn("CharBold (w:b on)", off_label)
        self.assertEqual(on.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), "BoldOn")
        self.assertEqual(off.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), "BoldOff")
        self.assertEqual(
            [bold_of(styles, s) for s in ("BoldOn", "BoldOff", "CharBold", "CharBoldOff")],
            [True, False, True, False])
        self.assertEqual(cell_styles(on), [("in Normal", None, None)])
        self.assertEqual(cell_styles(off), [("in Normal", None, None),
                                            ("in CharBoldOff", None, "CharBoldOff"),
                                            ("in CharBold", None, "CharBold")])
        self.assertIsNone(styles["Normal"].find(f"{W}rPr"))

        # E-table-style-header-rows-on and -off: rows 1 and 2 repeat as header rows, the
        # style formats only its first row, and w:tblLook asks for that alone.
        for name in ("E-table-style-header-rows-on", "E-table-style-header-rows-off"):
            styles, children = doc_parts(name)
            (label, table), = cases(children)
            self.assertEqual(label, "rows 1 and 2 are header rows (w:tblHeader); the style "
                                    "formats its first row and bands rows by one")
            doc = next(d for d in self.docs if d.name == name)
            said = name.rsplit("-", 1)[1]
            self.assertIn(f"differentiateMultirowTableHeaders {said}:", doc.purpose)
            self.assertEqual(table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), "HeaderRows")
            self.assertEqual(sorted(conditions(styles["HeaderRows"])),
                             ["band1Horz", "band2Horz", "firstRow"])
            size = styles["HeaderRows"].find(f"{W}tblPr/{W}tblStyleRowBandSize")
            self.assertEqual(size.get(f"{W}val"), "1")
            self.assertEqual(look_of(table), {
                "firstRow": "1", "lastRow": "0", "firstColumn": "0", "lastColumn": "0",
                "noHBand": "0", "noVBand": "1"})
            headers = [tr.find(f"{W}trPr/{W}tblHeader") is not None
                       for tr in table.findall(f"{W}tr")]
            self.assertEqual(headers, [True, True, False, False, False, False], name)
            self.assertEqual(cell_texts(table), named(6, 3), name)

    def test_markup_word_does_not_allow_stays_in_its_own_document(self):
        # A file Word refuses must cost no other case: a band size above 3 (MS-OI29500
        # §17.7.6.7(c)) only in E-table-style-bands-5, and the elements a table style's
        # w:tblPr may not hold (§17.7.6.4(a)) only in E-table-style-not-allowed.
        disallowed = {"bidiVisual", "tblLayout", "tblLook", "tblOverlap", "tblpPr", "tblStyle",
                      "tblW"}
        for doc in self.docs:
            styles = part(rules.parts(doc), "word/styles.xml")
            bands, held = [], set()
            for style in styles.iter(f"{W}style"):
                tbl_pr = style.find(f"{W}tblPr")
                if tbl_pr is None:
                    continue
                for tag in ("tblStyleRowBandSize", "tblStyleColBandSize"):
                    size = tbl_pr.find(f"{W}{tag}")
                    if size is not None:
                        bands.append(int(size.get(f"{W}val")))
                held |= {c.tag.replace(W, "") for c in tbl_pr} & disallowed
            over = [size for size in bands if not 0 <= size <= 3]
            self.assertEqual(bool(over), doc.name == "E-table-style-bands-5", doc.name)
            self.assertEqual(bool(held), doc.name == "E-table-style-not-allowed", doc.name)

    def test_line_and_drawing_cases_have_the_markup_their_labels_name(self):
        def cases(name):
            """Per paragraph with a case label: the label without its number, the sample
            runs (text, run properties by tag, tab or not) and the w:pPr of the paragraph
            that holds them: the label's own, or that of the paragraph after it."""
            doc = next(doc for doc in self.docs if doc.name == name)
            body = part(rules.parts(doc), "word/document.xml").find(f"{W}body")
            found, n = [], 0
            for paragraph in body.findall(f"{W}p"):
                runs = paragraph.findall(f"{W}r")
                first = "".join(t.text for t in runs[0].iter(f"{W}t")) if runs else ""
                if not first.startswith("Case "):
                    # A sample paragraph of its own: its runs and its w:pPr are the case's.
                    label, samples, _ = found[-1]
                    samples.extend(sample(r) for r in runs)
                    found[-1] = (label, samples, paragraph.find(f"{W}pPr"))
                    continue
                n += 1
                self.assertTrue(first.startswith(f"Case {n:02d}: "), first)
                label = first.removeprefix(f"Case {n:02d}: ").rstrip(": ")
                found.append((label, [sample(r) for r in runs[1:]], paragraph.find(f"{W}pPr")))
            return found

        def sample(r):
            props = r.find(f"{W}rPr")
            tags = {} if props is None else {
                c.tag.replace(W, ""): {k.replace(W, ""): v for k, v in c.attrib.items()}
                for c in props
            }
            # The text in order, a soft hyphen (w:softHyphen) as U+00AD.
            value = "".join(
                (c.text or "") if c.tag == f"{W}t" else "\u00ad"
                for c in r
                if c.tag in (f"{W}t", f"{W}softHyphen")
            )
            return value, tags, r.find(f"{W}tab") is not None

        def tabs(p_pr):
            stops = [] if p_pr is None else p_pr.findall(f"{W}tabs/{W}tab")
            return [{k.replace(W, ""): v for k, v in t.attrib.items()} for t in stops]

        # E-spacing-condensed: every sample run is condensed by the label's amount.
        found = cases("E-spacing-condensed")
        self.assertEqual(len(found), 10)
        for label, samples, p_pr in found:
            points = int(re.match(r"condensed by (\d) pt", label).group(1))
            text_runs = [s for s in samples if not s[2]]
            self.assertTrue(text_runs, label)
            expected = {"spacing": {"val": str(-points * 20)}}
            if "24 pt text" in label:
                expected.update(sz={"val": "48"}, szCs={"val": "48"})
            for _, tags, _ in text_runs:
                self.assertEqual(tags, expected, label)
            if "soft hyphens" in label:
                # One word of 150 letters, a soft hyphen after each syllable, no space.
                word = text_runs[0][0]
                self.assertNotIn(" ", word, label)
                self.assertEqual(len(word.replace("\u00ad", "")), 150, label)
                self.assertEqual(word.count("\u00ad"), 69, label)
                ind = p_pr.find(f"{W}ind")
                self.assertEqual(int(ind.get(f"{W}right")), rules.TEXT_W - 3000)
                # At -6 pt most letters at 11 pt would take no room.
                self.assertEqual("24 pt text" in label, points == 6, label)
            if "zero-width" in label:
                self.assertEqual(text_runs[0][0].count("\u200b"), 2)
            if "right tab" in label:
                self.assertEqual(tabs(p_pr), [{"val": "right", "pos": str(rules.TEXT_W)}])
                self.assertTrue(samples[0][2], label)

        # E-tab-margin: the stops the labels name.
        found = cases("E-tab-margin")
        edge = rules.TEXT_W + 360, rules.PAGE_W - rules.MARGIN + 360
        expected = [
            ("a left stop 18 pt into the right margin", [{"val": "left", "pos": str(edge[0])}]),
            ("a left stop 18 pt past the right edge of the page",
             [{"val": "left", "pos": str(edge[1])}]),
            ("a right stop 18 pt into the right margin", [{"val": "right", "pos": str(edge[0])}]),
        ]
        self.assertEqual([(label, tabs(p_pr)) for label, _, p_pr in found[:3]], expected)
        for label, samples, _ in found[3:]:
            count = int(re.match(r"(\d+) x, then an opening bracket and two tabs", label).group(1))
            self.assertEqual(
                [(value, is_tab) for value, _, is_tab in samples],
                [("x" * count + "(", False), ("", True), ("", True), ("end", False)], label)
        self.assertEqual(len(found), 8)

        # E-highlight: each sample the label calls highlighted is.
        found = cases("E-highlight")
        self.assertEqual(len(found), 8)
        for label, samples, p_pr in found:
            marked = [tags for _, tags, _ in samples if "highlight" in tags]
            self.assertTrue(marked, label)
            for tags in marked:
                self.assertEqual(tags["highlight"], {"val": "darkYellow"}, label)
            raised = {"superscript": "superscript", "subscript": "subscript"}
            for word, value in raised.items():
                if f"highlighted {word}" in label:
                    self.assertEqual(marked[0]["vertAlign"], {"val": value}, label)
            if "raised 6 pt" in label:
                self.assertEqual(marked[0]["position"], {"val": "12"}, label)
            if "exact 9 pt" in label:
                spacing = p_pr.find(f"{W}spacing")
                self.assertEqual((spacing.get(f"{W}line"), spacing.get(f"{W}lineRule")),
                                 ("180", "exact"))
            if "justified" in label:
                self.assertEqual(p_pr.find(f"{W}jc").get(f"{W}val"), "both")
                self.assertIn("highlighted   highlighted", samples[0][0], label)
                self.assertNotIn("highlighted    highlighted", samples[0][0], label)
            if "run shading" in label:
                self.assertIn("shd", marked[0], label)
            if "tab" in label:
                self.assertTrue(any(is_tab and "highlight" in tags
                                    for _, tags, is_tab in samples), label)

        # E-underline: every kind of ST_Underline once, in its own case.
        found = cases("E-underline")
        kinds = [
            "single", "words", "double", "thick", "dotted", "dottedHeavy", "dash", "dashedHeavy",
            "dashLong", "dashLongHeavy", "dotDash", "dashDotHeavy", "dotDotDash",
            "dashDotDotHeavy", "wave", "wavyHeavy", "wavyDouble",
        ]
        for (label, samples, _), kind in zip(found, kinds):
            self.assertEqual(label, f"underline {kind}")
            ((_, tags, _),) = samples
            self.assertEqual(tags, {"u": {"val": kind}}, label)
        rest = found[len(kinds):]
        self.assertEqual(len(rest), 6)
        for label, samples, p_pr in rest:
            for _, tags, _ in samples:
                expected_kind = "words" if label.startswith("words only") else "single"
                self.assertEqual(tags["u"], {"val": expected_kind}, label)
            if "small capitals" in label:
                self.assertIn("smallCaps", samples[0][1])
            if "superscript" in label:
                self.assertEqual([t.get("vertAlign") for _, t, _ in samples],
                                 [None, {"val": "superscript"}, None])
            if "under a tab" in label:
                self.assertTrue(samples[1][2], label)
            if "soft hyphen" in label:
                self.assertIn("\u00ad", samples[0][0], label)
                ind = p_pr.find(f"{W}ind")
                self.assertEqual(int(ind.get(f"{W}right")), rules.TEXT_W - 3000, label)
            if "three spaces" in label:
                self.assertIn("underlined   underlined", samples[0][0], label)
                self.assertNotIn("underlined    underlined", samples[0][0], label)
        trailing = [(label, p_pr) for label, _, p_pr in rest[-2:]]
        self.assertEqual(
            [(label, p_pr is not None and p_pr.find(f"{W}jc") is not None)
             for label, p_pr in trailing],
            [("underlined words ending a line in three spaces, left aligned", False),
             ("underlined words ending a line in three spaces, justified", True)])
        # E-underline-trailspace has the same two cases, with the setting on.
        again = cases("E-underline-trailspace")
        self.assertEqual([(l, s) for l, s, _ in again], [(l, s) for l, s, _ in rest[-2:]])

        # E-strike.
        found = cases("E-strike")
        self.assertEqual(
            sorted(label for label, _, _ in found),
            sorted(f"{k} strikethrough, {n} pt" for k in ("single", "double") for n in (11, 24)))
        for label, ((_, tags, _),), _ in found:
            kind = "strike" if label.startswith("single") else "dstrike"
            size = str(int(re.search(r"(\d+) pt", label).group(1)) * 2)
            self.assertEqual(tags, {kind: {}, "sz": {"val": size}, "szCs": {"val": size}}, label)

        # E-tab-leader: every leader to every kind of stop, then the three variants. Each
        # label is a paragraph of its own, so that its text does not pass the stop: the
        # sample paragraph starts with its tab.
        doc = next(doc for doc in self.docs if doc.name == "E-tab-leader")
        for paragraph in part(rules.parts(doc), "word/document.xml").iter(f"{W}p"):
            text = "".join(t.text for t in paragraph.iter(f"{W}t"))
            if text.startswith("Case "):
                self.assertIsNone(paragraph.find(f".//{W}tab"), text)
                self.assertIsNone(paragraph.find(f"{W}pPr/{W}tabs"), text)
            else:
                first = paragraph.find(f"{W}r")
                self.assertIsNotNone(first.find(f"{W}tab"), text)
        found = cases("E-tab-leader")
        leaders = ["dot", "hyphen", "middleDot", "underscore", "heavy"]
        stops = ["left", "right", "center", "decimal"]
        self.assertEqual(
            [label for label, _, _ in found[:20]],
            [f"{leader} leader to a {kind} stop at 216 pt" for leader in leaders for kind in stops])
        for label, samples, p_pr in found[:20]:
            leader, kind = re.match(r"(\w+) leader to a (\w+) stop", label).groups()
            self.assertEqual(tabs(p_pr), [{"val": kind, "leader": leader, "pos": "4320"}])
            self.assertTrue(samples[0][2], label)
        variants = {
            "dot leader at 1 pt": {"sz": {"val": "2"}, "szCs": {"val": "2"}},
            "dot leader at a character scale of 50 %": {"w": {"val": "50"}},
            "dot leader under an underlined tab": {"u": {"val": "single"}},
        }
        self.assertEqual([label for label, _, _ in found[20:]], list(variants))
        for label, samples, p_pr in found[20:]:
            self.assertEqual(tabs(p_pr), [{"val": "left", "leader": "dot", "pos": "4320"}])
            self.assertEqual((samples[0][1], samples[0][2]), (variants[label], True), label)

        # E-bar-tab: a bar stop at 144 pt in every case, and the spacing the label names.
        found = cases("E-bar-tab")
        self.assertEqual(len(found), 7)
        for label, _, p_pr in found:
            self.assertEqual(tabs(p_pr), [{"val": "bar", "pos": "2880"}], label)
            spacing = p_pr.find(f"{W}spacing")
            values = {} if spacing is None else {
                k.replace(W, ""): v for k, v in spacing.attrib.items()
            }
            expected = {
                "1.5 lines spacing": {"line": "360", "lineRule": "auto"},
                "exact 18 pt spacing": {"line": "360", "lineRule": "exact"},
                "12 pt before and after": {"before": "240", "after": "240"},
            }.get(label, {})
            self.assertEqual(values, expected, label)
            shading = p_pr.find(f"{W}shd")
            self.assertEqual(shading is not None, label == "dark paragraph shading", label)

        # E-auto-color: every grey from 10 % to 90 % and the colours, as run shading and as
        # paragraph shading, with no colour of the text's own.
        found = cases("E-auto-color")
        self.assertEqual(len(found), 28)
        greys = {f"{k * 10} % grey" for k in range(1, 10)}
        named = set()
        for label, samples, p_pr in found:
            kind, name, fill = re.fullmatch(r"(run|paragraph) shading (.+) \((\w{6})\)",
                                            label).groups()
            named.add((kind, name))
            if name in greys:
                level = round(255 * (1 - int(name.split()[0]) / 100))
                self.assertEqual(fill, f"{level:02X}" * 3, label)
            ((_, tags, _),) = samples
            self.assertNotIn("color", tags, label)
            if kind == "run":
                self.assertEqual(tags["shd"]["fill"], fill, label)
                self.assertIsNone(p_pr, label)
            else:
                self.assertEqual(tags, {}, label)
                self.assertEqual(p_pr.find(f"{W}shd").get(f"{W}fill"), fill, label)
        names = greys | {"red", "green", "blue", "yellow", "dark blue"}
        self.assertEqual(named, {(k, n) for k in ("run", "paragraph") for n in names})

        # E-superscript.
        found = cases("E-superscript")
        self.assertEqual(
            sorted(label for label, _, _ in found),
            sorted(f"{k} at {n} pt" for k in ("superscript", "subscript") for n in (11, 24)))
        for label, (base, raised), _ in found:
            kind, size = re.fullmatch(r"(\w+) at (\d+) pt", label).groups()
            half = {"val": str(int(size) * 2)}
            self.assertEqual(base[1], {"sz": half, "szCs": half}, label)
            self.assertEqual(raised[1], {"sz": half, "szCs": half, "vertAlign": {"val": kind}})

    def test_page_cases_have_the_markup_their_labels_name(self):
        R = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"

        def load(name):
            doc = next(doc for doc in self.docs if doc.name == name)
            parts = dict(rules.parts(doc))
            body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")
            rels = ElementTree.fromstring(parts["word/_rels/document.xml.rels"])
            targets = {r.get("Id"): r.get("Target") for r in rels}
            return doc, parts, body, targets

        def lines_of(paragraph):
            """The lines of a paragraph: its text split at line breaks."""
            out = [""]
            for element in paragraph.iter():
                if element.tag == f"{W}t":
                    out[-1] += element.text or ""
                elif element.tag == f"{W}br":
                    out.append("")
            return out

        def flag(paragraph, tag):
            return paragraph.find(f"{W}pPr/{W}{tag}") is not None

        def sections(body, doc):
            """Per section: its paragraphs and its w:sectPr."""
            found, current = [], []
            for paragraph in body.findall(f"{W}p"):
                current.append(paragraph)
                sect = paragraph.find(f"{W}pPr/{W}sectPr")
                if sect is not None:
                    found.append((current, sect))
                    current = []
            found.append((current, body.find(f"{W}sectPr")))
            return found

        def kind(sect):
            element = sect.find(f"{W}type")
            return None if element is None else element.get(f"{W}val")

        def header_text(parts, targets, sect, element="headerReference"):
            refs = sect.findall(f"{W}{element}")
            return ["".join(t.text for t in ElementTree.fromstring(
                parts["word/" + targets[r.get(f"{R}id")]]).iter(f"{W}t")) for r in refs]

        # Every paragraph of the page documents has exact 12 pt lines, so that line counts
        # are what the labels say.
        names = ["E-section-odd", "E-section-odd-first", "E-section-continuous-headers",
                 "E-section-next-column", "E-keeplines-long", "E-keepnext-chain",
                 "E-page-break-before-top", "E-para-shading", "E-header-tall",
                 "E-header-space-after", "E-header-negative-margin"]
        for name in names:
            _, parts, body, _ = load(name)
            # The headers and footers too, so that their heights are known whatever the font.
            stories = [body] + [ElementTree.fromstring(data) for part_name, data in parts.items()
                                if re.fullmatch(r"word/(header|footer)\d+\.xml", part_name)]
            for story in stories:
                for paragraph in story.findall(f"{W}p"):
                    spacing = paragraph.find(f"{W}pPr/{W}spacing")
                    self.assertIsNotNone(spacing, name)
                    self.assertEqual((spacing.get(f"{W}line"), spacing.get(f"{W}lineRule")),
                                     ("240", "exact"), name)
        self.assertEqual(rules.LINES_PER_PAGE, int((rules.PAGE_H - 2 * rules.MARGIN) / 240))

        # E-section-odd: each section's label says its start type and numbering, and the
        # parity of the page the section before ends on, as the start types and page counts
        # give it (no page number is restarted before Section 6).
        doc, parts, body, targets = load("E-section-odd")
        found = sections(body, doc)
        self.assertEqual(len(found), 7)
        parity = {"odd": 1, "even": 0}
        last_page = 0  # of the section before, by its parity only
        for n, (paragraphs, sect) in enumerate(found, 1):
            label = lines_of(paragraphs[0])[0]
            said = re.fullmatch(
                rf"Section {n}: (?:one page|(odd|even) page start(?: after a section that ends "
                r"on an? (odd|even) page)?(, page numbers restarted at 1)?(; two pages)?)",
                label)
            self.assertIsNotNone(said, label)
            start, before, restart, two = said.groups()
            self.assertEqual(kind(sect), None if start is None else f"{start}Page", label)
            numbers = sect.find(f"{W}pgNumType")
            self.assertEqual(numbers is not None and numbers.get(f"{W}start") == "1",
                             bool(restart), label)
            if before is not None:
                self.assertEqual(last_page % 2, parity[before], label)
            pages = len(paragraphs)
            self.assertEqual(pages, 2 if two else 1, label)
            for paragraph in paragraphs[1:]:
                self.assertTrue(flag(paragraph, "pageBreakBefore"), label)
            first_page = last_page + 1
            if start is not None and first_page % 2 != parity[start]:
                first_page += 1
            last_page = first_page + pages - 1
            # The header and footer are named once, by the first section.
            refs = sect.findall(f"{W}headerReference") + sect.findall(f"{W}footerReference")
            self.assertEqual(len(refs), 2 if n == 1 else 0, label)
        # Section 6 begins after an odd page, on an even physical page numbered 1, and so
        # tells the two rules apart; Section 7 follows a section of one page.
        self.assertIn("ends on an odd page, page numbers restarted", lines_of(found[5][0][0])[0])
        self.assertEqual(len(found[5][0]), 1)
        self.assertEqual(header_text(parts, targets, found[0][1]), ["header"])
        footer = parts["word/" + targets[found[0][1].find(f"{W}footerReference").get(f"{R}id")]]
        self.assertIn(b'w:instr=" PAGE "', footer)

        # E-section-odd-first.
        doc, parts, body, targets = load("E-section-odd-first")
        (first, sect), (second, last) = sections(body, doc)
        self.assertIn("even page start", lines_of(first[0])[0])
        self.assertEqual((kind(sect), kind(last)), ("evenPage", None))

        # E-section-continuous-headers.
        doc, parts, body, targets = load("E-section-continuous-headers")
        (first, sect), (second, last) = sections(body, doc)
        self.assertEqual(len(lines_of(first[0])), 29)
        self.assertEqual(len(lines_of(second[0])), 60)
        self.assertEqual((kind(sect), kind(last)), (None, "continuous"))
        self.assertEqual(header_text(parts, targets, sect), ["Header A"])
        self.assertEqual(header_text(parts, targets, last), ["Header B"])
        self.assertIn("Header A", lines_of(first[0])[0])
        self.assertIn("Header B", lines_of(second[0])[0])

        # E-section-next-column.
        doc, parts, body, targets = load("E-section-next-column")
        (_, sect), (_, last) = sections(body, doc)
        self.assertEqual((kind(sect), kind(last)), (None, "nextColumn"))

        # E-keeplines-long.
        _, _, body, _ = load("E-keeplines-long")
        before, long = body.findall(f"{W}p")
        self.assertEqual(len(lines_of(before)), 29)
        self.assertEqual(len(lines_of(long)), 80)
        self.assertTrue(len(lines_of(long)) > rules.LINES_PER_PAGE)
        self.assertEqual((flag(before, "keepLines"), flag(long, "keepLines")), (False, True))

        # E-keepnext-chain: each case's filler has the lines its label says, and the chain
        # is that many one-line paragraphs, all but the last with keep with next.
        doc, _, body, _ = load("E-keepnext-chain")
        paragraphs = body.findall(f"{W}p")
        starts = [i for i, p in enumerate(paragraphs)
                  if re.match(r"Case \d\d: \d+ lines", lines_of(p)[0])]
        self.assertEqual(len(starts), 5)
        for number, start in enumerate(starts, 1):
            label = lines_of(paragraphs[start])[0]
            self.assertTrue(label.startswith(f"Case {number:02d}: "), label)
            count = int(re.search(r": (\d+) lines", label).group(1))
            self.assertEqual(len(lines_of(paragraphs[start])), count, label)
            self.assertEqual(flag(paragraphs[start], "pageBreakBefore"), number > 1, label)
            size = int(re.search(r"chain of (\d+)", label).group(1))
            chain = paragraphs[start + 1 : start + 1 + size]
            self.assertEqual([lines_of(p)[0] for p in chain],
                             [f"Case {number:02d} chain {k}" for k in range(1, size + 1)], label)
            keeps = [flag(p, "keepNext") for p in chain]
            if "section break" in label:
                # The chain's last paragraph keeps with the next, which is past the break: the
                # chain's section ends there, and the type of the break is that of the
                # section after it, in its own w:sectPr (ECMA-376 §17.6.22).
                self.assertEqual(keeps, [True] * size, label)
                self.assertIsNotNone(chain[-1].find(f"{W}pPr/{W}sectPr"), label)
                after_chain = paragraphs[start + 1 + size :]
                sect = next((p.find(f"{W}pPr/{W}sectPr") for p in after_chain
                             if p.find(f"{W}pPr/{W}sectPr") is not None), body.find(f"{W}sectPr"))
                said = re.search(r"ends at a (next page|continuous) section break", label)
                self.assertIsNotNone(said, label)
                expected = {"next page": "nextPage", "continuous": "continuous"}[said.group(1)]
                self.assertEqual(kind(sect), expected, label)
            else:
                self.assertEqual(keeps, [True] * (size - 1) + ["before a paragraph with page "
                                 "break before" in label], label)
            after = paragraphs[start + 1 + size] if start + 1 + size < len(paragraphs) else None
            if "page break before" in label:
                self.assertTrue(flag(after, "pageBreakBefore"), label)
            if "fits a page" in label:
                self.assertTrue(size <= rules.LINES_PER_PAGE < count + size, label)
            if "70 lines" in label:
                self.assertTrue(size > rules.LINES_PER_PAGE, label)
            if number in (3, 5):
                self.assertTrue(count + size <= rules.LINES_PER_PAGE, label)
            if number == 4:
                self.assertEqual(len(lines_of(after)), 2, label)
                self.assertTrue(count + size <= rules.LINES_PER_PAGE < count + size + 2, label)

        # E-page-break-before-top.
        _, _, body, _ = load("E-page-break-before-top")
        first, fill, second = body.findall(f"{W}p")
        self.assertTrue(flag(first, "pageBreakBefore") and flag(second, "pageBreakBefore"))
        self.assertEqual(1 + len(lines_of(fill)), rules.LINES_PER_PAGE)
        self.assertFalse(flag(fill, "pageBreakBefore"))

        # E-para-shading.
        _, _, body, _ = load("E-para-shading")
        paragraphs = body.findall(f"{W}p")
        shaded = [p for p in paragraphs if p.find(f"{W}pPr/{W}shd") is not None]
        self.assertEqual(len(shaded), 6)
        for paragraph in shaded:
            label = lines_of(paragraph)[0]
            ind = paragraph.find(f"{W}pPr/{W}ind")
            values = {} if ind is None else {k.replace(W, ""): v for k, v in ind.attrib.items()}
            expected = {}
            if "hanging first line" in label:
                expected = {"left": "1440", "hanging": "720"}
            if "negative indents" in label:
                expected = {"left": "-360", "right": "-360"}
            self.assertEqual(values, expected, label)
            spacing = paragraph.find(f"{W}pPr/{W}spacing")
            said = re.search(r"(\d+) pt before and after", label)
            points = (str(int(said.group(1)) * 20),) * 2 if said else (None, None)
            self.assertEqual((spacing.get(f"{W}before"), spacing.get(f"{W}after")), points, label)
        # Cases 01 to 03 stand apart, between unshaded paragraphs; Cases 04 and 05 touch.
        cases = [paragraphs.index(p) for p in shaded[:5]]
        self.assertEqual([b - a for a, b in zip(cases, cases[1:])], [2, 2, 2, 1])
        self.assertIn("first of two shaded paragraphs in a row", lines_of(shaded[3])[0])
        self.assertIn("second of two shaded paragraphs in a row", lines_of(shaded[4])[0])
        # Case 06 starts on the line its label says, on a page of its own, and splits.
        split, before = shaded[-1], paragraphs[-2]
        self.assertIs(paragraphs[-1], split)
        self.assertTrue(flag(before, "pageBreakBefore"))
        first = int(re.search(r"starts on line (\d+)", lines_of(split)[0]).group(1))
        self.assertEqual(len(lines_of(before)) + 1, first)
        self.assertEqual(len(lines_of(split)), 10)
        # At least two lines on each page, so widow control leaves the split where it is.
        on_first = rules.LINES_PER_PAGE - first + 1
        self.assertTrue(2 <= on_first <= len(lines_of(split)) - 2, on_first)

        # The header documents.
        for name, top, header_lines, footer_lines in (
            ("E-header-tall", "1440", 8, 8),
            ("E-header-space-after", "1440", 2, None),
            ("E-header-negative-margin", "-1440", 8, 8),
        ):
            doc, parts, body, targets = load(name)
            sect = body.find(f"{W}sectPr")
            margins = sect.find(f"{W}pgMar")
            self.assertEqual((margins.get(f"{W}top"), margins.get(f"{W}bottom")), (top, top))
            distance = int(margins.get(f"{W}header")) / 20
            self.assertEqual(int(margins.get(f"{W}footer")) / 20, distance, name)
            margin = abs(int(top)) / 20
            header = ElementTree.fromstring(
                parts["word/" + targets[sect.find(f"{W}headerReference").get(f"{R}id")]])
            got = sum(len(lines_of(p)) for p in header.findall(f"{W}p"))
            self.assertEqual(got, header_lines, name)
            footer = sect.find(f"{W}footerReference")
            if footer_lines is None:
                self.assertIsNone(footer, name)
            else:
                root = ElementTree.fromstring(parts["word/" + targets[footer.get(f"{R}id")]])
                self.assertEqual(sum(len(lines_of(p)) for p in root.findall(f"{W}p")),
                                 footer_lines, name)
            (paragraph,) = body.findall(f"{W}p")
            self.assertEqual(len(lines_of(paragraph)), 70, name)
            if name == "E-header-space-after":
                last = header.findall(f"{W}p")[-1]
                self.assertEqual(last.find(f"{W}pPr/{W}spacing").get(f"{W}after"), "480")
                # Inside the margin without the space after, past it with it.
                self.assertTrue(distance + 12 * header_lines < margin
                                < distance + 12 * header_lines + 24, name)
            else:
                # The header and the footer are taller than their margins.
                self.assertTrue(distance + 12 * header_lines > margin, name)
                self.assertTrue(distance + 12 * footer_lines > margin, name)

    def test_second_page_cases_have_the_markup_their_labels_name(self):
        R = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"

        def load(name):
            doc = next(doc for doc in self.docs if doc.name == name)
            parts = dict(rules.parts(doc))
            body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")
            rels = ElementTree.fromstring(parts["word/_rels/document.xml.rels"])
            targets = {r.get("Id"): r.get("Target") for r in rels}
            return parts, body, targets

        def lines_of(paragraph):
            """The lines of a paragraph: its text split at line and page breaks."""
            out = [""]
            for element in paragraph.iter():
                if element.tag == f"{W}t":
                    out[-1] += element.text or ""
                elif element.tag == f"{W}br":
                    out.append("")
            return out

        def page_breaks(paragraph):
            return [b for b in paragraph.iter(f"{W}br") if b.get(f"{W}type") == "page"]

        def flag(paragraph, tag):
            element = paragraph.find(f"{W}pPr/{W}{tag}")
            return element is not None and element.get(f"{W}val") != "0"

        def sections(body):
            """Per section: its paragraphs and its w:sectPr."""
            found, current = [], []
            for paragraph in body.findall(f"{W}p"):
                current.append(paragraph)
                sect = paragraph.find(f"{W}pPr/{W}sectPr")
                if sect is not None:
                    found.append((current, sect))
                    current = []
            found.append((current, body.find(f"{W}sectPr")))
            return found

        def kind(sect):
            element = sect.find(f"{W}type")
            return None if element is None else element.get(f"{W}val")

        def story(parts, targets, sect, element, ref_type="default"):
            """The root of the header or footer a section names for a kind of page."""
            ref = next((r for r in sect.findall(f"{W}{element}")
                        if r.get(f"{W}type") == ref_type), None)
            return None if ref is None else ElementTree.fromstring(
                parts["word/" + targets[ref.get(f"{R}id")]])

        def story_text(root):
            return "".join(t.text for t in root.iter(f"{W}t"))

        def instructions(root):
            return [f.get(f"{W}instr").strip() for f in root.iter(f"{W}fldSimple")]

        def story_lines(root):
            return sum(len(lines_of(p)) for p in root.findall(f"{W}p"))

        lines_per_page, page_height = rules.LINES_PER_PAGE, rules.PAGE_H / 20
        distance = 708 / 20  # of headers and footers from the page's edge

        # Every paragraph of these documents and of their headers and footers has exact
        # 12 pt lines, so that line counts are what the labels say.
        for name in ("E-section-restart-parity", "E-section-continuous-title",
                     "E-keepnext-break", "E-header-no-room", "E-header-taller-than-page",
                     "E-header-space-before", "E-para-shading-borders", "E-number-formats"):
            parts, body, _ = load(name)
            roots = [body] + [ElementTree.fromstring(data) for part_name, data in parts.items()
                              if re.fullmatch(r"word/(header|footer)\d+\.xml", part_name)]
            for root in roots:
                for paragraph in root.findall(f"{W}p"):
                    spacing = paragraph.find(f"{W}pPr/{W}spacing")
                    self.assertEqual((spacing.get(f"{W}line"), spacing.get(f"{W}lineRule")),
                                     ("240", "exact"), name)

        # E-section-restart-parity: each section's start type, first number and pages are
        # those of its label. The page numbers are followed under both readings of a restart
        # with an odd or even start (#301): "restart + 1", and the restart ignored, where the
        # numbering continues. A blank page comes, under both, where a section continues the
        # numbering and its next number has the wrong parity. Each restart tells the two
        # readings apart, and each "ends on" claim of a label holds under both.
        parts, body, targets = load("E-section-restart-parity")
        found = sections(body)
        self.assertEqual(len(found), 5)
        last = {"plus one": 0, "ignored": 0}  # the last page number of the section before
        for n, (paragraphs, sect) in enumerate(found, 1):
            label = lines_of(paragraphs[0])[0]
            said = re.fullmatch(
                rf"Section {n}: (?:one page|(odd|even|next) page start(?: after a section that "
                r"ends on (?:page (\d+)|an? (odd|even) page number))?(?:, page numbers "
                r"(?:restarted at (\d+)|(continued)))?(; two pages)?)", label)
            self.assertIsNotNone(said, label)
            start, page, parity, restart, _, two = said.groups()
            expected_kind = None if start in (None, "next") else f"{start}Page"
            self.assertEqual(kind(sect), expected_kind, label)
            numbers = sect.find(f"{W}pgNumType")
            self.assertEqual(None if numbers is None else numbers.get(f"{W}start"), restart, label)
            for reading, before in last.items():
                if page is not None:
                    self.assertEqual(before, int(page), (label, reading))
                if parity is not None:
                    self.assertEqual(before % 2, {"odd": 1, "even": 0}[parity], (label, reading))
            self.assertEqual(len(paragraphs), 2 if two else 1, label)
            for paragraph in paragraphs[1:]:
                self.assertTrue(flag(paragraph, "pageBreakBefore"), label)
            first = {}
            for reading, before in last.items():
                number = before + 1
                if restart is not None and reading == "plus one":
                    number = int(restart)
                if expected_kind and (number % 2 == 1) != (start == "odd"):
                    number += 1  # the next number, or a blank page before it
                first[reading] = number
                last[reading] = number + len(paragraphs) - 1
            if restart is not None:
                self.assertNotEqual(first["plus one"], first["ignored"], label)
            refs = sect.findall(f"{W}headerReference") + sect.findall(f"{W}footerReference")
            self.assertEqual(len(refs), 2 if n == 1 else 0, label)
        footer = story(parts, targets, found[0][1], "footerReference")
        self.assertEqual(instructions(footer), ["PAGE", "NUMPAGES", "SECTIONPAGES"])
        self.assertEqual(story_text(story(parts, targets, found[0][1], "headerReference")),
                         "header")
        # The questions of #298 the sections ask: an even start restarted at 5 after page 1,
        # an odd start restarted at 0, and an even start that continues after an even number
        # and so gets a blank page.
        self.assertEqual([(kind(sect), None if sect.find(f"{W}pgNumType") is None
                           else sect.find(f"{W}pgNumType").get(f"{W}start"))
                          for _, sect in found],
                         [(None, None), ("evenPage", "5"), ("evenPage", None), ("oddPage", "0"),
                          (None, None)])

        # E-section-continuous-title: Section 1 names Header A and Footer A; Section 2 is
        # continuous, has a title page and names its own default and first-page header and
        # footer, as the labels say; it starts on page 1 and runs onto page 3.
        parts, body, targets = load("E-section-continuous-title")
        (first, sect), (second, last) = sections(body)
        self.assertEqual((kind(sect), kind(last)), (None, "continuous"))
        self.assertIsNone(sect.find(f"{W}titlePg"))
        self.assertIsNotNone(last.find(f"{W}titlePg"))
        self.assertEqual(story_text(story(parts, targets, sect, "headerReference")), "Header A")
        self.assertIsNone(story(parts, targets, sect, "headerReference", "first"))
        for element, ref_type, said in (
            ("headerReference", "default", "Header B"),
            ("headerReference", "first", "First header B"),
            ("footerReference", "default", "Footer B"),
            ("footerReference", "first", "First footer B"),
        ):
            root = story(parts, targets, last, element, ref_type)
            self.assertTrue(story_text(root).startswith(said), said)
            self.assertIn(said, lines_of(second[0])[0])
        for footer in (story(parts, targets, sect, "footerReference"),
                       story(parts, targets, last, "footerReference"),
                       story(parts, targets, last, "footerReference", "first")):
            self.assertEqual(instructions(footer), ["PAGE", "NUMPAGES", "SECTIONPAGES"])
        before, after = len(lines_of(first[0])), len(lines_of(second[0]))
        self.assertEqual((before, after), (29, 100))
        self.assertTrue(before < lines_per_page < before + after - lines_per_page)

        # E-keepnext-break: per case, the lines before, the kept paragraph with its page
        # break, and the paragraph kept together, all without widow control. Each label fits
        # one line (Calibri 11 pt is under 6 pt a character), so the line counts are exact:
        # Case 01's kept line ends the page of the lines before, and its 6 lines and the 3
        # fit the next page; Case 02's 56 lines and the 3 do not.
        _, body, _ = load("E-keepnext-break")
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), 9)
        for number in range(1, 4):
            filler, kept, together = paragraphs[3 * number - 3 : 3 * number]
            label = lines_of(filler)[0]
            said = re.fullmatch(rf"Case {number:02d}: (\d+) lines, then (a paragraph with a page "
                                r"break|a heading after a page break), kept with the next", label)
            self.assertIsNotNone(said, label)
            self.assertLessEqual(len(label), 75, label)
            count = int(said.group(1))
            self.assertEqual(len(lines_of(filler)), count, label)
            self.assertEqual(flag(filler, "pageBreakBefore"), number > 1, label)
            for paragraph in (filler, kept, together):
                self.assertFalse(flag(paragraph, "widowControl"), label)
                self.assertIsNotNone(paragraph.find(f"{W}pPr/{W}widowControl"), label)
            self.assertTrue(flag(kept, "keepNext"), label)
            self.assertEqual(len(page_breaks(kept)), 1, label)
            self.assertTrue(flag(together, "keepLines"), label)
            self.assertFalse(flag(together, "keepNext"), label)
            self.assertEqual(len(lines_of(together)), 3, label)
            if "heading" in said.group(2):
                # The heading's first run is the page break.
                self.assertIsNotNone(kept.find(f"{W}r").find(f"{W}br"), label)
                self.assertEqual(lines_of(kept)[0], "", label)
                self.assertEqual(count, lines_per_page - 2, label)
                continue
            after = len(lines_of(kept)) - 1
            self.assertEqual(lines_of(kept)[0], f"Case {number:02d} kept: A")
            # The line before the break ends the page of the lines before it, with a line to
            # spare in case a label wraps after all.
            self.assertTrue(count + 1 < lines_per_page, label)
            self.assertEqual(after + 3 <= lines_per_page, number == 1, label)
        self.assertEqual([len(lines_of(p)) - 1 for p in paragraphs[1:6:3]], [6, 56])
        self.assertEqual([len(lines_of(p)) for p in paragraphs[0::3]], [56, 20, 56])

        # The header documents: the heights the purposes claim.
        parts, body, targets = load("E-header-no-room")
        sect = body.find(f"{W}sectPr")
        header = story_lines(story(parts, targets, sect, "headerReference"))
        footer = story_lines(story(parts, targets, sect, "footerReference"))
        self.assertEqual((header, footer), (33, 33))
        # The header ends below where the footer starts.
        self.assertTrue(distance + 12 * header > page_height - distance - 12 * footer)
        parts, body, targets = load("E-header-taller-than-page")
        sect = body.find(f"{W}sectPr")
        self.assertIsNone(sect.find(f"{W}footerReference"))
        header = story_lines(story(parts, targets, sect, "headerReference"))
        self.assertEqual(header, 80)
        self.assertTrue(12 * header > page_height)
        parts, body, targets = load("E-header-space-before")
        sect = body.find(f"{W}sectPr")
        root = story(parts, targets, sect, "headerReference")
        first_paragraph = root.findall(f"{W}p")[0]
        self.assertEqual(first_paragraph.find(f"{W}pPr/{W}spacing").get(f"{W}before"), "480")
        self.assertIn("24 pt before", lines_of(first_paragraph)[0])
        header = story_lines(root)
        margin = rules.MARGIN / 20
        self.assertTrue(distance + 12 * header < margin < distance + 12 * header + 24)
        for name in ("E-header-no-room", "E-header-taller-than-page", "E-header-space-before"):
            _, body, _ = load(name)
            (paragraph,) = body.findall(f"{W}p")
            said = int(re.search(r"(\d+) lines", lines_of(paragraph)[0]).group(1))
            self.assertEqual(len(lines_of(paragraph)), said, name)

        # E-footer-tall-bottom: a footer of 8 lines and no header; each case a first line of
        # the height its label says and 52 lines of 12 pt, all without widow control. The
        # cases' 53rd lines end over the whole range E-header-tall leaves open.
        parts, body, targets = load("E-footer-tall-bottom")
        sect = body.find(f"{W}sectPr")
        self.assertIsNone(sect.find(f"{W}headerReference"))
        self.assertEqual(story_lines(story(parts, targets, sect, "footerReference")), 8)
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), 2 * len(rules.FOOTER_FIRST_LINES))
        ends = []
        for n in range(len(rules.FOOTER_FIRST_LINES)):
            first, rest = paragraphs[2 * n : 2 * n + 2]
            label = lines_of(first)[0]
            height = int(re.fullmatch(rf"Case {n + 1:02d}: a first line of (\d+) pt, then 52 "
                                      r"lines of 12 pt", label).group(1))
            spacing = first.find(f"{W}pPr/{W}spacing")
            self.assertEqual((spacing.get(f"{W}line"), spacing.get(f"{W}lineRule")),
                             (str(height * 20), "exact"), label)
            self.assertEqual(rest.find(f"{W}pPr/{W}spacing").get(f"{W}line"), "240", label)
            self.assertEqual(len(lines_of(rest)), 52, label)
            for paragraph in (first, rest):
                self.assertFalse(flag(paragraph, "widowControl"), label)
                self.assertIsNotNone(paragraph.find(f"{W}pPr/{W}widowControl"), label)
            self.assertEqual(flag(first, "pageBreakBefore"), n > 0, label)
            ends.append(rules.MARGIN / 20 + height + 12 * 52)
        self.assertEqual(sorted(ends), ends)
        self.assertTrue(ends[0] <= 707.4 and ends[-1] >= 719.4, ends)

        # E-para-shading-borders: the borders and spacing each label names; the two pairs
        # touch, the other cases stand between unshaded paragraphs.
        _, body, _ = load("E-para-shading-borders")
        paragraphs = body.findall(f"{W}p")
        shaded = [p for p in paragraphs if p.find(f"{W}pPr/{W}shd") is not None]
        self.assertEqual(len(shaded), 7)
        for n, paragraph in enumerate(shaded, 1):
            label = lines_of(paragraph)[0]
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            sides = {c.tag.replace(W, ""): c.get(f"{W}space")
                     for c in paragraph.findall(f"{W}pPr/{W}pBdr/*")}
            expected = {}
            box = re.search(r"box border(?: (\d) pt from the text)?", label)
            if box:
                space = box.group(1) or "4"
                expected = {side: space for side in ("top", "left", "bottom", "right")}
            if "left border only, 4 pt" in label:
                expected = {"left": "4"}
            self.assertEqual(sides, expected, label)
            spacing = paragraph.find(f"{W}pPr/{W}spacing")
            values = (spacing.get(f"{W}before"), spacing.get(f"{W}after"))
            self.assertEqual(values, ("240" if "12 pt before" in label else None,
                                      "240" if "12 pt after" in label else None), label)
        places = [paragraphs.index(p) for p in shaded]
        self.assertEqual([b - a for a, b in zip(places, places[1:])], [2, 2, 2, 1, 2, 1])
        for first, second in ((shaded[3], shaded[4]), (shaded[5], shaded[6])):
            self.assertIn("first of two shaded paragraphs", lines_of(first)[0])
            self.assertIn("second of two shaded paragraphs", lines_of(second)[0])

        # E-number-formats: each section's numbers start where its label says, and each line
        # holds the field its text names.
        _, body, _ = load("E-number-formats")
        found = sections(body)
        self.assertEqual(len(found), len(rules.NUMBER_STARTS) + 3)
        for n, (paragraphs, sect) in enumerate(found, 1):
            label = lines_of(paragraphs[0])[0]
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            numbers = sect.find(f"{W}pgNumType")
            said = re.search(r"(Chicago )?page numbers restarted at (\d+)", label)
            if said:
                self.assertEqual(numbers.get(f"{W}start"), said.group(2), label)
                self.assertEqual(numbers.get(f"{W}fmt"), "chicago" if said.group(1) else None)
            else:
                self.assertIsNone(numbers, label)
            for paragraph in paragraphs[1:]:
                line = lines_of(paragraph)[0]
                instr = re.fullmatch(r"(.+?)( in German)?: 1", line)
                self.assertIsNotNone(instr, line)
                simple = [f.get(f"{W}instr") for f in paragraph.iter(f"{W}fldSimple")]
                codes = "".join(t.text for t in paragraph.iter(f"{W}instrText"))
                if instr.group(2):
                    # Every run of the field, its code too, is in German.
                    self.assertEqual((simple, codes), ([], f" {instr.group(1)} "), line)
                    field_runs = [r for r in paragraph.findall(f"{W}r")][1:]
                    self.assertEqual(len(field_runs), 5, line)
                    for r in field_runs:
                        self.assertEqual(r.find(f"{W}rPr/{W}lang").get(f"{W}val"), "de-DE")
                else:
                    self.assertEqual((simple, codes), ([f" {instr.group(1)} "], ""), line)
            if n <= len(rules.NUMBER_STARTS):
                switches = [lines_of(p)[0] for p in paragraphs[1:]
                            if "German" not in lines_of(p)[0]]
                self.assertEqual(switches, [f"PAGE \\* {s}: 1" for s in rules.NUMBER_SWITCHES])
        self.assertEqual(rules.NUMBER_STARTS, [0, 27, 53, 1665, 4000, 4999])
        self.assertEqual(rules.NUMBER_SWITCHES, [
            "Arabic", "ArabicDash", "ALPHABETIC", "alphabetic", "ROMAN", "roman", "Ordinal",
            "CardText", "OrdText", "Hex", "DollarText"])
        # Chicago numbers at 116 and 117 (MS-OI29500 2.1.548 k: nothing past 116), German on
        # the page numbered 27.
        chicago = [lines_of(paragraphs[0])[0] for paragraphs, _ in found
                   if "Chicago" in lines_of(paragraphs[0])[0]]
        self.assertEqual(chicago, ["Case 07: Chicago page numbers restarted at 116",
                                   "Case 08: Chicago page numbers restarted at 117"])
        german = [(lines_of(paragraphs[0])[0], lines_of(p)[0])
                  for paragraphs, _ in found for p in paragraphs if "German" in lines_of(p)[0]]
        self.assertEqual(german, [("Case 02: page numbers restarted at 27", f"PAGE \\* {s} in "
                                   "German: 1") for s in ("Ordinal", "OrdText", "CardText")])
        # 27 and 53 are two and three letters, 1665 is 65 (ECMA-376 §17.18.59, upperLetter).
        self.assertEqual([(n - 1) // 26 + 1 for n in (27, 53, 1665)], [2, 3, 65])

        # E-bold-missing: each sample is in the font, size and style its label names, after
        # a label in the document's default font.
        _, body, _ = load("E-bold-missing")
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), 14)
        seen = []
        for n, paragraph in enumerate(paragraphs, 1):
            label_run, sample = paragraph.findall(f"{W}r")
            label = "".join(t.text for t in label_run.iter(f"{W}t"))
            font, size, style = re.fullmatch(
                rf"Case {n:02d}: (Lucida Console|Impact) (\d+) pt (regular|bold|italic|bold "
                r"italic): ", label).groups()
            self.assertIsNone(label_run.find(f"{W}rPr"), label)
            props = sample.find(f"{W}rPr")
            self.assertEqual(props.find(f"{W}rFonts").get(f"{W}ascii"), font, label)
            self.assertEqual(props.find(f"{W}sz").get(f"{W}val"), str(int(size) * 2), label)
            self.assertEqual(props.find(f"{W}b") is not None, "bold" in style, label)
            self.assertEqual(props.find(f"{W}i") is not None, "italic" in style, label)
            seen.append((font, size, style))
        expected = [(24, "regular")] + [(size, "bold") for size in (8, 11, 24, 48)] + [
            (24, "italic"), (24, "bold italic")]
        self.assertEqual(seen, [(font, str(size), style) for font in ("Lucida Console", "Impact")
                                for size, style in expected])

        # E-soft-hyphen-font: a label paragraph, then the two halves in the default font with
        # the soft hyphen or the w:sym between them, in a narrow paragraph where the label
        # says "end of a line"; there the word is wider than the paragraph.
        _, body, _ = load("E-soft-hyphen-font")
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), 8)
        for n in range(4):
            label = lines_of(paragraphs[2 * n])[0]
            sample = paragraphs[2 * n + 1]
            self.assertTrue(label.startswith(f"Case {n + 1:02d}: "), label)
            first, middle, last = sample.findall(f"{W}r")
            for half in (first, last):
                self.assertIsNone(half.find(f"{W}rPr"), label)
                self.assertEqual(half.find(f"{W}t").text, "Hamburgefontsiv", label)
            soft = middle.find(f"{W}softHyphen")
            symbol = middle.find(f"{W}sym")
            if "w:sym" in label or (n == 3):
                self.assertIsNone(soft, label)
                self.assertEqual((symbol.get(f"{W}font"), symbol.get(f"{W}char")),
                                 ("Noto Sans", "F0AD"), label)
            else:
                self.assertIsNone(symbol, label)
                self.assertIsNotNone(soft, label)
                props = middle.find(f"{W}rPr")
                self.assertEqual(props.find(f"{W}rFonts").get(f"{W}ascii"), "Noto Sans")
                self.assertEqual(props.find(f"{W}sz").get(f"{W}val"), "40")
            ind = sample.find(f"{W}pPr/{W}ind")
            narrow = "end of a line" in label
            self.assertEqual(ind is not None, narrow, label)
            if narrow:
                self.assertEqual(int(ind.get(f"{W}right")), rules.TEXT_W - 3000, label)
            else:
                self.assertIn("middle of a line", label)

    def test_second_drawing_label_and_table_cases_have_the_markup_their_labels_name(self):
        def load(name):
            doc = next(doc for doc in self.docs if doc.name == name)
            parts = dict(rules.parts(doc))
            return doc, parts, ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")

        def props_of(element, path):
            """The children of a properties element as {tag: {attribute: value}}."""
            found = element.find(path)
            return {} if found is None else {
                c.tag.replace(W, ""): {k.replace(W, ""): v for k, v in c.attrib.items()}
                for c in found
            }

        def cases(name):
            """Per case: the label without its number, the sample runs (text, run
            properties, tab or not) and the w:pPr of the paragraph that holds them."""
            _, _, body = load(name)
            found, n = [], 0
            for paragraph in body.findall(f"{W}p"):
                runs = paragraph.findall(f"{W}r")
                first = "".join(t.text for t in runs[0].iter(f"{W}t")) if runs else ""
                sample = [("".join(t.text for t in r.iter(f"{W}t")), props_of(r, f"{W}rPr"),
                           r.find(f"{W}tab") is not None) for r in runs]
                if not first.startswith("Case "):
                    label, samples, _ = found[-1]
                    found[-1] = (label, samples + sample, paragraph.find(f"{W}pPr"))
                    continue
                n += 1
                self.assertTrue(first.startswith(f"Case {n:02d}: "), first)
                label = first.removeprefix(f"Case {n:02d}: ").rstrip(": ")
                found.append((label, sample[1:], paragraph.find(f"{W}pPr")))
            return found

        def tabs(p_pr):
            stops = [] if p_pr is None else p_pr.findall(f"{W}tabs/{W}tab")
            return [{k.replace(W, ""): v for k, v in t.attrib.items()} for t in stops]

        def spacing_of(p_pr):
            return props_of(p_pr, ".").get("spacing", {}) if p_pr is not None else {}

        size_24 = {"sz": {"val": "48"}, "szCs": {"val": "48"}}
        single = {"u": {"val": "single"}}

        # E-underline-24: every kind at 24 pt, then the three cases across runs.
        found = cases("E-underline-24")
        kinds = rules.UNDERLINES
        self.assertEqual(len(kinds), 17)
        for (label, samples, _), kind in zip(found, kinds):
            self.assertEqual(label, f"underline {kind} at 24 pt")
            ((_, tags, _),) = samples
            self.assertEqual(tags, {**size_24, "u": {"val": kind}}, label)
        rest = found[len(kinds):]
        self.assertEqual([label for label, _, _ in rest], [
            "single across runs of 11, 24 and 11 pt",
            "single under a superscript whose neighbours are not underlined",
            "double across runs of 11 and 24 pt",
        ])
        self.assertEqual([tags for _, tags, _ in rest[0][1]],
                         [single, {**size_24, **single}, single])
        self.assertEqual([tags for _, tags, _ in rest[1][1]],
                         [{}, {**single, "vertAlign": {"val": "superscript"}}, {}])
        double = {"u": {"val": "double"}}
        self.assertEqual([tags for _, tags, _ in rest[2][1]], [double, {**size_24, **double}])

        # E-superscript-sizes: every size its purpose names, twice: superscript and subscript.
        doc, _, _ = load("E-superscript-sizes")
        listed = re.search(r"at ([\d, ]+) and (\d+) pt", doc.purpose)
        sizes = [int(n) for n in listed.group(1).split(", ")] + [int(listed.group(2))]
        self.assertEqual(sizes, [8, 9, 10, 12, 16, 20, 36, 48])
        found = cases("E-superscript-sizes")
        self.assertEqual([label for label, _, _ in found],
                         [f"{kind} at {size} pt" for size in sizes
                          for kind in ("superscript", "subscript")])
        for label, (base, raised), _ in found:
            kind, size = re.fullmatch(r"(\w+) at (\d+) pt", label).groups()
            half = {"val": str(int(size) * 2)}
            self.assertEqual(base[1], {"sz": half, "szCs": half}, label)
            self.assertEqual(raised[1], {"sz": half, "szCs": half, "vertAlign": {"val": kind}})

        # E-auto-color-threshold: run shading of every grey between the ones where text stayed
        # black (404040) and turned white (333333), and of a red the two luma formulas put on
        # either side of their thresholds; then bar tabs on dark paragraph shading.
        found = cases("E-auto-color-threshold")
        fills = rules.AUTO_COLOR_THRESHOLD_FILLS
        self.assertEqual(len(found), len(fills) + 2)
        for (label, samples, p_pr), fill in zip(found, fills):
            self.assertEqual(label, f"run shading {fill}")
            ((_, tags, _),) = samples
            self.assertEqual(tags, {"shd": {"val": "clear", "color": "auto", "fill": fill}})
            self.assertIsNone(p_pr, label)
        greys = [int(fill[:2], 16) for fill in fills if fill == fill[:2] * 3]
        self.assertEqual(greys, list(range(0x34, 0x40)))
        # The red: black text by Rec. 601 luma (above 0.251), white by Rec. 709 (below 0.200).
        red = fills[-1]
        self.assertEqual(red[2:], "0000")
        level = int(red[:2], 16) / 255
        self.assertTrue(0.299 * level > 0.251 and 0.2126 * level < 0.200, red)
        for (label, _, p_pr), fill in zip(found[len(fills):], ("333333", "000000")):
            self.assertEqual(label, f"a bar tab on paragraph shading {fill}")
            self.assertEqual(tabs(p_pr), [{"val": "bar", "pos": "2880"}], label)
            self.assertEqual(p_pr.find(f"{W}shd").get(f"{W}fill"), fill, label)

        # E-highlight-tall: the spacing each label names, and the highlight.
        found = cases("E-highlight-tall")
        highlight = {"highlight": {"val": "darkYellow"}}
        expected = {
            "highlighted, 1.5 lines spacing": {"line": "360", "lineRule": "auto"},
            "highlighted, double spacing": {"line": "480", "lineRule": "auto"},
            "highlighted, at least 24 pt spacing": {"line": "480", "lineRule": "atLeast"},
            "highlighted, exact 24 pt spacing": {"line": "480", "lineRule": "exact"},
        }
        self.assertEqual([label for label, _, _ in found[:4]], list(expected))
        for label, samples, p_pr in found[:4]:
            self.assertEqual(spacing_of(p_pr), expected[label], label)
            self.assertEqual([tags for _, tags, _ in samples], [highlight], label)
        label, samples, p_pr = found[4]
        self.assertEqual(label, "highlighted 11 pt text on a line with a 24 pt run")
        self.assertEqual([tags for _, tags, _ in samples], [highlight, size_24])
        self.assertIsNone(p_pr)

        # E-underline-trailspace-margin: words, spaces and a word, all underlined, in a
        # paragraph after the label's, so the words start the line; the spaces end where
        # the label says, in Carlito, and the word after them cannot share their line.
        found = cases("E-underline-trailspace-margin")
        self.assertEqual(len(found), 4)
        _, _, body = load("E-underline-trailspace-margin")
        self.assertEqual([len(p.findall(f"{W}r")) for p in body.findall(f"{W}p")], [1] * 8)
        margin, edge = rules.TEXT_W / 20, (rules.PAGE_W - rules.MARGIN) / 20
        past = []
        for label, ((value, tags, _),), p_pr in found:
            words, count, where = re.fullmatch(
                r"(\d+) underlined words, (\d+) spaces and an underlined word, left aligned; "
                r"in Calibri 11 pt the spaces end (.+)", label).groups()
            words = " ".join(["underlined"] * int(words))
            self.assertEqual(value, words + " " * int(count) + "underlined")
            self.assertEqual(tags, single, label)
            self.assertIsNone(p_pr, label)
            end = carlito_width(words + " " * int(count))
            self.assertLess(carlito_width(words), margin, label)
            self.assertGreater(end + carlito_width("underlined"), margin, label)
            distance, side, beyond = re.fullmatch(
                r"(\d+) pt (before|past) the right margin(?:, (\d+) pt past the edge of the "
                r"page)?", where).groups()
            self.assertEqual(side, "past" if end > margin else "before", label)
            self.assertAlmostEqual(abs(end - margin), int(distance), delta=1, msg=label)
            self.assertEqual(beyond is not None, end > edge, label)
            if beyond is not None:
                self.assertAlmostEqual(end - edge, int(beyond), delta=1, msg=label)
            past.append(end - margin)
        # Before the margin, just past it, well past it, and past the edge of the page.
        self.assertTrue(past[0] < -5 and 5 < past[1] < 20 < past[2] < edge - margin < past[3])

        # E-tab-margin-alone in modes 15, 14 and 12: the same cases; each sample paragraph has
        # the stop its label names, and its tab is underlined where the label says so.
        in_margin, past_edge = rules.TEXT_W + 360, rules.PAGE_W - rules.MARGIN + 360
        for suffix in ("", "-14", "-12"):
            found = cases(f"E-tab-margin-alone{suffix}")
            self.assertEqual(len(found), 3, suffix)
            # Each label has a paragraph of its own, so the tab follows "before" alone.
            _, _, body = load(f"E-tab-margin-alone{suffix}")
            self.assertEqual([len(p.findall(f"{W}r")) for p in body.findall(f"{W}p")],
                             [1, 3] * 3, suffix)
            for label, samples, p_pr in found:
                place = re.search(r"a left stop 18 pt (into the right margin|past the right edge "
                                  r"of the page)", label).group(1)
                pos = in_margin if place == "into the right margin" else past_edge
                stop = {"val": "left", "pos": str(pos)}
                if "dot leader" in label:
                    stop = {"val": "left", "leader": "dot", "pos": str(pos)}
                self.assertEqual(tabs(p_pr), [stop], label)
                self.assertEqual([(value, is_tab) for value, _, is_tab in samples],
                                 [("before", False), ("", True), ("after", False)], label)
                underlined = samples[1][1] == single
                self.assertEqual(underlined, "tab underlined" in label, label)

        # E-list-label-height: each list case's level and mark have the sizes and underline
        # its label names; then two paragraphs outside any list.
        doc, parts, body = load("E-list-label-height")
        numbering = ElementTree.fromstring(parts["word/numbering.xml"])
        levels = {a.get(f"{W}abstractNumId"): a.find(f"{W}lvl")
                  for a in numbering.iter(f"{W}abstractNum")}
        instances = {n.get(f"{W}numId"): n.find(f"{W}abstractNumId").get(f"{W}val")
                     for n in numbering.iter(f"{W}num")}
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), 6)
        size_20 = {"sz": {"val": "40"}, "szCs": {"val": "40"}}
        for n, paragraph in enumerate(paragraphs, 1):
            label = "".join(t.text for t in paragraph.iter(f"{W}t"))
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            mark = props_of(paragraph, f"{W}pPr/{W}rPr")
            num_id = paragraph.find(f"{W}pPr/{W}numPr/{W}numId")
            if label.endswith("no list, mark 20 pt") or label.endswith("no list, mark 11 pt"):
                self.assertIsNone(num_id, label)
                self.assertEqual(mark, size_20 if "20 pt" in label else {}, label)
                continue
            level = props_of(levels[instances[num_id.get(f"{W}val")]], f"{W}rPr")
            self.assertEqual(level, size_20 if "level 20 pt" in label else {}, label)
            expected = {}
            if "mark 20 pt" in label:
                expected = size_20
            if "mark underlined" in label:
                expected = single
            self.assertEqual(mark, expected, label)
        self.assertEqual(len(set(instances.values())), 4)
        self.assertEqual(list(numbering.iter(f"{W}lvlOverride")), [])

        # E-table-style-defaults-size: document defaults 12 pt and right-aligned, Normal 11 pt
        # and left-aligned, the table style Big 14 pt and centred; mode 15 with the setting
        # on, and mode 14.
        # The control: Normal right-aligned over defaults that leave the alignment out.
        for name, mode, normal_jc in (("E-table-style-defaults-size", 15, "left"),
                                      ("E-table-style-defaults-size-14", 14, "left"),
                                      ("E-table-style-defaults-size-right-14", 14, "right")):
            doc, parts, body = load(name)
            styles = ElementTree.fromstring(parts["word/styles.xml"])
            defaults = props_of(styles, f"{W}docDefaults/{W}rPrDefault/{W}rPr")
            self.assertEqual((defaults["sz"], defaults["szCs"]), ({"val": "24"}, {"val": "24"}))
            defaults = props_of(styles, f"{W}docDefaults/{W}pPrDefault/{W}pPr")
            self.assertEqual(defaults.get("jc"), {"val": "right"} if normal_jc == "left" else None,
                             name)
            normal = next(s for s in styles.iter(f"{W}style") if s.get(f"{W}styleId") == "Normal")
            self.assertEqual(props_of(normal, f"{W}rPr"), {"sz": {"val": "22"},
                                                           "szCs": {"val": "22"}}, name)
            self.assertEqual(props_of(normal, f"{W}pPr"), {"jc": {"val": normal_jc}}, name)
            big = next(s for s in styles.iter(f"{W}style") if s.get(f"{W}styleId") == "Big")
            self.assertEqual(props_of(big, f"{W}rPr"), {"sz": {"val": "28"},
                                                        "szCs": {"val": "28"}}, name)
            self.assertEqual(props_of(big, f"{W}pPr"), {"jc": {"val": "center"}}, name)
            self.assertEqual(doc.mode, mode)
            label = "".join(t.text for t in body.find(f"{W}p").iter(f"{W}t"))
            if normal_jc == "left":
                self.assertIn("document defaults 12 pt and right-aligned, Normal 11 pt and "
                              "left-aligned", label)
            else:
                self.assertIn("document defaults 12 pt, Normal 11 pt and right-aligned", label)
            # The table is in Big, and its cells' text is short enough that left, centre and
            # right put it in different places.
            (table,) = body.findall(f"{W}tbl")
            self.assertEqual(table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), "Big")
            for cell in table.iter(f"{W}tc"):
                width = int(cell.find(f"{W}tcPr/{W}tcW").get(f"{W}w")) / 20
                value = "".join(t.text for t in cell.iter(f"{W}t"))
                self.assertLess(carlito_width(value, 14), width / 3, value)
                # Nothing but the styles sizes and aligns the cells' text.
                self.assertIsNone(cell.find(f"{W}p/{W}pPr"), value)
                self.assertIsNone(cell.find(f"{W}p/{W}r/{W}rPr"), value)

        def table_styles(parts):
            styles = ElementTree.fromstring(parts["word/styles.xml"])
            return {s.get(f"{W}styleId"): s for s in styles.iter(f"{W}style")
                    if s.get(f"{W}type") == "table"}

        def formats(style):
            return [p.get(f"{W}type") for p in style.findall(f"{W}tblStylePr")]

        def look_of(table):
            look = table.find(f"{W}tblPr/{W}tblLook")
            return {k.replace(W, ""): v for k, v in look.attrib.items()}

        def band_size(style, kind):
            found = style.find(f"{W}tblPr/{W}tblStyle{kind}BandSize")
            return None if found is None else found.get(f"{W}val")

        def shape(table):
            rows = table.findall(f"{W}tr")
            return len(rows), len(rows[0].findall(f"{W}tc"))

        def header_rows(table):
            return [n for n, row in enumerate(table.findall(f"{W}tr"), 1)
                    if row.find(f"{W}trPr/{W}tblHeader") is not None]

        on = {"firstRow": "1", "lastRow": "1", "firstColumn": "1", "lastColumn": "1",
              "noHBand": "1", "noVBand": "1"}
        off = {key: "0" for key in on} | {"noHBand": "1", "noVBand": "1"}

        # Every conditional format has a text colour and a grey of its own.
        colours = {kind: (color, fill) for kind, color, fill in rules.CONDITIONS}
        self.assertEqual(len(set(colours.values())), len(colours))

        def format_colours(found):
            color, fill = found.find(f"{W}rPr/{W}color"), found.find(f"{W}tcPr/{W}shd")
            return (None if color is None else color.get(f"{W}val"),
                    None if fill is None else fill.get(f"{W}fill"))

        # E-table-style-rows-cols: a style without corner formats, then band formats only,
        # with a first or a last row or column on; bands by one.
        _, parts, body = load("E-table-style-rows-cols")
        styles = table_styles(parts)
        self.assertEqual(formats(styles["NoCorners"]),
                         ["firstRow", "lastRow", "firstCol", "lastCol"])
        self.assertEqual(formats(styles["RowBandsOnly"]), ["band1Horz", "band2Horz"])
        self.assertEqual(formats(styles["ColBandsOnly"]), ["band1Vert", "band2Vert"])
        self.assertEqual((band_size(styles["RowBandsOnly"], "Row"),
                          band_size(styles["RowBandsOnly"], "Col")), ("1", None))
        self.assertEqual((band_size(styles["ColBandsOnly"], "Row"),
                          band_size(styles["ColBandsOnly"], "Col")), (None, "1"))
        tables = body.findall(f"{W}tbl")
        found = cases("E-table-style-rows-cols")
        expected = [
            ("NoCorners", on, (5, 5)),
            ("RowBandsOnly", off | {"firstRow": "1", "noHBand": "0"}, (6, 3)),
            ("ColBandsOnly", off | {"firstColumn": "1", "noVBand": "0"}, (3, 6)),
            # The last row or column is the fourth: band 2 if the bands count it.
            ("RowBandsOnly", off | {"lastRow": "1", "noHBand": "0"}, (4, 3)),
            ("ColBandsOnly", off | {"lastColumn": "1", "noVBand": "0"}, (3, 4)),
        ]
        self.assertEqual(len(tables), len(expected))
        self.assertEqual(len(found), len(expected))
        for (label, _, _), table, (style, look, size) in zip(found, tables, expected):
            self.assertEqual(table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), style)
            self.assertEqual(look_of(table), look, label)
            self.assertEqual(shape(table), size, label)
            if style != "NoCorners":
                kind = "row" if style == "RowBandsOnly" else "column"
                which = "first" if look[f"first{kind.title()}"] == "1" else "last"
                self.assertEqual(label, f"a style with {kind} band formats only; {which} {kind} "
                                 f"flag on, {kind} bands on")

        # E-table-style-header-rows-3: three header rows of 70, a first-row format with a
        # 3 pt bottom border, row bands by one; 70 rows of at least 12 pt are more than a
        # page. The setting on and off in mode 15, and mode 14, which has none.
        for suffix, mode, values in (("on", 15, {"differentiateMultirowTableHeaders": 1}),
                                     ("off", 15, {"differentiateMultirowTableHeaders": 0}),
                                     ("14", 14, {})):
            doc, parts, body = load(f"E-table-style-header-rows-3-{suffix}")
            self.assertEqual((doc.mode, dict(doc.compat_values)), (mode, values))
            style = table_styles(parts)["ThreeHeaders"]
            self.assertEqual(formats(style), ["firstRow", "band1Horz", "band2Horz"])
            self.assertEqual(band_size(style, "Row"), "1")
            for found in style.findall(f"{W}tblStylePr"):
                self.assertEqual(format_colours(found), colours[found.get(f"{W}type")], suffix)
            first = style.find(f"{W}tblStylePr")
            bottom = first.find(f"{W}tcPr/{W}tcBorders/{W}bottom")
            self.assertEqual((bottom.get(f"{W}val"), bottom.get(f"{W}sz")), ("single", "24"))
            (table,) = body.findall(f"{W}tbl")
            self.assertEqual(shape(table), (70, 3))
            self.assertEqual(header_rows(table), [1, 2, 3])
            self.assertEqual(look_of(table), off | {"firstRow": "1", "noHBand": "0"})
            self.assertTrue(70 * 12 > rules.PAGE_H / 20 - 2 * rules.MARGIN / 20)

        # E-table-style-header-rows-format: the header rows each label names, the format of
        # the style each table is in, and the flags its label names.
        _, parts, body = load("E-table-style-header-rows-format")
        styles = table_styles(parts)
        self.assertEqual(formats(styles["HeaderBorders"]), ["firstRow"])
        self.assertEqual(formats(styles["HeaderCorners"]),
                         ["firstRow", "firstCol", "lastCol", "neCell", "nwCell"])
        self.assertEqual(formats(styles["HeaderBands"]), ["firstRow", "band1Horz", "band2Horz"])
        self.assertEqual(band_size(styles["HeaderBands"], "Row"), "1")
        for style_id in ("HeaderBorders", "HeaderCorners", "HeaderBands"):
            for found in styles[style_id].findall(f"{W}tblStylePr"):
                self.assertEqual(format_colours(found), colours[found.get(f"{W}type")], style_id)
        # The first-row format's borders differ from the style's own, which are all single
        # and ½ pt, and from each other.
        own = styles["HeaderBorders"].find(f"{W}tblPr/{W}tblBorders")
        self.assertEqual({(b.get(f"{W}val"), b.get(f"{W}sz")) for b in own}, {("single", "4")})
        format_borders = styles["HeaderBorders"].find(f"{W}tblStylePr/{W}tcPr/{W}tcBorders")
        self.assertEqual({b.tag.replace(W, ""): (b.get(f"{W}val"), b.get(f"{W}sz"))
                          for b in format_borders},
                         {"top": ("double", "4"), "bottom": ("single", "24"),
                          "insideH": ("dotted", "8")})
        tables = body.findall(f"{W}tbl")
        found = cases("E-table-style-header-rows-format")
        expected = [
            ("HeaderBorders", off | {"firstRow": "1"}, (4, 3)),
            ("HeaderCorners", off | {"firstRow": "1", "firstColumn": "1", "lastColumn": "1"},
             (4, 4)),
            ("HeaderBands", off | {"firstRow": "1", "noHBand": "0"}, (6, 3)),
            ("HeaderBands", off | {"noHBand": "0"}, (6, 3)),
        ]
        self.assertEqual(len(tables), len(expected))
        self.assertEqual(len(found), len(expected))
        for (label, _, _), table, (style, look, size) in zip(found, tables, expected):
            self.assertEqual(table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), style)
            self.assertEqual(look_of(table), look, label)
            self.assertEqual(shape(table), size, label)
            named = re.match(r"rows (\d) and (\d) are header rows", label).groups()
            self.assertEqual(header_rows(table), [int(n) for n in named], label)
            self.assertEqual("first row flag off" in label, look["firstRow"] == "0", label)
            self.assertEqual("no bands" in label, look["noHBand"] == "1", label)
            self.assertEqual("row bands on" in label, look["noHBand"] == "0", label)
        self.assertEqual(header_rows(tables[2]), [1, 3])

        # E-table-style-two-defaults: Normal Table and two styles of their own are marked
        # w:default; each of the two gives the table a colour and a grey; the table names
        # no style.
        _, parts, body = load("E-table-style-two-defaults")
        styles = ElementTree.fromstring(parts["word/styles.xml"]).iter(f"{W}style")
        marked = [s for s in styles if s.get(f"{W}type") == "table"
                  and s.get(f"{W}default") == "1"]
        self.assertEqual([s.get(f"{W}styleId") for s in marked],
                         ["TableNormal", "FirstDefault", "SecondDefault"])
        looks = [(s.find(f"{W}rPr/{W}color").get(f"{W}val"),
                  s.find(f"{W}tcPr/{W}shd").get(f"{W}fill")) for s in marked[1:]]
        self.assertEqual(looks, [("C00000", "D9D9D9"), ("0070C0", "A6A6A6")])
        self.assertIsNone(marked[0].find(f"{W}tcPr"))
        (table,) = body.findall(f"{W}tbl")
        self.assertIsNone(table.find(f"{W}tblPr/{W}tblStyle"))
        ((label, _, _),) = cases("E-table-style-two-defaults")
        self.assertIn("FirstDefault (red text, light grey) and SecondDefault (blue text, dark "
                      "grey)", label)

        def margins(element):
            return {c.tag.replace(W, ""): (c.get(f"{W}w"), c.get(f"{W}type"))
                    for c in ([] if element is None else element)}

        def fixed(table):
            return table.find(f"{W}tblPr/{W}tblLayout").get(f"{W}type") == "fixed"

        def wraps(cell):
            """Whether the cell's text is wider than the cell, in Carlito 11 pt."""
            width = int(cell.find(f"{W}tcPr/{W}tcW").get(f"{W}w")) / 20
            return carlito_width("".join(t.text for t in cell.iter(f"{W}t"))) > width

        # E-table-cell-margins: the table's margins of 5 pt, and each cell's own margins of
        # the type its text names; fixed widths, and text that wraps in each cell.
        _, _, body = load("E-table-cell-margins")
        (table,) = body.findall(f"{W}tbl")
        self.assertTrue(fixed(table))
        self.assertTrue(all(wraps(cell) for cell in table.iter(f"{W}tc")))
        five = ("100", "dxa")
        self.assertEqual(margins(table.find(f"{W}tblPr/{W}tblCellMar")),
                         {side: five for side in rules.SIDES})
        cells = table.findall(f"{W}tr/{W}tc")
        self.assertEqual(len(cells), 4)
        expected = [("pct", "500"), ("auto", "200"), ("nil", "0"), None]
        for cell, want in zip(cells, expected):
            value = "".join(t.text for t in cell.iter(f"{W}t"))
            own = margins(cell.find(f"{W}tcPr/{W}tcMar"))
            if want is None:
                self.assertTrue(value.startswith("no cell margins of its own"), value)
                self.assertEqual(own, {})
            else:
                kind, width = want
                self.assertTrue(value.startswith(f"cell margins of type {kind}: "), value)
                self.assertEqual(own, {side: (width, kind) for side in rules.SIDES})

        # E-table-cell-margins-nil: one table per side, whose only cell margin is that side,
        # of type nil, in a style whose margin on that side differs from 0 and from Normal
        # Table's, so the side shows whether Word ignores nil.
        found = cases("E-table-cell-margins-nil")
        _, parts, body = load("E-table-cell-margins-nil")
        styles = {s.get(f"{W}styleId"): s
                  for s in ElementTree.fromstring(parts["word/styles.xml"]).iter(f"{W}style")}
        normal = margins(styles["TableNormal"].find(f"{W}tblPr/{W}tblCellMar"))
        style = margins(styles["Margins10"].find(f"{W}tblPr/{W}tblCellMar"))
        tables = body.findall(f"{W}tbl")
        self.assertEqual(len(tables), 4)
        self.assertEqual(len(found), 4)
        for (label, _, _), table, side in zip(found, tables, rules.SIDES):
            points = int(re.fullmatch(rf"the table's {side} cell margin of type nil; the style "
                                      r"Margins10 has (\d+) pt on every side", label).group(1))
            self.assertEqual(table.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val"), "Margins10")
            self.assertEqual(margins(table.find(f"{W}tblPr/{W}tblCellMar")), {side: ("0", "nil")})
            self.assertEqual(style[side], (str(points * 20), "dxa"), label)
            self.assertNotIn(int(style[side][0]), (0, int(normal[side][0])), label)
            self.assertTrue(fixed(table))
            self.assertTrue(all(wraps(cell) for cell in table.iter(f"{W}tc")), label)

        # E-table-float-align: each table floats by the one alignment its label names.
        _, _, body = load("E-table-float-align")
        tables = body.findall(f"{W}tbl")
        found = cases("E-table-float-align")
        self.assertEqual(len(tables), 2)
        placed = []
        for (label, _, p_pr), table in zip(found, tables):
            attribute, value = re.search(r"only w:(tblp[XY]Spec) (\w+)", label).groups()
            attributes = table.find(f"{W}tblPr/{W}tblpPr").attrib
            width = table.find(f"{W}tblPr/{W}tblW")
            self.assertEqual((width.get(f"{W}w"), width.get(f"{W}type")), ("4000", "dxa"))
            self.assertLess(4000, rules.TEXT_W / 2)
            self.assertEqual({k.replace(W, ""): v for k, v in attributes.items()},
                             {attribute: value}, label)
            placed.append((attribute, value))
        # #172's case 11 asks about these two.
        self.assertEqual(placed, [("tblpYSpec", "bottom"), ("tblpXSpec", "center")])
        labels = [p for p in body.findall(f"{W}p")
                  if "".join(t.text for t in p.iter(f"{W}t")).startswith("Case ")]
        self.assertEqual([p.find(f"{W}pPr/{W}pageBreakBefore") is not None for p in labels],
                         [False, True])
        # Each table comes right after its label, and text after it.
        children = list(body)
        for label, table in zip(labels, tables):
            self.assertIs(children[children.index(label) + 1], table)
            after = children[children.index(table) + 1]
            text = "".join(t.text for t in after.iter(f"{W}t"))
            self.assertTrue(text.startswith("Text around"))
            # Enough text to flow around the table for several lines.
            self.assertGreater(carlito_width(text), 5 * rules.TEXT_W / 20)

        # E-bold-italic-missing: each sample is in the font, size and style its label names.
        _, _, body = load("E-bold-italic-missing")
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), 16)
        styles_seen = set()
        for n, paragraph in enumerate(paragraphs, 1):
            label_run, sample = paragraph.findall(f"{W}r")
            label = "".join(t.text for t in label_run.iter(f"{W}t"))
            font, size, style = re.fullmatch(
                rf"Case {n:02d}: (Garamond|Liberation Sans) (11|20) pt (regular|italic|bold|bold "
                r"italic): ", label).groups()
            styles_seen.add((font, size, style))
            self.assertIsNone(label_run.find(f"{W}rPr"), label)
            props = props_of(sample, f"{W}rPr")
            self.assertEqual(props["rFonts"]["ascii"], font, label)
            self.assertEqual(props["sz"], {"val": str(int(size) * 2)}, label)
            self.assertEqual("b" in props, "bold" in style, label)
            self.assertEqual("i" in props, "italic" in style, label)
        self.assertEqual(len(styles_seen), 16)

        # E-list-level-linked-no-level and -defaults: each case's paragraph is in the style
        # its label names and nothing else; the style and the styles it is based on give the
        # instance and the level the label says; level 2 of that instance's definition, and
        # no other level, names the style; each level renders a label and an indent of its
        # own.
        def list_parts(name):
            _, parts, body = load(name)
            styles = {s.get(f"{W}styleId"): s for s in
                      ElementTree.fromstring(parts["word/styles.xml"]).iter(f"{W}style")}
            numbering = ElementTree.fromstring(parts["word/numbering.xml"])
            definitions = {a.get(f"{W}abstractNumId"): a
                           for a in numbering.iter(f"{W}abstractNum")}
            instances = {n.get(f"{W}numId"): definitions[n.find(f"{W}abstractNumId").get(
                f"{W}val")] for n in numbering.iter(f"{W}num")}
            return parts, styles, numbering, instances, body.findall(f"{W}p")

        def num_pr_of(element, path):
            found = element.find(path)
            return {} if found is None else {
                c.tag.replace(W, ""): c.get(f"{W}val") for c in found}

        def inherited(styles, style_id):
            """The w:numPr values the style gives, itself or through w:basedOn."""
            values = {}
            while style_id is not None:
                for key, value in num_pr_of(styles[style_id], f"{W}pPr/{W}numPr").items():
                    values.setdefault(key, value)
                based = styles[style_id].find(f"{W}basedOn")
                style_id = None if based is None else based.get(f"{W}val")
            return values

        def first_label(levels, lvl):
            """The label the level renders for the first item of each level: every %N its
            template holds becomes 1 in the number format of level N - 1."""
            ones = {"decimal": "1", "lowerLetter": "a", "upperLetter": "A", "lowerRoman": "i",
                    "upperRoman": "I"}
            formats = [ones[level.find(f"{W}numFmt").get(f"{W}val")] for level in levels]
            return re.sub(r"%([1-9])", lambda m: formats[int(m.group(1)) - 1],
                          lvl.find(f"{W}lvlText").get(f"{W}val"))

        def check_list_case(styles, instances, paragraph, style_id, label):
            self.assertEqual(props_of(paragraph, f"{W}pPr"), {"pStyle": {"val": style_id}})
            instance, level = re.search(r"gives instance (\d+) and (?:no level|level (\d+))",
                                        label).groups()
            given = {"numId": instance} | ({"ilvl": level} if level is not None else {})
            self.assertEqual(inherited(styles, style_id), given, label)
            naming = [(lvl.get(f"{W}ilvl"), definition is instances[instance])
                      for definition in instances.values() for lvl in definition.iter(f"{W}lvl")
                      if num_pr_of(lvl, ".").get("pStyle") == style_id]
            self.assertEqual(naming, [("2", True)], label)
            self.assertRegex(label, rf"; level 2 names (the style|{style_id})$")
            levels = instances[instance].findall(f"{W}lvl")
            self.assertEqual(len({first_label(levels, lvl) for lvl in levels}), len(levels), label)
            self.assertEqual(len({lvl.find(f"{W}pPr/{W}ind").get(f"{W}left") for lvl in levels}),
                             len(levels), label)
            return levels

        _, styles, _, instances, paragraphs = list_parts("E-list-level-linked-no-level")
        self.assertEqual(len(paragraphs), 4)
        labels = ["".join(t.text for t in p.iter(f"{W}t")) for p in paragraphs]
        for n, label in enumerate(labels, 1):
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
        # Case 01 and its second paragraph, in the first list: lower-case letters.
        levels = check_list_case(styles, instances, paragraphs[0], "LinkedNoLevel",
                                 labels[0])
        self.assertEqual([(lvl.find(f"{W}numFmt").get(f"{W}val"),
                           lvl.find(f"{W}lvlText").get(f"{W}val")) for lvl in levels],
                         [("lowerLetter", "%1)"), ("lowerLetter", "(%2)"),
                          ("lowerLetter", "[%3]")])
        self.assertTrue(labels[1].endswith("a second paragraph in the same style"))
        self.assertEqual(props_of(paragraphs[1], f"{W}pPr"), {"pStyle": {"val": "LinkedNoLevel"}})
        # Case 03: level 9, past the last of nine levels.
        levels = check_list_case(styles, instances, paragraphs[2], "LinkedLevel9",
                                 labels[2])
        self.assertIn("level 9, past the last of its nine levels", labels[2])
        self.assertEqual([lvl.get(f"{W}ilvl") for lvl in levels], [str(i) for i in range(9)])
        # Case 04: a style of nothing of its own, based on one that gives level 0.
        levels = check_list_case(styles, instances, paragraphs[3], "LinkedBasedOn",
                                 labels[3])
        self.assertIn("LinkedBasedOn writes nothing and is based on LinkedBase", labels[3])
        based_on = styles["LinkedBasedOn"]
        self.assertEqual([c.tag.replace(W, "") for c in based_on], ["name", "basedOn"])
        self.assertEqual(based_on.find(f"{W}basedOn").get(f"{W}val"), "LinkedBase")
        self.assertEqual(num_pr_of(styles["LinkedBase"], f"{W}pPr/{W}numPr"),
                         {"ilvl": "0", "numId": "3"})
        self.assertEqual(len(levels), 3)
        # The three lists use definitions of their own.
        self.assertEqual(len({id(d) for d in instances.values()}), 3)

        # E-list-level-linked-defaults: the document defaults give level 1 and no list.
        parts, styles, _, instances, paragraphs = list_parts("E-list-level-linked-defaults")
        defaults = ElementTree.fromstring(parts["word/styles.xml"]).find(
            f"{W}docDefaults/{W}pPrDefault/{W}pPr")
        # Level 1: neither the level that names the style nor level 0.
        self.assertEqual(num_pr_of(defaults, f"{W}numPr"), {"ilvl": "1"})
        doc = next(doc for doc in self.docs if doc.name == "E-list-level-linked-defaults")
        self.assertIn("the document defaults give level 1 and level 2 of the list names the style",
                      doc.purpose)
        labels = ["".join(t.text for t in p.iter(f"{W}t")) for p in paragraphs]
        self.assertEqual(len(paragraphs), 2)
        self.assertIn("the document defaults give level 1", labels[0])
        check_list_case(styles, instances, paragraphs[0], "LinkedDefaults", labels[0])
        self.assertTrue(labels[1].startswith("Case 02: a paragraph in Normal, which has level 1 "
                                             "from the document defaults but no list"))
        self.assertIsNone(paragraphs[1].find(f"{W}pPr"))

    def test_table_autofit_and_page_cases_have_the_markup_their_labels_name(self):
        # #368: table autofit, rows across pages and repeated header rows.
        def body(name):
            doc = next(doc for doc in self.docs if doc.name == name)
            return ElementTree.fromstring(
                dict(rules.parts(doc))["word/document.xml"]).find(f"{W}body")

        def attrs(element, path):
            found = element.find(path)
            return None if found is None else {
                k.replace(W, ""): v for k, v in found.attrib.items()}

        def labelled_tables(name):
            """{case number: its table}, from the "Case NN:" paragraph before each table."""
            found, number = {}, None
            for child in body(name):
                if child.tag == f"{W}p":
                    words = "".join(t.text or "" for t in child.iter(f"{W}t"))
                    match = re.match(r"Case (\d\d): ", words)
                    number = int(match.group(1)) if match else number
                elif child.tag == f"{W}tbl":
                    found[number] = child
            return found

        def cell_widths(tbl):
            return [attrs(tc, f"{W}tcPr/{W}tcW") for tc in tbl.find(f"{W}tr").findall(f"{W}tc")]

        tables = labelled_tables("E-table-autofit-15")
        self.assertEqual(sorted(tables), [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12])
        for n, tbl in tables.items():
            fixed = attrs(tbl, f"{W}tblPr/{W}tblLayout")
            self.assertEqual(fixed, {"type": "fixed"} if n == 10 else None, n)
        self.assertEqual(attrs(tables[1], f"{W}tblPr/{W}tblW"), {"w": "0", "type": "auto"})
        self.assertEqual(cell_widths(tables[1]), [None] * 3)
        expected = {2: ("5000", "pct", "0", "auto"), 3: ("0", "auto", "1440", "dxa"),
                    4: ("0", "auto", "4000", "dxa"), 5: ("0", "auto", "2000", "pct")}
        for name in ("E-table-autofit-15", "E-table-autofit-14", "E-table-autofit-constrained-14",
                     "E-table-autofit-grow-14"):
            found = labelled_tables(name)
            for n, (tbl_w, tbl_type, tc_w, tc_type) in expected.items():
                with self.subTest(name, case=n):
                    self.assertEqual(attrs(found[n], f"{W}tblPr/{W}tblW"),
                                     {"w": tbl_w, "type": tbl_type})
                    self.assertEqual(cell_widths(found[n]), [{"w": tc_w, "type": tc_type}] * 3)
        # Cases 11 and 12, which the settings of mode 14 act on (ISO/IEC 29500 §17.15.3), in
        # every autofit document: minimum widths wider than the text width, and a table
        # beside a floating picture with square wrap.
        v = "{urn:schemas-microsoft-com:vml}"
        for name in ("E-table-autofit-15", "E-table-autofit-14", "E-table-autofit-constrained-14",
                     "E-table-autofit-grow-14"):
            found = labelled_tables(name)
            with self.subTest(name):
                self.assertEqual(sorted(found)[-2:], [11, 12])
                words = ["".join(t.text for t in tc.iter(f"{W}t"))
                         for tc in found[11].find(f"{W}tr").findall(f"{W}tc")]
                self.assertEqual(words, [rules.AUTOFIT_WORD] * 3)
                # Calibri's twin Carlito: three of the word are wider than the text width.
                self.assertGreater(3 * carlito_width(rules.AUTOFIT_WORD), rules.TEXT_W / 20)
                children = list(body(name))
                labels = ["".join(t.text or "" for t in c.iter(f"{W}t")) for c in children]
                at = next(i for i, text in enumerate(labels) if text.startswith("Case 12: "))
                self.assertEqual(children[at + 2].tag, f"{W}tbl")
                shape = children[at + 1].find(f".//{v}shape")
                self.assertIn("position:absolute", shape.get("style"))
                style = dict(i.split(":", 1) for i in shape.get("style").split(";"))
                self.assertEqual((style["margin-left"], style["margin-top"], style["width"],
                                  style["height"], style["mso-position-horizontal-relative"],
                                  style["mso-position-vertical-relative"]),
                                 ("0", "0", "216pt", "108pt", "margin", "paragraph"))
                # The table must be narrowed to fit beside the picture: its content, the
                # widest text of each column with 5.4 pt cell margins on each side, is wider
                # than the room the picture leaves, and its longest words are not.
                tbl = children[at + 2]
                self.assertIsNone(tbl.find(f"{W}tblPr/{W}tblLayout"))
                # No preferred width anywhere: the only kind of table Word narrows to fit
                # beside a floating object (MS-OI29500 2.1.1777).
                self.assertEqual(attrs(tbl, f"{W}tblPr/{W}tblW"), {"w": "0", "type": "auto"})
                self.assertEqual({tuple(attrs(tc, f"{W}tcPr/{W}tcW").items())
                                  for tc in tbl.iter(f"{W}tc")},
                                 {(("w", "0"), ("type", "auto"))})
                columns = zip(*[["".join(t.text for t in tc.iter(f"{W}t"))
                                 for tc in tr.findall(f"{W}tc")] for tr in tbl.findall(f"{W}tr")])
                content = sum(max(carlito_width(t) for t in col) + 10.8 for col in columns)
                # The room beside the picture, less the 9 pt wrap distance Word writes on
                # its VML shapes.
                room = rules.TEXT_W / 20 - 216 - 9
                self.assertGreater(content, room)
                words = [w for tr in tbl.findall(f"{W}tr") for tc in tr.findall(f"{W}tc")
                         for w in "".join(t.text for t in tc.iter(f"{W}t")).split()]
                self.assertLess(3 * (max(carlito_width(w) for w in words) + 10.8), room)
                self.assertEqual(shape.find("{urn:schemas-microsoft-com:office:word}wrap")
                                 .get("type"), "square")
        # Case 3 sums to less than the text width, case 4 to more, case 5 to 120 %.
        self.assertLess(3 * 1440, rules.TEXT_W)
        self.assertGreater(3 * 4000, rules.TEXT_W)
        # Case 6: the long word alone in a narrow column; case 10: the same word, fixed.
        for n, column in ((6, "720"), (10, "1440")):
            first = tables[n].find(f"{W}tr/{W}tc")
            self.assertEqual(attrs(first, f"{W}tcPr/{W}tcW")["w"], column)
            self.assertEqual("".join(t.text for t in first.iter(f"{W}t")), rules.AUTOFIT_WORD)
        self.assertNotIn(" ", rules.AUTOFIT_WORD)
        # Case 7: a nested table in the first cell; case 8: a spanning cell of 300 pt.
        self.assertIsNotNone(tables[7].find(f"{W}tr/{W}tc/{W}tbl"))
        first_row = tables[8].find(f"{W}tr/{W}tc")
        self.assertEqual(attrs(first_row, f"{W}tcPr/{W}gridSpan"), {"val": "2"})
        self.assertEqual(attrs(first_row, f"{W}tcPr/{W}tcW"), {"w": "6000", "type": "dxa"})

        def only_table(name):
            (tbl,) = body(name).findall(f"{W}tbl")
            borders = tbl.find(f"{W}tblPr/{W}tblBorders")
            self.assertEqual({b.get(f"{W}sz") for b in borders}, {"48"}, name)
            return tbl

        def lines_before_table(name):
            return sum(1 for child in body(name).iter() if child.tag == f"{W}t"
                       and (child.text or "").startswith("Filler"))

        for name, cant_split, lines, filler in (
            ("E-table-row-split", False, 40, 30),
            ("E-table-row-cant-split", True, 40, 30),
            ("E-table-row-cant-split-tall", True, 80, 0),
        ):
            with self.subTest(name):
                rows = only_table(name).findall(f"{W}tr")
                self.assertEqual(rows[2].find(f"{W}trPr/{W}cantSplit") is not None, cant_split)
                self.assertEqual(len(rows[2].find(f"{W}tc").findall(f"{W}p")), lines)
                self.assertEqual(lines_before_table(name), filler)
        rows = only_table("E-table-header-repeat").findall(f"{W}tr")
        self.assertEqual(len(rows), 62)
        self.assertEqual([r.find(f"{W}trPr/{W}tblHeader") is not None for r in rows],
                         [True, True] + [False] * 60)
        rows = only_table("E-table-header-repeat-tall").findall(f"{W}tr")
        self.assertEqual([r.find(f"{W}trPr/{W}tblHeader") is not None for r in rows],
                         [True, True] + [False] * 60)
        self.assertEqual([len(r.find(f"{W}tc").findall(f"{W}p")) for r in rows[:2]], [35, 35])
        nested = only_table("E-table-row-split-nested").find(f"{W}tr/{W}tc/{W}tbl")
        self.assertEqual(len(nested.findall(f"{W}tr")), 30)
        self.assertEqual(lines_before_table("E-table-row-split-nested"), 30)
        # Rows of exact height whose content is taller than the row (the maintainer's
        # addition to #368).
        exact = labelled_tables("E-table-row-exact-height")
        self.assertEqual(sorted(exact), [1, 2, 3, 4, 5, 6])
        rows = {n: tbl.findall(f"{W}tr")[1] for n, tbl in exact.items()}
        self.assertEqual({n: attrs(r, f"{W}trPr/{W}trHeight") for n, r in rows.items()},
                         {1: {"val": "600", "hRule": "exact"}, 2: {"val": "160", "hRule": "exact"},
                          **{n: {"val": "400", "hRule": "exact"} for n in (3, 4, 5, 6)}})
        self.assertEqual(len(rows[1].find(f"{W}tc").findall(f"{W}p")), 5)
        for n, align in ((3, None), (4, {"val": "center"}), (5, {"val": "bottom"})):
            first = rows[n].find(f"{W}tc")
            self.assertEqual(attrs(first, f"{W}tcPr/{W}vAlign"), align, n)
            self.assertEqual(attrs(first, f"{W}p/{W}r/{W}rPr/{W}sz"), {"val": "56"}, n)
        shape = rows[6].find(f"{W}tc/{W}p/{W}r/{W}pict/{v}shape")
        self.assertIn("height:60pt", shape.get("style"))

    def test_vml_picture_cases_have_the_markup_their_labels_name(self):
        v = "{urn:schemas-microsoft-com:vml}"
        r_ns = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
        doc = next(doc for doc in self.docs if doc.name == "E-vml-pictures")
        parts = dict(rules.parts(doc))
        body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")
        shapes = list(body.iter(f"{v}shape"))
        styles = [dict(item.split(":", 1) for item in shape.get("style").split(";"))
                  for shape in shapes]
        self.assertEqual([s.get("position") for s in styles],
                         [None, None, "absolute", "absolute", "absolute"])
        self.assertEqual([(s["width"], s["height"]) for s in styles],
                         [("72pt", "36pt"), ("36pt", "48pt")] + [("72pt", "72pt")] * 3)
        self.assertEqual([s.get("mso-position-vertical-relative") for s in styles[2:]],
                         ["paragraph", "paragraph", "page"])
        self.assertEqual([s.get("mso-position-horizontal-relative") for s in styles[2:]],
                         ["margin", "margin", "page"])
        self.assertEqual([(s.get("margin-left"), s.get("margin-top")) for s in styles[2:]],
                         [("144pt", "6pt"), ("36pt", "0pt"), ("300pt", "150pt")])
        self.assertLess(int(styles[3]["z-index"]), 0)
        # Case 05 starts a page of its own.
        label = next(p for p in body.iter(f"{W}p")
                     if "".join(t.text or "" for t in p.iter(f"{W}t")).startswith("Case 05: "))
        self.assertIsNotNone(label.find(f"{W}pPr/{W}pageBreakBefore"))
        w10 = "{urn:schemas-microsoft-com:office:word}"
        self.assertEqual([None if shape.find(f"{w10}wrap") is None
                          else shape.find(f"{w10}wrap").get("type") for shape in shapes],
                         [None, None, "square", "none", "square"])
        # Each picture names a PNG part of the package, of the pixel size its label implies.
        rels = ElementTree.fromstring(parts["word/_rels/document.xml.rels"])
        targets = {rel.get("Id"): rel.get("Target") for rel in rels}
        for shape in shapes:
            target = targets[shape.find(f"{v}imagedata").get(f"{r_ns}id")]
            data = parts[f"word/{target}"]
            self.assertTrue(data.startswith(b"\x89PNG\r\n\x1a\n"), target)
        self.assertEqual(struct.unpack(">II", parts["word/media/image1.png"][16:24]), (96, 48))
        self.assertIn(b'Extension="png"', parts["[Content_Types].xml"])

    def wrap_cases(self, name: str) -> dict[int, tuple[str, ElementTree.Element]]:
        """Each case of a wrap document by number: its label and the paragraph after it,
        which holds the case's object."""
        doc = next(doc for doc in self.docs if doc.name == name)
        body = ElementTree.fromstring(dict(rules.parts(doc))["word/document.xml"]).find(
            f"{W}body")
        paragraphs = body.findall(f"{W}p")
        cases = {}
        for p, after in zip(paragraphs, paragraphs[1:]):
            label = "".join(t.text or "" for t in p.iter(f"{W}t"))
            if label.startswith("Case "):
                cases[int(label[5:7])] = (label, after)
        return cases

    def test_drawingml_wrap_cases_have_the_markup_their_labels_name(self):
        wp = "{http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing}"
        emu = rules.EMU_PER_PT

        def summary(after: ElementTree.Element) -> tuple:
            anchor = after.find(f".//{wp}anchor")
            wrap = next(child for child in anchor
                        if child.tag.startswith(wp) and child.tag[len(wp):].startswith("wrap"))
            across, down = (anchor.find(f"{wp}position{axis}") for axis in "HV")

            def place(position):
                offset = position.find(f"{wp}posOffset")
                value = (int(offset.text) / emu if offset is not None
                         else position.find(f"{wp}align").text)
                return position.get("relativeFrom"), value

            polygon = [(int(point.get("x")), int(point.get("y")))
                       for point in wrap.iter() if point.tag in (f"{wp}start", f"{wp}lineTo")]
            edited = wrap.find(f"{wp}wrapPolygon")
            extent = anchor.find(f"{wp}extent")
            return (wrap.tag[len(wp):], wrap.get("wrapText"), place(across), place(down),
                    tuple(int(anchor.get(f"dist{side}")) / emu for side in "TBLR"),
                    anchor.get("behindDoc"), tuple(polygon),
                    None if edited is None else edited.get("edited"),
                    (int(extent.get("cx")) / emu, int(extent.get("cy")) / emu))

        margin = ("margin", 0.0)
        top = ("paragraph", 0.0)
        usual = (0.0, 0.0, 9.0, 9.0)
        square = (108.0, 108.0)
        # The polygons, in the space of 21600 x 21600 (MS-OI29500 2.1.1354), each closed.
        whole = ((0, 0), (0, 21600), (21600, 21600), (21600, 0), (0, 0))
        triangle = ((0, 0), (0, 21600), (21600, 10800), (0, 0))
        diamond = ((10800, 0), (0, 10800), (10800, 21600), (21600, 10800), (10800, 0))
        middle = ((5400, 5400), (5400, 16200), (16200, 16200), (16200, 5400), (5400, 5400))
        open_top = ((0, 0), (0, 21600), (21600, 21600), (21600, 0), (18900, 0), (18900, 18900),
                    (2700, 18900), (2700, 0), (0, 0))
        expected = {
            "E-wrap-square": [
                ("wrapSquare", "bothSides", margin, top, usual, "0", (), None, square),
                ("wrapSquare", "bothSides", ("margin", "right"), top, usual, "0", (), None,
                 square),
                ("wrapSquare", "bothSides", ("margin", "center"), top, usual, "0", (), None,
                 square),
                ("wrapSquare", "largest", ("margin", 120.0), top, usual, "0", (), None, square),
                ("wrapSquare", "left", ("margin", 200.0), top, usual, "0", (), None, square),
                ("wrapSquare", "right", ("margin", 200.0), top, usual, "0", (), None, square),
                ("wrapSquare", "bothSides", margin, top, (0.0,) * 4, "0", (), None, square),
                ("wrapSquare", "bothSides", margin, top, (18.0,) * 4, "0", (), None, square),
                ("wrapSquare", "bothSides", margin, ("paragraph", 36.0), usual, "0", (), None,
                 square),
                ("wrapSquare", "bothSides", margin, top, usual, "0", (), None, square),
            ],
            "E-wrap-tight": [
                ("wrapTight", "bothSides", margin, top, usual, "0", whole, "0", square),
                ("wrapSquare", "bothSides", margin, top, usual, "0", (), None, square),
                ("wrapTight", "bothSides", margin, top, usual, "0", triangle, "1", square),
                ("wrapTight", "bothSides", ("margin", "center"), top, usual, "0", diamond, "1",
                 square),
                ("wrapTight", "bothSides", margin, top, usual, "0", middle, "1", square),
                ("wrapTight", "bothSides", margin, top, (18.0, 18.0, 9.0, 9.0), "0", whole,
                 "0", square),
                ("wrapTight", "bothSides", ("margin", "center"), top, usual, "0", open_top, "1",
                 square),
                ("wrapThrough", "bothSides", ("margin", "center"), top, usual, "0", open_top, "1",
                 square),
            ],
            "E-wrap-top-bottom": [
                ("wrapTopAndBottom", None, margin, top, usual, "0", (), None, (144.0, 72.0)),
                ("wrapTopAndBottom", None, margin, top, (12.0, 12.0, 9.0, 9.0), "0", (), None,
                 (144.0, 72.0)),
                ("wrapTopAndBottom", None, ("margin", "center"), top, usual, "0", (), None,
                 (72.0, 72.0)),
                ("wrapTopAndBottom", None, margin, ("paragraph", 40.0), usual, "0", (), None,
                 (144.0, 72.0)),
                ("wrapTopAndBottom", None, margin, top, usual, "0", (), None, (144.0, 72.0)),
                ("wrapTopAndBottom", None, margin, ("page", 150.0), usual, "0", (), None,
                 (144.0, 72.0)),
            ],
            "E-wrap-textbox": [
                ("wrapSquare", "bothSides", margin, top, usual, "0", (), None, (180.0, 90.0)),
                ("wrapSquare", "bothSides", ("margin", "center"), top, usual, "0", (), None,
                 (180.0, 90.0)),
                ("wrapTight", "bothSides", margin, top, usual, "0", whole, "0", (180.0, 90.0)),
                ("wrapTopAndBottom", None, margin, top, usual, "0", (), None, (180.0, 90.0)),
                ("wrapNone", None, margin, top, usual, "0", (), None, (180.0, 90.0)),
                ("wrapNone", None, margin, top, usual, "1", (), None, (180.0, 90.0)),
                ("wrapSquare", "bothSides", margin, top, usual, "0", (), None, (180.0, 90.0)),
            ],
        }
        for name, cases in expected.items():
            found = self.wrap_cases(name)
            self.assertEqual(sorted(found), list(range(1, len(cases) + 1)), name)
            for n, case in enumerate(cases, 1):
                with self.subTest(name=name, case=n):
                    self.assertEqual(summary(found[n][1]), case)
        # Each case's paragraph holds 24 sentences after its object, enough to run beside it
        # and on below it (case 06 of E-wrap-top-bottom 40, on a page of its own).
        for name in expected:
            for n, (label, after) in self.wrap_cases(name).items():
                text = "".join(t.text or "" for t in after.iter(f"{W}t"))
                sentences = 40 if (name, n) == ("E-wrap-top-bottom", 6) else 24
                if (name, n) != ("E-wrap-square", 10):
                    with self.subTest(name=name, case=n):
                        self.assertIn(f"Text {sentences:02d} of case {n:02d} ", text)
                        self.assertNotIn(f"Text {sentences + 1:02d} of case {n:02d} ", text)
        # Case 10 of E-wrap-square: a one-line anchor paragraph, then the 24 sentences in two
        # paragraphs that wrap around its picture.
        doc = next(doc for doc in self.docs if doc.name == "E-wrap-square")
        body = ElementTree.fromstring(dict(rules.parts(doc))["word/document.xml"]).find(
            f"{W}body")
        texts = ["".join(t.text or "" for t in p.iter(f"{W}t")) for p in body.findall(f"{W}p")]
        at = next(i for i, text in enumerate(texts) if text.startswith("Case 10: "))
        self.assertEqual(texts[at + 1], "The anchor of case 10, one line.")
        # Four sentences, short enough that the next paragraph starts beside the picture too.
        self.assertTrue(texts[at + 2].startswith("Text 01 of case 10 ")
                        and texts[at + 2].endswith("Text 04 of case 10 wraps around the object."))
        self.assertTrue(texts[at + 3].startswith("Text 05 of case 10 ")
                        and texts[at + 3].endswith("Text 24 of case 10 wraps around the object."))
        # Each label is kept with the paragraph after it.
        for name in expected:
            doc = next(doc for doc in self.docs if doc.name == name)
            labels = [p for p in ElementTree.fromstring(dict(rules.parts(doc))[
                "word/document.xml"]).iter(f"{W}p")
                if "".join(t.text or "" for t in p.iter(f"{W}t")).startswith("Case ")]
            with self.subTest(name=name):
                self.assertTrue(all(p.find(f"{W}pPr/{W}keepNext") is not None for p in labels))
        # Case 05 of E-wrap-top-bottom holds its anchor after the paragraph's third sentence.
        label, after = self.wrap_cases("E-wrap-top-bottom")[5]
        runs = after.findall(f"{W}r")
        self.assertTrue("".join(t.text or "" for t in runs[0].iter(f"{W}t")).endswith(
            "Text 03 of case 05 wraps around the object. "))
        self.assertIsNotNone(runs[1].find(f".//{wp}anchor"))
        # The text boxes are filled light grey with an outline, but the white one of case 07.
        a = "{http://schemas.openxmlformats.org/drawingml/2006/main}"
        boxes = self.wrap_cases("E-wrap-textbox")
        fills = [after.find(f".//{a}solidFill/{a}srgbClr").get("val") for _, after in
                 (boxes[n] for n in sorted(boxes))]
        self.assertEqual(fills, ["BFBFBF"] * 6 + ["FFFFFF"])
        self.assertIsNotNone(boxes[7][1].find(f".//{a}ln/{a}noFill"))
        # Two cases to a page: every odd case after the first starts a page, and case 10 of
        # E-wrap-square; in E-wrap-top-bottom cases 03, 05 and 06.
        for name in expected:
            doc = next(doc for doc in self.docs if doc.name == name)
            body = ElementTree.fromstring(dict(rules.parts(doc))["word/document.xml"])
            starts = []
            for p in body.iter(f"{W}p"):
                label = "".join(t.text or "" for t in p.iter(f"{W}t"))
                if label.startswith("Case ") and p.find(f"{W}pPr/{W}pageBreakBefore") is not None:
                    starts.append(int(label[5:7]))
            with self.subTest(name=name):
                self.assertEqual(starts, [3, 5, 6] if name == "E-wrap-top-bottom"
                                 else [n for n in range(3, len(expected[name]) + 1)
                                       if n % 2 == 1 or n == 10])

    def test_vml_wrap_cases_have_the_markup_their_labels_name(self):
        v = "{urn:schemas-microsoft-com:vml}"
        w10 = "{urn:schemas-microsoft-com:office:word}"

        def summary(after: ElementTree.Element) -> tuple:
            shape = after.find(f".//{v}shape")
            style = dict(item.split(":", 1) for item in shape.get("style").split(";"))
            wrap = shape.find(f"{w10}wrap")
            if wrap is None:
                wrap = ElementTree.Element("none")
            distances = tuple(style.get(f"mso-wrap-distance-{side}")
                              for side in ("left", "top", "right", "bottom"))
            self.assertEqual(style.get("position"), "absolute")
            return (wrap.get("type"), wrap.get("side"), style.get("margin-left"),
                    style.get("mso-position-horizontal"), shape.get("wrapcoords"), distances,
                    int(style["z-index"]) < 0, shape.get("fillcolor"), shape.get("stroked"))

        none = (None,) * 4
        pictures = [
            ("square", None, "0", None, None, none, False, None, None),
            ("square", None, "0", "center", None, none, False, None, None),
            ("tight", None, "0", None, None, none, False, None, None),
            ("tight", None, "0", None, "0 0 0 21600 21600 10800 0 0", none, False, None, None),
            ("topAndBottom", None, "0", None, None, none, False, None, None),
            ("square", "left", "200pt", None, None, none, False, None, None),
            ("square", None, "0", None, None, ("18pt",) * 4, False, None, None),
            ("through", None, "0", "center",
             "0 0 0 21600 21600 21600 21600 0 18900 0 18900 18900 2700 18900 2700 0 0 0",
             none, False, None, None),
            (None, None, "0", None, None, none, False, None, None),
        ]
        boxes = [
            ("square", None, "0", None, None, none, False, "#BFBFBF", None),
            ("tight", None, "0", None, None, none, False, "#BFBFBF", None),
            ("topAndBottom", None, "0", None, None, none, False, "#BFBFBF", None),
            ("none", None, "0", None, None, none, False, "#BFBFBF", None),
            ("none", None, "0", None, None, none, True, "#BFBFBF", None),
            ("square", None, "0", None, None, none, False, "#FFFFFF", "f"),
        ]
        for name, cases in (("E-wrap-vml", pictures), ("E-wrap-vml-textbox", boxes)):
            found = self.wrap_cases(name)
            self.assertEqual(sorted(found), list(range(1, len(cases) + 1)), name)
            for n, case in enumerate(cases, 1):
                with self.subTest(name=name, case=n):
                    self.assertEqual(summary(found[n][1]), case)
        # The wrap follows the text box's v:textbox, inside its v:shape.
        shape = self.wrap_cases("E-wrap-vml-textbox")[1][1].find(f".//{v}shape")
        self.assertEqual([child.tag for child in shape], [f"{v}textbox", f"{w10}wrap"])

    def test_symbol_font_cases_have_the_markup_their_labels_name(self):
        # The font is the family scripts/make-symbol-font.py names, which Word has installed.
        self.assertEqual(rules.SYMBOL_FONT, load("make-symbol-font").FAMILY)
        doc = next(doc for doc in self.docs if doc.name == "E-symbol-font")
        parts = dict(rules.parts(doc))
        body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")
        symbol, other = rules.SYMBOL_FONT, "Liberation Sans"
        every = {slot: symbol for slot in ("ascii", "hAnsi", "eastAsia", "cs")}

        def fonts(r):
            found = r.find(f"{W}rPr/{W}rFonts")
            return {} if found is None else {k[len(W):]: v for k, v in found.attrib.items()}

        cases = {}
        for p in body.iter(f"{W}p"):
            runs = p.findall(f"{W}r")
            label = "".join(t.text or "" for t in runs[0].iter(f"{W}t")) if runs else ""
            if label.startswith("Case "):
                self.assertEqual(fonts(runs[0])["ascii"], other, label)
                cases[int(label[5:7])] = (p, runs[1:])
        self.assertEqual(sorted(cases), list(range(1, 25)))

        def sample(n):
            """The text and fonts of each run of case n after its label."""
            return [("".join(t.text or "" for t in r.iter(f"{W}t")), fonts(r))
                    for r in cases[n][1]]

        def codes(value):
            return "".join(chr(0xF000 + ord(c)) for c in value)

        letters = "ABC xyz 0123 !?&"
        self.assertEqual(sample(1), [(letters, every)])
        self.assertEqual(sample(2), [(codes(letters), every)])
        self.assertEqual(sample(3)[0][0], letters)
        self.assertEqual(sample(3)[0][1]["ascii"], "Noto Sans")
        for n, chars in ((4, ["F041", "F062", "F033"]), (5, ["0041", "0062", "0033"])):
            syms = [r.find(f"{W}sym") for r in cases[n][1]]
            self.assertEqual([s.get(f"{W}font") for s in syms], [symbol] * 3, n)
            self.assertEqual([s.get(f"{W}char") for s in syms], chars, n)
        self.assertEqual(sample(6), [("Äéß·", every)])
        # Cyrillic, Greek and the euro sign: none is a code of the font, U+0020 to U+00FF or
        # U+F020 to U+F0FF.
        self.assertEqual(sample(7), [("\u0416\u0436 \u03a9\u03c9 \u20ac", every)])
        for c in sample(7)[0][0].replace(" ", ""):
            self.assertFalse(0x20 <= ord(c) <= 0xFF or 0xF020 <= ord(c) <= 0xF0FF, c)
        alone = {"ascii": other, "hAnsi": other, "eastAsia": other, "cs": other}
        for n, value, slots in (
            (8, "ABC", {"ascii": symbol}),
            (9, "ABC", {"hAnsi": symbol}),
            (10, codes("ABC"), {"ascii": symbol}),
            (11, codes("ABC"), {"hAnsi": symbol}),
            (12, codes("ABC"), {"eastAsia": symbol, "hint": "eastAsia"}),
        ):
            self.assertEqual(sample(n), [(value, {**alone, **slots})], n)
        for n, toggle in ((13, "caps"), (14, "smallCaps")):
            self.assertEqual(sample(n), [("abc", every)], n)
            self.assertIsNotNone(cases[n][1][0].find(f"{W}rPr/{W}{toggle}"), n)
        bold, italic = cases[15][1]
        self.assertEqual(sample(15), [("ABC", every), (" ABC", every)])
        self.assertIsNotNone(bold.find(f"{W}rPr/{W}b"))
        self.assertIsNone(bold.find(f"{W}rPr/{W}i"))
        self.assertIsNotNone(italic.find(f"{W}rPr/{W}i"))
        self.assertIsNone(italic.find(f"{W}rPr/{W}b"))
        self.assertEqual(sample(16), [("A B  C\u00a0D", every)])
        sym = cases[17][1][0]
        self.assertEqual(sym.find(f"{W}sym").get(f"{W}char"), "F0B7")
        self.assertEqual(sym.find(f"{W}sym").get(f"{W}font"), symbol)
        self.assertEqual(fonts(sym)["ascii"], other)
        self.assertEqual(sym.find(f"{W}rPr/{W}sz").get(f"{W}val"), "40")
        self.assertEqual(sample(18), [(codes("\u00c4\u00e9\u00df\u00b7"), every)])
        # Codes of the symbol range that Noto Sans maps nothing to at U+007F to U+009F.
        self.assertEqual(sample(19), [("\uf07f\uf080\uf09f", every)])
        # Cases 20 and 21 are list items whose bullet is in the symbol font.
        numbering = ElementTree.fromstring(parts["word/numbering.xml"])
        abstract = {a.get(f"{W}abstractNumId"): a for a in numbering.iter(f"{W}abstractNum")}
        instances = {n.get(f"{W}numId"): n.find(f"{W}abstractNumId").get(f"{W}val")
                     for n in numbering.iter(f"{W}num")}
        for n, bullet in ((20, ""), (21, "·")):
            num_id = cases[n][0].find(f"{W}pPr/{W}numPr/{W}numId").get(f"{W}val")
            lvl = abstract[instances[num_id]].find(f"{W}lvl")
            self.assertEqual(lvl.find(f"{W}numFmt").get(f"{W}val"), "bullet", n)
            self.assertEqual(lvl.find(f"{W}lvlText").get(f"{W}val"), bullet, n)
            self.assertEqual(lvl.find(f"{W}rPr/{W}rFonts").get(f"{W}ascii"), symbol, n)
        # Cases 22 and 23: the next paragraph is shaded, five lines in the symbol font.
        paragraphs = list(body.iter(f"{W}p"))
        for n, size in ((22, "22"), (23, "40")):
            shaded = paragraphs[paragraphs.index(cases[n][0]) + 1]
            self.assertIsNotNone(shaded.find(f"{W}pPr/{W}shd"), n)
            (r,) = shaded.findall(f"{W}r")
            self.assertEqual(fonts(r), every, n)
            self.assertEqual(r.find(f"{W}rPr/{W}sz").get(f"{W}val"), size, n)
            self.assertEqual(len(r.findall(f"{W}br")), 4, n)
        # Case 24: the next paragraph is justified, one run of words in the symbol font, wider
        # than two text columns in Carlito (Calibri's twin), so wider still in Noto Sans.
        justified = paragraphs[paragraphs.index(cases[24][0]) + 1]
        self.assertEqual(justified.find(f"{W}pPr/{W}jc").get(f"{W}val"), "both")
        (r,) = justified.findall(f"{W}r")
        self.assertEqual(fonts(r), every)
        words = "".join(t.text or "" for t in r.iter(f"{W}t"))
        self.assertGreater(carlito_width(words), 2 * rules.TEXT_W / 20)
        # The font table gives the symbol font the symbol character set, and no other font
        # a character set.
        table = ElementTree.fromstring(parts["word/fontTable.xml"])
        charsets = {f.get(f"{W}name"): f.find(f"{W}charset") for f in table}
        self.assertEqual(charsets[symbol].get(f"{W}val"), "02")
        self.assertEqual([name for name, c in charsets.items() if c is not None], [symbol])

    def test_batch_8_documents_have_the_markup_their_labels_name(self):
        by_name = {doc.name: doc for doc in self.docs}

        def load(name):
            doc = by_name[name]
            parts = dict(rules.parts(doc))
            return doc, parts, ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")

        def texts(element):
            return "".join(t.text or "" for t in element.iter(f"{W}t"))

        def val(element, path, name="val"):
            found = element.find(path)
            return None if found is None else found.get(f"{W}{name}")

        def borders(table):
            return {side.tag[len(W):]: int(side.get(f"{W}sz"))
                    for side in table.find(f"{W}tblPr/{W}tblBorders")}

        def labels(body):
            return [p for p in body.findall(f"{W}p") if texts(p).startswith("Case ")]

        # E-table-split-borders: four cases, each on a page of its own, with the borders
        # the purpose names; the nested table has its own.
        _, _, body = load("E-table-split-borders")
        # The label of each case: "Case NN: " and not the paragraph after its table.
        starts = [p for p in body.findall(f"{W}p")
                  if re.match(r"Case \d\d: ", texts(p)) and "after the table" not in texts(p)]
        self.assertEqual([texts(p)[:7] for p in starts],
                         ["Case 01", "Case 02", "Case 03", "Case 04"])
        self.assertEqual([p.find(f"{W}pPr/{W}pageBreakBefore") is not None for p in starts],
                         [False, True, True, True])
        tables = body.findall(f"{W}tbl")
        self.assertEqual(len(tables), 4)
        for table in tables:
            self.assertEqual(borders(table), rules.SPLIT_BORDERS)
        self.assertEqual(rules.SPLIT_BORDERS,
                         {"top": 48, "bottom": 24, "insideH": 12, "left": 4, "right": 4,
                          "insideV": 4})
        rows = [table.findall(f"{W}tr") for table in tables]
        self.assertEqual([len(r) for r in rows], [4, 3, 42, 1])
        for case_rows in (rows[0], rows[1]):
            self.assertEqual(texts(case_rows[2].find(f"{W}tc")).count("Long"), 40)
        self.assertEqual([r.find(f"{W}trPr/{W}tblHeader") is not None for r in rows[2]],
                         [True, True] + [False] * 40)
        (inner,) = tables[3].findall(f".//{W}tbl")
        self.assertEqual(borders(inner), rules.INNER_BORDERS)
        self.assertEqual(len(inner.findall(f"{W}tr")), 30)
        # Each table starts on its case's page and crosses it: counted in lines of Calibri
        # 11 pt (13.43 pt; 51 fill the 697.9 pt of the body, borders left aside), the label,
        # the filler and the rows before the long one leave room for the long one to start,
        # and the whole table does not fit.
        page_lines = int(697.9 / 13.43)

        def cell_lines(tc):
            nested = [len(t.findall(f"{W}tr")) for t in tc.findall(f"{W}tbl")]
            written = [p for p in tc.findall(f"{W}p") if texts(p)]
            return len(written) + sum(nested)

        children = list(body)
        for case, (table, filler) in enumerate(zip(tables, (30, 30, 20, 30)), 1):
            at = children.index(table)
            before = children[at - filler - 1:at]
            self.assertTrue(texts(before[0]).startswith(f"Case {case:02d}: "), case)
            self.assertEqual([texts(p) for p in before[1:]],
                             [f"Case {case:02d} filler {k:02d}" for k in range(1, filler + 1)])
            lines = [max(cell_lines(tc) for tc in tr.findall(f"{W}tc"))
                     for tr in table.findall(f"{W}tr")]
            longest = lines.index(max(lines))
            self.assertLess(1 + filler + sum(lines[:longest]) + 1, page_lines - 5, case)
            self.assertGreater(1 + filler + sum(lines), page_lines + 5, case)

        # E-table-autofit-shares.
        _, parts, body = load("E-table-autofit-shares")
        self.assertNotIn("word/media/image1.png", parts)
        tables = body.findall(f"{W}tbl")
        self.assertEqual(len(tables), 6)
        span = tables[5].findall(f"{W}tr")
        self.assertEqual([val(tc, f"{W}tcPr/{W}gridSpan") for tc in span[0]], ["2", None])
        self.assertEqual([val(tc, f"{W}tcPr/{W}tcW", "w") for tc in span[0]], ["6000", "1000"])
        self.assertEqual([val(tc, f"{W}tcPr/{W}tcW", "w") for tc in span[1]],
                         ["1000", "2000", "1000"])
        self.assertEqual(len(tables[5].findall(f"{W}tblGrid/{W}gridCol")), 3)
        span = tables[0].findall(f"{W}tr")
        self.assertEqual(val(span[0], f"{W}tc/{W}tcPr/{W}gridSpan"), "3")
        self.assertEqual(val(span[0], f"{W}tc/{W}tcPr/{W}tcW", "w"), "8000")
        self.assertEqual([val(tc, f"{W}tcPr/{W}tcW", "w") for tc in span[1]],
                         ["1000", "2000", "3000"])
        span = tables[1].findall(f"{W}tr")
        self.assertEqual(val(span[0], f"{W}tc/{W}tcPr/{W}gridSpan"), "2")
        self.assertEqual(val(span[0], f"{W}tc/{W}tcPr/{W}tcW", "w"), "6000")
        self.assertEqual([val(tc, f"{W}tcPr/{W}tcW", "w") for tc in span[1]], ["3000", "1000"])
        words = [texts(tc) for tc in tables[2].iter(f"{W}tc")]
        self.assertEqual(words, list(rules.SHARE_WORDS))
        self.assertEqual(len({carlito_width(w) for w in words}), 3)
        # Their minimum widths, cell margins included, exceed the 451.3 pt of text.
        self.assertGreater(sum(carlito_width(w) + 10.8 for w in words), rules.TEXT_W / 20)
        for table, eighths in ((tables[3], 48), (tables[4], 24)):
            self.assertEqual(val(table, f"{W}tblPr/{W}tblW", "type"), "pct")
            self.assertEqual(set(borders(table).values()), {eighths})

        # E-table-autofit-beside-*: the picture and the two tables beside it.
        for name, mode in (("E-table-autofit-beside-15", 15), ("E-table-autofit-beside-14", 14),
                           ("E-table-autofit-beside-constrained-14", 14)):
            doc, parts, body = load(name)
            self.assertEqual(doc.mode, mode)
            self.assertIn("word/media/image1.png", parts)
            shapes = list(body.iter("{urn:schemas-microsoft-com:vml}shape"))
            self.assertEqual(len(shapes), 2, name)
            for shape in shapes:
                self.assertIn("width:216pt;height:108pt", shape.get("style"))
                self.assertIn("mso-position-horizontal-relative:margin", shape.get("style"))
            autofit, fixed = body.findall(f"{W}tbl")
            starts = [p for p in body.findall(f"{W}p") if texts(p).startswith("Case ")]
            self.assertEqual([p.find(f"{W}pPr/{W}pageBreakBefore") is not None for p in starts],
                             [False, True], name)
            self.assertIsNone(autofit.find(f"{W}tblPr/{W}tblLayout"))
            cells = [[texts(tc) for tc in tr.findall(f"{W}tc")] for tr in autofit.iter(f"{W}tr")]
            widest = sum(max(carlito_width(row[c]) for row in cells) + 10.8 for c in range(3))
            # About 120 pt: well within the 226 pt beside the picture and its 9 pt gap.
            self.assertLess(abs(widest - 120), 5, name)
            self.assertEqual(val(fixed, f"{W}tblPr/{W}tblLayout", "type"), "fixed")
            self.assertEqual(val(fixed, f"{W}tblPr/{W}tblW", "w"), "3000")

        # E-table-header-repeat-limit and -single-tall.
        for name, heads in (("E-table-header-repeat-limit", [22, 22]),
                            ("E-table-header-repeat-single-tall", [60])):
            _, _, body = load(name)
            (table,) = body.findall(f"{W}tbl")
            rows = table.findall(f"{W}tr")
            self.assertEqual(len(rows), len(heads) + 10, name)
            for row, count in zip(rows, heads):
                self.assertIsNotNone(row.find(f"{W}trPr/{W}tblHeader"))
                self.assertEqual(texts(row.find(f"{W}tc")).count("Head"), count)
            self.assertIsNone(rows[len(heads)].find(f"{W}trPr/{W}tblHeader"))
            after = [texts(p) for p in body.findall(f"{W}p")][-2:]
            self.assertEqual(after[-1], "End of the document.")
            self.assertEqual(after[0] == "After the table.", len(heads) == 2, name)

        # E-table-row-exact-valign.
        _, _, body = load("E-table-row-exact-valign")
        seen = []
        for table in body.findall(f"{W}tbl"):
            row = table.findall(f"{W}tr")[1]
            seen.append((val(row, f"{W}trPr/{W}trHeight"), val(row, f"{W}trPr/{W}trHeight", "hRule"),
                         val(row, f"{W}tc/{W}tcPr/{W}vAlign"), texts(row.find(f"{W}tc"))))
        self.assertEqual(seen, [("800", "exact", "top", "Short"), ("800", "exact", "center", "Short"),
                                ("800", "exact", "bottom", "Short"),
                                ("800", "atLeast", "center", "Short")])

        # E-table-nested-empty-para-14: the first cell ends with a nested table and then an
        # empty paragraph, or one of text.
        doc, _, body = load("E-table-nested-empty-para-14")
        self.assertEqual(doc.mode, 14)
        ends = []
        for table in body.findall(f"{W}tbl"):
            first = table.find(f"{W}tr/{W}tc")
            children = [child.tag for child in first if child.tag != f"{W}tcPr"]
            self.assertEqual(children[-2:], [f"{W}tbl", f"{W}p"])
            ends.append(texts(list(first)[-1]))
        self.assertEqual(ends, ["", "After the nested table"])

        # E-header-cut: one section per case, with its page size, header distance and part.
        _, parts, body = load("E-header-cut")
        sections = list(body.iter(f"{W}sectPr"))
        self.assertEqual(len(sections), 6)
        sizes = [(val(s, f"{W}pgSz", "w"), val(s, f"{W}pgSz", "h")) for s in sections]
        a4, letter, a5 = ("11906", "16838"), ("12240", "15840"), ("8391", "11906")
        self.assertEqual(sizes, [a4, a4, a4, letter, a5, a4])
        self.assertEqual([val(s, f"{W}pgMar", "header") for s in sections],
                         ["708", "708", "1440", "708", "708", "708"])
        rels = ElementTree.fromstring(parts["word/_rels/document.xml.rels"])
        targets = {rel.get("Id"): rel.get("Target") for rel in rels}
        r_id = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}id"
        # A section without a default header or footer of its own shows the previous one's
        # (E-section-restart-parity); what each section shows:
        shown, header, footer = [], None, None
        for s in sections:
            for ref in s:
                if ref.tag in (f"{W}headerReference", f"{W}footerReference"):
                    self.assertEqual(ref.get(f"{W}type"), "default")
                    if ref.tag == f"{W}headerReference":
                        header = targets[ref.get(r_id)]
                    else:
                        footer = targets[ref.get(r_id)]
            shown.append((header, footer))
        self.assertEqual(shown, [(f"header{n}.xml", None) for n in range(1, 6)]
                         + [("header6.xml", "footer6.xml")])
        (empty,) = ElementTree.fromstring(parts["word/header6.xml"]).findall(f"{W}p")
        self.assertEqual(texts(empty), "")
        self.assertIsNone(empty.find(f"{W}pPr/{W}shd"))
        for target, count, height in (("header1.xml", 400, "20"), ("header2.xml", 200, "40"),
                                      ("footer6.xml", 200, "40")):
            paragraphs = ElementTree.fromstring(parts[f"word/{target}"]).findall(f"{W}p")
            self.assertEqual(len(paragraphs), count, target)
            self.assertEqual({(val(p, f"{W}pPr/{W}spacing", "line"),
                               val(p, f"{W}pPr/{W}spacing", "lineRule")) for p in paragraphs},
                             {(height, "exact")}, target)
            self.assertEqual({texts(p) for p in paragraphs}, {""}, target)
            # Bands of 20 (1 pt) or 10 (2 pt) lines, alternately grey and dark grey.
            band = 20 if height == "20" else 10
            fills = [val(p, f"{W}pPr/{W}shd", "fill") for p in paragraphs]
            self.assertEqual(fills, [("D9D9D9", "808080")[(n // band) % 2] for n in range(count)],
                             target)
        for target in ("header3.xml", "header4.xml", "header5.xml"):
            (paragraph,) = ElementTree.fromstring(parts[f"word/{target}"]).findall(f"{W}p")
            self.assertEqual(len(paragraph.findall(f"{W}r/{W}br")), 39, target)
            self.assertEqual(val(paragraph, f"{W}pPr/{W}spacing", "line"), "240", target)

        # E-number-formats-letters: the sections restart at the numbers the purpose names.
        _, _, body = load("E-number-formats-letters")
        sections = list(body.iter(f"{W}sectPr"))
        self.assertEqual([int(val(s, f"{W}pgNumType", "start")) for s in sections],
                         [520, 780, 781, 1040, 1300, 1664, 5000, 10000])
        instructions = [f.get(f"{W}instr").strip() for f in body.iter(f"{W}fldSimple")]
        self.assertEqual(len(instructions), 40)
        self.assertEqual(set(instructions), {f"PAGE \\* {s}" for s in
                                             ("Arabic", "ALPHABETIC", "alphabetic", "ROMAN",
                                              "roman")})

        # E-superscript-steps: every half point from 8 to 14 pt in both fonts.
        _, _, body = load("E-superscript-steps")
        found = []
        for p in labels(body):
            runs = p.findall(f"{W}r")[1:]
            self.assertEqual([val(r, f"{W}rPr/{W}vertAlign") for r in runs],
                             [None, "superscript", None, "subscript"])
            self.assertEqual(len({val(r, f"{W}rPr/{W}sz") for r in runs}), 1)
            found.append((val(runs[0], f"{W}rPr/{W}rFonts", "ascii"),
                          int(val(runs[0], f"{W}rPr/{W}sz"))))
        self.assertEqual(found, [(font, hp) for font in ("Calibri", "Liberation Sans")
                                 for hp in range(16, 29)])

        # E-table-float-center-margins: unequal margins, four anchors, placed from the page.
        _, _, body = load("E-table-float-center-margins")
        (section,) = body.iter(f"{W}sectPr")
        self.assertEqual((val(section, f"{W}pgMar", "left"), val(section, f"{W}pgMar", "right")),
                         ("2880", "720"))
        placed = [(val(t, f"{W}tblPr/{W}tblpPr", "horzAnchor"),
                   val(t, f"{W}tblPr/{W}tblpPr", "vertAnchor"),
                   val(t, f"{W}tblPr/{W}tblpPr", "tblpXSpec"),
                   val(t, f"{W}tblPr/{W}tblpPr", "tblpY")) for t in body.findall(f"{W}tbl")]
        self.assertEqual(placed, [(None, "page", "center", "2000"),
                                  ("margin", "page", "center", "4400"),
                                  ("page", "page", "center", "6800"),
                                  ("text", "page", "center", "9200")])

        # E-table-style-defaults-size-13-12-14 and -10-11-14: the sizes the name gives.
        for defaults_pt, normal_pt in ((13, 12), (10, 11)):
            name = f"E-table-style-defaults-size-{defaults_pt}-{normal_pt}-14"
            doc, parts, body = load(name)
            self.assertEqual(doc.mode, 14)
            styles = ElementTree.fromstring(parts["word/styles.xml"])
            self.assertEqual(val(styles, f"{W}docDefaults/{W}rPrDefault/{W}rPr/{W}sz"),
                             str(2 * defaults_pt), name)
            self.assertIsNone(styles.find(f"{W}docDefaults/{W}pPrDefault/{W}pPr/{W}jc"), name)
            normal = next(s for s in styles.iter(f"{W}style") if s.get(f"{W}styleId") == "Normal")
            self.assertEqual(val(normal, f"{W}rPr/{W}sz"), str(2 * normal_pt), name)
            self.assertEqual(val(normal, f"{W}pPr/{W}jc"), "left", name)
            big = next(s for s in styles.iter(f"{W}style") if s.get(f"{W}styleId") == "Big")
            self.assertEqual(val(big, f"{W}rPr/{W}sz"), "28")
            (table,) = body.findall(f"{W}tbl")
            self.assertEqual(val(table, f"{W}tblPr/{W}tblStyle"), "Big")
            self.assertIn(f"document defaults {defaults_pt} pt, Normal {normal_pt} pt",
                          texts(body.find(f"{W}p")))

        # E-underline-trailspace-indent: the spaces end where the labels say, for Carlito's
        # space (Calibri's twin) and the 72 pt right indent.
        _, _, body = load("E-underline-trailspace-indent")
        samples = [p for p in body.findall(f"{W}p") if p.find(f"{W}pPr/{W}ind") is not None]
        self.assertEqual([val(p, f"{W}pPr/{W}ind", "right") for p in samples], ["1440"] * 4)
        right_margin = rules.TEXT_W / 20
        for sample, (count, _) in zip(samples, rules.TRAILING_SPACES_INDENT):
            value = texts(sample)
            self.assertEqual(value.count(" "), 4 + count)
            self.assertIsNotNone(sample.find(f"{W}r/{W}rPr/{W}u"))
            words = carlito_width(" ".join(["underlined"] * 5))
            end = words + count * carlito_width(" ")
            label = dict(rules.TRAILING_SPACES_INDENT)[count]
            if label.startswith("11 pt before the right indent"):
                self.assertAlmostEqual(end, right_margin - 72 - 11, delta=2)
            elif label.startswith("36 pt past the right indent"):
                self.assertAlmostEqual(end, right_margin - 36, delta=2)
            elif label.startswith("11 pt past the right margin"):
                self.assertAlmostEqual(end, right_margin + 11, delta=2)
            else:
                self.assertAlmostEqual(end, right_margin + 101, delta=2)

        # E-para-shading-borders-sides: per paragraph its text's start, shading, the sides of
        # its border with their distance from the text, and its space before and after.
        _, _, body = load("E-para-shading-borders-sides")
        seen = []
        for p in body.findall(f"{W}p"):
            sides = p.find(f"{W}pPr/{W}pBdr")
            seen.append((
                texts(p).split(":")[0],
                val(p, f"{W}pPr/{W}shd", "fill"),
                None if sides is None else [(side.tag[len(W):], side.get(f"{W}space"))
                                             for side in sides],
                val(p, f"{W}pPr/{W}spacing", "before"),
                val(p, f"{W}pPr/{W}spacing", "after"),
            ))
        plain = ("not shaded", None, None, None, None)
        self.assertEqual(seen, [
            ("Case 01", "D9D9D9", [("left", "4"), ("right", "4")], None, None), plain,
            ("Case 02", "D9D9D9", [("top", "4"), ("bottom", "4")], None, None), plain,
            ("Case 03", "D9D9D9", [("left", "0")], None, None), plain,
            ("Case 04", "D9D9D9", None, None, "240"), ("Case 04", None, None, "240", None),
            plain,
            ("Case 05", None, None, None, "240"), ("Case 05", "D9D9D9", None, "240", None),
        ])

    def test_drawing_namespaces_are_declared_where_they_are_used(self):
        # A part declares wp, wps and mc only when it uses them (mc also for mc:Ignorable),
        # and an mc:Choice requires only prefixes its part declares on its root.
        for doc in self.docs:
            for name, data in rules.parts(doc):
                if not name.endswith(".xml") or b"<w:" not in data:
                    continue
                xml = data.decode()
                root = xml[xml.index("<w:"):xml.index(">", xml.index("<w:"))]
                for prefix in ("wp", "wps", "mc"):
                    used = f"<{prefix}:" in xml or f" {prefix}:Ignorable=" in root
                    self.assertEqual(f"xmlns:{prefix}=" in root, used,
                                     f"{doc.name}: {name}: {prefix}")
                for required in re.findall(r'<mc:Choice Requires="([^"]*)"', xml):
                    for prefix in required.split():
                        self.assertIn(f"xmlns:{prefix}=", root, f"{doc.name}: {name}")

    def test_drawing_markup_follows_the_schema_order(self):
        # The sequences of ECMA-376 Part 1 for wp:inline (§20.4.2.8), wp:anchor (§20.4.2.3),
        # the shape properties (§20.1.2.2.35), bodyPr (§21.1.2.1.1) and pic:pic (§20.2.2.5),
        # and of [MS-ODRAWXML] 2.3.4 for wps:wsp: every child at its place in the sequence,
        # once, and the required ones there. Word refuses a document that breaks them.
        a = "{http://schemas.openxmlformats.org/drawingml/2006/main}"
        wp = "{http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing}"
        wps = "{http://schemas.microsoft.com/office/word/2010/wordprocessingShape}"
        pic = "{http://schemas.openxmlformats.org/drawingml/2006/picture}"
        wrap = {f"{wp}{k}" for k in ("wrapNone", "wrapSquare", "wrapTight", "wrapThrough",
                                      "wrapTopAndBottom")}
        fill = {f"{a}{k}" for k in ("noFill", "solidFill", "gradFill", "blipFill", "pattFill",
                                     "grpFill")}
        shape_properties = ([{f"{a}xfrm"}, {f"{a}prstGeom"}, fill, {f"{a}ln"}], [])
        sequences = {
            f"{wp}inline": ([{f"{wp}extent"}, {f"{wp}effectExtent"}, {f"{wp}docPr"},
                             {f"{wp}cNvGraphicFramePr"}, {f"{a}graphic"}], [0, 2, 4]),
            f"{wp}anchor": ([{f"{wp}simplePos"}, {f"{wp}positionH"}, {f"{wp}positionV"},
                             {f"{wp}extent"}, {f"{wp}effectExtent"}, wrap, {f"{wp}docPr"},
                             {f"{wp}cNvGraphicFramePr"}, {f"{a}graphic"}], [0, 1, 2, 3, 5, 6, 8]),
            f"{wps}wsp": ([{f"{wps}cNvPr"}, {f"{wps}cNvSpPr"}, {f"{wps}spPr"}, {f"{wps}style"},
                           {f"{wps}txbx"}, {f"{wps}bodyPr"}], [1, 2, 5]),
            f"{wps}spPr": shape_properties,
            f"{pic}spPr": shape_properties,
            f"{wps}bodyPr": ([{f"{a}prstTxWarp"},
                              {f"{a}noAutofit", f"{a}normAutofit", f"{a}spAutoFit"}], []),
            f"{pic}pic": ([{f"{pic}nvPicPr"}, {f"{pic}blipFill"}, {f"{pic}spPr"}], [0, 1, 2]),
            f"{pic}nvPicPr": ([{f"{pic}cNvPr"}, {f"{pic}cNvPicPr"}], [0, 1]),
            f"{pic}blipFill": ([{f"{a}blip"}, {f"{a}srcRect"}, {f"{a}tile", f"{a}stretch"}],
                               []),
            f"{a}xfrm": ([{f"{a}off"}, {f"{a}ext"}], []),
        }
        seen = set()
        for doc in self.docs:
            parts = dict(rules.parts(doc))
            for name, data in parts.items():
                if not re.fullmatch(r"word/(document|header\d+|footer\d+)\.xml", name):
                    continue
                for element in ElementTree.fromstring(data).iter():
                    if element.tag not in sequences:
                        continue
                    seen.add(element.tag)
                    order, required = sequences[element.tag]
                    places = []
                    for child in element:
                        place = [k for k, tags in enumerate(order) if child.tag in tags]
                        self.assertEqual(len(place), 1, f"{doc.name}: {child.tag} in {element.tag}")
                        places.append(place[0])
                    where = f"{doc.name}: {element.tag}"
                    self.assertEqual(places, sorted(set(places)), where)
                    for k in required:
                        self.assertIn(k, places, f"{where} lacks {order[k]}")
        self.assertEqual(seen, set(sequences))

    def test_box_case_labels_stay_with_their_boxes(self):
        # A label whose drawing follows in one of the next two paragraphs is kept on the page
        # of that paragraph (w:keepNext), and so is a paragraph between them, so that no
        # label ends a page without its box.
        wp = "{http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing}"
        v = "{urn:schemas-microsoft-com:vml}"

        def drawn(p):
            return any(p.find(f".//{tag}") is not None
                       for tag in (f"{wp}inline", f"{wp}anchor", f"{v}shape"))

        seen = 0
        for doc in self.docs:
            # The floating boxes of E-textbox-inline-floating are placed from the margin and
            # the page, wherever their anchors fall.
            if not doc.name.startswith(("E-textbox-", "E-shape-colors")) or (
                doc.name == "E-textbox-inline-floating"
            ):
                continue
            body = ElementTree.fromstring(dict(rules.parts(doc))["word/document.xml"])
            paragraphs = body.find(f"{W}body").findall(f"{W}p")
            for k, p in enumerate(paragraphs):
                label = "".join(t.text or "" for t in p.findall(f"{W}r/{W}t"))
                following = paragraphs[k + 1:k + 3]
                if not label.startswith("Case ") or drawn(p) or not any(map(drawn, following)):
                    continue
                seen += 1
                held = next(n for n, q in enumerate(following) if drawn(q))
                for kept in [p] + following[:held]:
                    self.assertIsNotNone(kept.find(f"{W}pPr/{W}keepNext"), f"{doc.name}: {label}")
        self.assertGreater(seen, 30)

    def test_drawing_revision_and_form_field_ids_are_unique(self):
        # Word refuses a document whose drawings share a docPr id; revisions and the ranges
        # of moves have ids of their own, a move's two ranges share a name, and no two legacy
        # form fields have one name.
        wp = "{http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing}"
        for doc in self.docs:
            parts = dict(rules.parts(doc))
            roots = [ElementTree.fromstring(data) for name, data in parts.items()
                     if re.fullmatch(r"word/(document|header\d+|footer\d+)\.xml", name)]
            drawings = [e.get("id") for root in roots for e in root.iter(f"{wp}docPr")]
            self.assertEqual(len(drawings), len(set(drawings)), doc.name)
            revisions = [e.get(f"{W}id") for root in roots for e in root.iter()
                         if e.tag[len(W):] in ("del", "ins", "moveFrom", "moveTo",
                                               "moveFromRangeStart", "moveToRangeStart")]
            self.assertEqual(len(revisions), len(set(revisions)), doc.name)
            moves = {}
            for root in roots:
                for kind in ("moveFrom", "moveTo"):
                    for e in root.iter(f"{W}{kind}RangeStart"):
                        moves.setdefault(e.get(f"{W}name"), []).append(kind)
            for name, kinds in moves.items():
                self.assertEqual(sorted(kinds), ["moveFrom", "moveTo"], f"{doc.name}: {name}")
            forms = [name.get(f"{W}val") for root in roots for data in root.iter(f"{W}ffData")
                     for name in data.iter(f"{W}name")]
            self.assertEqual(len(forms), len(set(forms)), doc.name)

    def test_batch_9_documents_have_the_markup_their_labels_name(self):
        by_name = {doc.name: doc for doc in self.docs}
        a = "{http://schemas.openxmlformats.org/drawingml/2006/main}"
        wp = "{http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing}"
        wps = "{http://schemas.microsoft.com/office/word/2010/wordprocessingShape}"
        v = "{urn:schemas-microsoft-com:vml}"
        emu = 12700

        def load(name):
            doc = by_name[name]
            parts = dict(rules.parts(doc))
            return doc, parts, ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")

        def texts(element):
            return "".join(t.text or "" for t in element.iter(f"{W}t"))

        def labels(body):
            return [texts(p) for p in body.findall(f"{W}p") if texts(p).startswith("Case ")]

        def boxes(element):
            return list(element.iter(f"{wps}wsp"))

        def insets(shape):
            body_pr = shape.find(f"{wps}bodyPr")
            return tuple(body_pr.get(f"{side}Ins") for side in "ltrb")

        def size(element):
            ext = element.find(f".//{wp}extent")
            return int(ext.get("cx")) / emu, int(ext.get("cy")) / emu

        def box_paragraphs(shape):
            return shape.findall(f"{wps}txbx/{W}txbxContent/{W}p")

        def twips(p, path, name):
            found = p.find(f"{W}pPr/{path}")
            return None if found is None else found.get(f"{W}{name}")

        # E-textbox-insets: four inline boxes of 216 x 90 pt with the insets their labels
        # name, in EMUs.
        _, _, body = load("E-textbox-insets")
        shapes = boxes(body)
        self.assertEqual([insets(s) for s in shapes], [
            (None,) * 4, ("0",) * 4, (str(18 * emu),) * 4,
            (str(36 * emu), str(12 * emu), "0", "0")])
        self.assertEqual({size(d) for d in body.iter(f"{wp}inline")}, {(216, 90)})
        self.assertEqual(len(labels(body)), 4)
        for shape in shapes:
            self.assertEqual(shape.find(f"{wps}cNvSpPr").get("txBox"), "1")
            line = shape.find(f"{wps}spPr/{a}ln")
            self.assertEqual(line.get("w"), "6350")

        # E-textbox-anchor: top, centre and bottom, with one line and with six.
        _, _, body = load("E-textbox-anchor")
        shapes = boxes(body)
        self.assertEqual([s.find(f"{wps}bodyPr").get("anchor") for s in shapes],
                         ["t", "ctr", "b"] * 2)
        self.assertEqual([len(box_paragraphs(s)) for s in shapes], [1] * 3 + [6] * 3)
        # Six lines of Calibri 11 pt (13.43 pt each) are taller than the box.
        self.assertEqual({size(d) for d in body.iter(f"{wp}inline")}, {(216, 48)})
        self.assertGreater(6 * 13.43, size(body.find(f".//{wp}inline"))[1])
        for label, shape in zip(labels(body), shapes):
            self.assertIn(f"anchor {shape.find(f'{wps}bodyPr').get('anchor')} ", label)

        # E-textbox-spacing: 12 pt before the first paragraph and after the last only where
        # the label says so, at each anchor, insets of 0.
        _, _, body = load("E-textbox-spacing")
        shapes = boxes(body)
        self.assertEqual([s.find(f"{wps}bodyPr").get("anchor") for s in shapes],
                         ["t", "t", "b", "b", "ctr", "ctr", "ctr"])
        self.assertEqual({insets(s) for s in shapes}, {("0",) * 4})
        spaced = [(twips(box_paragraphs(s)[0], f"{W}spacing", "before"),
                   twips(box_paragraphs(s)[-1], f"{W}spacing", "after")) for s in shapes]
        # At the centre, space before alone and after alone: both kept and both dropped
        # would put the text of one box with both at the same place.
        self.assertEqual(spaced, [(None, None), ("240", None), (None, None), (None, "240"),
                                  (None, None), ("240", None), (None, "240")])

        # E-textbox-autofit: spAutoFit on the boxes written too short and too tall.
        _, _, body = load("E-textbox-autofit")
        shapes = boxes(body)
        fits = [s.find(f"{wps}bodyPr")[0].tag[len(a):] for s in shapes]
        self.assertEqual(fits, ["spAutoFit", "spAutoFit", "noAutofit", "spAutoFit"])
        self.assertEqual([size(d)[1] for d in body.iter(f"{wp}inline")], [20, 120, 20, 20])
        self.assertEqual([len(box_paragraphs(s)) for s in shapes], [4, 1, 4, 1])
        last = box_paragraphs(shapes[3])[0]
        self.assertEqual((twips(last, f"{W}spacing", "before"),
                          twips(last, f"{W}spacing", "after")), ("240", "240"))

        # E-textbox-fill-outline: three dark fills without an outline, outlines of 6 pt
        # without an alignment and aligned inside, and one of 0.5 pt; no w:color on the text.
        _, _, body = load("E-textbox-fill-outline")
        shapes = boxes(body)
        fills = [s.find(f"{wps}spPr/{a}solidFill/{a}srgbClr") for s in shapes]
        self.assertEqual([f.get("val") if f is not None else None for f in fills],
                         ["404040", "7F7F7F", "002060", None, None, None])
        lines = [s.find(f"{wps}spPr/{a}ln") for s in shapes]
        self.assertEqual([(ln.get("w"), ln.get("algn"), ln.find(f"{a}noFill") is not None)
                          for ln in lines],
                         [(None, None, True)] * 3 + [("76200", None, False),
                                                     ("76200", "in", False),
                                                     ("6350", None, False)])
        self.assertEqual(list(body.iter(f"{W}color")), [])

        # E-textbox-lists: one list (numId 1) in the body, in a box of the body and in a box
        # of the header.
        _, parts, body = load("E-textbox-lists")
        (box,) = boxes(body)
        in_box = [texts(p) for p in box_paragraphs(box)
                  if p.find(f"{W}pPr/{W}numPr/{W}numId").get(f"{W}val") == "1"]
        self.assertEqual(in_box, ["Box item, first", "Box item, second"])
        in_body = [texts(p) for p in body.findall(f"{W}p")
                   if p.find(f"{W}pPr/{W}numPr/{W}numId") is not None]
        self.assertEqual(len(in_body), 5)
        header = ElementTree.fromstring(parts["word/header1.xml"])
        (header_box,) = boxes(header)
        self.assertEqual(
            [p.find(f"{W}pPr/{W}numPr/{W}numId").get(f"{W}val")
             for p in box_paragraphs(header_box)], ["1"])
        numbering = ElementTree.fromstring(parts["word/numbering.xml"])
        self.assertEqual([n.get(f"{W}numId") for n in numbering.iter(f"{W}num")], ["1"])

        # E-textbox-footer-page: the footer's box holds a PAGE field; three pages.
        _, parts, body = load("E-textbox-footer-page")
        footer = ElementTree.fromstring(parts["word/footer1.xml"])
        (box,) = boxes(footer)
        self.assertEqual([f.get(f"{W}instr") for f in box.iter(f"{W}fldSimple")], [" PAGE "])
        breaks = [b for b in body.iter(f"{W}br") if b.get(f"{W}type") == "page"]
        self.assertEqual(len(breaks), 2)

        # E-textbox-inline-floating: two inline boxes in lines of text, and two floating ones
        # placed from the margin and from the page, not wrapped.
        _, _, body = load("E-textbox-inline-floating")
        self.assertEqual([size(d) for d in body.iter(f"{wp}inline")], [(72, 18), (72, 48)])
        anchors = list(body.iter(f"{wp}anchor"))
        placed = [(d.find(f"{wp}positionH").get("relativeFrom"),
                   int(d.find(f"{wp}positionH/{wp}posOffset").text) / emu,
                   d.find(f"{wp}positionV").get("relativeFrom"),
                   int(d.find(f"{wp}positionV/{wp}posOffset").text) / emu) for d in anchors]
        self.assertEqual(placed, [("margin", 144, "margin", 360), ("page", 300, "page", 600)])
        self.assertEqual([d.find(f"{wp}wrapNone") is not None for d in anchors], [True, True])
        inline_paragraphs = [p for p in body.findall(f"{W}p") if p.find(f".//{wp}inline")
                             is not None]
        for p in inline_paragraphs:
            self.assertTrue(texts(p).endswith(" after."), texts(p))

        # E-textbox-vml: four floating VML text boxes with the insets and anchors their
        # labels name, each in the paragraph of its anchor, and an inline one; every one
        # tall enough for its text inside the insets.
        _, _, body = load("E-textbox-vml")
        shapes = list(body.iter(f"{v}shape"))
        self.assertEqual([s.find(f"{v}textbox").get("inset") for s in shapes],
                         [None, "18pt,18pt,18pt,18pt", "0,0,0,0", "0,0,0,0", None])
        styles = [dict(item.split(":", 1) for item in s.get("style").split(";"))
                  for s in shapes]
        self.assertEqual([s.get("v-text-anchor") for s in styles],
                         [None, None, "middle", "bottom", None])
        self.assertEqual([s.get("position") for s in styles], ["absolute"] * 4 + [None])
        self.assertEqual({(s["width"], s["height"]) for s in styles}, {("216pt", "90pt")})
        for style in styles[:4]:
            self.assertEqual((style["margin-left"], style["margin-top"],
                              style["mso-position-horizontal-relative"]),
                             ("144pt", "12pt", "margin"))
        for n, shape in enumerate(shapes[:4], 1):
            holder = next(p for p in body.findall(f"{W}p") if shape in list(p.iter(f"{v}shape")))
            self.assertTrue(texts(holder).endswith(f"The anchor of case {n:02d}."), n)
        # Three lines of 13.43 pt fit inside the insets of 18 pt.
        self.assertLess(3 * 13.43 + 36, float(styles[1]["height"].removesuffix("pt")))

        # E-textbox-from-paragraph: one line in a box of 144 x 36 pt, top and bottom insets
        # of 0, anchored at the centre, top and bottom, placed at the top of a paragraph
        # with 4 pt before, and once with none.
        _, _, body = load("E-textbox-from-paragraph")
        anchors = list(body.iter(f"{wp}anchor"))
        self.assertEqual([boxes(d)[0].find(f"{wps}bodyPr").get("anchor") for d in anchors],
                         ["ctr", "t", "b", "ctr"])
        self.assertEqual({insets(boxes(d)[0]) for d in anchors},
                         {(str(emu * 72 // 10), "0", str(emu * 72 // 10), "0")})
        self.assertEqual({(d.find(f"{wp}positionV").get("relativeFrom"),
                           d.find(f"{wp}positionV/{wp}posOffset").text,
                           d.find(f"{wp}positionH").get("relativeFrom"),
                           d.find(f"{wp}positionH/{wp}posOffset").text) for d in anchors},
                         {("paragraph", "0", "margin", str(144 * emu))})
        self.assertEqual({size(d) for d in anchors}, {(144, 36)})
        self.assertEqual([len(box_paragraphs(boxes(d)[0])) for d in anchors], [1] * 4)
        children = body.findall(f"{W}p")
        holders = [p for p in children if p.find(f".//{wp}anchor") is not None]
        # The label and the empty paragraph stay on the page of the box (w:keepNext).
        for holder in holders:
            at = children.index(holder)
            for kept in children[at - 2:at]:
                self.assertIsNotNone(kept.find(f"{W}pPr/{W}keepNext"))
        self.assertEqual([twips(p, f"{W}spacing", "before") for p in holders],
                         ["80", "80", "80", "0"])
        # Before each, an empty paragraph without space after, so that the space before
        # takes effect: Word lays out the larger of the two.
        for holder in holders:
            before = children[children.index(holder) - 1]
            self.assertEqual((texts(before), twips(before, f"{W}spacing", "after")), ("", None))
            self.assertTrue(texts(children[children.index(holder) - 2]).startswith("Case "))

        # E-textbox-bar-tab: the same paragraph with a bar tab at 72 pt and 12 pt before
        # and after, in a box 288 pt wide and in the body, 288 pt wide too.
        _, _, body = load("E-textbox-bar-tab")
        (box,) = boxes(body)
        (inside,) = box_paragraphs(box)
        outside = [p for p in body.findall(f"{W}p") if p.find(f"{W}pPr/{W}tabs") is not None]
        self.assertEqual(len(outside), 1)
        for p in (inside, outside[0]):
            stop = p.find(f"{W}pPr/{W}tabs/{W}tab")
            self.assertEqual((stop.get(f"{W}val"), stop.get(f"{W}pos")), ("bar", "1440"))
            self.assertEqual((twips(p, f"{W}spacing", "before"),
                              twips(p, f"{W}spacing", "after")), ("240", "240"))
        self.assertEqual(texts(inside), texts(outside[0]))
        self.assertEqual(rules.TEXT_W - int(twips(outside[0], f"{W}ind", "right")), 288 * 20)
        self.assertEqual(size(body.find(f".//{wp}inline"))[0], 288)

        # E-shape-colors: shapes without text; RGB and accent1 the same colour, shaded and
        # tinted 50 %, a fill from the theme through wps:style, and outlines of 6 pt.
        doc, parts, body = load("E-shape-colors")
        theme = ElementTree.fromstring(parts["word/theme/theme1.xml"])
        self.assertEqual(theme.find(f".//{a}accent1/{a}srgbClr").get("val"), "4F81BD")
        self.assertEqual(theme.find(f".//{a}accent2/{a}srgbClr").get("val"), "CC6633")
        first_fill = theme.find(f".//{a}fillStyleLst")[0]
        self.assertEqual(first_fill.find(f"{a}schemeClr").get("val"), "phClr")
        shapes = boxes(body)
        self.assertEqual([s.find(f"{wps}txbx") for s in shapes], [None] * 9)

        def colour(element):
            if element is None:
                return None
            (color,) = list(element)
            transforms = tuple((t.tag[len(a):], t.get("val")) for t in color)
            return color.tag[len(a):], color.get("val"), transforms

        shade, tint = (("shade", "50000"),), (("tint", "50000"),)
        rgb, accent = ("srgbClr", "4F81BD"), ("schemeClr", "accent1")
        self.assertEqual([colour(s.find(f"{wps}spPr/{a}solidFill")) for s in shapes], [
            (*rgb, ()), (*rgb, shade), (*accent, ()), (*accent, shade), (*rgb, tint),
            (*accent, tint), None, None, None])
        self.assertEqual(colour(shapes[6].find(f"{wps}style/{a}fillRef")), (*accent, shade))
        self.assertEqual(shapes[6].find(f"{wps}style/{a}fillRef").get("idx"), "1")
        self.assertIsNone(shapes[6].find(f"{wps}spPr/{a}noFill"))
        self.assertEqual([colour(s.find(f"{wps}spPr/{a}ln/{a}solidFill")) for s in shapes[7:]],
                         [(*rgb, shade), (*accent, shade)])
        self.assertEqual([s.find(f"{wps}spPr/{a}ln").get("w") for s in shapes[7:]],
                         ["76200"] * 2)
        for s in shapes[:7]:
            self.assertIsNotNone(s.find(f"{wps}spPr/{a}ln/{a}noFill"))
        self.assertEqual({size(d) for d in body.iter(f"{wp}inline")}, {(100, 40)})

        # E-paragraph-units-sizes: the sizes of the style, mark and text that each label
        # names, beside indents and spacing in units and in points.
        doc, parts, body = load("E-paragraph-units-sizes")
        styles = ElementTree.fromstring(parts["word/styles.xml"])
        (size14,) = [s for s in styles.iter(f"{W}style") if s.get(f"{W}styleId") == "Size14"]
        self.assertEqual(size14.find(f"{W}rPr/{W}sz").get(f"{W}val"), "28")
        cases = [p for p in body.findall(f"{W}p") if texts(p).startswith("Case ")]

        def half(element):
            return None if element is None else int(element.get(f"{W}val")) / 2

        written = [(twips(p, f"{W}pStyle", "val"), half(p.find(f"{W}pPr/{W}rPr/{W}sz")),
                    {half(r.find(f"{W}rPr/{W}sz")) for r in p.findall(f"{W}r")}) for p in cases]
        self.assertEqual(written, [("Size14", 11, {10}), (None, 14, {10}), (None, 10, {14}),
                                   ("Size14", None, {None}), ("Size14", 11, {10}),
                                   (None, 14, {10})])
        for p in cases[:4]:
            self.assertEqual({k: twips(p, f"{W}ind", k) for k in
                              ("left", "leftChars", "hanging", "hangingChars")},
                             {"left": "1440", "leftChars": "300", "hanging": "360",
                              "hangingChars": "100"})
        for p in cases[4:]:
            self.assertEqual((twips(p, f"{W}spacing", "before"),
                              twips(p, f"{W}spacing", "beforeLines")), ("120", "100"))

        # E-paragraph-units-line-grid and -char-grid: the grid each names, in the section.
        for name, grid in (("E-paragraph-units-line-grid", {"type": "lines",
                                                            "linePitch": "480"}),
                           ("E-paragraph-units-char-grid", {"type": "linesAndChars",
                                                            "linePitch": "360",
                                                            "charSpace": "40960"})):
            _, _, body = load(name)
            (doc_grid,) = body.iter(f"{W}docGrid")
            self.assertEqual({k[len(W):]: val for k, val in doc_grid.attrib.items()}, grid)
        _, _, body = load("E-paragraph-units-line-grid")
        cases = [p for p in body.findall(f"{W}p") if texts(p).startswith("Case ")]
        self.assertEqual([(twips(p, f"{W}spacing", "before"),
                           twips(p, f"{W}spacing", "beforeLines"),
                           twips(p, f"{W}spacing", "after"),
                           twips(p, f"{W}spacing", "afterLines")) for p in cases],
                         [("120", "100", None, None), ("120", "50", None, None),
                          ("120", None, None, None), (None, None, "120", "100")])
        _, _, body = load("E-paragraph-units-char-grid")
        cases = [p for p in body.findall(f"{W}p") if texts(p).startswith("Case ")]
        self.assertEqual([{k: twips(p, f"{W}ind", k) for k in
                           ("left", "leftChars", "hanging", "hangingChars")} for p in cases],
                         [{"left": "1440", "leftChars": "200", "hanging": None,
                           "hangingChars": None},
                          {"left": "1440", "leftChars": None, "hanging": "360",
                           "hangingChars": "100"},
                          {"left": "1440", "leftChars": None, "hanging": None,
                           "hangingChars": None}])

        # E-hidden-pictures: the pictures of cases 2 to 6 are in hidden runs, that of case 1
        # is not, and the paragraph of case 6 is hidden with its mark.
        _, parts, body = load("E-hidden-pictures")
        picture_runs = [r for r in body.iter(f"{W}r")
                        if r.find(f"{W}drawing") is not None or r.find(f"{W}pict") is not None]
        self.assertEqual([r.find(f"{W}rPr/{W}vanish") is not None for r in picture_runs],
                         [False, True, True, True, True, True])
        def floats(r):
            shape = r.find(f".//{v}shape")
            return (r.find(f".//{wp}anchor") is not None
                    or shape is not None and "position:absolute" in shape.get("style"))

        self.assertEqual([floats(r) for r in picture_runs],
                         [False, False, False, True, True, False])
        hidden_marks = [p for p in body.findall(f"{W}p")
                        if p.find(f"{W}pPr/{W}rPr/{W}vanish") is not None]
        self.assertEqual(len(hidden_marks), 1)
        self.assertIn(picture_runs[5], list(hidden_marks[0]))

        # E-table-row-exact-cut: row 2 of exact height, a cell with margins, a paragraph
        # indented past the cell, and a nested table with a picture taller than the row.
        _, _, body = load("E-table-row-exact-cut")
        tables = body.findall(f"{W}tbl")
        self.assertEqual(len(tables), 3)
        rows = [t.findall(f"{W}tr")[1] for t in tables]
        self.assertEqual([(r.find(f"{W}trPr/{W}trHeight").get(f"{W}val"),
                           r.find(f"{W}trPr/{W}trHeight").get(f"{W}hRule")) for r in rows],
                         [("800", "exact"), ("600", "exact"), ("800", "exact")])
        first = [r.find(f"{W}tc") for r in rows]
        margins = first[0].find(f"{W}tcPr/{W}tcMar")
        self.assertEqual([(m.tag[len(W):], m.get(f"{W}w")) for m in margins],
                         [("top", "200"), ("bottom", "200")])
        ind = first[1].find(f"{W}p/{W}pPr/{W}ind")
        self.assertEqual((ind.get(f"{W}left"), ind.get(f"{W}right")), ("-400", "-400"))
        (inner,) = first[2].findall(f"{W}tbl")
        (picture,) = inner.iter(f"{v}shape")
        self.assertIn("height:90pt", picture.get("style"))
        height = int(rows[2].find(f"{W}trPr/{W}trHeight").get(f"{W}val")) / 20
        self.assertGreater(90, height)

        # The row-break documents: each case on pages of its own, its label, 40 filler lines
        # of exact 12 pt and a table whose first row crosses into the next page.
        def crossing_cases(name):
            _, parts, body = load(name)
            children = list(body)
            tables = body.findall(f"{W}tbl")
            for case, table in enumerate(tables, 1):
                at = children.index(table)
                label, filler = children[at - 41], children[at - 40:at]
                self.assertTrue(texts(label).startswith(f"Case {case:02d}: "), name)
                self.assertEqual(label.find(f"{W}pPr/{W}pageBreakBefore") is not None,
                                 case > 1, name)
                self.assertEqual([texts(p) for p in filler],
                                 [f"Filler {k:02d}" for k in range(1, 41)], name)
                row = [tr for tr in table.findall(f"{W}tr")
                       if tr.find(f"{W}trPr/{W}tblHeader") is None][0]
                tallest = max(len(tc.findall(f"{W}p")) for tc in row.findall(f"{W}tc"))
                # 58 lines of 12 pt fill a page: the row starts on it and crosses it.
                self.assertLess(1 + 40 + 1, rules.LINES_PER_PAGE, name)
                self.assertGreater(1 + 40 + tallest, rules.LINES_PER_PAGE, name)
                for p in table.iter(f"{W}p"):
                    rule = p.find(f"{W}pPr/{W}spacing")
                    self.assertEqual((rule.get(f"{W}line"), rule.get(f"{W}lineRule")),
                                     ("240", "exact"), name)
            return parts, body, tables

        def starts(table):
            """The first paragraph of the crossing row's second cell and of the next row."""
            rows = table.findall(f"{W}tr")
            return rows[0].findall(f"{W}tc")[1].find(f"{W}p"), rows[1].find(f"{W}tc/{W}p")

        _, body, tables = crossing_cases("E-row-break-fields")
        self.assertEqual(len(tables), 4)
        for table, kind in zip(tables, ("IF", "FORMCHECKBOX", "FORMDROPDOWN", "FORMTEXT")):
            for p in starts(table):
                (first_run, second) = list(p.findall(f"{W}r"))[:2]
                self.assertEqual(first_run.find(f"{W}fldChar").get(f"{W}fldCharType"), "begin")
                self.assertEqual(second.find(f"{W}instrText").text.split()[0], kind)
                begins = [c for c in p.iter(f"{W}fldChar")
                          if c.get(f"{W}fldCharType") == "begin"]
                self.assertEqual(len(begins), 2 if kind == "IF" else 1)
                data = begins[0].find(f"{W}ffData")
                self.assertEqual(data is None, kind == "IF")
        _, body, tables = crossing_cases("E-row-break-hidden")
        self.assertEqual(len(tables), 3)
        hidden = [starts(t) for t in tables]
        for p in hidden[0]:
            self.assertEqual(p.find(f"{W}r/{W}rPr/{W}rStyle").get(f"{W}val"), "HiddenChar")
        for p in hidden[1]:
            self.assertEqual(p.find(f"{W}pPr/{W}pStyle").get(f"{W}val"), "HiddenPara")
        for p in hidden[2]:
            self.assertIsNotNone(p.find(f"{W}r/{W}rPr/{W}vanish"))
        _, body, tables = crossing_cases("E-row-break-revisions")
        self.assertEqual(len(tables), 3)
        for table, kind in zip(tables, ("del", "moveFromRangeStart", "ins")):
            for p in starts(table):
                content = [e for e in p if e.tag != f"{W}pPr"]
                self.assertEqual(content[0].tag, f"{W}{kind}")
        crossing = [p for p in body.findall(f"{W}p") if "Paragraph line 01" in texts(p)]
        (crossing,) = crossing
        order = [e.tag[len(W):] for e in crossing if e.tag != f"{W}pPr"]
        self.assertEqual(order[order.index("del"):order.index("del") + 4],
                         ["del", "moveFromRangeStart", "moveFrom", "moveFromRangeEnd"])
        self.assertEqual(texts(crossing).count("Paragraph line"), 30)
        authors = {e.get(f"{W}author") for e in body.iter() if e.get(f"{W}author")}
        self.assertEqual(authors, {"Officewright"})

        parts, body, tables = crossing_cases("E-row-break-numbers")
        self.assertEqual(len(tables), 3)
        settings = parts["word/settings.xml"].decode()
        self.assertLess(settings.index("<w:defaultTabStop"),
                        settings.index("<w:evenAndOddHeaders/>"))
        self.assertLess(settings.index("<w:evenAndOddHeaders/>"),
                        settings.index("<w:characterSpacingControl"))
        refs = {r.get(f"{W}type"): r for r in body.iter(f"{W}headerReference")}
        self.assertEqual(sorted(refs), ["default", "even"])
        even = ElementTree.fromstring(parts["word/header2.xml"])
        self.assertEqual(texts(even), "1")
        self.assertEqual([f.get(f"{W}instr") for f in even.iter(f"{W}fldSimple")], [" PAGE "])
        rows = [[tr for tr in t.findall(f"{W}tr") if tr.find(f"{W}trPr/{W}tblHeader") is None]
                for t in tables]
        self.assertEqual([texts(r[1].find(f"{W}tc")) for r in rows], ["2", "4", "6"])
        header_rows = [tr for tr in tables[1].findall(f"{W}tr")
                       if tr.find(f"{W}trPr/{W}tblHeader") is not None]
        self.assertEqual([[texts(tc) for tc in tr.findall(f"{W}tc")] for tr in header_rows],
                         [["#", "Item", "Note"]])
        self.assertEqual([len(tc.findall(f"{W}p")) for tc in rows[2][0].findall(f"{W}tc")],
                         [20, 1, 20])
        for name in ("E-row-break-fields", "E-row-break-hidden", "E-row-break-revisions"):
            self.assertNotIn("<w:evenAndOddHeaders/>",
                             dict(rules.parts(by_name[name]))["word/settings.xml"].decode())

    def test_emus_are_whole(self):
        self.assertEqual(rules.emu(0.5), 6350)
        with self.assertRaisesRegex(ValueError, "not a whole number of EMUs"):
            rules.emu(0.00001)

    def test_dmap_font_cases_have_the_markup_their_labels_name(self):
        doc = next(doc for doc in self.docs if doc.name == "E-dmap-font")
        parts = dict(rules.parts(doc))
        body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")

        def texts(element):
            return "".join(t.text or "" for t in element.iter(f"{W}t"))

        # Each label, then the paragraph of its sample: one run in one font in all four slots.
        paragraphs = body.findall(f"{W}p")
        cases = [(texts(p), paragraphs[k + 1]) for k, p in enumerate(paragraphs)
                 if texts(p).startswith("Case ")]
        self.assertEqual(len(cases), 3)
        # What a label says of its sample's characters, and the text that is.
        characters = {"A, B, U+E000 and Z": "AB\ue000Z", "the same characters": "AB\ue000Z",
                      "A and B alone": "AB"}
        written = []
        for label, sample in cases:
            (r,) = sample.findall(f"{W}r")
            fonts = r.find(f"{W}rPr/{W}rFonts")
            (font,) = {fonts.get(f"{W}{slot}") for slot in ("ascii", "hAnsi", "eastAsia", "cs")}
            size = int(r.find(f"{W}rPr/{W}sz").get(f"{W}val")) / 2
            self.assertIn(f" in {font}, ", label)
            self.assertIn(f", {size:g} pt", label)
            if font == rules.DMAP_FONT:
                self.assertIn("in all four slots", label)
            (claim,) = [k for k in characters if label.split(": ", 1)[1].startswith(k)]
            self.assertEqual(texts(r), characters[claim], label)
            written.append((font, size, texts(r)))
        self.assertEqual(written, [("Officewright Test DMAP", 24, "AB\ue000Z"),
                                   ("Noto Sans", 24, "AB\ue000Z"),
                                   ("Officewright Test DMAP", 24, "AB")])
        # The font table names both fonts, the DMAP font without a character set of its own.
        table = ElementTree.fromstring(parts["word/fontTable.xml"])
        entries = {f.get(f"{W}name"): f for f in table.iter(f"{W}font")}
        self.assertIn("Noto Sans", entries)
        self.assertEqual(list(entries["Officewright Test DMAP"]), [])

    def test_note_parts_are_typed_and_every_reference_resolves(self):
        # ECMA-376 §17.11: a document with notes has a footnotes and an endnotes part, each
        # with its separator and continuation separator, which the document-wide properties
        # of the settings name; Word matches each reference to its note by id (MS-OI29500
        # 2.1.304 a), and the notes start with their mark in the reference style.
        relationship = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/"
        content = "application/vnd.openxmlformats-officedocument.wordprocessingml."
        documents_with_notes = 0
        for doc in self.docs:
            listed = rules.parts(doc)
            parts = dict(listed)
            settings = ElementTree.fromstring(parts["word/settings.xml"])
            if not (doc.footnotes or doc.endnotes):
                self.assertEqual(note_parts(listed), [], doc.name)
                self.assertIsNone(settings.find(f"{W}footnotePr"), doc.name)
                self.assertIsNone(settings.find(f"{W}endnotePr"), doc.name)
                continue
            documents_with_notes += 1
            self.assertEqual(note_parts(listed), ["word/footnotes.xml", "word/endnotes.xml"])
            types = ElementTree.fromstring(parts["[Content_Types].xml"])
            overrides = {e.get("PartName"): e.get("ContentType")
                         for e in types.iter(f"{CT}Override")}
            rels = {r.get("Target"): r.get("Type") for r in
                    ElementTree.fromstring(parts["word/_rels/document.xml.rels"])
                    .iter(f"{PR}Relationship")}
            body = ElementTree.fromstring(parts["word/document.xml"])
            # The stories that hold references: the body, the headers and the footers.
            stories = [body] + [ElementTree.fromstring(data) for name, data in listed
                                if name.startswith(("word/header", "word/footer"))]
            # The custom marks of the references (w:customMarkFollows): the text of the run
            # after the reference's run.
            custom = {}
            for p in (p for story in stories for p in story.iter(f"{W}p")):
                runs = p.findall(f"{W}r")
                for k, r in enumerate(runs):
                    for ref in r:
                        if ref.get(f"{W}customMarkFollows") == "1":
                            custom[(ref.tag, ref.get(f"{W}id"))] = runs[k + 1].find(f"{W}t").text
            for kind, notes in (("footnote", doc.footnotes), ("endnote", doc.endnotes)):
                name = f"word/{kind}s.xml"
                self.assertEqual(overrides[f"/{name}"], f"{content}{kind}s+xml", doc.name)
                self.assertEqual(rels[f"{kind}s.xml"], f"{relationship}{kind}s", doc.name)
                root = ElementTree.fromstring(parts[name])
                self.assertEqual(root.tag, f"{W}{kind}s", doc.name)
                entries = root.findall(f"{W}{kind}")
                by_id = {e.get(f"{W}id"): e for e in entries}
                self.assertEqual(len(by_id), len(entries), f"{doc.name}: ids repeat")
                special = {i: e.get(f"{W}type") for i, e in by_id.items() if e.get(f"{W}type")}
                expected = {"-1": "separator", "0": "continuationSeparator"}
                if kind == "footnote" and doc.continuation_notice:
                    expected[str(len(notes) + 1)] = "continuationNotice"
                self.assertEqual(special, expected, doc.name)
                for i, mark in (("-1", "separator"), ("0", "continuationSeparator")):
                    self.assertIsNotNone(by_id[i].find(f"{W}p/{W}r/{W}{mark}"), doc.name)
                # The settings name the special notes, and only them.
                named = [e.get(f"{W}id") for e in settings.find(f"{W}{kind}Pr")
                         if e.tag == f"{W}{kind}"]
                self.assertEqual(named, sorted(special, key=int), doc.name)
                # Each note is referenced once, from the body, a header or a footer, by a run
                # in the reference style.
                normal = [i for i in by_id if i not in special]
                self.assertEqual(normal, [str(n) for n in range(1, len(notes) + 1)], doc.name)
                references = [r.get(f"{W}id") for story in stories
                              for r in story.iter(f"{W}{kind}Reference")]
                self.assertEqual(sorted(references, key=int), normal, doc.name)
                style = f"{kind.capitalize()}Reference"
                for story in stories:
                    for r in story.iter(f"{W}r"):
                        if r.find(f"{W}{kind}Reference") is not None:
                            self.assertEqual(r.find(f"{W}rPr/{W}rStyle").get(f"{W}val"), style)
                    self.assertIsNone(story.find(f".//{W}{kind}Ref"), doc.name)
                for i in normal:
                    paragraphs = by_id[i].findall(f"{W}p")
                    for p in paragraphs:
                        self.assertEqual(p.find(f"{W}pPr/{W}pStyle").get(f"{W}val"),
                                         f"{kind.capitalize()}Text", doc.name)
                    first = paragraphs[0].find(f"{W}r")
                    self.assertEqual(first.find(f"{W}rPr/{W}rStyle").get(f"{W}val"), style)
                    mark = custom.get((f"{W}{kind}Reference", i))
                    if mark is None:
                        self.assertIsNotNone(first.find(f"{W}{kind}Ref"), f"{doc.name}: {i}")
                    else:
                        # A note with a custom mark shows that mark, not its number.
                        self.assertIsNone(by_id[i].find(f".//{W}{kind}Ref"), doc.name)
                        self.assertEqual(first.find(f"{W}t").text, mark, doc.name)
                    # One space follows the mark; no later line or paragraph of the note starts
                    # with one, since Word draws a space at the start of a line.
                    words = [t.text for t in by_id[i].iter(f"{W}t")][1 if mark else 0:]
                    self.assertRegex(words[0], r"^ \S", f"{doc.name}: {i}")
                    for word in words[1:]:
                        self.assertNotRegex(word, r"^\s", f"{doc.name}: {i}")
        self.assertEqual(documents_with_notes, 17)

    def test_settings_follow_the_schema_order(self):
        # CT_Settings 17.15.1.78 is a sequence (here the elements the script writes), and so
        # are the document-wide footnote and endnote properties (17.11.12 and 17.11.4): first
        # the properties a section can give its notes too, then the special notes.
        order = ["zoom", "defaultTabStop", "evenAndOddHeaders", "characterSpacingControl",
                 "footnotePr", "endnotePr", "compat", "themeFontLang", "clrSchemeMapping",
                 "decimalSymbol", "listSeparator"]
        for doc in self.docs:
            settings = part(rules.parts(doc), "word/settings.xml")
            tags = [child.tag.replace(W, "") for child in settings]
            for tag in tags:
                self.assertIn(tag, order, doc.name)
            self.assertEqual([order.index(t) for t in tags],
                             sorted(order.index(t) for t in tags), f"{doc.name}: {tags}")
            self.assertEqual(len(tags), len(set(tags)), doc.name)
            for kind in ("footnote", "endnote"):
                props = settings.find(f"{W}{kind}Pr")
                if props is None:
                    continue
                note_order = ["pos", "numFmt", "numStart", "numRestart", kind]
                tags = [child.tag.replace(W, "") for child in props]
                self.assertEqual([note_order.index(t) for t in tags],
                                 sorted(note_order.index(t) for t in tags), f"{doc.name}: {tags}")
                # Three special notes at most: separator, continuation separator and notice.
                self.assertLessEqual(tags.count(kind), 3, doc.name)

    def test_refuses_note_properties_without_notes(self):
        doc = next(doc for doc in self.docs if doc.name == "E-line-exact")
        for field, value in (("footnote_props", rules.note_props(pos="beneathText")),
                             ("endnote_props", rules.note_props(pos="sectEnd")),
                             ("continuation_notice", rules.para(rules.run("notice")))):
            mistyped = dataclasses.replace(doc, **{field: value})
            with self.assertRaisesRegex(ValueError, "note properties without notes"):
                rules.settings_xml(mistyped)

    def test_note_documents_have_the_markup_their_labels_name(self):
        by_name = {doc.name: doc for doc in self.docs}
        exact_12 = "240"

        def load(name):
            doc = by_name[name]
            parts = dict(rules.parts(doc))
            body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")
            settings = ElementTree.fromstring(parts["word/settings.xml"])
            notes = {kind: {e.get(f"{W}id"): e for e in
                            ElementTree.fromstring(parts[f"word/{kind}s.xml"]).findall(f"{W}{kind}")}
                     for kind in ("footnote", "endnote")}
            return body, settings, notes

        def texts(element):
            return "".join(t.text or "" for t in element.iter(f"{W}t"))

        def refs(element, kind="footnote"):
            return [r.get(f"{W}id") for r in element.iter(f"{W}{kind}Reference")]

        def note_lines(note):
            """The lines of a note of one paragraph: its line breaks and one."""
            (p,) = note.findall(f"{W}p")
            return len(p.findall(f".//{W}br")) + 1

        def props(element, kind="footnote"):
            """The note properties of a w:sectPr or the settings, as {name: value}."""
            found = element.find(f"{W}{kind}Pr")
            if found is None:
                return {}
            return {c.tag.replace(W, ""): c.get(f"{W}val") for c in found if c.tag != f"{W}{kind}"}

        def sections(body):
            """Each section's paragraphs and w:sectPr."""
            result, current = [], []
            for p in body.findall(f"{W}p"):
                current.append(p)
                sect = p.find(f"{W}pPr/{W}sectPr")
                if sect is not None:
                    result.append((current, sect))
                    current = []
            result.append((current, body.find(f"{W}sectPr")))
            return result

        def pages(paragraphs):
            """The paragraphs of each page, as w:pageBreakBefore starts them."""
            result = []
            for p in paragraphs:
                if not result or p.find(f"{W}pPr/{W}pageBreakBefore") is not None:
                    result.append([])
                result[-1].append(p)
            return result

        def line(p):
            spacing = p.find(f"{W}pPr/{W}spacing")
            return spacing.get(f"{W}lineRule"), int(spacing.get(f"{W}line"))

        def no_space(element, name):
            """The w:spacing of a paragraph, a style or w:pPrDefault adds no space before or
            after."""
            spacing = element.find(f"{W}pPr/{W}spacing")
            for attribute in ("before", "after", "beforeLines", "afterLines"):
                self.assertIn(spacing.get(f"{W}{attribute}"), (None, "0"), f"{name}: {attribute}")
            for attribute in ("beforeAutospacing", "afterAutospacing"):
                self.assertIn(spacing.get(f"{W}{attribute}"), (None, "0", "false", "off"), name)

        def note_heights(name):
            """The heights of the separator's line and of a line of the footnote text, from
            the document: the separator's paragraph and the FootnoteText style are single
            lines without space before or after, through Normal, which adds nothing to the
            defaults; the separator in the default size, the note in the style's size."""
            parts = dict(rules.parts(by_name[name]))
            styles = ElementTree.fromstring(parts["word/styles.xml"])
            single = ("auto", 240)
            by_id = {st.get(f"{W}styleId"): st for st in styles.iter(f"{W}style")}
            default = styles.find(f"{W}docDefaults/{W}rPrDefault/{W}rPr/{W}sz")
            self.assertIsNone(by_id["Normal"].find(f"{W}rPr"), name)
            self.assertIsNone(by_id["Normal"].find(f"{W}pPr"), name)
            defaults = styles.find(f"{W}docDefaults/{W}pPrDefault")
            self.assertEqual(line(defaults), single, name)
            no_space(defaults, name)
            text_style = by_id["FootnoteText"]
            self.assertEqual(text_style.find(f"{W}basedOn").get(f"{W}val"), "Normal")
            self.assertEqual(line(text_style), single, name)
            no_space(text_style, name)
            separator = ElementTree.fromstring(parts["word/footnotes.xml"]).find(
                f"{W}footnote[@{W}type='separator']/{W}p")
            # The separator's paragraph is in Normal: no style of its own, no run properties.
            self.assertIsNone(separator.find(f"{W}pPr/{W}pStyle"), name)
            self.assertEqual(line(separator), single, name)
            no_space(separator, name)
            self.assertIsNone(separator.find(f"{W}r/{W}rPr"), name)
            # Word's built-in note text is 10 pt (note_styles()), the rule documents' text 11 pt.
            size = int(text_style.find(f"{W}rPr/{W}sz").get(f"{W}val")) / 2
            self.assertEqual((int(default.get(f"{W}val")) / 2, size), (11, 10), name)
            return (carlito_line_height(int(default.get(f"{W}val")) / 2),
                    carlito_line_height(size))

        # E-footnote-separator: a note from line 2 of three, then one from Line 10 of a page of
        # 70 lines of 12 pt.
        body, settings, notes = load("E-footnote-separator")
        first, second = pages(body.findall(f"{W}p"))
        self.assertEqual([texts(p) for p in first[1:]], [
            "Body line 2 holds the reference.", "Body line 3, the last line of the text."])
        self.assertEqual([refs(p) for p in first], [[], ["1"], []])
        self.assertTrue(texts(second[0]).startswith("Case 02: "))
        lines_ = second[1:71]
        self.assertEqual([texts(p) for p in lines_],
                         [f"Line {k:02d}" + (" holds a reference" if k == 10 else "")
                          for k in range(1, 71)])
        self.assertEqual({line(p) for p in lines_}, {("exact", 240)})
        self.assertEqual([k for k, p in enumerate(lines_, 1) if refs(p)], [10])
        self.assertIn("Line 10", texts(second[0]))
        self.assertGreater(len(lines_), rules.LINES_PER_PAGE)
        self.assertEqual([note_lines(notes["footnote"][i]) for i in ("1", "2")], [1, 1])

        # E-footnote-two-notes: the references of each case, the three lines of note 1, the
        # spacing of notes 5 and 6, and the two paragraphs of note 7.
        body, settings, notes = load("E-footnote-two-notes")
        cases = pages(body.findall(f"{W}p"))
        self.assertEqual([[refs(p) for p in page[1:4]] for page in cases], [
            [["1"], [], ["2"]], [["3", "4"], [], []], [["5"], ["6"], []], [["7"], [], []]])
        self.assertIn("lines 1 and 3; the first note has three lines", texts(cases[0][0]))
        self.assertEqual(note_lines(notes["footnote"]["1"]), 3)
        self.assertIn("the same line", texts(cases[1][0]))
        self.assertIn("6 pt before and after", texts(cases[2][0]))
        for i in ("5", "6"):
            spacing = notes["footnote"][i].find(f"{W}p/{W}pPr/{W}spacing")
            self.assertEqual((spacing.get(f"{W}before"), spacing.get(f"{W}after")), ("120", "120"))
        self.assertIsNone(notes["footnote"]["4"].find(f"{W}p/{W}pPr/{W}spacing"))
        self.assertIn("two paragraphs", texts(cases[3][0]))
        self.assertEqual(len(notes["footnote"]["7"].findall(f"{W}p")), 2)

        # E-footnote-no-room: label, spacer, reference line and a line of 48 pt fill the body
        # but for the room the label names. Below the separator, that room holds as many lines
        # of the note as the cases say: a part of each note but the one-line note, which fits
        # whole, and nothing at 10 pt. Under widow control (on by default, ECMA-376 §17.3.1.44)
        # a three-line note cannot be cut, a four-line one can be cut after two lines.
        body, settings, notes = load("E-footnote-no-room")
        separator, note_line = note_heights("E-footnote-no-room")
        # Widow control is on for a note that does not set it: the defaults turn it on (no
        # w:val, or a true one), and neither Normal nor FootnoteText sets it.
        styles = ElementTree.fromstring(dict(rules.parts(by_name["E-footnote-no-room"]))[
            "word/styles.xml"])
        control = styles.find(f"{W}docDefaults/{W}pPrDefault/{W}pPr/{W}widowControl")
        self.assertIsNotNone(control)
        self.assertIn(control.get(f"{W}val"), (None, "1", "true", "on"))
        for st in styles.iter(f"{W}style"):
            if st.get(f"{W}styleId") in ("Normal", "FootnoteText"):
                self.assertIsNone(st.find(f"{W}pPr/{W}widowControl"), st.get(f"{W}styleId"))
        cases = pages(body.findall(f"{W}p"))
        self.assertEqual(len(cases), len(rules.NO_ROOM_CASES))
        found = []
        for n, case in enumerate(cases, 1):
            label, room, reference, after = case[:4]
            count, widow, left = re.match(
                r"Case \d+: a (\d)-line note, widow control (on|off), (\d+) pt left",
                texts(label)).groups()
            count, left = int(count), int(left)
            self.assertEqual([line(p) for p in (label, reference)], [("exact", 240)] * 2)
            self.assertEqual(texts(room), "")
            self.assertEqual(line(room)[0], "exact")
            self.assertEqual(240 + line(room)[1] + 240 + left * 20, rules.PAGE_H - 2 * rules.MARGIN)
            self.assertEqual(refs(reference), [str(n)])
            self.assertEqual(line(after), ("exact", 960))
            self.assertGreater(48, left)
            footnote = notes["footnote"][str(n)]
            self.assertEqual(note_lines(footnote), count)
            control = footnote.find(f"{W}p/{W}pPr/{W}widowControl")
            if widow == "on":
                self.assertIsNone(control, texts(label))
            else:
                self.assertEqual(control.get(f"{W}val"), "0", texts(label))
            # The lines of the note that fit below the separator.
            fitting = max(0, int((left - separator) // note_line))
            found.append((count, widow, min(fitting, count)))
        self.assertEqual(found, [(3, "off", 1), (3, "on", 1), (4, "on", 2), (3, "off", 0),
                                 (1, "on", 1)])

        # E-footnote-long and -notice: a note of 40 lines from Line 30 of 60, which cannot fit
        # below its reference, though the separator and a part of it can; the continuation
        # notice in the second only.
        for name, notice in (("E-footnote-long", False), ("E-footnote-long-notice", True)):
            body, settings, notes = load(name)
            separator, note_line = note_heights(name)
            paragraphs = body.findall(f"{W}p")
            self.assertEqual([texts(p)[:7] for p in paragraphs[:60]],
                             [f"Line {k:02d}" for k in range(1, 61)])
            self.assertEqual({line(p) for p in paragraphs}, {("exact", 240)})
            (at,) = [k for k, p in enumerate(paragraphs, 1) if refs(p)]
            room = rules.BODY_HEIGHT - 12 * at
            count = note_lines(notes["footnote"]["1"])
            self.assertGreater(separator + count * note_line, room, name)
            self.assertLessEqual(separator + note_line, room, name)
            self.assertIn(f"a {count}-line footnote from Line {at}", texts(paragraphs[0]))
            self.assertEqual("continuation notice" in texts(paragraphs[0]), notice)
            self.assertEqual("2" in notes["footnote"], notice, name)

        # E-footnote-beneath: each section's placement is the one its label names, and the
        # settings give none.
        body, settings, notes = load("E-footnote-beneath")
        self.assertEqual(props(settings), {})
        placed = []
        for paragraphs, sect in sections(body):
            label = texts(paragraphs[0])
            given = re.search(r"w:pos (\w+) in the section", label)
            self.assertEqual(props(sect), {"pos": given.group(1)} if given else {}, label)
            if not given:
                self.assertIn("no w:footnotePr", label)
                self.assertIsNone(sect.find(f"{W}footnotePr"))
            self.assertEqual(len(refs(body)), 4)
            self.assertEqual(sum(len(refs(p)) for p in paragraphs), 1, label)
            placed.append(props(sect).get("pos"))
        self.assertEqual(placed, ["beneathText", "sectEnd", "pageBottom", None])

        # E-footnote-beneath-settings: beneath the text in the settings, nothing in the section.
        body, settings, notes = load("E-footnote-beneath-settings")
        self.assertEqual(props(settings), {"pos": "beneathText"})
        ((paragraphs, sect),) = sections(body)
        self.assertIsNone(sect.find(f"{W}footnotePr"))
        self.assertIn("w:pos beneathText in the settings only", texts(paragraphs[0]))
        self.assertEqual(refs(body), ["1"])

        # E-footnote-numbering: each section's numbering as its label names it, the references
        # on each of its pages, and the custom mark between two numbered notes.
        body, settings, notes = load("E-footnote-numbering")
        self.assertEqual(props(settings), {})
        expected = [
            ({"numFmt": "upperRoman", "numStart": "5"}, [2, 1]),
            ({"numFmt": "decimal", "numRestart": "eachSect"}, [2]),
            ({"numRestart": "continuous"}, [1]),
            ({"numFmt": "chicago", "numRestart": "eachPage"}, [5, 2]),
            ({"numFmt": "decimal", "numRestart": "eachSect"}, [3]),
        ]
        found = sections(body)
        self.assertEqual(len(found), len(expected))
        for (paragraphs, sect), (numbering, counts) in zip(found, expected):
            label = texts(paragraphs[0])
            self.assertEqual(props(sect), numbering, label)
            self.assertEqual([sum(len(refs(p)) for p in page) for page in pages(paragraphs)],
                             counts, label)
            words = {"upperRoman": "upperRoman", "decimal": "decimal", "chicago": "chicago"}
            if "numFmt" in numbering:
                self.assertIn(words[numbering["numFmt"]], label)
            else:
                self.assertIn("no number format", label)
            if "numStart" in numbering:
                self.assertIn(f"from {numbering['numStart']}", label)
            restart = {"eachSect": "restart in each section", "eachPage": "restart on each page",
                       "continuous": "numbered on", None: "numbered on"}
            self.assertIn(restart[numbering.get("numRestart")], label)
        last = found[-1][0]
        marked = [r for p in last for r in p.iter(f"{W}footnoteReference")]
        self.assertEqual([r.get(f"{W}customMarkFollows") for r in marked], [None, "1", None])
        self.assertIn("a custom mark between notes", texts(last[0]))

        # E-footnote-numbering-settings: lower-case letters from 3, restarting on each page, in
        # the settings; section 1 has no w:footnotePr, section 2 upper-case letters only.
        body, settings, notes = load("E-footnote-numbering-settings")
        self.assertEqual(props(settings),
                         {"numFmt": "lowerLetter", "numStart": "3", "numRestart": "eachPage"})
        (first, first_sect), (second, second_sect) = sections(body)
        self.assertIsNone(first_sect.find(f"{W}footnotePr"))
        self.assertIn("no w:footnotePr in the section", texts(first[0]))
        self.assertEqual(props(second_sect), {"numFmt": "upperLetter"})
        self.assertIn("upperLetter in the section", texts(second[0]))
        self.assertEqual([[sum(len(refs(p)) for p in page) for page in pages(paragraphs)]
                          for paragraphs in (first, second)], [[2, 1], [1, 1]])

        # E-footnote-table: one reference in a table of one row, one in line 3 of a row that
        # starts after 41 lines, which fits on page 1 with its note, and one in line 18 of
        # such a row, past the 17 lines page 1 holds without a note.
        body, settings, notes = load("E-footnote-table")
        separator, note_line = note_heights("E-footnote-table")
        tables = body.findall(f"{W}tbl")
        self.assertEqual([refs(t) for t in tables], [["1"], ["2"], ["3"]])
        self.assertEqual(refs(body), ["1", "2", "3"])
        top = [p for p in body if p.tag == f"{W}p"]
        labels = [texts(p) for p in top if texts(p).startswith("Case ")]
        self.assertEqual(len(labels), 3)
        for table, label in zip(tables[1:], labels[1:]):
            first_cell = table.find(f"{W}tr/{W}tc")
            cell_lines = first_cell.findall(f"{W}p")
            self.assertEqual(len(cell_lines), 20)
            self.assertEqual({line(p) for p in cell_lines}, {("exact", 240)})
            (at,) = [k for k, p in enumerate(cell_lines, 1) if refs(p)]
            self.assertIn(f"in line {at:02d} of a row cut across pages", label)
            # The label and 40 filler lines of 12 pt right before the table, as crossing_case().
            elements = list(body)
            start = next(k for k, e in enumerate(elements) if texts(e) == label)
            between = elements[start + 1:elements.index(table)]
            self.assertEqual([texts(p) for p in between],
                             [f"Filler {k:02d}" for k in range(1, 41)])
            self.assertEqual({line(p) for p in [elements[start], *between]}, {("exact", 240)})
            before = (1 + len(between)) * 12
            if at == 3:
                self.assertLessEqual(before + at * 12 + separator + note_line, rules.BODY_HEIGHT)
            else:
                self.assertEqual(at, 18)
                self.assertLessEqual(before + (at - 1) * 12, rules.BODY_HEIGHT)
                self.assertGreater(before + at * 12, rules.BODY_HEIGHT)

        # E-endnote-doc-end: no placement anywhere (the end of the document), three endnotes,
        # the last of two lines, and no footnote.
        body, settings, notes = load("E-endnote-doc-end")
        self.assertEqual(props(settings, "endnote"), {})
        self.assertEqual([props(s, "endnote") for _, s in sections(body)], [{}, {}])
        self.assertEqual(refs(body, "endnote"), ["1", "2", "3"])
        self.assertEqual(refs(body), [])
        self.assertEqual([note_lines(notes["endnote"][i]) for i in "123"], [1, 1, 2])

        # E-endnote-doc-end-full: 58 lines of 12 pt fill the page, references in Line 01 and
        # Line 57.
        body, settings, notes = load("E-endnote-doc-end-full")
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), rules.LINES_PER_PAGE)
        self.assertEqual({line(p) for p in paragraphs}, {("exact", 240)})
        self.assertLessEqual(rules.LINES_PER_PAGE * 12, rules.BODY_HEIGHT)
        self.assertGreater((rules.LINES_PER_PAGE + 1) * 12, rules.BODY_HEIGHT)
        self.assertEqual([texts(p) for p in paragraphs if refs(p, "endnote")],
                         ["Line 01 holds a reference", "Line 57 holds a reference"])
        self.assertIn("endnotes from lines 01 and 57", texts(paragraphs[0]))

        # E-endnote-section-end and -section: endnotes at the end of each section, by the
        # settings or by each section; section 2 is continuous, section 3 on the next page.
        for name, in_settings in (("E-endnote-section-end", True),
                                  ("E-endnote-section-end-section", False)):
            body, settings, notes = load(name)
            at_end = {"pos": "sectEnd"}
            self.assertEqual(props(settings, "endnote"), at_end if in_settings else {}, name)
            found = sections(body)
            self.assertEqual([props(s, "endnote") for _, s in found],
                             [{} if in_settings else at_end] * 3, name)
            kinds = [s.find(f"{W}type") for _, s in found]
            self.assertEqual([k.get(f"{W}val") if k is not None else None for k in kinds],
                             [None, "continuous", None])
            self.assertEqual([len(refs(p, "endnote")) for p in body.findall(f"{W}p")
                              if refs(p, "endnote")], [1, 1, 1, 1])
            where = "the settings" if in_settings else "each section"
            self.assertIn(f"w:pos sectEnd for endnotes in {where}", texts(found[0][0][0]))

        # E-footnote-header: note 1 from the header and note 2 from the footer, which the one
        # section shows on both its pages, and notes 3 and 4 from the body.
        body, settings, notes = load("E-footnote-header")
        parts = dict(rules.parts(by_name["E-footnote-header"]))
        header = ElementTree.fromstring(parts["word/header1.xml"])
        footer = ElementTree.fromstring(parts["word/footer1.xml"])
        self.assertEqual((header.tag, refs(header)), (f"{W}hdr", ["1"]))
        self.assertEqual((footer.tag, refs(footer)), (f"{W}ftr", ["2"]))
        ((paragraphs, sect),) = sections(body)
        self.assertEqual([[r for p in page for r in refs(p)] for page in pages(paragraphs)],
                         [["3"], ["4"]])
        self.assertEqual({(r.tag.replace(W, ""), r.get(f"{W}type")) for r in sect
                          if r.tag in (f"{W}headerReference", f"{W}footerReference")},
                         {("headerReference", "default"), ("footerReference", "default")})
        self.assertIn("the header (note 1), the footer (note 2) and the body", texts(paragraphs[0]))

        # E-footnote-hidden: the second of three references is hidden text, and only it.
        body, settings, notes = load("E-footnote-hidden")
        self.assertIn("the second of them hidden text (w:vanish)", texts(body.find(f"{W}p")))
        hidden = {}
        for r in body.iter(f"{W}r"):
            ref = r.find(f"{W}footnoteReference")
            if ref is not None:
                hidden[ref.get(f"{W}id")] = r.find(f"{W}rPr/{W}vanish") is not None
            else:
                self.assertIsNone(r.find(f"{W}rPr/{W}vanish"), texts(r))
        self.assertEqual(hidden, {"1": False, "2": True, "3": False})
        # Only the reference is hidden: no run of the notes themselves is.
        for note_id, element in notes["footnote"].items():
            self.assertIsNone(element.find(f".//{W}rPr/{W}vanish"), f"footnote {note_id}")

        # E-footnote-start-continuous: no notes and no w:footnotePr in section 1; a continuous
        # section 2 from 5 and a section 3 on the next page from 9, both numbered on.
        body, settings, notes = load("E-footnote-start-continuous")
        self.assertEqual(props(settings), {})
        found = sections(body)
        starts = [s.find(f"{W}type") for _, s in found]
        self.assertEqual([t.get(f"{W}val") if t is not None else None for t in starts],
                         [None, "continuous", None])
        self.assertEqual([props(s) for _, s in found], [{}, {"numStart": "5"}, {"numStart": "9"}])
        self.assertEqual([sum(len(refs(p)) for p in paragraphs) for paragraphs, _ in found],
                         [0, 2, 1])
        labels = [texts(paragraphs[0]) for paragraphs, _ in found]
        self.assertIn("no footnotes", labels[0])
        self.assertIn("a continuous section, w:numStart 5, numbered on", labels[1])
        self.assertIn("on the next page, w:numStart 9, numbered on", labels[2])

        # A later section starts where its w:type says (the next page by default, continuous
        # in E-endnote-section-end), never with a page break of its own; later pages of one
        # section start with w:pageBreakBefore.
        for name in by_name:
            if name.startswith(("E-footnote-", "E-endnote-")):
                body, _, _ = load(name)
                for paragraphs, _ in sections(body)[1:]:
                    self.assertIsNone(paragraphs[0].find(f"{W}pPr/{W}pageBreakBefore"),
                                      f"{name}: {texts(paragraphs[0])}")

        # Every paragraph of these documents' bodies fits one line in Carlito, Calibri's twin,
        # so that the lines they count are the lines Word draws.
        # A cell is half the text's width, less its margins of 108 twips on either side.
        width = rules.TEXT_W / 20
        cell_width = (rules.TEXT_W // 2 - 2 * 108) / 20
        for name in by_name:
            if not name.startswith(("E-footnote-", "E-endnote-")):
                continue
            body, _, _ = load(name)
            in_cells = {id(p) for tc in body.iter(f"{W}tc") for p in tc.iter(f"{W}p")}
            for p in body.iter(f"{W}p"):
                room = cell_width if id(p) in in_cells else width
                self.assertLess(carlito_width(texts(p)) + 12, room, f"{name}: {texts(p)}")

    def test_pictures_are_typed_png_parts_that_resolve(self):
        # Every VML picture of every document is Word's picture frame, and every VML text box
        # Word's text box shape type (each shape type defined once, before its first use);
        # every VML and DrawingML picture names a picture relationship to a part of the
        # package, and that part is a well-formed PNG whose rows all use filter 0.
        v = "{urn:schemas-microsoft-com:vml}"
        a = "{http://schemas.openxmlformats.org/drawingml/2006/main}"
        r_ns = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
        kinds = {"_x0000_t75": (f"{v}imagedata", rules.PICTURE_FRAME),
                 "_x0000_t202": (f"{v}textbox", rules.TEXT_BOX_TYPE)}
        seen = drawn = 0
        for doc in self.docs:
            parts = dict(rules.parts(doc))
            document = ElementTree.fromstring(parts["word/document.xml"])
            shapes = list(document.iter(f"{v}shape"))
            blips = list(document.iter(f"{a}blip"))
            types = list(document.iter(f"{v}shapetype"))
            order = list(document.iter())
            for shape_type in types:
                child, markup = kinds[shape_type.get("id")]
                users = [s for s in shapes if s.get("type") == f"#{shape_type.get('id')}"]
                self.assertTrue(users, doc.name)
                self.assertLess(order.index(shape_type), order.index(users[0]), doc.name)
                self.assertIn(markup.encode(), parts["word/document.xml"], doc.name)
                for shape in users:
                    self.assertIsNotNone(shape.find(child), doc.name)
            self.assertEqual(len(types), len({t.get("id") for t in types}), doc.name)
            self.assertEqual({s.get("type") for s in shapes},
                             {f"#{t.get('id')}" for t in types}, doc.name)
            pictures = [s.find(f"{v}imagedata").get(f"{r_ns}id") for s in shapes
                        if s.find(f"{v}imagedata") is not None]
            drawn += bool(blips)
            pictures += [blip.get(f"{r_ns}embed") for blip in blips]
            if not pictures:
                self.assertEqual(doc.media, (), doc.name)
                continue
            seen += 1
            rels = {rel.get("Id"): rel for rel in ElementTree.fromstring(
                parts["word/_rels/document.xml.rels"])}
            for picture in pictures:
                rel = rels[picture]
                self.assertEqual(rel.get("Type"), f"{rules.REL}/image", doc.name)
                png = parts[f"word/{rel.get('Target')}"]
                self.assertTrue(png.startswith(b"\x89PNG\r\n\x1a\n"), doc.name)
                at, idat, width = 8, b"", None
                while at < len(png):
                    (size,) = struct.unpack(">I", png[at:at + 4])
                    kind, data = png[at + 4:at + 8], png[at + 8:at + 8 + size]
                    (crc,) = struct.unpack(">I", png[at + 8 + size:at + 12 + size])
                    self.assertEqual(crc, zlib.crc32(kind + data) & 0xFFFFFFFF, doc.name)
                    if kind == b"IHDR":
                        width, height = struct.unpack(">II", data[:8])
                    elif kind == b"IDAT":
                        idat += data
                    at += 12 + size
                # One stored deflate block (RFC 1951, §3.2.4), not compressed data.
                self.assertEqual(idat[:3], b"\x78\x01\x01", doc.name)
                pixels = zlib.decompress(idat)
                self.assertEqual(len(pixels), (width + 1) * height, doc.name)
                self.assertEqual(set(pixels[::width + 1]), {0}, doc.name)
        self.assertGreaterEqual(seen, 7)
        self.assertGreaterEqual(drawn, 1)
        # The shape types are Word's picture frame and text box, as copies Word re-saved
        # write them.
        for folder, name, kind, markup in (
            ("business", "contract-ec-data-processing-clauses-bg", "_x0000_t75",
             rules.PICTURE_FRAME),
            ("business", "letter-cfpb-validation-notice", "_x0000_t202", rules.TEXT_BOX_TYPE),
        ):
            resaved = SCRIPTS.parent / f"tests/corpus/resaved/{folder}/{name}.docx"
            with zipfile.ZipFile(resaved) as archive:
                written = archive.read("word/document.xml").decode()
            found = re.search(rf'<v:shapetype [^>]*id="{kind}".*?</v:shapetype>', written, re.S)
            # Word adds a w14:anchorId to the shape type of a shape it also wrote in DrawingML.
            self.assertEqual(markup, re.sub(r' w14:anchorId="[^"]*"', "", found.group(0)))
        # The pixels are stored, not compressed, so the bytes are the same on every zlib.
        self.assertEqual(rules.stored_zlib(b"abc"), b"\x78\x01\x01\x03\x00\xfc\xffabc"
                         + struct.pack(">I", zlib.adler32(b"abc")))
        self.assertEqual(zlib.decompress(rules.stored_zlib(bytes(70000))), bytes(70000))

    def test_refuses_a_setting_word_does_not_write_in_the_mode(self):
        doc = next(doc for doc in self.docs if doc.name == "E-opentype-on")
        for mode, values in ((15, (("enableOpenTypeFeature", 0),)),
                             (12, (("enableOpenTypeFeatures", 0),))):
            mistyped = dataclasses.replace(doc, mode=mode, compat_values=values)
            with self.assertRaisesRegex(ValueError, "not a setting Word writes in this mode"):
                rules.settings_xml(mistyped)

    def test_refuses_a_colour_mapping_attribute_that_does_not_exist(self):
        doc = next(doc for doc in self.docs if doc.color_mapping)
        mistyped = dataclasses.replace(doc, color_mapping=(("t3", "accent3"),))
        with self.assertRaisesRegex(ValueError, "no attribute t3"):
            rules.settings_xml(mistyped)

    def test_refuses_a_grid_that_does_not_fit_the_cells(self):
        # zip() would drop the cells the grid has no column for.
        for widths, grid in (([1000] * 4, None), (None, [1000] * 2)):
            with self.assertRaisesRegex(ValueError, "a column for every cell"):
                rules.styled_table("Grid", rules.named_cells(2, 3), widths=widths, grid=grid)


if __name__ == "__main__":
    unittest.main()
