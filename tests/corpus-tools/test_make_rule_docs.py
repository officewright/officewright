"""Checks scripts/make-rule-docs.py and the documents it committed to tests/corpus/word-rules/.

The documents' markup is checked against the ECMA-376 schemas by hand (see the script's
pull request); these tests keep the committed files, the generator and SOURCES.md in step
and check the package structure that Word needs to open a file at all.
"""

import dataclasses
import importlib.util
import json
import re
import shutil
import struct
import sys
import tempfile
import unicodedata
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


def carlito_width(text, size=11, font=CARLITO):
    """The width in points of text in Carlito, Calibri's metric twin, or in the TrueType
    font, at size points: the advances of hmtx for the glyphs that its (3, 1) cmap, of
    format 4, maps the text to."""
    data = font.read_bytes()
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
                        # A theme attribute names the theme's font for its slot (CT_Fonts
                        # spells the complex script one w:cstheme), or none, which the East
                        # Asian slot has.
                        if attribute in (f"{W}asciiTheme", f"{W}hAnsiTheme",
                                         f"{W}eastAsiaTheme", f"{W}cstheme"):
                            value = rules.THEME_FONTS[rules.THEME_SLOTS[value]]
                            if not value:
                                continue
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
            elif doc.name.endswith("-11"):
                self.assertEqual(modes, [11], doc.name)
            else:
                self.assertEqual(modes, [15], doc.name)
            # The settings Word writes only into documents in mode 15.
            others = {
                s.get(f"{W}name")
                for s in compat.iter(f"{W}compatSetting")
                if s.get(f"{W}name") != "compatibilityMode"
            }
            # E-table-style-override-off-11pt-14 writes one in mode 14, as another application
            # might (#532).
            self.assertEqual(bool(others),
                             modes == [15] or doc.name == "E-table-style-override-off-11pt-14",
                             doc.name)
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
                "E-whitespace-compat": {"doNotUseIndentAsNumberingTabStop"},
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
            for name in ("E-table-style-override-off-11pt", "E-table-style-override-off-12pt",
                         "E-table-style-override-off-11pt-14"):
                named[name] = ("overrideTableStyleFontSizeAndJustification", ["0"])
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
                # E-whitespace-compat writes each with a space around it (#243).
                if doc.name == "E-whitespace-compat":
                    expected = [" 1 "]
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
        # only show in the render. The documents about how Word matches style ids (#40) name
        # ids in another case and a style's name, and a style of no type of the schema, on
        # purpose: only those values are left out here (BatchFourteenStyles checks them).
        odd_names = {"E-style-id-case": {"mixedcase", "MIXEDCASE", "linkedchar"},
                     "E-style-id-missing": {"NoIdStyle", "No Id Style"}}
        odd_types = {"E-style-type-unknown": {"UnknownType"}}
        for doc in self.docs:
            odd = odd_names.get(doc.name, set())
            untyped = odd_types.get(doc.name, set())
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
            self.assertLessEqual(named - odd, defined, doc.name)
            # The odd values are named and match no id as written.
            self.assertLessEqual(odd, named - defined, doc.name)
            # And of the right type: a paragraph or a list level names a paragraph style, a
            # run or a paragraph mark a character style, and a style is based on its own type.
            kind = {s.get(f"{W}styleId"): s.get(f"{W}type") for s in styles.iter(f"{W}style")}
            self.assertLessEqual(untyped, {i for i, k in kind.items() if k == "unknown"},
                                 doc.name)
            for root in roots:
                for tag, expected in (
                    ("pStyle", "paragraph"), ("rStyle", "character"), ("tblStyle", "table")
                ):
                    for n in root.iter(f"{W}{tag}"):
                        if n.get(f"{W}val") in odd | untyped:
                            continue
                        self.assertEqual(kind[n.get(f"{W}val")], expected, doc.name)
            for style in styles.iter(f"{W}style"):
                base = style.find(f"{W}basedOn")
                if base is not None and base.get(f"{W}val") not in odd:
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

    def doc_parts(self, name: str) -> dict[str, bytes]:
        return dict(rules.parts(next(doc for doc in self.docs if doc.name == name)))

    def test_cell_mark_height_sizes_are_the_ones_its_labels_name(self):
        for name, mode in (("E-cell-mark-height", "15"), ("E-cell-mark-height-14", "14")):
            with self.subTest(name=name):
                parts = self.doc_parts(name)
                settings = ElementTree.fromstring(parts["word/settings.xml"])
                self.assertEqual([s.get(f"{W}val") for s in settings.iter(f"{W}compatSetting")
                                  if s.get(f"{W}name") == "compatibilityMode"], [mode])
                # Every font the document names is Liberation Sans, the defaults' too.
                fonts = {v for part in parts.values() if part.startswith(b"<")
                         for element in re.findall(rb"<w:rFonts [^>]*>", part)
                         for v in re.findall(rb'w:(?:ascii|hAnsi|eastAsia|cs)="([^"]*)"',
                                             element)}
                self.assertEqual(fonts, {b"Liberation Sans"})
                body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")
                tables = body.findall(f"{W}tbl")
                self.assertEqual(len(tables), 3)

                def marks(tbl):
                    found = []
                    for p in tbl.iter(f"{W}p"):
                        size = p.find(f"{W}pPr/{W}rPr/{W}sz")
                        found.append(None if size is None else size.get(f"{W}val"))
                    return found

                def breaks(tbl):
                    return [r.find(f"{W}rPr/{W}sz").get(f"{W}val") for r in tbl.iter(f"{W}r")
                            if r.find(f"{W}br") is not None]

                self.assertEqual(marks(tables[0]), [None, "40"])
                self.assertEqual(breaks(tables[1]), ["18", "18", "22", "22", "40", "40"])
                self.assertEqual(marks(tables[1]), ["18"] * 3)
                self.assertEqual(marks(tables[2]), ["18", "40", "18", "18"])
                text_sizes = {r.find(f"{W}rPr/{W}sz").get(f"{W}val")
                              for r in body.iter(f"{W}r") if re.match(
                                  r"Case \d\d cell", "".join(t.text or "" for t in r.iter(
                                      f"{W}t")))}
                self.assertEqual(text_sizes, {"18"})
                header = ElementTree.fromstring(parts["word/header1.xml"])
                self.assertEqual(len(header.findall(f"{W}tbl")), 2)
                # Each line between the breaks fits its cell (a third of the column, less
                # the cell margins of 5.4 pt), so a cell has the three lines its label says.
                liberation = CARLITO.parents[1] / "liberation" / "LiberationSans-Regular.ttf"
                room = (rules.TEXT_W // 3 - 2 * 108) / 20
                for cell in tables[1].iter(f"{W}tc"):
                    lines, line = [], ""
                    for r in cell.iter(f"{W}r"):
                        if r.find(f"{W}br") is not None:
                            lines.append(line)
                            line = ""
                        line += "".join(t.text or "" for t in r.iter(f"{W}t"))
                    lines.append(line)
                    self.assertEqual(len(lines), 3)
                    for text in lines:
                        self.assertLess(carlito_width(text, 9, liberation), room, text)

    def test_inline_vml_text_box_cases_have_their_space_before(self):
        v = "{urn:schemas-microsoft-com:vml}"
        wp = "{http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing}"
        body = ElementTree.fromstring(self.doc_parts("E-textbox-vml-inline")[
            "word/document.xml"]).find(f"{W}body")
        holders = [p for p in body.findall(f"{W}p")
                   if "".join(t.text or "" for t in p.iter(f"{W}t")).startswith("Before the")]
        before = [p.find(f"{W}pPr/{W}spacing").get(f"{W}before") for p in holders]
        self.assertEqual(before, ["0", "240", "240", "240"])
        kinds = []
        for p in holders:
            shape = p.find(f".//{v}shape")
            if shape is not None:
                self.assertNotIn("position", shape.get("style"))
                kinds.append("box" if shape.find(f"{v}textbox") is not None else "picture")
            else:
                self.assertIsNotNone(p.find(f".//{wp}inline"))
                kinds.append("drawingml box")
        self.assertEqual(kinds, ["box", "box", "picture", "drawingml box"])

    def test_override_off_documents_write_the_setting_0(self):
        cases = {"E-table-style-override-off-11pt": ("15", "22", "24"),
                 "E-table-style-override-off-12pt": ("15", "24", "26"),
                 "E-table-style-override-off-11pt-14": ("14", "22", "24")}
        for name, (mode, normal, defaults) in cases.items():
            with self.subTest(name=name):
                parts = self.doc_parts(name)
                settings = {s.get(f"{W}name"): s.get(f"{W}val") for s in ElementTree.fromstring(
                    parts["word/settings.xml"]).iter(f"{W}compatSetting")}
                self.assertEqual(settings["compatibilityMode"], mode)
                self.assertEqual(settings["overrideTableStyleFontSizeAndJustification"], "0")
                if mode == "14":
                    self.assertEqual(set(settings), {"compatibilityMode",
                                                     "overrideTableStyleFontSizeAndJustification"})
                styles = ElementTree.fromstring(parts["word/styles.xml"])
                normal_style = next(s for s in styles.iter(f"{W}style")
                                    if s.get(f"{W}styleId") == "Normal")
                self.assertEqual(normal_style.find(f"{W}rPr/{W}sz").get(f"{W}val"), normal)
                self.assertEqual(normal_style.find(f"{W}pPr/{W}jc").get(f"{W}val"), "left")
                doc_defaults = styles.find(f"{W}docDefaults")
                self.assertEqual(doc_defaults.find(f".//{W}rPrDefault//{W}sz").get(f"{W}val"),
                                 defaults)
                self.assertEqual(doc_defaults.find(f".//{W}pPrDefault//{W}jc").get(f"{W}val"),
                                 "right")
                big = next(s for s in styles.iter(f"{W}style") if s.get(f"{W}styleId") == "Big")
                self.assertEqual(big.find(f"{W}rPr/{W}sz").get(f"{W}val"), "28")
                self.assertEqual(big.find(f"{W}pPr/{W}jc").get(f"{W}val"), "center")

    def test_cell_margins_style_cases_have_the_margins_their_labels_name(self):
        parts = self.doc_parts("E-table-cell-margins-style")
        styles = {s.get(f"{W}styleId"): s for s in
                  ElementTree.fromstring(parts["word/styles.xml"]).iter(f"{W}style")}

        def sides(mar):
            return [(c.tag[len(W):], c.get(f"{W}w"), c.get(f"{W}type")) for c in mar]

        whole = styles["Margins15"].find(f"{W}tcPr/{W}tcMar")
        self.assertEqual(sides(whole), [(s, "300", "dxa") for s in rules.SIDES])
        first = styles["FirstColMargins15"]
        self.assertIsNone(first.find(f"{W}tcPr"))
        conditional = first.find(f"{W}tblStylePr")
        self.assertEqual(conditional.get(f"{W}type"), "firstCol")
        self.assertEqual(sides(conditional.find(f"{W}tcPr/{W}tcMar")),
                         [(s, "300", "dxa") for s in rules.SIDES])
        body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")
        tables = body.findall(f"{W}tbl")
        self.assertEqual([t.find(f"{W}tblPr/{W}tblStyle").get(f"{W}val") for t in tables],
                         ["Margins15", "FirstColMargins15"])
        for tbl in tables:
            self.assertEqual(sides(tbl.find(f"{W}tblPr/{W}tblCellMar")),
                             [(s, "100", "dxa") for s in rules.SIDES])
        own = [[(c.get(f"{W}w"), c.get(f"{W}type")) for c in mar] if mar is not None else None
               for mar in (tc.find(f"{W}tcPr/{W}tcMar") for tc in tables[0].iter(f"{W}tc"))]
        self.assertEqual(own, [[("0", "nil")] * 4, [("500", "pct")] * 4, [("200", "auto")] * 4,
                               None])
        look = tables[1].find(f"{W}tblPr/{W}tblLook")
        self.assertEqual(look.get(f"{W}firstColumn"), "1")
        self.assertEqual(look.get(f"{W}firstRow"), "0")
        first_cell = next(tables[1].iter(f"{W}tc"))
        self.assertEqual([c.get(f"{W}type") for c in first_cell.find(f"{W}tcPr/{W}tcMar")],
                         ["nil"] * 4)

    def test_code_128_and_ean_13_encode_their_text(self):
        # Each symbol is 3 bars and 3 spaces over 11 modules (the stop symbol 4 bars over
        # 13), and no two are alike.
        self.assertEqual(len(rules.CODE128), 107)
        self.assertEqual(len(set(rules.CODE128)), 107)
        self.assertTrue(all(len(w) == 6 and sum(map(int, w)) == 11 for w in rules.CODE128[:106]))
        self.assertEqual((len(rules.CODE128[106]), sum(map(int, rules.CODE128[106]))), (7, 13))
        # Reading the modules back gives the start symbol of code set B, the text, and the
        # check symbol.
        modules = rules.code128_modules("Ab-12")
        self.assertEqual(modules[:10] + modules[-10:], [False] * 20)
        widths = [len(m.group()) for m in re.finditer(r"1+|0+", "".join(
            "1" if d else "0" for d in modules[10:-10]))]
        joined = "".join(map(str, widths))
        symbols = [rules.CODE128.index(joined[i:i + 6]) for i in range(0, len(joined) - 7, 6)]
        self.assertEqual(joined[-7:], rules.CODE128[106])
        start, *text, check = symbols
        self.assertEqual(start, 104)
        self.assertEqual("".join(chr(v + 32) for v in text), "Ab-12")
        self.assertEqual(check, (104 + sum(n * v for n, v in enumerate(text, 1))) % 103)
        # Known values: Code 128's symbol 0 (space), 33 (A) and the stop; an EAN-13 check
        # digit of the GS1 example.
        self.assertEqual(rules.CODE128[0], "212222")
        self.assertEqual(rules.CODE128[33], "111323")
        # The barcode of the documents, module by module, as zxing-cpp 2.3 decodes the PNG
        # picture drawn from it back to OW-2026-0001 (checked once, not a dependency).
        self.assertEqual("".join("1" if d else "0" for d in rules.code128_modules("OW-2026-0001")),
                         "0000000000110100100001000111011011101000110100110111001100111001010011"
                         "1011001100111001011001110100100110111001001110110010011101100100111011"
                         "00100111001101110111101011000111010110000000000")
        self.assertEqual(rules.ean13_check("590123412345"), "5901234123457")
        with self.assertRaises(ValueError):
            rules.code128_modules("é")

    def test_the_qr_code_is_the_one_an_independent_encoder_gives(self):
        # INV-2026-0001 in byte mode, version 1, level M, mask 0, as the qrcode package of
        # PyPI (8.x) draws it with those settings.
        expected = [
            "#######..##.#.#######",
            "#.....#.####..#.....#",
            "#.###.#...#...#.###.#",
            "#.###.#....##.#.###.#",
            "#.###.#.#.#.#.#.###.#",
            "#.....#..##.#.#.....#",
            "#######.#.#.#.#######",
            "...........##........",
            "#.#.#.#..#.#....#..#.",
            "##.....#..##....#.##.",
            ".##.#.##....#...#..##",
            "#.##.........#.##....",
            "##.#####..#.#...###..",
            "........#.#.#.#.#..#.",
            "#######..##.###.#..##",
            "#.....#..#.#.#.#.#...",
            "#.###.#.###.##..#..#.",
            "#.###.#..#..##..#.##.",
            "#.###.#.##..#.#.###.#",
            "#.....#..#.#.#.###.#.",
            "#######.#.#.##..##.##",
        ]
        self.assertEqual(["".join("#" if d else "." for d in row)
                          for row in rules.qr_version1_m("INV-2026-0001")], expected)
        with self.assertRaises(ValueError):
            rules.qr_version1_m("fifteen bytes!!")

    def test_barcode_pictures_are_pngs_emfs_and_wmfs_of_the_codes(self):
        parts = self.doc_parts("E-barcode-pictures")
        bars, qr = rules.barcode_rows(), rules.qr_rows()
        # The codes the pictures draw: the barcode of 167 modules and its quiet zones of 10,
        # 40 modules tall; the QR code of 21 modules and its quiet zone of 4.
        self.assertEqual((len(bars[0]), len(bars)), (187, 40))
        self.assertEqual(bars[0], rules.code128_modules("OW-2026-0001"))
        self.assertEqual(qr[4:-4], [[False] * 4 + row + [False] * 4
                                    for row in rules.qr_version1_m("INV-2026-0001")])
        self.assertEqual(qr[:4] + qr[-4:], [[False] * 29] * 8)
        # Every pixel of the PNG pictures is its module's: black or white.
        for name, rows, size in (("image1.png", bars, (374, 80)), ("image2.png", qr, (116, 116))):
            data = parts[f"word/media/{name}"]
            width, height = struct.unpack(">II", data[16:24])
            self.assertEqual((width, height), size, name)
            scale = width // len(rows[0])
            pixels = zlib.decompress(data[data.index(b"IDAT") + 4:data.index(b"IEND") - 8])
            grid = [pixels[y * (width + 1) + 1:(y + 1) * (width + 1)] for y in range(height)]
            self.assertEqual(grid, [bytes(0 if d else 255 for d in row for _ in range(scale))
                                    for row in rows for _ in range(scale)], name)

        def painted(width: int, height: int, rects, unit: int) -> list[list[bool]]:
            """The modules a picture covers, from its filled rectangles in its units."""
            grid = [[False] * width for _ in range(height)]
            for left, top, right, bottom in rects:
                for y in range(top // unit, -(-(bottom - 1) // unit)):
                    for x in range(left // unit, -(-(right - 1) // unit)):
                        grid[y][x] = True
            return grid

        for name, rows, unit in (("image3.emf", bars, 20), ("image4.emf", qr, 20)):
            data = parts[f"word/media/{name}"]
            self.assertEqual((struct.unpack("<II", data[:8]), data[40:44]), ((1, 88), b" EMF"))
            self.assertEqual(struct.unpack("<I", data[48:52])[0], len(data))
            self.assertEqual(struct.unpack("<4i", data[8:24]),
                             (0, 0, len(rows[0]) * unit - 1, len(rows) * unit - 1))
            offset, records, brushes, selected, rects = 88, 1, {}, [], []
            while offset < len(data):
                kind, size = struct.unpack("<II", data[offset:offset + 8])
                payload = data[offset + 8:offset + size]
                if kind == 39:  # EMR_CREATEBRUSHINDIRECT: index, style, colour
                    index, style, colour, _ = struct.unpack("<IIII", payload)
                    brushes[index] = (style, colour)
                elif kind == 37:  # EMR_SELECTOBJECT
                    selected.append(struct.unpack("<I", payload)[0])
                elif kind == 43:  # EMR_RECTANGLE, one past its last pixel without a pen
                    self.assertEqual(selected, [1, 0x80000008], name)
                    left, top, right, bottom = struct.unpack("<4i", payload)
                    rects.append((left, top, right - 1, bottom - 1))
                    self.assertTrue(all(v % unit == 0 for v in rects[-1]), name)
                offset, records = offset + size, records + 1
            self.assertEqual(brushes, {1: (0, 0x000000)}, name)  # solid, black
            self.assertEqual(struct.unpack("<I", data[52:56])[0], records)
            self.assertEqual(painted(len(rows[0]), len(rows), rects, unit), rows, name)
        for name, rows, unit in (("image5.wmf", bars, 30), ("image6.wmf", qr, 60)):
            data = parts[f"word/media/{name}"]
            self.assertEqual(struct.unpack("<I", data[:4])[0], 0x9AC6CDD7)
            checksum = 0
            for (word,) in struct.iter_unpack("<H", data[:20]):
                checksum ^= word
            self.assertEqual(struct.unpack("<H", data[20:22])[0], checksum)
            self.assertEqual(struct.unpack("<hhhhH", data[6:16]),
                             (0, 0, len(rows[0]) * unit, len(rows) * unit, 1440))
            self.assertEqual(struct.unpack("<I", data[28:32])[0] * 2, len(data) - 22)
            offset, objects, selected, rects = 40, [], [], []
            while offset < len(data):
                size, function = struct.unpack("<IH", data[offset:offset + 6])
                words = struct.unpack(f"<{size - 3}h", data[offset + 6:offset + size * 2])
                if function == 0x02FC:  # META_CREATEBRUSHINDIRECT: style, colour, hatch
                    objects.append(("brush", words[0], words[1] | words[2] << 16))
                elif function == 0x02FA:  # META_CREATEPENINDIRECT: style ...
                    objects.append(("pen", words[0]))
                elif function == 0x012D:  # META_SELECTOBJECT
                    selected.append(objects[words[0]])
                elif function == 0x041B:  # META_RECTANGLE: bottom, right, top, left
                    bottom, right, top, left = words
                    rects.append((left, top, right - 1, bottom - 1))
                    self.assertTrue(all(v % unit == 0 for v in rects[-1]), name)
                elif function == 0x020C:  # META_SETWINDOWEXT: height, width
                    self.assertEqual(words, (len(rows) * unit, len(rows[0]) * unit))
                offset += size * 2
            self.assertEqual(selected, [("brush", 0, 0x000000), ("pen", 5)], name)  # PS_NULL
            self.assertEqual(painted(len(rows[0]), len(rows), rects, unit), rows, name)
        content_types = parts["[Content_Types].xml"]
        for ext, kind in (("png", "image/png"), ("emf", "image/x-emf"), ("wmf", "image/x-wmf")):
            self.assertIn(f'<Default Extension="{ext}" ContentType="{kind}"/>'.encode(),
                          content_types)

    def test_barcode_fields_have_the_codes_their_labels_name(self):
        body = ElementTree.fromstring(self.doc_parts("E-barcode-field")[
            "word/document.xml"]).find(f"{W}body")
        codes = [t.text.strip() for t in body.iter(f"{W}instrText")]
        self.assertEqual(codes, ['DISPLAYBARCODE "INV-2026-0001" QR \\q 1',
                                 'DISPLAYBARCODE "OW-2026-0001" CODE128 \\t',
                                 'DISPLAYBARCODE "5901234123457" EAN13 \\t',
                                 'DISPLAYBARCODE "OW-2026-0001" CODE128 \\h 720'])
        for p in body.findall(f"{W}p"):
            kinds = [c.get(f"{W}fldCharType") for c in p.iter(f"{W}fldChar")]
            if kinds:
                self.assertEqual(kinds, ["begin", "separate", "end"])
                runs = p.findall(f"{W}r")
                between = runs[runs.index(next(r for r in runs if r.find(
                    f"{W}fldChar[@{W}fldCharType='separate']") is not None)) + 1]
                self.assertIsNotNone(between.find(f"{W}fldChar"))  # the result is empty

    def test_cut_line_cases_have_the_markup_their_labels_name(self):
        body = ElementTree.fromstring(self.doc_parts("E-cut-lines-text")[
            "word/document.xml"]).find(f"{W}body")
        borders = [(side.tag[len(W):], side.get(f"{W}val"), side.get(f"{W}sz"),
                    side.get(f"{W}space"), side.get(f"{W}color"))
                   for p in body.iter(f"{W}pBdr") for side in p]
        self.assertEqual(borders, [
            ("bottom", "single", "4", "1", "000000"), ("bottom", "thick", "12", "1", "000000"),
            ("bottom", "double", "4", "1", "000000"), ("bottom", "dashed", "4", "1", "000000"),
            ("bottom", "dotted", "4", "1", "000000"),
            ("bottom", "dashSmallGap", "4", "1", "000000"),
            ("top", "single", "4", "1", "000000"),
            ("bottom", "single", "6", "1", "auto"), ("bottom", "double", "6", "1", "auto"),
        ])
        texts = ["".join(t.text or "" for t in p.iter(f"{W}t")) for p in body.findall(f"{W}p")]
        self.assertIn("- " * 76, texts)
        self.assertIn("✂ " + "- " * 73, texts)
        self.assertIn("_" * 80, texts)
        # The document's lines of characters each fit on one line of the column in Carlito,
        # the scissors given a generous 15 pt, as Carlito has no glyph for them.
        column = rules.TEXT_W / 20
        lines = [t for t in texts if t and set(t) <= set("- _\u2702")]
        self.assertEqual(len(lines), 3)
        for line in lines:
            scissors = line.count("\u2702")
            self.assertLess(15 * scissors + carlito_width(line.replace("\u2702", "")), column,
                            line)
        leaders = [(t.get(f"{W}val"), t.get(f"{W}pos"), t.get(f"{W}leader"))
                   for t in body.iter(f"{W}tab") if t.get(f"{W}val")]
        self.assertEqual(leaders, [("right", str(rules.TEXT_W), leader)
                                   for leader in ("hyphen", "dot", "underscore")])
        v = "{urn:schemas-microsoft-com:vml}"
        o = "{urn:schemas-microsoft-com:office:office}"
        a = "{http://schemas.openxmlformats.org/drawingml/2006/main}"
        wp = "{http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing}"
        body = ElementTree.fromstring(self.doc_parts("E-cut-lines-shapes")[
            "word/document.xml"]).find(f"{W}body")
        rules_hr = [(r.get(f"{o}hr"), r.get(f"{o}hrstd"), r.get(f"{o}hrpct"), r.get("style"))
                    for r in body.iter(f"{v}rect")]
        self.assertEqual(rules_hr, [("t", "t", None, "width:0;height:1.5pt"),
                                    ("t", "t", "500", "width:225.65pt;height:1.5pt")])
        drawings = [(d.tag[len(wp):], ln.find(f"{a}prstDash") is not None,
                     d.find(f".//{a}prstGeom").get("prst"))
                    for d in body.iter() if d.tag in (f"{wp}inline", f"{wp}anchor")
                    for ln in d.iter(f"{a}ln")]
        self.assertEqual(drawings, [("inline", False, "line"), ("inline", True, "line"),
                                    ("anchor", False, "line"), ("anchor", True, "line")])
        lines = [("position:absolute" in (ln.get("style") or ""),
                  ln.find(f"{v}stroke") is not None, ln.get("from"), ln.get("to"))
                 for ln in body.iter(f"{v}line")]
        self.assertEqual(lines, [(False, False, "0,6pt", "432pt,6pt"),
                                 (False, True, "0,6pt", "432pt,6pt"),
                                 (True, False, "0,6pt", "432pt,6pt"),
                                 (True, True, "0,6pt", "432pt,6pt")])

    def test_barcode_font_text_draws_the_code_128_symbols(self):
        # The characters as Libre Barcode 128 draws them back into the values, checked once
        # by drawing them with the font (Pillow) and decoding the image (zxing-cpp 2.3):
        # the check symbol of BB is 0, drawn by U+00C2; that of AA is 100, by U+00C8.
        self.assertEqual(rules.code128_font_text("OW-2026-0001"),
                         "\u00ccOW-2026-0001\\\u00ce")
        self.assertEqual(rules.code128_font_text("BB")[-2:], "\u00c2\u00ce")
        self.assertEqual(rules.code128_font_text("AA")[-2:], "\u00c8\u00ce")
        # For any value, the characters stand for the symbols code128_modules draws.
        for value in ("OW-2026-0001", "BB", "AA", "Ab-12 ~"):
            with self.subTest(value=value):
                symbols = [0 if c == "\u00c2" else ord(c) - 32 if ord(c) < 127 else ord(c) - 100
                           for c in rules.code128_font_text(value)]
                widths = "".join(rules.CODE128[v] for v in symbols)
                modules = rules.code128_modules(value)[10:-10]
                runs = "".join(str(len(m.group())) for m in re.finditer(
                    r"1+|0+", "".join("1" if d else "0" for d in modules)))
                self.assertEqual(widths, runs)
        with self.assertRaises(ValueError):
            rules.code128_font_text("é")

    def test_barcode_font_cases_have_the_markup_their_labels_name(self):
        parts = self.doc_parts("E-barcode-font")
        body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")
        bars = rules.code128_font_text("OW-2026-0001")

        def slots(font):
            return {slot: font for slot in ("ascii", "hAnsi", "eastAsia", "cs")}

        def props(rpr):
            """Each property of the run properties rpr, with its attributes."""
            return [] if rpr is None else [
                (e.tag[len(W):], {k[len(W):]: v for k, v in e.attrib.items()}) for e in rpr]

        def runs(p):
            """The text and the properties of each run of paragraph p, which holds nothing
            but its runs and paragraph properties that give its mark no run properties."""
            self.assertEqual([e.tag for e in p if e.tag != f"{W}pPr"],
                             [f"{W}r"] * len(p.findall(f"{W}r")))
            self.assertIsNone(p.find(f"{W}pPr/{W}rPr"))
            found = []
            for r in p.findall(f"{W}r"):
                self.assertEqual([e.tag for e in r if e.tag != f"{W}rPr"], [f"{W}t"])
                found.append((r.find(f"{W}t").text, props(r.find(f"{W}rPr"))))
            return found

        def barcode(half_points):
            return [("rFonts", slots("Libre Barcode 128")), ("sz", {"val": half_points}),
                    ("szCs", {"val": half_points})]

        self.assertEqual([e.tag for e in body], [f"{W}p"] * 8 + [f"{W}sectPr"])
        self.assertEqual([runs(p) for p in body.findall(f"{W}p")], [
            # Case 01: the barcode at 36 pt alone in its paragraph, and its value in the
            # paragraph below it, in the default font.
            [("Case 01: the Code 128 barcode of OW-2026-0001 in Libre Barcode 128 at 36 pt, "
              "a paragraph of its own, its text below it in Calibri", [])],
            [(bars, barcode("72"))],
            [("OW-2026-0001", [])],
            # Case 02: the barcode at 24 pt inside a line of text in the default font.
            [("Case 02: the same barcode in Libre Barcode 128 at 24 pt, inline in a line of "
              "text", [])],
            [("Total due: 125.00 ", []), (bars, barcode("48")), (" Thank you.", [])],
            # Case 03: the same characters in the default font and size.
            [("Case 03: the same characters in Calibri, 11 pt: the text the barcode is made "
              "of", [])],
            [(bars, [])],
            [("End of the document.", [])],
        ])
        # The default font is Calibri at 11 pt, and no style of a paragraph or run gives
        # another: no paragraph or run names a style, and no style has run properties.
        styles = ElementTree.fromstring(parts["word/styles.xml"])
        self.assertEqual(props(styles.find(f"{W}docDefaults/{W}rPrDefault/{W}rPr")), [
            ("rFonts", slots("Calibri")), ("sz", {"val": "22"}), ("szCs", {"val": "22"}),
            ("lang", {"val": "en-US", "eastAsia": "en-US", "bidi": "ar-SA"})])
        self.assertIsNone(body.find(f".//{W}pStyle"))
        self.assertIsNone(body.find(f".//{W}rStyle"))
        self.assertEqual(styles.findall(f".//{W}style//{W}rPr"), [])
        normal = styles.find(f"{W}style[@{W}type='paragraph'][@{W}default='1']")
        self.assertEqual(normal.get(f"{W}styleId"), "Normal")
        self.assertIn(b'w:name="Libre Barcode 128"', parts["word/fontTable.xml"])

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
        shape_properties = ([{f"{a}xfrm"}, {f"{a}prstGeom"}, fill, {f"{a}ln"},
                             {f"{a}effectLst", f"{a}effectDag"}], [])
        sequences = {
            f"{wp}inline": ([{f"{wp}extent"}, {f"{wp}effectExtent"}, {f"{wp}docPr"},
                             {f"{wp}cNvGraphicFramePr"}, {f"{a}graphic"}], [0, 2, 4]),
            f"{wp}anchor": ([{f"{wp}simplePos"}, {f"{wp}positionH"}, {f"{wp}positionV"},
                             {f"{wp}extent"}, {f"{wp}effectExtent"}, wrap, {f"{wp}docPr"},
                             {f"{wp}cNvGraphicFramePr"}, {f"{a}graphic"}], [0, 1, 2, 3, 5, 6, 8]),
            f"{wps}wsp": ([{f"{wps}cNvPr"}, {f"{wps}cNvSpPr", f"{wps}cNvCnPr"}, {f"{wps}spPr"},
                           {f"{wps}style"}, {f"{wps}txbx"}, {f"{wps}bodyPr"}], [1, 2, 5]),
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
        self.assertEqual(documents_with_notes, 33)

    def test_settings_follow_the_schema_order(self):
        # CT_Settings 17.15.1.78 is a sequence (here the elements the script writes), and so
        # are the document-wide footnote and endnote properties (17.11.12 and 17.11.4): first
        # the properties a section can give its notes too, then the special notes.
        order = ["zoom", "mirrorMargins", "defaultTabStop", "evenAndOddHeaders",
                 "characterSpacingControl", "footnotePr", "endnotePr", "compat",
                 "themeFontLang", "clrSchemeMapping", "decimalSymbol", "listSeparator"]
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

        # Every line of these documents' bodies, a paragraph or a part of one between line
        # breaks, fits the width in Carlito, Calibri's twin, so that the lines they count are
        # the lines Word draws.
        # A cell is half the text's width, less its margins of 108 twips on either side.
        width = rules.TEXT_W / 20
        cell_width = (rules.TEXT_W // 2 - 2 * 108) / 20

        def body_lines(p):
            found = [""]
            for element in p.iter():
                if element.tag == f"{W}t":
                    found[-1] += element.text or ""
                elif element.tag == f"{W}br":
                    found.append("")
            return found

        for name in by_name:
            if not name.startswith(("E-footnote-", "E-endnote-")):
                continue
            body, _, _ = load(name)
            in_cells = {id(p) for tc in body.iter(f"{W}tc") for p in tc.iter(f"{W}p")}
            for p in body.iter(f"{W}p"):
                room = cell_width if id(p) in in_cells else width
                for value in body_lines(p):
                    self.assertLess(carlito_width(value) + 12, room, f"{name}: {value}")

    # Batch 13.

    def batch_13(self, name):
        """The parts of a batch 13 document, its body's children and its compatibility
        mode."""
        parts = self.doc_parts(name)
        body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")
        settings = ElementTree.fromstring(parts["word/settings.xml"])
        mode = [s.get(f"{W}val") for s in settings.iter(f"{W}compatSetting")
                if s.get(f"{W}name") == "compatibilityMode"]
        return parts, body, mode

    @staticmethod
    def b13_texts(element):
        return "".join(t.text or "" for t in element.iter(f"{W}t"))

    @staticmethod
    def b13_lines(paragraph):
        """The parts of a paragraph between its line breaks."""
        found = [""]
        for element in paragraph.iter():
            if element.tag == f"{W}t":
                found[-1] += element.text or ""
            elif element.tag == f"{W}br":
                found.append("")
        return found

    def b13_sections(self, parts, body):
        """Each section's label, w:sectPr, and header and footer parts by their kind."""
        rels = ElementTree.fromstring(parts["word/_rels/document.xml.rels"])
        targets = {r.get("Id"): r.get("Target") for r in rels}
        paragraphs = body.findall(f"{W}p")
        sects = [p.find(f"{W}pPr/{W}sectPr") for p in paragraphs]
        found, label = [], None
        for p, sect in zip(paragraphs, sects):
            if self.b13_texts(p).startswith("Case "):
                label = self.b13_texts(p)
            if sect is not None:
                found.append((label, sect))
        found.append((label, body.find(f"{W}sectPr")))
        out = []
        for label, sect in found:
            stories = {}
            for ref in sect:
                if ref.tag in (f"{W}headerReference", f"{W}footerReference"):
                    rid = ref.get("{http://schemas.openxmlformats.org/officeDocument/2006/"
                                  "relationships}id")
                    stories[ref.tag[len(W):-len("Reference")]] = ElementTree.fromstring(
                        parts[f"word/{targets[rid]}"])
            out.append((label, sect, stories))
        return out

    @staticmethod
    def b13_story_lines(story):
        """The lines of a header or footer: (height in points, count) of each paragraph of
        exact line spacing outside tables, in order."""
        found = []
        for p in story.findall(f"{W}p"):
            line = p.find(f"{W}pPr/{W}spacing")
            if line is not None and line.get(f"{W}lineRule") == "exact":
                count = len(p.findall(f".//{W}br")) + 1
                found.append((int(line.get(f"{W}line")) / 20, count))
        return found

    def test_header_cut_documents_have_the_markup_their_labels_name(self):
        def size(sect):
            pg = sect.find(f"{W}pgSz")
            return int(pg.get(f"{W}w")), int(pg.get(f"{W}h"))

        def margins(sect):
            mar = sect.find(f"{W}pgMar")
            return {k: int(mar.get(f"{W}{k}")) for k in ("top", "bottom", "header", "footer")}

        sizes = {"A4": (rules.PAGE_W, rules.PAGE_H), "A5": rules.A5, "Letter": rules.LETTER,
                 "A3": rules.A3}

        # Every section's body is one line: its label fits the section's width in Carlito,
        # Calibri's twin, so that the body asks for the room of one line only.
        for doc in self.docs:
            if not doc.name.startswith("E-header-cut-"):
                continue
            parts, body, _ = self.batch_13(doc.name)
            for label, sect, _ in self.b13_sections(parts, body):
                mar = sect.find(f"{W}pgMar")
                width = (size(sect)[0] - int(mar.get(f"{W}left")) - int(mar.get(f"{W}right"))) / 20
                self.assertLess(carlito_width(label) + 12, width, f"{doc.name}: {label}")

        # Threshold: headers of 24 to 40 lines on A5, no footers, the room each label names.
        for name, body_line in (("E-header-cut-threshold", 12), ("E-header-cut-threshold-24", 24)):
            parts, body, _ = self.batch_13(name)
            found = self.b13_sections(parts, body)
            self.assertEqual(len(found), 17, name)
            for count, (label, sect, stories) in zip(range(24, 41), found):
                self.assertEqual(size(sect), rules.A5, label)
                self.assertEqual(set(stories), {"header"}, label)
                self.assertEqual(self.b13_story_lines(stories["header"]), [(12, count)], label)
                m = margins(sect)
                room = (rules.A5[1] - m["bottom"] - m["header"]) / 20 - 12 * count
                self.assertIn(f"header of {count} lines, {room:.1f} pt left", label)
            for p in body.findall(f"{W}p"):
                line = p.find(f"{W}pPr/{W}spacing")
                self.assertEqual((line.get(f"{W}line"), line.get(f"{W}lineRule")),
                                 (str(body_line * 20), "exact"), name)

        # The others: the header, footer, distance and page each label names, and an uncut
        # header and footer that leave the body less room than a line.
        expected = {
            "E-header-cut-distance": [((12, 33),), ((12, 35),), ((12, 33),)],
            "E-header-cut-page-sizes": [((12, 40),), ((12, 50),)],
            "E-header-cut-line-heights": [((16, 30),), ((8, 60),), ((15, 32),)],
        }
        for name, headers in expected.items():
            parts, body, _ = self.batch_13(name)
            found = self.b13_sections(parts, body)
            self.assertEqual(len(found), len(headers), name)
            for header, (label, sect, stories) in zip(headers, found):
                lines = self.b13_story_lines(stories["header"])
                self.assertEqual(tuple(lines), header, label)
                (height, count), = lines
                self.assertIn(f"header of {count} lines" if "page-sizes" not in name
                              else f"header and footer of {count} lines each", label)
                if name == "E-header-cut-line-heights":
                    self.assertIn(f"of exact {height:g} pt", label)
                footer = self.b13_story_lines(stories["footer"])
                self.assertEqual(footer, [(12, count if "page-sizes" in name else 33)], label)
                m = margins(sect)
                self.assertIn(f"{m['header'] / 20:g} pt from the edge" if "distance" in name
                              else "", label)
                page = next(k for k, v in sizes.items() if v == size(sect))
                self.assertTrue(label.endswith(f"on {page}") if "page-sizes" in name
                                else page == "A4", label)
                header_end = m["header"] / 20 + height * count
                footer_start = (size(sect)[1] - m["footer"]) / 20 - 12 * footer[0][1]
                room = max(0.0, footer_start - header_end)
                if name == "E-header-cut-distance":
                    self.assertIn(f"{room:.1f} pt left", label)
                else:
                    self.assertEqual(room, 0, label)

        # The table: 10 lines, 30 rows of exact 12 pt, a line, over a footer of 33 lines.
        parts, body, _ = self.batch_13("E-header-cut-table")
        (label, _, stories), = self.b13_sections(parts, body)
        header = stories["header"]
        self.assertEqual([child.tag[len(W):] for child in header], ["p", "tbl", "p"])
        rows = header.find(f"{W}tbl").findall(f"{W}tr")
        self.assertEqual(len(rows), 30)
        self.assertEqual({(r.find(f"{W}trPr/{W}trHeight").get(f"{W}val"),
                           r.find(f"{W}trPr/{W}trHeight").get(f"{W}hRule")) for r in rows},
                         {("240", "exact")})
        self.assertEqual(self.b13_story_lines(header)[0], (12, 10))
        self.assertEqual(self.b13_story_lines(stories["footer"]), [(12, 33)])
        self.assertIn("header of 10 lines and 30 table rows of 12 pt", label)

        # The first line: a line, then a table row, of exact 400 pt in header and footer.
        parts, body, _ = self.batch_13("E-header-cut-first-line")
        (line_label, _, lines), (row_label, _, rows) = self.b13_sections(parts, body)
        self.assertIn("header and footer of a 400 pt line, then a 12 pt line", line_label)
        self.assertIn("header and footer of a table of a 400 pt row and a 12 pt row", row_label)
        for kind in ("header", "footer"):
            first, second = lines[kind].findall(f"{W}p")
            self.assertEqual(first.find(f"{W}pPr/{W}spacing").get(f"{W}line"), "8000")
            self.assertEqual(second.find(f"{W}pPr/{W}spacing").get(f"{W}line"), "240")
            table = rows[kind].find(f"{W}tbl")
            heights = [(r.find(f"{W}trPr/{W}trHeight").get(f"{W}val"),
                        r.find(f"{W}trPr/{W}trHeight").get(f"{W}hRule"))
                       for r in table.findall(f"{W}tr")]
            self.assertEqual(heights, [("8000", "exact"), ("240", "exact")])
            self.assertEqual(rows[kind][0].tag, f"{W}tbl", "the table comes first")

        # The margins: fixed (negative) margins of 420 and 415 pt, headers of 30 and 10 lines
        # ending above the body; 30 lines are taller than 0.4 of the page, 10 are not.
        parts, body, _ = self.batch_13("E-header-cut-margins")
        found = self.b13_sections(parts, body)
        for count, (label, sect, stories) in zip((30, 10), found):
            m = margins(sect)
            self.assertEqual((m["top"], m["bottom"]), (-8400, -8300), label)
            self.assertIn(f"fixed margins {-m['top'] / 20:g} and {-m['bottom'] / 20:g} pt", label)
            self.assertEqual(set(stories), {"header"}, label)
            self.assertEqual(self.b13_story_lines(stories["header"]), [(12, count)], label)
            self.assertIn(f"header of {count} lines", label)
            room = (rules.PAGE_H + m["top"] + m["bottom"]) / 20
            self.assertIn(f"({room:.1f} pt left)", label)
            self.assertLess(m["header"] / 20 + 12 * count, -m["top"] / 20, label)
            self.assertEqual(12 * count > 0.4 * rules.PAGE_H / 20, count == 30, label)

    def test_exact_row_margin_cases_have_the_markup_their_labels_name(self):
        _, body, _ = self.batch_13("E-table-row-exact-margins")
        labels = [self.b13_texts(p) for p in body.findall(f"{W}p")
                  if self.b13_texts(p).startswith("Case ")]
        tables = body.findall(f"{W}tbl")
        self.assertEqual(len(tables), len(labels))
        for label, table in zip(labels, tables):
            row = table.findall(f"{W}tr")[1]
            height = row.find(f"{W}trPr/{W}trHeight")
            self.assertEqual((height.get(f"{W}val"), height.get(f"{W}hRule")), ("800", "exact"))
            self.assertIn("row 2 is exactly 40 pt high", label)
            first = row.find(f"{W}tc")
            own = {side.tag[len(W):]: side.get(f"{W}w")
                   for side in first.findall(f"{W}tcPr/{W}tcMar/*")}
            table_mar = {side.tag[len(W):]: side.get(f"{W}w")
                         for side in table.findall(f"{W}tblPr/{W}tblCellMar/*")}
            vertical = {k: v for k, v in table_mar.items() if k in ("top", "bottom")}
            if "its first cell has top and bottom margins of 10 pt" in label:
                self.assertEqual(own, {"top": "200", "bottom": "200"})
            elif "a top margin of 10 pt" in label:
                self.assertEqual(own, {"top": "200"})
            elif "a bottom margin of 10 pt" in label:
                self.assertEqual(own, {"bottom": "200"})
            else:
                self.assertEqual(own, {}, label)
            self.assertEqual(vertical, {"top": "200", "bottom": "200"}
                             if "the table's cell margins are 10 pt" in label else {}, label)
            lines = len(first.findall(f"{W}p"))
            self.assertEqual(lines, 6 if "holds 6 lines" in label else 1, label)
        self.assertEqual(len(labels), 6)

    def test_sym_plain_font_cases_have_the_markup_their_labels_name(self):
        _, body, _ = self.batch_13("E-sym-plain-font")
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), 20)
        for label_p, case in zip(paragraphs[::2], paragraphs[1::2]):
            label = self.b13_texts(label_p)
            (symbol,) = case.iter(f"{W}sym")
            font, code = symbol.get(f"{W}font"), symbol.get(f"{W}char")
            self.assertIn(f"a w:sym of {font} {code}", label)
            self.assertIn(font, ("Noto Sans", "Liberation Sans"))
            runs = case.findall(f"{W}r")
            self.assertEqual([self.b13_texts(r) for r in runs],
                             ["Hamburgefontsiv", "", "Hamburgefontsiv"], label)
            self.assertIsNone(runs[0].find(f"{W}rPr"), "the halves are in the default, Calibri")
            indent = case.find(f"{W}pPr/{W}ind")
            narrow = "at the end of a line" in label
            self.assertEqual(indent is not None, narrow, label)
            if narrow:
                self.assertEqual(rules.TEXT_W - int(indent.get(f"{W}right")), 3000)
                self.assertIn("150 pt wide", label)

    def test_greek_caps_cases_have_the_markup_their_labels_name(self):
        _, body, _ = self.batch_13("E-greek-caps")
        paragraphs = body.findall(f"{W}p")
        texts = {"letters with tonos and dialytika": rules.GREEK_TONOS,
                 "combining U+0301": rules.GREEK_COMBINING,
                 "capitals typed with their tonos": rules.GREEK_CAPITALS,
                 "polytonic": rules.GREEK_POLYTONIC}
        cases = {}
        for label_p, case in zip(paragraphs[::2], paragraphs[1::2]):
            label = self.b13_texts(label_p)
            (r,) = case.findall(f"{W}r")
            fonts = r.find(f"{W}rPr/{W}rFonts")
            self.assertEqual(fonts.get(f"{W}ascii"), "Noto Sans", label)
            toggles = {e.tag[len(W):] for e in r.find(f"{W}rPr")} & {"caps", "smallCaps"}
            lang = r.find(f"{W}rPr/{W}lang")
            cases[label] = (self.b13_texts(r), toggles, None if lang is None
                            else lang.get(f"{W}val"))
        labels = list(cases)
        self.assertEqual(len(labels), 13)
        for n, label in enumerate(labels, 1):
            value, toggles, lang = cases[label]
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            if "the same in w:smallCaps" in label:
                before = cases[labels[n - 2]]
                self.assertEqual((value, toggles, lang), (before[0], {"smallCaps"}, before[2]))
                continue
            if "with w:lang" in label:
                k = int(label.split("case 0")[1][0])
                model = cases[labels[k - 1]]
                self.assertEqual((value, toggles), model[:2], label)
                self.assertIn(f"w:lang {lang}", label)
                continue
            self.assertIsNone(lang, label)
            self.assertEqual(toggles, {"caps"} if "w:caps" in label else {"smallCaps"}, label)
            (expected,) = [v for words, v in texts.items() if words in label]
            self.assertEqual(value, expected, label)
        self.assertIn("Μάιος", rules.GREEK_TONOS)

        def marks(text):
            """Each letter of text with the combining marks its canonical decomposition has."""
            letters = []
            for c in unicodedata.normalize("NFD", text):
                if unicodedata.combining(c):
                    letters[-1][1].add(c)
                elif not c.isspace():
                    letters.append((c, set()))
            return letters

        tonos, dialytika = "\u0301", "\u0308"
        precomposed = [c for c in rules.GREEK_TONOS if not c.isspace()]
        self.assertEqual(len(precomposed), len(marks(rules.GREEK_TONOS)), "precomposed only")
        with_tonos = [base for base, m in marks(rules.GREEK_TONOS) if tonos in m]
        self.assertGreaterEqual(len(with_tonos), 8)
        self.assertTrue(all(base.islower() for base in with_tonos))
        self.assertEqual(sum(dialytika in m for _, m in marks(rules.GREEK_TONOS)), 4)
        self.assertEqual(sum(m == {tonos, dialytika} for _, m in marks(rules.GREEK_TONOS)), 2)
        self.assertNotEqual(unicodedata.normalize("NFC", rules.GREEK_COMBINING),
                            rules.GREEK_COMBINING, "the accents are combining marks")
        self.assertEqual(rules.GREEK_COMBINING.count(tonos), 8)
        self.assertEqual(unicodedata.normalize("NFC", rules.GREEK_COMBINING),
                         "άέήίόύώ Μάιος", "the letters of the precomposed case")
        capitals = [base for base, m in marks(rules.GREEK_CAPITALS) if tonos in m]
        self.assertGreaterEqual(len(capitals), 7)
        self.assertTrue(all(base.isupper() for base in capitals))
        polytonic = set().union(*(m for _, m in marks(rules.GREEK_POLYTONIC)))
        self.assertTrue(polytonic & {"\u0313", "\u0314"}, "a breathing")
        self.assertIn("\u0342", polytonic, "a perispomeni")
        for name, text in (("capitals", rules.GREEK_CAPITALS),
                           ("polytonic", rules.GREEK_POLYTONIC)):
            letters = [c for c in text if not c.isspace()]
            self.assertEqual(len(letters), len(marks(text)), f"{name}: precomposed only")
        for text in (rules.GREEK_TONOS, rules.GREEK_CAPITALS):
            accented = [c for c in text if tonos in unicodedata.normalize("NFD", c)]
            self.assertTrue(all(0x0370 <= ord(c) <= 0x03FF for c in accented),
                            "letters with tonos of Greek and Coptic, not their oxia twins")

    def test_batch_13_cell_mark_cases_have_the_markup_their_labels_name(self):
        parts, body, _ = self.batch_13("E-cell-mark-height-picture")
        styles = ElementTree.fromstring(parts["word/styles.xml"])
        defaults = styles.find(f"{W}docDefaults/{W}rPrDefault/{W}rPr")
        self.assertEqual(defaults.find(f"{W}rFonts").get(f"{W}ascii"), "Liberation Sans")
        self.assertEqual(defaults.find(f"{W}sz").get(f"{W}val"), "22")

        def mark(p):
            size = p.find(f"{W}pPr/{W}rPr/{W}sz")
            return None if size is None else size.get(f"{W}val")

        (table,) = body.findall(f"{W}tbl")
        cells = table.findall(f"{W}tr/{W}tc")
        self.assertEqual([mark(c.find(f"{W}p")) for c in cells], ["18", "22", "40"])
        wp = "{http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing}"
        for c in cells:
            (p,) = c.findall(f"{W}p")
            self.assertEqual(self.b13_texts(p), "", "only the picture")
            (extent,) = p.iter(f"{wp}extent")
            self.assertEqual(int(extent.get("cy")), 20 * 12700)
        pictures = [p for p in body.findall(f"{W}p") if list(p.iter(f"{wp}inline"))]
        self.assertEqual([mark(p) for p in pictures], ["18", "22", "40"])
        ids = [d.get("id") for d in body.iter(f"{wp}docPr")]
        self.assertEqual(len(ids), len(set(ids)))

        parts, body, _ = self.batch_13("E-cell-mark-height-merged")
        styles = ElementTree.fromstring(parts["word/styles.xml"])
        defaults = styles.find(f"{W}docDefaults/{W}rPrDefault/{W}rPr")
        self.assertEqual(defaults.find(f"{W}sz").get(f"{W}val"), "18")
        labels = [self.b13_texts(p) for p in body.findall(f"{W}p")
                  if self.b13_texts(p).startswith("Case ")]
        self.assertEqual(len(labels), 2)
        self.assertIn("rows 2 and 3 hold 8 pt text with 9 pt paragraph marks (the document "
                      "default), their second and third cells merged vertically", labels[0])
        merged, plain = body.findall(f"{W}tbl")
        for table, is_merged in ((merged, True), (plain, False)):
            rows = table.findall(f"{W}tr")
            self.assertEqual(len(rows), 4)
            for r in (rows[0], rows[3]):
                runs = list(r.iter(f"{W}r"))
                self.assertTrue(runs)
                self.assertTrue(all(run.find(f"{W}rPr/{W}sz") is None for run in runs),
                                "rows 1 and 4 are in the 9 pt default")
            for r in rows[1:3]:
                for c in r.findall(f"{W}tc"):
                    merge = c.find(f"{W}tcPr/{W}vMerge")
                    continued = merge is not None and merge.get(f"{W}val") is None
                    sizes = {None if (s := run.find(f"{W}rPr/{W}sz")) is None else s.get(f"{W}val")
                             for run in c.iter(f"{W}r")}
                    self.assertEqual(sizes, set() if continued else {"16"}, "8 pt text")
                    self.assertIsNone(c.find(f"{W}p/{W}pPr"), "the mark takes the default")
            merges = [[None if (m := c.find(f"{W}tcPr/{W}vMerge")) is None
                       else m.get(f"{W}val", "continue") for c in r.findall(f"{W}tc")]
                      for r in rows]
            if is_merged:
                self.assertEqual(merges[1:3], [[None, "restart", "restart"],
                                               [None, "continue", "continue"]])
            else:
                self.assertEqual({m for row in merges for m in row}, {None})

    def test_batch_13_note_documents_have_the_markup_their_labels_name(self):
        body_height = (rules.PAGE_H - 2 * rules.MARGIN) / 20

        def load(name):
            parts, body, mode = self.batch_13(name)
            notes = {e.get(f"{W}id"): e for e in ElementTree.fromstring(
                parts["word/footnotes.xml"]).findall(f"{W}footnote")}
            return body, mode, notes

        def refs(element):
            return [r.get(f"{W}id") for r in element.iter(f"{W}footnoteReference")]

        def note_lines(note):
            return sum(len(self.b13_lines(p)) for p in note.findall(f"{W}p"))

        def height(p):
            line = p.find(f"{W}pPr/{W}spacing")
            return int(line.get(f"{W}line")) / 20

        def pages(body):
            """The body's paragraphs, a list per page started by w:pageBreakBefore or a
            section break."""
            found = [[]]
            for p in body.findall(f"{W}p"):
                if p.find(f"{W}pPr/{W}pageBreakBefore") is not None:
                    found.append([])
                found[-1].append(p)
                if p.find(f"{W}pPr/{W}sectPr") is not None:
                    found.append([])
            return [page for page in found if page]

        def left(page, reference_line):
            """The room below the line at reference_line of a page: the body's height less
            the lines above it and its own."""
            return body_height - sum(height(p) for p in page[:reference_line + 1])

        # The modes, and E-footnote-no-room-14 as E-footnote-no-room but for the mode.
        for name in ("E-footnote-no-room-14", "E-footnote-cut-lines-14"):
            self.assertEqual(self.batch_13(name)[2], ["14"], name)
        for name in ("E-footnote-no-room", "E-footnote-cut-lines"):
            self.assertEqual(self.batch_13(name)[2], ["15"], name)
        a = self.doc_parts("E-footnote-no-room")
        b = self.doc_parts("E-footnote-no-room-14")
        self.assertEqual(a["word/document.xml"], b["word/document.xml"])
        self.assertEqual(a["word/footnotes.xml"], b["word/footnotes.xml"])

        # Space after: 32 pt left in both cases, a note of one line with 12 pt after in
        # section 1, without in section 2.
        body, _, notes = load("E-footnote-space-after")
        found = pages(body)
        self.assertEqual(len(found), 2)
        for n, page in enumerate(found, 1):
            (at,) = [k for k, p in enumerate(page) if refs(p)]
            self.assertAlmostEqual(left(page, at), 32)
            self.assertIn("32 pt left" if n == 1 else "the same note", self.b13_texts(page[0]))
            (note_id,) = refs(page[at])
            spacing = notes[note_id].find(f"{W}p/{W}pPr/{W}spacing")
            self.assertEqual(None if spacing is None else spacing.get(f"{W}after"),
                             "240" if n == 1 else None)
            self.assertEqual(note_lines(notes[note_id]), 1)
        self.assertIsNotNone(found[0][-1].find(f"{W}pPr/{W}sectPr"), "a section of its own")

        # After a note: line 1 references a note of one line, then a 3-line note with
        # widow control (the default) and the room the label names.
        body, _, notes = load("E-footnote-no-room-after-note")
        for page, room in zip(pages(body), rules.AFTER_NOTE_CASES):
            first, second = refs(page[0]), [r for p in page[1:] for r in refs(p)]
            self.assertEqual((len(first), len(second)), (1, 1))
            self.assertEqual(note_lines(notes[first[0]]), 1)
            self.assertEqual(note_lines(notes[second[0]]), 3)
            self.assertIsNone(notes[second[0]].find(f".//{W}widowControl"))
            (at,) = [k for k, p in enumerate(page) if refs(p) and k > 0]
            self.assertAlmostEqual(left(page, at), room)
            self.assertIn(f"{room} pt left", self.b13_texts(page[0]))

        # The first case leaves room for one line of the second note under the separator
        # and the first note, the second for none.
        separator, note_line = 13.4, 12.2
        self.assertEqual([int((room - separator - note_line) // note_line)
                          for room in rules.AFTER_NOTE_CASES], [1, 0])

        # Longer than a page: 120 lines from the line after the label, 40 lines after it.
        body, _, notes = load("E-footnote-longer-than-page")
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(refs(paragraphs[1]), ["1"])
        self.assertEqual(note_lines(notes["1"]), 120)
        self.assertEqual(len(paragraphs) - 3, 40)

        # Widow room: 55 pt below the reference line, a 4-line note, then a short line.
        body, _, notes = load("E-footnote-widow-room")
        page = body.findall(f"{W}p")
        (at,) = [k for k, p in enumerate(page) if refs(p)]
        self.assertAlmostEqual(left(page, at), 55)
        self.assertEqual(note_lines(notes["1"]), 4)
        self.assertEqual(self.b13_texts(page[at + 1]), "Case 01: a short line")
        # 55 pt hold the separator (13.4 pt) and three lines of 12.2 pt, not four.
        self.assertLess(13.4 + 3 * 12.2, 55)
        self.assertGreater(13.4 + 4 * 12.2, 55)

        # Cut lines: a paragraph of four lines without widow control, line 2 referencing a
        # note of 20 lines and line 3 one of one line, 100 pt below line 2.
        for name in ("E-footnote-cut-lines", "E-footnote-cut-lines-14"):
            body, _, notes = load(name)
            page = body.findall(f"{W}p")
            (at,) = [k for k, p in enumerate(page) if refs(p)]
            four = page[at]
            self.assertEqual(four.find(f"{W}pPr/{W}widowControl").get(f"{W}val"), "0")
            values = self.b13_lines(four)
            self.assertEqual(len(values), 4)
            per_line, line = [[]], 0
            for element in four.iter():
                if element.tag == f"{W}br":
                    per_line.append([])
                elif element.tag == f"{W}footnoteReference":
                    per_line[-1].append(element.get(f"{W}id"))
            self.assertEqual(per_line, [[], ["1"], ["2"], []])
            self.assertEqual((note_lines(notes["1"]), note_lines(notes["2"])), (20, 1))
            above = sum(height(p) for p in page[:at])
            self.assertAlmostEqual(body_height - above - 24, 100)

        # Rest beside text: 60 pt below each reference line, a 50-line note, then 12 lines
        # kept together, or 6 kept with the next 6.
        body, _, notes = load("E-footnote-rest-beside-text")
        found = pages(body)
        for n, page in enumerate(found[:2], 1):
            (at,) = [k for k, p in enumerate(page) if refs(p)]
            self.assertAlmostEqual(left(page, at), 60)
            self.assertEqual(note_lines(notes[str(n)]), 50)
            after = [p for p in page[at + 1:]
                     if not self.b13_texts(p).startswith((f"Case {n:02d}: ", "End of"))]
            self.assertEqual(self.b13_texts(page[at + len(after) + 1]),
                             f"Case {n:02d}: the line after")
            if n == 1:
                (kept,) = after
                self.assertIsNotNone(kept.find(f"{W}pPr/{W}keepLines"))
                self.assertEqual(len(self.b13_lines(kept)), 12)
                self.assertIn("12 lines kept together", self.b13_texts(page[0]))
            else:
                first, second = after
                self.assertIsNotNone(first.find(f"{W}pPr/{W}keepNext"))
                self.assertIsNone(second.find(f"{W}pPr/{W}keepNext"))
                self.assertEqual([len(self.b13_lines(p)) for p in after], [6, 6])
                self.assertIn("6 lines kept with the next 6", self.b13_texts(page[0]))

    def test_link_cases_have_the_markup_their_labels_name(self):
        _, body, _ = self.batch_13("E-link-boxes")
        links = list(body.iter(f"{W}hyperlink"))
        starts = {b.get(f"{W}name"): b for b in body.iter(f"{W}bookmarkStart")}
        ends = {b.get(f"{W}id") for b in body.iter(f"{W}bookmarkEnd")}
        # Every link goes to a bookmark of the document, each bookmark is closed, and each
        # link's text is coloured and underlined.
        self.assertEqual([a.get(f"{W}anchor") for a in links],
                         [f"Target_{c}" for c in "ABCDE"])
        self.assertEqual(set(starts), {f"Target_{c}" for c in "ABCDE"})
        self.assertEqual({b.get(f"{W}id") for b in starts.values()}, ends)
        for a in links:
            (r,) = a.findall(f"{W}r")
            self.assertEqual(r.find(f"{W}rPr/{W}color").get(f"{W}val"), "0563C1")
            self.assertEqual(r.find(f"{W}rPr/{W}u").get(f"{W}val"), "single")
        paragraphs = {self.b13_texts(p)[:7]: p for p in body.findall(f"{W}p")}
        for case, line in (("Case 01", None), ("Case 02", ("360", "auto")),
                           ("Case 03", ("600", "exact")), ("Case 04", None)):
            spacing = paragraphs[case].find(f"{W}pPr/{W}spacing")
            self.assertEqual(None if spacing is None else (spacing.get(f"{W}line"),
                                                           spacing.get(f"{W}lineRule")), line)
        # Case 04's link is longer than a line of the text's width in Calibri's twin.
        (long_link,) = paragraphs["Case 04"].iter(f"{W}hyperlink")
        self.assertGreater(carlito_width(self.b13_texts(long_link)), rules.TEXT_W / 20)
        self.assertLess(carlito_width(self.b13_texts(long_link)), 2 * rules.TEXT_W / 20 - 100)
        # Case 05's link is in a table cell.
        (cell_link,) = [a for tc in body.iter(f"{W}tc") for a in tc.iter(f"{W}hyperlink")]
        self.assertEqual(cell_link.get(f"{W}anchor"), "Target_E")
        # The targets: no length at a paragraph's start (A) and in mid-line (C), around words
        # in mid-line (B, E) and around a whole paragraph's words (D), on a page of their own.
        targets = {self.b13_texts(p)[:8]: p for p in body.findall(f"{W}p")
                   if self.b13_texts(p).startswith("Target ")}
        self.assertIsNotNone(targets["Target A"].find(f"{W}pPr/{W}pageBreakBefore"))

        def around(p):
            """The text before, inside and after the paragraph's bookmark."""
            parts, where = ["", "", ""], 0
            for element in p:
                if element.tag == f"{W}bookmarkStart":
                    where = 1
                elif element.tag == f"{W}bookmarkEnd":
                    where = 2
                else:
                    parts[where] += self.b13_texts(element)
            return [bool(value) for value in parts]

        self.assertEqual(around(targets["Target A"]), [False, False, True])
        self.assertEqual(around(targets["Target B"]), [True, True, True])
        self.assertEqual(around(targets["Target C"]), [True, False, True])
        self.assertEqual(around(targets["Target D"]), [False, True, False])
        self.assertEqual(around(targets["Target E"]), [True, True, True])

    def test_pictures_are_typed_png_parts_that_resolve(self):
        # Every VML picture of every document is Word's picture frame, and every VML text box
        # Word's text box shape type (each shape type defined once, before its first use),
        # but for the picture of E-vml-px-effects that has no type; every VML and DrawingML
        # picture of the body, a header, a footer or a note names a picture relationship of
        # its own part to a part of the package, and that part is a well-formed PNG whose rows
        # all use filter 0.
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
            untyped = [s for s in shapes if s.get("type") is None]
            self.assertEqual({s.get("type") for s in shapes if s not in untyped},
                             {f"#{t.get('id')}" for t in types}, doc.name)
            if untyped:
                self.assertEqual(doc.name, "E-vml-px-effects")
                self.assertTrue(all(s.find(f"{v}imagedata") is not None for s in untyped))
            drawn += bool(blips)
            pictures = []
            for name in parts:
                if not re.fullmatch(r"word/(document|header\d+|footer\d+|footnotes|endnotes)"
                                    r"\.xml", name):
                    continue
                story = ElementTree.fromstring(parts[name])
                named = [s.find(f"{v}imagedata").get(f"{r_ns}id") for s in story.iter(f"{v}shape")
                         if s.find(f"{v}imagedata") is not None]
                named += [blip.get(f"{r_ns}embed") for blip in story.iter(f"{a}blip")]
                pictures += [(name, picture) for picture in named]
            if not pictures:
                self.assertEqual(doc.media, (), doc.name)
                continue
            seen += 1
            for name, picture in pictures:
                rels = {rel.get("Id"): rel for rel in ElementTree.fromstring(
                    parts[f"word/_rels/{name.removeprefix('word/')}.rels"])}
                rel = rels[picture]
                self.assertEqual(rel.get("Type"), f"{rules.REL}/image", doc.name)
                png = parts[f"word/{rel.get('Target')}"]
                # EMF and WMF pictures (E-barcode-pictures) by their signatures: an EMF header
                # record ([MS-EMF] 2.3.4.2), a placeable WMF's key ([MS-WMF] 2.3.2.3).
                if rel.get("Target").endswith(".emf"):
                    self.assertEqual((png[:4], png[40:44]), (b"\x01\x00\x00\x00", b" EMF"))
                    continue
                if rel.get("Target").endswith(".wmf"):
                    self.assertEqual(png[:4], struct.pack("<I", 0x9AC6CDD7))
                    continue
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
        # A document that says it writes settings in any mode writes only the values given.
        written = rules.settings_xml(dataclasses.replace(
            doc, mode=14, compat_values=(("enableOpenTypeFeatures", 0),),
            settings_in_any_mode=True))
        self.assertEqual(re.findall(r'w:name="(\w+)"[^>]*w:val="(\d+)"', written),
                         [("compatibilityMode", "14"), ("enableOpenTypeFeatures", "0")])
        with self.assertRaisesRegex(ValueError, "not a setting Word writes in this mode"):
            rules.settings_xml(dataclasses.replace(doc, mode=14, compat_values=(
                ("enableOpenTypeFeature", 0),), settings_in_any_mode=True))
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



def font_tables(font):
    """The tables of a TrueType font file, by tag."""
    data = font.read_bytes()
    (count,) = struct.unpack(">H", data[4:6])
    tables = {}
    for i in range(count):
        tag, _, offset, length = struct.unpack(">4sIII", data[12 + 16 * i : 28 + 16 * i])
        tables[tag] = data[offset : offset + length]
    return tables


def win_metrics(font, size):
    """The OS/2 Windows ascent and descent of a font at size points: the single line Word
    sets is their sum."""
    tables = font_tables(font)
    (units,) = struct.unpack(">H", tables[b"head"][18:20])
    ascent, descent = struct.unpack(">HH", tables[b"OS/2"][74:78])
    return ascent * size / units, descent * size / units


def has_glyph(font, character):
    """Whether the (3, 1) cmap of format 4 maps the character to a glyph other than 0."""
    cmap = font_tables(font)[b"cmap"]
    (subtables,) = struct.unpack(">H", cmap[2:4])
    for i in range(subtables):
        platform, encoding, offset = struct.unpack(">HHI", cmap[4 + 8 * i : 12 + 8 * i])
        if (platform, encoding) == (3, 1):
            sub = cmap[offset:]
    segments = struct.unpack(">H", sub[6:8])[0] // 2
    ends = struct.unpack(f">{segments}H", sub[14 : 14 + 2 * segments])
    at = 16 + 2 * segments
    starts = struct.unpack(f">{segments}H", sub[at : at + 2 * segments])
    deltas = struct.unpack(f">{segments}h", sub[at + 2 * segments : at + 4 * segments])
    ranges_at = at + 4 * segments
    ranges = struct.unpack(f">{segments}H", sub[ranges_at : ranges_at + 2 * segments])
    code = ord(character)
    for k in range(segments):
        if starts[k] <= code <= ends[k]:
            if ranges[k] == 0:
                return (code + deltas[k]) & 0xFFFF != 0
            place = ranges_at + 2 * k + ranges[k] + 2 * (code - starts[k])
            (found,) = struct.unpack(">H", sub[place : place + 2])
            return found != 0 and (found + deltas[k]) & 0xFFFF != 0
    return False


class Batch14A(unittest.TestCase):
    """The documents of batch 14, part A (#586, #601, #587, #582, #497, #604, #546, #346):
    every claim of a label is checked against the markup, and the arithmetic of the cases
    against the fonts' metrics."""

    FONTS = CARLITO.parents[2] / "fonts-word"
    LIBERATION = CARLITO.parents[1] / "liberation" / "LiberationSans-Regular.ttf"
    NOTO = FONTS / "noto-truetype" / "NotoSans-Regular.ttf"
    BARCODE = FONTS / "libre-barcode-128" / "LibreBarcode128-Regular.ttf"
    # An A4 page with margins of 1 inch: the body's height and the text's width in points.
    BODY = (16838 - 2 * 1440) / 20
    TEXT = (11906 - 2 * 1440) / 20

    @classmethod
    def setUpClass(cls):
        cls.docs = {doc.name: doc for doc in rules.documents()}

    def load(self, name):
        """A document's parts, its body and its compatibility mode."""
        parts = dict(rules.parts(self.docs[name]))
        body = ElementTree.fromstring(parts["word/document.xml"]).find(f"{W}body")
        settings = ElementTree.fromstring(parts["word/settings.xml"])
        mode = [s.get(f"{W}val") for s in settings.iter(f"{W}compatSetting")
                if s.get(f"{W}name") == "compatibilityMode"]
        return parts, body, mode

    @staticmethod
    def texts(element):
        return "".join(t.text or "" for t in element.iter(f"{W}t"))

    @staticmethod
    def segments(paragraph):
        """The lines of a paragraph on each page its page breaks start: [1, 56] for a line,
        a page break and 56 lines."""
        found = [1]
        for br in paragraph.iter(f"{W}br"):
            if br.get(f"{W}type") == "page":
                found.append(1)
            else:
                found[-1] += 1
        return found

    @staticmethod
    def first_line(paragraph):
        found = ""
        for element in paragraph.iter():
            if element.tag == f"{W}br":
                break
            if element.tag == f"{W}t":
                found += element.text or ""
        return found

    @staticmethod
    def has(paragraph, path):
        return paragraph.find(path) is not None

    @staticmethod
    def widow(paragraph):
        """The paragraph's own widow control: True, False, or None when it sets none."""
        element = paragraph.find(f"{W}pPr/{W}widowControl")
        return None if element is None else element.get(f"{W}val") != "0"

    @staticmethod
    def line_rule(paragraph):
        element = paragraph.find(f"{W}pPr/{W}spacing")
        if element is None or element.get(f"{W}line") is None:
            return None
        return element.get(f"{W}lineRule"), int(element.get(f"{W}line"))

    def assert_one_line(self, label, size=11, font=CARLITO):
        self.assertLess(carlito_width(label, size, font) + 12, self.TEXT, label)

    def assert_twins(self, names):
        """Documents that differ in their compatibility mode alone: every part is the same
        bytes but the settings, whose w:compat carries the mode, and the core properties,
        which carry the purpose."""
        compat = re.compile(rb"<w:compat>.*?</w:compat>")
        first = dict(rules.parts(self.docs[names[0]]))
        for name in names[1:]:
            parts = dict(rules.parts(self.docs[name]))
            self.assertEqual(sorted(parts), sorted(first), name)
            for part_name, data in parts.items():
                if part_name == "docProps/core.xml":
                    continue
                if part_name == "word/settings.xml":
                    data = compat.sub(b"", data)
                    expected = compat.sub(b"", first[part_name])
                else:
                    expected = first[part_name]
                self.assertTrue(data == expected, f"{name}: {part_name}")

    def test_subscript_drop_has_every_size_in_every_font(self):
        _, body, mode = self.load("E-subscript-drop")
        self.assertEqual(mode, ["15"])
        # #586: 12, 24, 36, 48 and 72 pt, and every point from 20 to 48 pt.
        sizes = [12] + list(range(20, 49)) + [72]
        fonts = {"Calibri": CARLITO, "Liberation Sans": self.LIBERATION,
                 "Noto Sans": self.NOTO, "Libre Barcode 128": self.BARCODE}
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), len(fonts) * len(sizes))
        cases = [(font, size) for font in fonts for size in sizes]
        for n, (p, (font, size)) in enumerate(zip(paragraphs, cases), 1):
            label, base, script = p.findall(f"{W}r")
            self.assertEqual(self.texts(label), f"Case {n:02d}: {font} {size} pt, subscript: ")
            self.assertIsNone(label.find(f"{W}rPr"))
            for r, value in ((base, "Base"), (script, "123")):
                self.assertEqual(self.texts(r), value)
                slots = r.find(f"{W}rPr/{W}rFonts").attrib
                self.assertEqual(
                    {slots[f"{W}{s}"] for s in ("ascii", "hAnsi", "eastAsia", "cs")}, {font})
                for tag in ("sz", "szCs"):
                    self.assertEqual(r.find(f"{W}rPr/{W}{tag}").get(f"{W}val"), str(2 * size))
            self.assertIsNone(base.find(f"{W}rPr/{W}vertAlign"))
            self.assertEqual(script.find(f"{W}rPr/{W}vertAlign").get(f"{W}val"), "subscript")
            # Each font starts a page.
            self.assertEqual(self.has(p, f"{W}pPr/{W}pageBreakBefore"),
                             font != "Calibri" and size == 12, n)
            # One line: the label in Calibri 11 pt, "Base", and the subscript at 65 % of the
            # size.
            width = (carlito_width(self.texts(label)) + carlito_width("Base", size, fonts[font])
                     + carlito_width("123", 0.65 * size, fonts[font]))
            self.assertLess(width, self.TEXT, n)

    # Keep with next (#586 item 2): lines of exact 12 pt, 58 to a page.

    def describe(self, p):
        return (self.segments(p), self.has(p, f"{W}pPr/{W}keepNext"),
                self.has(p, f"{W}pPr/{W}keepLines"), self.has(p, f"{W}pPr/{W}pageBreakBefore"))

    def test_keepnext_last_lines_cases_have_the_markup_their_labels_name(self):
        _, body, _ = self.load("E-keepnext-last-lines")
        per_page = int(self.BODY // 12)
        self.assertEqual(per_page, 58)
        for p in body.iter(f"{W}p"):
            self.assertEqual(self.line_rule(p), ("exact", 240))
        cases, end = {}, None
        for element in body:
            if element.tag == f"{W}sectPr":
                continue
            label = self.first_line(element) if element.tag == f"{W}p" else ""
            if label.startswith("End of"):
                end = element
                continue
            if label.startswith("Case "):
                n = int(label[5:7])
                cases.setdefault(n, (label, []))
            cases[n][1].append(element)
        self.assertIsNotNone(end)
        no, yes = False, True
        expected = {
            # (widow control, the paragraphs as (lines per page, keep with next, keep lines,
            # page break before), and the words of the label)
            1: (yes, [([20], no, no, no), ([1, 56], yes, no, no), ([3], no, yes, no)],
                ["20 lines", "a page break and 56 lines"]),
            2: (no, [([51], no, no, yes), ([6], yes, no, no), ([3], no, yes, no)],
                ["51 lines", "6 lines", "3 lines kept together"]),
            3: (yes, [([51], no, no, yes), ([6], yes, no, no), ([3], no, yes, no)],
                ["51 lines", "6 lines", "3 lines kept together"]),
            4: (no, [([56], yes, no, yes), ([3], no, yes, no)],
                ["56 lines from page break before", "3 lines kept together"]),
            5: (no, [([20], no, no, yes), ([1, 56], yes, no, no), ([58], no, yes, no)],
                ["20 lines", "a page break and 56 lines", "58 kept together"]),
            6: (no, [([56], yes, no, yes), "table", ([1], no, no, no)],
                ["56 lines from page break before", "row 1 may not split"]),
            7: (no, [([10], yes, no, yes)] + [([1], yes, no, no)] * 54 + [([1], no, no, no)],
                ["kept 10 lines", "55 one-line paragraphs, 54 kept"]),
        }
        self.assertEqual(sorted(cases), sorted(expected))
        for n, (widow, shapes, words) in expected.items():
            label, elements = cases[n]
            self.assert_one_line(label)
            state = "on" if widow else "off"
            self.assertTrue(label.endswith((f"widow control {state}", f"widow {state}")), label)
            for word in words:
                self.assertIn(word, label)
            found = ["table" if e.tag == f"{W}tbl" else self.describe(e) for e in elements]
            self.assertEqual(found, shapes, n)
            for e in elements:
                for p in ([e] if e.tag == f"{W}p" else e.iter(f"{W}p")):
                    self.assertIs(self.widow(p), widow, n)
        lines = {n: [s[0] for s in shapes if s != "table"] for n, (_, shapes, _) in
                 expected.items()}
        # Case 01 and 05: the lines after the break fit a page, the lines kept together after
        # them do not; 58 lines fit a page alone, but not after the kept paragraph's last.
        for n in (1, 5):
            after, kept = lines[n][1][1], lines[n][2][0]
            self.assertLessEqual(after, per_page)
            self.assertGreater(after + kept, per_page)
        self.assertLessEqual(lines[5][2][0], per_page)
        self.assertGreater(1 + lines[5][2][0], per_page)
        # Cases 02 and 03: the 6 lines fit below the 51, the 3 kept together do not.
        for n in (2, 3):
            before, kept, together = (s[0] for s in lines[n])
            self.assertLessEqual(before + kept, per_page)
            self.assertGreater(before + kept + together, per_page)
        # Case 04: 56 and 3 lines are more than a page; case 07's chain is longer than one.
        self.assertGreater(sum(s[0] for s in lines[4]), per_page)
        self.assertGreater(sum(s[0] for s in lines[7]), per_page)
        # Case 06: the table's first row of 3 lines may not split and does not fit below the
        # 56 lines; its second row has 1 line.
        table = next(e for e in cases[6][1] if e.tag == f"{W}tbl")
        first, second = table.findall(f"{W}tr")
        self.assertTrue(self.has(first, f"{W}trPr/{W}cantSplit"))
        self.assertFalse(self.has(second, f"{W}trPr/{W}cantSplit"))
        self.assertEqual([self.segments(p) for p in first.iter(f"{W}p")], [[3], [1]])
        self.assertEqual([self.segments(p) for p in second.iter(f"{W}p")], [[1], [1]])
        self.assertGreater(12 * (lines[6][0][0] + 3), self.BODY)

    def test_keepnext_last_lines_section_cases_start_sections(self):
        _, body, _ = self.load("E-keepnext-last-lines-section")
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), 5)
        sects = [p.find(f"{W}pPr/{W}sectPr") for p in paragraphs]
        self.assertEqual([s is not None for s in sects], [True, False, True, False, False])
        # Sections 2 and 3 start on a new page: no w:type, which means nextPage.
        for sect in (sects[0], sects[2], body.find(f"{W}sectPr")):
            self.assertIsNone(sect.find(f"{W}type"))
        for n, (kept, together, widow) in enumerate(
                ((paragraphs[1], paragraphs[2], False), (paragraphs[3], paragraphs[4], True)), 1):
            label = self.first_line(kept)
            self.assertTrue(label.startswith(f"Case {n:02d}: a section starts with a kept "
                                             "paragraph of 56 lines; 3 lines kept together"))
            self.assertTrue(label.endswith(f"widow {'on' if widow else 'off'}"), label)
            self.assert_one_line(label)
            self.assertEqual(self.describe(kept), ([56], True, False, False))
            self.assertEqual(self.describe(together), ([3], False, True, False))
            for p in (kept, together):
                self.assertIs(self.widow(p), widow)
                self.assertEqual(self.line_rule(p), ("exact", 240))
            self.assertGreater(56 + 3, int(self.BODY // 12))

    # Multiple line spacing at the foot of a page (#586 item 3).

    def test_line_multiple_foot_cases_end_their_text_where_their_labels_say(self):
        line_11 = carlito_line_height(11)
        line_10 = carlito_line_height(10)
        descent_11 = win_metrics(CARLITO, 11)[1]
        documents = {"E-line-multiple-foot": ["15"], "E-line-multiple-foot-14": ["14"],
                     "E-line-multiple-foot-12": [], "E-line-multiple-foot-11": ["11"]}
        first = self.load("E-line-multiple-foot")[0]
        for name, mode in documents.items():
            self.assertEqual(self.load(name)[2], mode, name)
        self.assert_twins(list(documents))
        parts, body, _ = self.load("E-line-multiple-foot")
        notes = {e.get(f"{W}id"): e for e in ElementTree.fromstring(
            parts["word/footnotes.xml"]).findall(f"{W}footnote")}
        # Widow control is off in every paragraph of the body.
        for p in body.iter(f"{W}p"):
            self.assertIs(self.widow(p), False)
        spacings = {("auto", 720): "spaced 3 lines (w:line 720)",
                    ("auto", 276): "spaced 1.15 lines (w:line 276)",
                    ("atLeast", 800): "at least 40 pt", ("exact", 800): "exactly 40 pt"}
        pages = []
        for element in body:
            if element.tag == f"{W}sectPr":
                continue
            if not pages or self.has(element, f"{W}pPr/{W}pageBreakBefore"):
                pages.append([])
            pages[-1].append(element)
        found = []
        for page in pages[:-1] + [pages[-1][:-1]]:
            title = page[0]
            label = self.texts(title) if title.find(f".//{W}br") is None else self.first_line(title)
            self.assert_one_line(label)
            n = int(label[5:7])
            tables = [e for e in page if e.tag == f"{W}tbl"]
            if tables:
                (table,) = tables
                top = table.find(f"{W}tblPr/{W}tblBorders/{W}top")
                self.assertEqual(self.line_rule(title), ("exact", 400))
                start = 20 + int(top.get(f"{W}sz")) / 8
                cells = table.find(f"{W}tr").findall(f"{W}tc")
                sequence = cells[0].findall(f"{W}p")
            else:
                start, sequence = 0, [e for e in page if e.tag == f"{W}p"]
            y = start
            for k, p in enumerate(sequence):
                rule, line = self.line_rule(p)
                if not self.texts(p):
                    # The empty paragraph of exact height.
                    self.assertEqual(rule, "exact")
                    y += line / 20
                elif (rule, line) == ("exact", 400):
                    y += 20 * self.segments(p)[0]
                else:
                    break
            case = sequence[k]
            self.assertEqual(self.segments(case), [2], label)
            if rule == "auto":
                height, text_end = line / 240 * line_11, line_11
            elif rule == "atLeast":
                height = max(line / 20, line_11)
                text_end = height
            else:
                # A line of exact height has its baseline at 0.8 of it (lines.rs, from Word's
                # render of E-line-exact).
                height, text_end = line / 20, 0.8 * line / 20 + descent_11
            end = y + height + text_end
            refs = [r.get(f"{W}id") for r in title.iter(f"{W}footnoteReference")]
            limit = self.BODY
            if refs:
                (ref,) = refs
                note_lines = sum(len(self.segments(p)) * self.segments(p)[0]
                                 for p in notes[ref].findall(f"{W}p"))
                self.assertEqual(note_lines, 1)
                # The separator is a line of the 11 pt default text, the note 10 pt.
                limit -= line_11 + note_lines * line_10
            side = -1 if " 1 pt above " in label else 1
            self.assertIn(" 1 pt above " if side < 0 else " 1 pt below ", label)
            self.assertAlmostEqual(end - limit, side, delta=0.05, msg=label)
            self.assertIn(spacings[(rule, line)], label)
            self.assertEqual(" in a cell;" in label, bool(tables), label)
            self.assertTrue(label.endswith("the footnotes" if refs else "the bottom margin"))
            if tables:
                # The cell goes on below the case's lines, so its row is cut over pages.
                rest = sequence[k + 1:]
                self.assertEqual([self.line_rule(p) for p in rest], [("exact", 400)])
                self.assertGreater(end + 20 * self.segments(rest[0])[0], limit)
            if (rule, side) == ("exact", -1):
                # The exact line's box reaches past the margin, its text does not.
                self.assertAlmostEqual(end - text_end + height - limit, 4.05, delta=0.05)
            self.assertEqual(self.texts(page[-1]), f"Case {n:02d}: the paragraph after")
            found.append(("cell" if tables else "note" if refs else "body", rule, line, side))
        self.assertEqual(len(found), 16)
        both = [(-1,), (1,)]
        self.assertEqual(sorted(found), sorted(
            (where, *spacing, *side) for where, kinds in (
                ("body", [("auto", 720), ("auto", 276), ("atLeast", 800), ("exact", 800)]),
                ("note", [("auto", 720), ("auto", 276)]),
                ("cell", [("auto", 720), ("auto", 276)]))
            for spacing in kinds for side in both))
        self.assertEqual(len(notes) - 2, 4)

    def test_table_header_repeat_off_cases_have_the_markup_their_labels_name(self):
        _, body, _ = self.load("E-table-header-repeat-off")
        children = [e for e in body if e.tag != f"{W}sectPr"]
        # The labels are the paragraphs right before a table.
        labels = [k for k, e in enumerate(children[:-1])
                  if e.tag == f"{W}p" and children[k + 1].tag == f"{W}tbl"]
        self.assertEqual(len(labels), 5)
        line = carlito_line_height(11)

        def rows(table):
            """Per row: header row or not, may split or not, and its first cell's lines."""
            return [(self.has(tr, f"{W}trPr/{W}tblHeader"), self.has(tr, f"{W}trPr/{W}cantSplit"),
                     len(tr.find(f"{W}tc").findall(f"{W}p"))) for tr in table.findall(f"{W}tr")]

        def borders(table):
            return {int(e.get(f"{W}sz")) for e in table.find(f"{W}tblPr/{W}tblBorders")}

        for n, at in enumerate(labels, 1):
            label = self.texts(children[at])
            self.assert_one_line(label)
            self.assertEqual(self.has(children[at], f"{W}pPr/{W}pageBreakBefore"), n > 1)
        # Case 01: a header row of 60 lines, 50 body rows, a paragraph.
        label, table, after = children[labels[0]:labels[0] + 3]
        self.assertEqual(rows(table), [(True, False, 60)] + [(False, False, 1)] * 50)
        self.assertIn("a header row of 60 lines; 50 body rows follow, then a paragraph",
                      self.texts(label))
        self.assertEqual(self.texts(after), "Case 01: the paragraph after the table")
        self.assertEqual(borders(table), {48})
        # Case 02: two header rows of 22 lines, a body row of 1 line and one of 10 that may not
        # split. With 6 pt borders the header rows take just under a page and leave room for
        # the short row below them on the first page, not for the tall one there or below
        # them on the next.
        label, table, after = children[labels[1]:labels[1] + 3]
        self.assertEqual(rows(table), [(True, False, 22), (True, False, 22), (False, False, 1),
                                       (False, True, 10)])
        self.assertIn("header rows 1 and 2 of 22 lines each, a body row of 1 line, one of 10 "
                      "that may not split", self.texts(label))
        self.assertEqual(borders(table), {48})
        heads = 44 * line + 3 * 6
        self.assertLess(heads, self.BODY)
        self.assertGreaterEqual(self.BODY - line - heads, line + 6)
        self.assertLess(self.BODY - line - heads - (line + 6), 10 * line + 6)
        self.assertLess(self.BODY - heads, 10 * line + 6)
        self.assertEqual(self.texts(after), "Case 02: the paragraph after the table")
        tall = [(True, False, 35), (True, False, 35)] + [(False, False, 1)] * 30
        # Case 03: two header rows of 35 lines, 30 body rows, at once a table of 2 columns.
        label, table, second, after = children[labels[2]:labels[2] + 4]
        self.assertEqual(rows(table), tall)
        self.assertEqual(second.tag, f"{W}tbl")
        self.assertEqual([len(tr.findall(f"{W}tc")) for tr in second.findall(f"{W}tr")], [2] * 3)
        self.assertIn("2 header rows of 35 lines, 30 body rows, then at once a second table of 2 "
                      "columns", self.texts(label))
        self.assertEqual(self.texts(after), "Case 03: the paragraph after the second table")
        # Case 04: the same table, then an empty paragraph that ends the section; the next
        # section is continuous.
        label, table, empty, after = children[labels[3]:labels[3] + 4]
        self.assertEqual(rows(table), tall)
        self.assertEqual(self.texts(empty), "")
        self.assertIsNotNone(empty.find(f"{W}pPr/{W}sectPr"))
        self.assertEqual(body.find(f"{W}sectPr/{W}type").get(f"{W}val"), "continuous")
        self.assertEqual([e for e in children if e.find(f"{W}pPr/{W}sectPr") is not None],
                         [empty])
        self.assertIn("then a continuous section break", self.texts(label))
        self.assertEqual(self.texts(after), "Case 04: the first paragraph of the continuous "
                         "section")
        # Case 05: the same table, then a paragraph with 24 pt before.
        label, table, after = children[labels[4]:labels[4] + 3]
        self.assertEqual(rows(table), tall)
        self.assertEqual(after.find(f"{W}pPr/{W}spacing").get(f"{W}before"), "480")
        self.assertIn("then a paragraph with 24 pt before", self.texts(label))
        for at in labels[2:]:
            self.assertEqual(borders(children[at + 1]), {48})
        self.assertEqual(self.texts(children[-1]), "End of the document.")

    # Justified lines (#601, #497), in Liberation Sans 11 pt.

    def justified(self, name):
        """The (label, case paragraph) pairs of a document of justified cases."""
        _, body, _ = self.load(name)
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs) % 2, 0, name)
        pairs = list(zip(paragraphs[::2], paragraphs[1::2]))
        for label, case in pairs:
            self.assert_one_line(self.texts(label))
            self.assertEqual(case.find(f"{W}pPr/{W}jc").get(f"{W}val"), "both")
            self.assertIsNone(label.find(f"{W}pPr/{W}jc"))
            for r in case.findall(f"{W}r"):
                fonts = r.find(f"{W}rPr/{W}rFonts").attrib
                self.assertEqual({fonts[f"{W}{s}"] for s in ("ascii", "hAnsi", "eastAsia", "cs")},
                                 {"Liberation Sans"})
                self.assertEqual(r.find(f"{W}rPr/{W}sz").get(f"{W}val"), "22")
        return [(self.texts(label), case) for label, case in pairs]

    def test_justify_nbsp_cases_have_the_spaces_their_labels_name(self):
        space = carlito_width(" ", font=self.LIBERATION)
        documents = {"E-justify-nbsp-14": ["14"], "E-justify-nbsp-15": ["15"]}
        for name, mode in documents.items():
            self.assertEqual(self.load(name)[2], mode)
        self.assert_twins(list(documents))
        cases = self.justified("E-justify-nbsp-15")
        self.assertEqual(len(cases), 9)
        seen = []
        for n, (label, case) in enumerate(cases, 1):
            self.assertTrue(label.startswith(f"Case {n:02d}: "))
            # The space the label names, by its code point and its Unicode name.
            (code,) = re.findall(r"U\+([0-9A-F]{4})", label)
            special = chr(int(code, 16))
            self.assertIn(f"{unicodedata.name(special).lower()} U+{code}", label)
            self.assertTrue(has_glyph(self.LIBERATION, special), label)
            # The paragraph's text, with each w:noBreakHyphen as U+2011.
            value = ""
            for element in case.iter():
                if element.tag == f"{W}t":
                    value += element.text
                elif element.tag == f"{W}noBreakHyphen":
                    value += "\u2011"
            first, rest = value.rsplit(" information ", 1)
            self.assertEqual(rest, "ends the case.")
            others = set(first) & {"\u00a0", "\u202f", "\u2007"}
            self.assertEqual(others, {special}, label)
            words = re.split(f"[ {special}\u2011]+", first)
            if "only " in label:
                # Line 1 has no other gaps than the special space.
                self.assertNotIn(" ", first)
                self.assertNotIn("\u2011", first)
                self.assertEqual(first.count(special), len(words) - 1)
                self.assertIn(f"between the {len(words)} words of line 1", label)
            elif "three " in label:
                (before,) = re.findall(r'three [^"]* after "(\w+)"', label)
                self.assertEqual(first.count(special * 3), 1)
                self.assertIn(f"{before}{special * 3}", first)
                self.assertNotIn(f"{before}{special * 4}", first)
                self.assertEqual(first.count("\u2011"), 1)
                self.assertIn(f"\u2011{special}", first)
                self.assertEqual(first.count(special), 4)
                self.assertIn("one after a non-breaking hyphen", label)
            else:
                (before,) = re.findall(r'one [^"]* after "(\w+)"', label)
                self.assertEqual(first.count(special), 1)
                self.assertIn(f"{before}{special}", first)
                self.assertNotIn("\u2011", first)
            seen.append((special, "only" if "only " in label else
                         "three" if "three " in label else "one"))
            self.assertEqual(len(words), 14, label)
            # Line 1 is about 30 pt narrower than the text, so it is widened; "information"
            # does not fit even with the line's spaces narrowed by a fifth.
            width = carlito_width(first.replace("\u2011", "-"), font=self.LIBERATION)
            self.assertAlmostEqual(self.TEXT - width, 30, delta=0.5, msg=label)
            over = width + space + carlito_width("information", font=self.LIBERATION) - self.TEXT
            self.assertGreater(over, (first.count(" ") + 1) * space / 5, label)
        self.assertEqual(sorted(seen), sorted((s, k) for s in ("\u00a0", "\u202f", "\u2007")
                                              for k in ("one", "three", "only")))

    def test_justify_narrow_cases_need_the_narrowing_their_labels_name(self):
        space = carlito_width(" ", font=self.LIBERATION)
        # #497: the same lines in every compatibility mode, 11 to 15.
        twins = {"E-justify-narrow": ["15"], "E-justify-narrow-14": ["14"],
                 "E-justify-narrow-12": [], "E-justify-narrow-11": ["11"]}
        for name, mode in twins.items():
            self.assertEqual(self.load(name)[2], mode)
        self.assert_twins(list(twins))
        cases = self.justified("E-justify-narrow")
        self.assertEqual(len(cases), 21)
        found = []
        pattern = (r'Case (\d\d): (\d+) spaces narrowed (\d+\.\d) % hold "(\w+)", '
                   r"(\d+\.\d) % of it past the margin")
        for n, (label, case) in enumerate(cases, 1):
            (number, count, share, word, past), = re.findall(pattern, label)
            self.assertEqual(int(number), n)
            value = self.texts(case)
            tail = f" {word} and the second line ends the case."
            self.assertTrue(value.endswith(tail), label)
            first = value[:-len(tail)]
            words = first.split(" ")
            self.assertEqual(len(words), int(count), label)
            self.assertNotIn(word, words)
            width = carlito_width(first, font=self.LIBERATION)
            word_width = carlito_width(word, font=self.LIBERATION)
            # The line fits; with the word, at the spaces' natural width, it is too wide by
            # the share of its spaces (the one before the word too) and of the word the
            # label names.
            self.assertLess(width, self.TEXT)
            over = width + space + word_width - self.TEXT
            self.assertGreater(over, 0)
            self.assertAlmostEqual(100 * over / (int(count) * space), float(share), delta=0.051,
                                   msg=label)
            self.assertAlmostEqual(100 * over / word_width, float(past), delta=0.051, msg=label)
            found.append((int(count), float(share), word, float(past)))
        # Cases 01 to 05: about 5, 10, 20, 25 and 30 % of 14 spaces, "information" less than
        # a quarter past the margin.
        for (count, share, word, past), target in zip(found[:5], (5, 10, 20, 25, 30)):
            self.assertEqual((count, word), (14, "information"))
            self.assertAlmostEqual(share, target, delta=0.75)
            self.assertLess(past, 25)
        # Cases 06 to 14: about 10 and 20 % of 14 spaces, words from a quarter to seven tenths
        # past the margin, wider words less far.
        for group, target in ((found[5:10], 10), (found[10:14], 20)):
            self.assertEqual({count for count, *_ in group}, {14})
            for _, share, _, _ in group:
                self.assertAlmostEqual(share, target, delta=0.5)
            pasts = [past for *_, past in group]
            self.assertEqual(pasts, sorted(pasts))
            self.assertLess(pasts[0], 30)
            self.assertGreater(pasts[-1], 65)
        # Cases 15 to 21: "time" about 30 % and "the" about half past the margin, with fewer
        # and fewer spaces.
        for group, word, target, counts in ((found[14:18], "time", 29.5, [20, 10, 7, 5]),
                                            (found[18:], "the", 50, [25, 12, 8])):
            self.assertEqual([count for count, *_ in group], counts)
            self.assertEqual({w for _, _, w, _ in group}, {word})
            for *_, past in group:
                self.assertAlmostEqual(past, target, delta=2.5)
            shares = [share for _, share, _, _ in group]
            self.assertEqual(shares, sorted(shares))
        self.assertLess(found[14][1], 11)
        self.assertGreater(found[17][1], 35)

    def test_endnote_numbering_settings_has_the_markup_its_labels_name(self):
        parts, body, _ = self.load("E-endnote-numbering-settings")
        settings = ElementTree.fromstring(parts["word/settings.xml"])
        props = [(e.tag[len(W):], e.get(f"{W}val")) for e in settings.find(f"{W}endnotePr")
                 if e.tag != f"{W}endnote"]
        # #587: upper Roman from 3, restarting in each section, at the end of the document.
        self.assertEqual(props, [("pos", "docEnd"), ("numFmt", "upperRoman"), ("numStart", "3"),
                                 ("numRestart", "eachSect")])
        self.assertEqual(list(settings.find(f"{W}footnotePr")),
                         [e for e in settings.find(f"{W}footnotePr") if e.tag == f"{W}footnote"])
        paragraphs = body.findall(f"{W}p")
        sects = [p.find(f"{W}pPr/{W}sectPr") for p in paragraphs]
        self.assertEqual([s is not None for s in sects], [False, False, True, False, False, False])
        first, last = sects[2], body.find(f"{W}sectPr")
        self.assertIsNone(first.find(f"{W}endnotePr"))
        self.assertEqual([(e.tag[len(W):], e.get(f"{W}val")) for e in last.find(f"{W}endnotePr")],
                         [("numFmt", "lowerLetter")])
        self.assertEqual(self.texts(paragraphs[0]), "Section 1: no w:endnotePr in the section")
        self.assertEqual(self.texts(paragraphs[3]),
                         "Section 2: lowerLetter in the section's w:endnotePr")
        refs = [[r.get(f"{W}id") for r in p.iter(f"{W}endnoteReference")] for p in paragraphs]
        self.assertEqual(refs, [[], ["1"], ["2"], [], ["3"], ["4"]])
        self.assertEqual(list(body.iter(f"{W}footnoteReference")), [])

    def test_table_vmerge_heights_cases_have_the_merges_their_labels_name(self):
        for name, mode in (("E-table-vmerge-heights", ["15"]),
                           ("E-table-vmerge-heights-14", ["14"])):
            parts, _, found = self.load(name)
            self.assertEqual(found, mode)
            styles = ElementTree.fromstring(parts["word/styles.xml"])
            fonts = styles.find(f"{W}docDefaults/{W}rPrDefault/{W}rPr/{W}rFonts").attrib
            self.assertEqual(set(fonts.values()), {"Liberation Sans"})
        self.assert_twins(["E-table-vmerge-heights", "E-table-vmerge-heights-14"])
        _, body, _ = self.load("E-table-vmerge-heights")
        children = [e for e in body if e.tag != f"{W}sectPr"]
        self.assertEqual(self.texts(children[-1]), "End of the document.")
        pairs = list(zip(children[:-1:2], children[1:-1:2]))
        self.assertEqual(len(pairs), 15)

        def merge(tc):
            element = tc.find(f"{W}tcPr/{W}vMerge")
            if element is None:
                return "own"
            return "restart" if element.get(f"{W}val") == "restart" else "on"

        def lines(tc):
            return sum(self.segments(p)[0] for p in tc.findall(f"{W}p") if self.texts(p))

        def grid(table):
            return [[(merge(tc), lines(tc)) for tc in tr.findall(f"{W}tc")]
                    for tr in table.findall(f"{W}tr")]

        def heights(table):
            return [(h.get(f"{W}val"), h.get(f"{W}hRule")) if h is not None else None
                    for h in (tr.find(f"{W}trPr/{W}trHeight") for tr in table.findall(f"{W}tr"))]

        def merged_only(m, own):
            return [[("restart", 1), ("own", 1), ("own", 1)],
                    [("on", 0), ("restart", m), ("restart", 1)],
                    [("own", own), ("on", 0), ("on", 0)],
                    [("own", 1)] * 3]

        tall = [[("own", 1)] * 3, [("restart", 5), ("own", 1), ("own", 1)],
                [("on", 0), ("own", 1), ("own", 1)], [("own", 1)] * 3]
        at_least, exact = ("400", "atLeast"), ("400", "exact")
        expected = [
            *((f"row 2 holds only merged cells, its merge {m} line{'s' * (m > 1)}; row 3's own "
               f"cell {own} line{'s' * (own > 1)}", merged_only(m, own), [None] * 4)
              for own in (1, 3) for m in (1, 2, 3)),
            ("as case 02, the merge's cell with 4 pt top and bottom margins (w:tcMar)",
             merged_only(2, 1), [None] * 4),
            ("as case 02, the merge's text with 6 pt space before", merged_only(2, 1), [None] * 4),
            ("a merge of 5 lines over rows 2 and 3, which have cells of 1 line of their own",
             tall, [None] * 4),
            ("row 2 holds only the rest of three merges of 1 line from row 1",
             [[("restart", 1)] * 3, [("on", 0)] * 3, [("own", 1)] * 3], [None] * 3),
            ("row 2 holds only the rest of three merges from row 1, the first of 3 lines",
             [[("restart", 3), ("restart", 1), ("restart", 1)], [("on", 0)] * 3,
              [("own", 1)] * 3], [None] * 3),
            ("as case 03, row 2 at least 20 pt (w:trHeight)", merged_only(3, 1),
             [None, at_least, None, None]),
            ("as case 03, row 2 exactly 20 pt (w:trHeight)", merged_only(3, 1),
             [None, exact, None, None]),
            ("as case 09, rows 2 and 3 at least 20 pt (w:trHeight)", tall,
             [None, at_least, at_least, None]),
            ("as case 09, rows 2 and 3 exactly 20 pt (w:trHeight)", tall,
             [None, exact, exact, None]),
        ]
        for n, ((label, table), (words, shape, rows)) in enumerate(zip(pairs, expected), 1):
            self.assertEqual(self.texts(label), f"Case {n:02d}: {words}")
            self.assert_one_line(self.texts(label), font=self.LIBERATION)
            self.assertEqual(grid(table), shape, n)
            self.assertEqual(heights(table), rows, n)
            # Every continuation has a merge above it in its column.
            columns = list(zip(*shape))
            for column in columns:
                for above, below in zip(column, column[1:]):
                    if below[0] == "on":
                        self.assertIn(above[0], ("restart", "on"), n)
            margins = [tc.find(f"{W}tcPr/{W}tcMar") for tc in table.iter(f"{W}tc")]
            before = [p.find(f"{W}pPr/{W}spacing") for p in table.iter(f"{W}p")]
            second = table.findall(f"{W}tr")[1].findall(f"{W}tc")[1]
            if n == 7:
                (mar,) = [m for m in margins if m is not None]
                self.assertIs(mar, second.find(f"{W}tcPr/{W}tcMar"))
                self.assertEqual([(e.tag[len(W):], e.get(f"{W}w"), e.get(f"{W}type"))
                                  for e in mar], [("top", "80", "dxa"), ("bottom", "80", "dxa")])
            else:
                self.assertEqual([m for m in margins if m is not None], [], n)
            if n == 8:
                (space,) = [s for s in before if s is not None]
                self.assertIs(space, second.find(f"{W}p/{W}pPr/{W}spacing"))
                self.assertEqual(space.get(f"{W}before"), "120")
            else:
                self.assertEqual([s for s in before if s is not None], [], n)
        # Cases 07 and 12 start a page; no page holds more than its body, even if every row
        # starting a merge were as tall as the merge and every other row at least a line.
        starts = [k for k, (label, _) in enumerate(pairs, 1)
                  if self.has(label, f"{W}pPr/{W}pageBreakBefore")]
        self.assertEqual(starts, [7, 12])
        ascent, descent = win_metrics(self.LIBERATION, 11)
        line = ascent + descent
        for first, last in ((1, 6), (7, 11), (12, 15)):
            height = 0
            for label, table in pairs[first - 1:last]:
                height += 12 + line
                for tr in table.findall(f"{W}tr"):
                    tallest = line
                    for tc in tr.findall(f"{W}tc"):
                        extra = 8 if tc.find(f"{W}tcPr/{W}tcMar") is not None else 0
                        if tc.find(f"{W}p/{W}pPr/{W}spacing") is not None:
                            extra += 6
                        tallest = max(tallest, lines(tc) * line + extra)
                    rule = tr.find(f"{W}trPr/{W}trHeight")
                    if rule is not None:
                        least = int(rule.get(f"{W}val")) / 20
                        exact = rule.get(f"{W}hRule") == "exact"
                        tallest = least if exact else max(tallest, least)
                    height += tallest + 0.5
                height += 0.5
            self.assertLess(height, self.BODY, (first, last))

    def test_footnote_pages_section_runs_its_note_two_pages_past_the_text(self):
        parts, body, _ = self.load("E-footnote-pages-section")
        rels = {r.get("Id"): r.get("Target") for r in
                ElementTree.fromstring(parts["word/_rels/document.xml.rels"])}
        R = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"

        def stories(sect):
            return {(ref.tag[len(W):], ref.get(f"{W}type")):
                    self.texts(ElementTree.fromstring(parts["word/" + rels[ref.get(f"{R}id")]]))
                    for ref in sect if ref.tag in (f"{W}headerReference", f"{W}footerReference")}

        first, second = body.findall(f"{W}p")
        self.assertEqual(self.texts(first), "Section 1: one line")
        section_1 = first.find(f"{W}pPr/{W}sectPr")
        section_2 = body.find(f"{W}sectPr")
        self.assertEqual(stories(section_1), {
            ("headerReference", "default"): "Header of section 1, default",
            ("footerReference", "default"): "Footer of section 1, default: page 1 of 1, 1 in "
                                            "this section"})
        self.assertEqual(stories(section_2), {
            ("headerReference", "default"): "Header of section 2, default",
            ("headerReference", "first"): "Header of section 2, first page",
            ("footerReference", "default"): "Footer of section 2, default: page 1 of 1, 1 in "
                                            "this section",
            ("footerReference", "first"): "Footer of section 2, first page: page 1 of 1, 1 in "
                                          "this section"})
        for name, data in parts.items():
            if name.startswith("word/footer"):
                instructions = [f.get(f"{W}instr").strip() for f in
                                ElementTree.fromstring(data).iter(f"{W}fldSimple")]
                self.assertEqual(instructions, ["PAGE", "NUMPAGES", "SECTIONPAGES"])
        self.assertEqual(section_2.find(f"{W}type").get(f"{W}val"), "continuous")
        self.assertIsNotNone(section_2.find(f"{W}titlePg"))
        self.assertIsNone(section_1.find(f"{W}titlePg"))
        label = self.texts(second)
        self.assertEqual(label, "Section 2: continuous, a title page; this line references a "
                         "note of 12 lines of 120 pt")
        self.assert_one_line(label)
        (ref,) = [r.get(f"{W}id") for r in second.iter(f"{W}footnoteReference")]
        notes = {e.get(f"{W}id"): e for e in ElementTree.fromstring(
            parts["word/footnotes.xml"]).findall(f"{W}footnote")}
        (p,) = notes[ref].findall(f"{W}p")
        self.assertEqual(self.segments(p), [12])
        self.assertEqual(self.line_rule(p), ("exact", 2400))
        # Below the two lines and the separator page 1 holds five lines of the note, each
        # later page five below the continuation separator: the last two are on page 3.
        line = carlito_line_height(11)
        on_first = int((self.BODY - 3 * line) // 120)
        on_later = int((self.BODY - line) // 120)
        self.assertEqual((on_first, on_later), (5, 5))
        self.assertEqual(12 - on_first - on_later, 2)

    # Number formats (#546).

    def number_sections(self, name):
        """Per section: its label, w:pgNumType start and format, and its fields as (the label
        before the field, the field's instruction, the field's languages)."""
        _, body, _ = self.load(name)
        found, current = [], None
        for p in body.findall(f"{W}p"):
            if current is None:
                current = [self.texts(p), []]
            else:
                runs = list(p)
                label = self.texts(runs[1])
                simple = p.find(f"{W}fldSimple")
                if simple is not None:
                    instr, langs = simple.get(f"{W}instr").strip(), set()
                else:
                    instr = self.texts_of(p, "instrText").strip()
                    langs = {r.find(f"{W}rPr/{W}lang").get(f"{W}val")
                             for r in p.findall(f"{W}r")[1:]}
                current[1].append((label, instr, langs))
            sect = p.find(f"{W}pPr/{W}sectPr")
            if sect is not None or p is body.findall(f"{W}p")[-1]:
                sect = sect if sect is not None else body.find(f"{W}sectPr")
                numbers = sect.find(f"{W}pgNumType")
                found.append((current[0], numbers.get(f"{W}start"), numbers.get(f"{W}fmt"),
                              current[1]))
                current = None
        return found

    @staticmethod
    def texts_of(element, tag):
        return "".join(t.text or "" for t in element.iter(f"{W}{tag}"))

    def test_number_formats_limits_cases_have_the_fields_their_labels_name(self):
        sections = self.number_sections("E-number-formats-limits")
        words = ["CardText", "OrdText", "DollarText", "Arabic"]
        expected = ([(999999, None, words), (1000000, None, words)]
                    + [(s, None, ["Hex", "Arabic"]) for s in (32767, 32768, 65536)]
                    + [(s, "chicago", [None, "Arabic"])
                       for s in (118, 120, 121, 200, 256, 257, 1000)]
                    + [(s, None, ["ROMAN", "Arabic"]) for s in (64999, 65000, 100000)]
                    + [(27, None, ["CardText \\* Caps", "OrdText \\* Upper", "Arabic"])])
        self.assertEqual(len(sections), len(expected))
        for n, ((label, start, fmt, fields), (number, page_format, switches)) in enumerate(
                zip(sections, expected), 1):
            what = "Chicago page numbers" if page_format else "page numbers"
            self.assertEqual(label, f"Case {n:02d}: {what} restarted at {number}")
            self.assertEqual((start, fmt), (str(number), page_format))
            instrs = ["PAGE" if s is None else f"PAGE \\* {s}" for s in switches]
            self.assertEqual([(lbl, i) for lbl, i, _ in fields], [(f"{i}: ", i) for i in instrs])

    def test_number_formats_languages_cases_have_the_languages_their_labels_name(self):
        sections = self.number_sections("E-number-formats-languages")
        starts = [0, 1, 3, 7, 8, 16, 17, 21, 100, 101, 1000, 1001, 1665, 999999]
        self.assertEqual(len(sections), len(starts) + 1)
        for n, ((label, start, fmt, fields), number) in enumerate(zip(sections, starts), 1):
            self.assertEqual(label, f"Case {n:02d}: page numbers restarted at {number}")
            self.assertEqual((start, fmt), (str(number), None))
            self.assertEqual(fields, [
                (f"PAGE \\* {s} in German: ", f"PAGE \\* {s}", {"de-DE"})
                for s in ("CardText", "OrdText", "Ordinal")] + [
                ("PAGE \\* Arabic: ", "PAGE \\* Arabic", set())])
        label, start, fmt, fields = sections[-1]
        self.assertEqual(label, f"Case {len(starts) + 1:02d}: page numbers restarted at 27")
        self.assertEqual((start, fmt), ("27", None))
        expected = [("German", "de-DE", "DollarText")] + [
            (name, lang, s) for name, lang in (("French", "fr-FR"), ("Spanish", "es-ES"),
                                               ("Russian", "ru-RU"))
            for s in ("CardText", "Ordinal")]
        self.assertEqual(fields, [(f"PAGE \\* {s} in {name}: ", f"PAGE \\* {s}", {lang})
                                  for name, lang, s in expected])

    def test_number_formats_labels_cases_have_the_numbering_their_labels_name(self):
        parts, body, _ = self.load("E-number-formats-labels")
        numbering = ElementTree.fromstring(parts["word/numbering.xml"])
        abstracts = {a.get(f"{W}abstractNumId"): a for a in numbering.findall(f"{W}abstractNum")}
        nums = {n.get(f"{W}numId"): abstracts[n.find(f"{W}abstractNumId").get(f"{W}val")]
                for n in numbering.findall(f"{W}num")}
        lists = [
            # (format, start, items, the language of the runs, of the mark)
            ("lowerLetter", 0, 3, None, None), ("lowerLetter", 781, 3, None, None),
            ("upperRoman", 0, 3, None, None), ("upperRoman", 781, 3, None, None),
            ("cardinalText", 0, 3, None, None), ("cardinalText", 781, 3, None, None),
            ("chicago", 116, 3, None, None),
            ("ordinal", 27, 2, "de-DE", "de-DE"), ("ordinal", 27, 2, "de-DE", "en-US"),
            ("cardinalText", 27, 2, "de-DE", "de-DE"), ("cardinalText", 27, 2, "de-DE", "en-US"),
            ("ordinalText", 27, 2, "de-DE", "de-DE"), ("ordinalText", 27, 2, "de-DE", "en-US"),
        ]
        paragraphs = body.findall(f"{W}p")
        at = 0
        for n, (fmt, start, items, lang, mark) in enumerate(lists, 1):
            label = self.texts(paragraphs[at])
            where = f", its runs in {lang} and its mark in {mark}" if lang else ""
            self.assertEqual(label, f"Case {n:02d}: a list in {fmt} from w:start {start}, "
                             f"{items} items{where}")
            self.assert_one_line(label)
            self.assertIsNone(paragraphs[at].find(f"{W}pPr/{W}numPr"))
            ids = set()
            for p in paragraphs[at + 1:at + 1 + items]:
                ids.add(p.find(f"{W}pPr/{W}numPr/{W}numId").get(f"{W}val"))
                self.assertEqual(p.find(f"{W}pPr/{W}numPr/{W}ilvl").get(f"{W}val"), "0")
                langs = {r.find(f"{W}rPr/{W}lang").get(f"{W}val") if r.find(f"{W}rPr/{W}lang")
                         is not None else None for r in p.findall(f"{W}r")}
                self.assertEqual(langs, {lang})
                found = p.find(f"{W}pPr/{W}rPr/{W}lang")
                self.assertEqual(None if found is None else found.get(f"{W}val"), mark)
            (num_id,) = ids
            (lvl,) = nums[num_id].findall(f"{W}lvl")
            self.assertEqual((lvl.find(f"{W}numFmt").get(f"{W}val"),
                              lvl.find(f"{W}start").get(f"{W}val"),
                              lvl.find(f"{W}lvlText").get(f"{W}val")), (fmt, str(start), "%1."))
            at += 1 + items
        # Then the footnote sections, each numbered by its own w:footnotePr.
        self.assertEqual(self.texts(paragraphs[at]), "End of the lists")
        self.assertIsNotNone(paragraphs[at].find(f"{W}pPr/{W}sectPr"))
        notes = [("lowerLetter", 781, 3, None), ("lowerLetter", 0, 2, None),
                 ("upperRoman", 0, 2, None), ("chicago", 116, 3, None),
                 ("cardinalText", 27, 2, "de-DE")]
        rest = paragraphs[at + 1:]
        sects = [p.find(f"{W}pPr/{W}sectPr") for p in rest][:-1] + [body.find(f"{W}sectPr")]
        k, first = 0, 1
        for n, (fmt, start, count, lang) in enumerate(notes, len(lists) + 1):
            where = f", the references in {lang}" if lang else ""
            self.assertEqual(self.texts(rest[k]),
                             f"Case {n:02d}: footnotes in {fmt} from w:numStart {start}{where}")
            refs = rest[k + 1:k + 1 + count]
            self.assertEqual([[r.get(f"{W}id") for r in p.iter(f"{W}footnoteReference")]
                              for p in refs], [[str(first + j)] for j in range(count)])
            for p in refs:
                run = next(r for r in p.findall(f"{W}r") if r.find(f"{W}footnoteReference")
                           is not None)
                found = run.find(f"{W}rPr/{W}lang")
                self.assertEqual(None if found is None else found.get(f"{W}val"), lang)
            sect = sects[k + count]
            self.assertEqual(all(s is None for s in sects[k:k + count]), True)
            props = [(e.tag[len(W):], e.get(f"{W}val")) for e in sect.find(f"{W}footnotePr")]
            self.assertEqual(props, [("numFmt", fmt), ("numStart", str(start)),
                                     ("numRestart", "eachSect")])
            k += 1 + count
            first += count
        self.assertEqual(k, len(rest))

    def test_highlight_spacing_cases_have_the_marks_their_labels_name(self):
        parts, body, _ = self.load("E-highlight-spacing")
        paragraphs = body.findall(f"{W}p")
        self.assertEqual(len(paragraphs), 26)
        # #346: Calibri 11 pt unless the label names a font or a size, which the document
        # defaults give and no style changes.
        styles = ElementTree.fromstring(parts["word/styles.xml"])
        defaults = styles.find(f"{W}docDefaults/{W}rPrDefault/{W}rPr")
        self.assertEqual({k[len(W):]: v for k, v in defaults.find(f"{W}rFonts").attrib.items()},
                         dict.fromkeys(("ascii", "hAnsi", "eastAsia", "cs"), "Calibri"))
        self.assertEqual([defaults.find(f"{W}{tag}").get(f"{W}val") for tag in ("sz", "szCs")],
                         ["22", "22"])
        for style in styles.findall(f"{W}style"):
            self.assertIsNone(style.find(f"{W}rPr"), style.get(f"{W}styleId"))
        marks = {"highlight yellow": ("highlight", "yellow"), "run shading D9D9D9": ("shd", "D9D9D9")}

        def mark_of(r):
            highlight = r.find(f"{W}rPr/{W}highlight")
            shading = r.find(f"{W}rPr/{W}shd")
            if highlight is not None:
                self.assertIsNone(shading)
                return "highlight", highlight.get(f"{W}val")
            if shading is not None:
                return "shd", shading.get(f"{W}fill")
            return None

        def value(r, tag, attribute="val"):
            element = r.find(f"{W}rPr/{W}{tag}")
            return None if element is None else element.get(f"{W}{attribute}")

        kinds, whats = [], []
        for n, p in enumerate(paragraphs, 1):
            label_run, *samples = p.findall(f"{W}r")
            label = self.texts(label_run)
            self.assertIsNone(mark_of(label_run))
            kind = next(k for k in marks if label.startswith(f"Case {n:02d}: {k}, "))
            kinds.append(kind)
            mark = marks[kind]
            what = label[len(f"Case {n:02d}: {kind}, "):-2]
            whats.append(what)
            found = [(mark_of(r) == mark, mark_of(r) is None) for r in samples]
            for marked, plain in found:
                self.assertTrue(marked or plain, label)
            marked = [r for r in samples if mark_of(r) == mark]
            plain = [r for r in samples if mark_of(r) is None]
            line = p.find(f"{W}pPr/{W}spacing")
            rule = None if line is None else (line.get(f"{W}lineRule"), line.get(f"{W}line"))
            if what.startswith("two lines spaced "):
                factor = what[len("two lines spaced "):-len(" lines")]
                (r,) = samples
                self.assertEqual(r, marked[0])
                self.assertEqual(len(r.findall(f"{W}br")), 1)
                self.assertEqual(rule, ("auto", str(round(float(factor) * 240))))
            elif what == "at least 24 pt spacing":
                self.assertEqual(rule, ("atLeast", "480"))
                self.assertEqual(len(marked), 1)
            elif what == "a 6 pt run between plain 11 pt runs":
                self.assertEqual([mark_of(r) is None for r in samples], [True, False, True])
                self.assertEqual([value(r, "sz") for r in samples], [None, "12", None])
            elif what == "an 11 pt run beside a plain 20 pt run":
                self.assertEqual([value(r, "sz") for r in marked + plain], [None, "40"])
            elif what == "an 11 pt run, the paragraph mark 20 pt":
                self.assertEqual(p.find(f"{W}pPr/{W}rPr/{W}sz").get(f"{W}val"), "40")
                self.assertEqual([value(r, "sz") for r in samples], [None])
            elif what.endswith(" 12 pt"):
                (r,) = marked
                self.assertEqual(value(r, "rFonts", "ascii"), what[:-len(" 12 pt")])
                self.assertEqual(value(r, "sz"), "24")
            elif what == "lowered 6 pt (w:position -12)":
                self.assertEqual([value(r, "position") for r in marked], ["-12"])
                self.assertEqual([value(r, "position") for r in plain], [None])
            elif what == "beside a plain run raised 6 pt (w:position 12)":
                self.assertEqual([value(r, "position") for r in marked], [None])
                self.assertEqual([value(r, "position") for r in plain], ["12"])
            elif what == "a tab raised 6 pt (w:position 12)":
                self.assertEqual(plain, [])
                tabs = [r for r in samples if r.find(f"{W}tab") is not None]
                self.assertEqual(samples.index(tabs[0]), 1)
                self.assertEqual([value(r, "position") for r in samples], [None, "12", None])
            else:
                self.fail(f"no check for {label}")
            if not what.startswith(("two lines", "at least")):
                self.assertIsNone(rule, label)
            if what != "an 11 pt run, the paragraph mark 20 pt":
                self.assertIsNone(p.find(f"{W}pPr/{W}rPr"), label)
            self.assertEqual(len(marked) >= 1, True, label)
            # Only the font cases name a font, and only they and the 6 pt and 20 pt runs a
            # size; every other run takes the defaults' Calibri 11 pt.
            fonts = what.endswith(" 12 pt")
            for r in samples:
                self.assertEqual(r.find(f"{W}rPr/{W}rFonts") is not None, fonts, label)
                self.assertEqual(value(r, "sz"), value(r, "szCs"), label)
            sizes = [value(r, "sz") for r in samples]
            named = {"a 6 pt run between plain 11 pt runs": [None, "12", None],
                     "an 11 pt run beside a plain 20 pt run": [None, "40"]}
            self.assertEqual(sizes, ["24"] if fonts else named.get(what, [None] * len(samples)),
                             label)
            # The first line, with room for the tab.
            self.assert_one_line(self.first_line(p) + " " * 12)
        # Each case once with each mark, the highlight first.
        self.assertEqual(kinds, ["highlight yellow", "run shading D9D9D9"] * 13)
        # The cases #346 asks for: its cases 1 to 6 and those of its comment of 2026-10-19.
        expected = [
            "two lines spaced 1.15 lines", "two lines spaced 1.5 lines",
            "two lines spaced 2 lines", "at least 24 pt spacing",
            "a 6 pt run between plain 11 pt runs", "an 11 pt run beside a plain 20 pt run",
            "an 11 pt run, the paragraph mark 20 pt", "Times New Roman 12 pt", "Arial 12 pt",
            "Cambria 12 pt", "lowered 6 pt (w:position -12)",
            "beside a plain run raised 6 pt (w:position 12)", "a tab raised 6 pt (w:position 12)",
        ]
        self.assertEqual(whats[::2], expected)
        self.assertEqual(whats[1::2], expected)


# Batch 14, part B: the relationships of headers, footers and notes, and the documents of
# floating and VML objects.

WP = "{http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing}"
DML = "{http://schemas.openxmlformats.org/drawingml/2006/main}"
PIC = "{http://schemas.openxmlformats.org/drawingml/2006/picture}"
WPS = "{http://schemas.microsoft.com/office/word/2010/wordprocessingShape}"
VML = "{urn:schemas-microsoft-com:vml}"
W10 = "{urn:schemas-microsoft-com:office:word}"
REL_ID = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
IMAGE = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image"
STORY = re.compile(r"word/(document|header\d+|footer\d+|footnotes|endnotes)\.xml")
EMU_PT = 12700
LIBERATION_SERIF = CARLITO.parents[1] / "liberation/LiberationSerif-Regular.ttf"
NOTO_TRUETYPE = CARLITO.parents[2] / "fonts-word/noto-truetype/NotoSans-Regular.ttf"
B14B = ("E-header-cut-first-picture", "E-anchor-page", "E-anchor-margin",
        "E-anchor-column-character", "E-anchor-paragraph-line", "E-anchor-inside-outside",
        "E-anchor-inside-outside-mirror", "E-anchor-stack", "E-anchor-in-cell",
        "E-anchor-effects", "E-anchor-header", "E-anchor-paragraph-spacing",
        "E-anchor-paragraph-header-note", "E-vml-anchor-relative", "E-vml-anchor-align",
        "E-vml-stack", "E-vml-crop-rotate", "E-vml-px-effects", "E-textbox-vml-inline-shapes",
        "E-hidden-drawings")


def b14_boxed(element):
    """The elements inside the text boxes (w:txbxContent) an element holds."""
    return {id(e) for box in element.iter(f"{W}txbxContent") for e in box.iter()}


def b14_text(element):
    """The text of an element, without that of the text boxes it holds."""
    boxed = b14_boxed(element)
    return "".join(t.text or "" for t in element.iter(f"{W}t") if id(t) not in boxed)


def b14_lines(paragraph):
    """The texts of a paragraph between its line breaks, and the drawings (wp:anchor,
    wp:inline, v:shape) of each line; the text boxes' own text and breaks left out."""
    found = [["", []]]
    boxed = b14_boxed(paragraph)
    for element in paragraph.iter():
        if id(element) in boxed:
            continue
        if element.tag == f"{W}t":
            found[-1][0] += element.text or ""
        elif element.tag == f"{W}br":
            found.append(["", []])
        elif element.tag in (f"{WP}anchor", f"{WP}inline", f"{VML}shape"):
            found[-1][1].append(element)
    return found


def b14_anchor(anchor):
    """What the markup of a wp:anchor says: its positions across and down as (relativeFrom,
    offset in points or the wp:align value), its size and effect extent in points, its layer,
    relativeHeight, layoutInCell, wrapping, whether its docPr is hidden and its picture."""
    def place(tag):
        position = anchor.find(f"{WP}{tag}")
        offset = position.find(f"{WP}posOffset")
        value = (int(offset.text) / EMU_PT if offset is not None
                 else position.find(f"{WP}align").text)
        return position.get("relativeFrom"), value

    extent, effects = anchor.find(f"{WP}extent"), anchor.find(f"{WP}effectExtent")
    blip = anchor.find(f".//{DML}blip")
    return {
        "across": place("positionH"), "down": place("positionV"),
        "size": (int(extent.get("cx")) / EMU_PT, int(extent.get("cy")) / EMU_PT),
        "effects": tuple(int(effects.get(side)) / EMU_PT for side in "ltrb"),
        "behind": anchor.get("behindDoc") == "1",
        "height": int(anchor.get("relativeHeight")),
        "in_cell": anchor.get("layoutInCell"),
        "wrap": next(c.tag[len(WP):] for c in anchor if c.tag[len(WP):].startswith("wrap")),
        "hidden": anchor.find(f"{WP}docPr").get("hidden") == "1",
        "picture": None if blip is None else blip.get(f"{REL_ID}embed"),
    }


def b14_css(shape):
    """The CSS properties of a VML shape's style."""
    return dict(item.split(":", 1) for item in shape.get("style").split(";"))


def b14_pages(body):
    """The body's paragraphs with the number of the page each starts on: a page break before
    a paragraph (w:pageBreakBefore), a w:br of type page in the paragraph before or a section
    break ending it starts a new page."""
    page, pending, out = 1, False, []
    for p in body.findall(f"{W}p"):
        if out and (pending or p.find(f"{W}pPr/{W}pageBreakBefore") is not None):
            page += 1
        out.append((page, p))
        pending = (any(br.get(f"{W}type") == "page" for br in p.iter(f"{W}br"))
                   or p.find(f"{W}pPr/{W}sectPr") is not None)
    return out


def b14_geometry(sect):
    """The page width and height and the margins left, right, top and bottom of a w:sectPr,
    in points, the gutter added to the left margin."""
    size, mar = sect.find(f"{W}pgSz"), sect.find(f"{W}pgMar")
    left = int(mar.get(f"{W}left")) + int(mar.get(f"{W}gutter"))
    return (int(size.get(f"{W}w")) / 20, int(size.get(f"{W}h")) / 20, left / 20,
            int(mar.get(f"{W}right")) / 20, int(mar.get(f"{W}top")) / 20,
            int(mar.get(f"{W}bottom")) / 20)


def b14_frame(facts, sect):
    """Where ECMA-376 places a picture positioned against the page, the margins or a margin
    area (§20.4.3.4 and §20.4.3.5): its left and top edges, in points from the page's."""
    width, height, left, right, top, bottom = b14_geometry(sect)
    areas = {"page": ((0, width), (0, height)), "margin": ((left, width - right),
                                                            (top, height - bottom)),
             "leftMargin": ((0, left), None), "rightMargin": ((width - right, width), None),
             "topMargin": (None, (0, top)), "bottomMargin": (None, (height - bottom, height))}
    edges = []
    for axis, (relative, value) in enumerate((facts["across"], facts["down"])):
        low, high = areas[relative][axis]
        size = facts["size"][axis]
        if not isinstance(value, str):
            edges.append(low + value)
        elif value in ("left", "top"):
            edges.append(low)
        elif value == "center":
            edges.append(low + (high - low - size) / 2)
        else:
            edges.append(high - size)
    return tuple(edges)


class PartRelationships(unittest.TestCase):
    """The image relationships of the body, the headers, the footers and the notes: each
    part's own relationships part (ECMA-376 Part 2, §9.3), one image relationship for each
    picture its markup names."""

    def setUp(self):
        self.docs = rules.documents()

    def test_every_story_has_a_relationship_for_each_picture_and_no_other(self):
        seen = set()
        for doc in self.docs:
            parts = dict(rules.parts(doc))
            types = ElementTree.fromstring(parts["[Content_Types].xml"])
            defaults = {e.get("Extension"): e.get("ContentType")
                        for e in types.iter(f"{CT}Default")}
            media = {f"rId{name.rsplit('.', 1)[0]}": name for name, _ in doc.media}
            for name in parts:
                if not STORY.fullmatch(name):
                    continue
                story = parts[name].decode()
                named = set(re.findall(r'<a:blip r:embed="([^"]+)"', story))
                named |= set(re.findall(r'<v:imagedata r:id="([^"]+)"', story))
                rels_name = f"word/_rels/{name.removeprefix('word/')}.rels"
                if rels_name not in parts:
                    # Only the body always has relationships; a story without pictures has none.
                    self.assertEqual(named, set(), f"{doc.name}: {name}")
                    continue
                rels = list(ElementTree.fromstring(parts[rels_name]).iter(f"{PR}Relationship"))
                ids = [rel.get("Id") for rel in rels]
                self.assertEqual(len(ids), len(set(ids)), f"{doc.name}: {rels_name}")
                images = {rel.get("Id"): rel.get("Target") for rel in rels
                          if rel.get("Type") == IMAGE}
                # Exactly the pictures the story names, each to its media part, whose
                # extension has a content type of an image.
                self.assertEqual(set(images), named, f"{doc.name}: {name}")
                for rel_id, target in images.items():
                    self.assertEqual(target, f"media/{media[rel_id]}", f"{doc.name}: {name}")
                    self.assertIn(f"word/{target}", parts, f"{doc.name}: {name}")
                    self.assertTrue(defaults[target.rsplit(".", 1)[1]].startswith("image/"))
                if name != "word/document.xml":
                    self.assertEqual(len(images), len(rels), f"{doc.name}: {rels_name}")
                    self.assertTrue(images, f"{doc.name}: {rels_name} is empty")
                    seen.add((doc.name, name))
            # Every relationships part of a story belongs to a part of the package.
            for name in parts:
                if name.startswith("word/_rels/"):
                    source = "word/" + name.removeprefix("word/_rels/").removesuffix(".rels")
                    self.assertIn(source, parts, f"{doc.name}: {name}")
        self.assertTrue({("E-header-cut-first-picture", "word/header1.xml"),
                         ("E-header-cut-first-picture", "word/footer1.xml"),
                         ("E-anchor-header", "word/header1.xml"),
                         ("E-anchor-paragraph-header-note", "word/header1.xml"),
                         ("E-anchor-paragraph-header-note", "word/footnotes.xml")} <= seen, seen)

    def test_pictures_are_read_whatever_the_order_of_their_attributes(self):
        markup = ('<a:blip r:embed="rIdimage2"/><v:imagedata o:title="" r:id="rIdimage1"/>'
                  '<a:blip r:embed="rIdimage2"/><v:imagedata r:id="rIdimage3"/>')
        self.assertEqual(rules.picture_ids(markup), ["rIdimage2", "rIdimage1", "rIdimage3"])
        self.assertEqual(rules.picture_ids('<a:blipFill r:embed="rIdimage1"/>'), [])

    def test_refuses_a_picture_the_document_does_not_have(self):
        doc = next(doc for doc in self.docs if doc.name == "E-anchor-header")
        stray = dataclasses.replace(doc, headers=tuple(
            (name, root.replace('r:embed="rIdimage1"', 'r:embed="rIdimage9"', 1))
            for name, root in doc.headers))
        with self.assertRaisesRegex(ValueError, "names a picture rIdimage9 the document"):
            rules.parts(stray)

    def test_drawing_ids_are_unique_in_every_story_and_note(self):
        # Word refuses drawings that share a docPr id, in any part; and VML shapes that share
        # an id in one document.
        for doc in self.docs:
            parts = dict(rules.parts(doc))
            roots = [ElementTree.fromstring(data) for name, data in parts.items()
                     if STORY.fullmatch(name)]
            drawings = [e.get("id") for root in roots for e in root.iter(f"{WP}docPr")]
            self.assertEqual(len(drawings), len(set(drawings)), doc.name)
            shapes = [e.get("id") for root in roots for tag in ("shape", "rect", "roundrect")
                      for e in root.iter(f"{VML}{tag}")]
            self.assertEqual(len(shapes), len(set(shapes)), doc.name)


class FloatingObjectDocuments(unittest.TestCase):
    """The documents of batch 14 part B: every claim of a label or comment against the markup."""

    def setUp(self):
        self.docs = {doc.name: doc for doc in rules.documents()}
        self.bodies = {}

    def parts(self, name):
        return dict(rules.parts(self.docs[name]))

    def body(self, name):
        """The document's w:body, parsed once, so that its elements can be found again."""
        if name not in self.bodies:
            self.bodies[name] = ElementTree.fromstring(
                self.parts(name)["word/document.xml"]).find(f"{W}body")
        return self.bodies[name]

    def cases(self, name):
        """The body's case labels (paragraphs starting "Case NN"), by number, with the page
        each starts on and the paragraph."""
        found = {}
        for page, p in b14_pages(self.body(name)):
            match = re.match(r"Case (\d\d)[:,]", b14_text(p))
            if match:
                found[int(match.group(1))] = (page, p)
        self.assertEqual(sorted(found), list(range(1, len(found) + 1)), name)
        return found

    def test_labels_are_one_line(self):
        # Every case label, and every line of text beside a picture, fits on one line of its
        # section in Carlito, Calibri's twin, with 12 pt to spare.
        for name in B14B:
            body = self.body(name)
            sections = [p.find(f"{W}pPr/{W}sectPr") for p in body.findall(f"{W}p")]
            sections = [s for s in sections if s is not None] + [body.find(f"{W}sectPr")]
            widths = [b14_geometry(s) for s in sections]
            section = 0
            for p in body.findall(f"{W}p"):
                width, _, left, right, _, _ = widths[section]
                if p.find(f"{W}pPr/{W}ind") is not None:
                    left += int(p.find(f"{W}pPr/{W}ind").get(f"{W}left")) / 20
                sizes = {r.get(f"{W}val") for r in p.iter(f"{W}sz")}
                if b14_text(p).startswith("Case ") and sizes <= {"22"}:
                    for line, _ in b14_lines(p):
                        self.assertLess(carlito_width(line) + 12, width - left - right,
                                        f"{name}: {line}")
                if p.find(f"{W}pPr/{W}sectPr") is not None:
                    section += 1

    def test_header_cut_first_picture(self):
        name = "E-header-cut-first-picture"
        parts, body = self.parts(name), self.body(name)
        (label,) = [b14_text(p) for p in body.findall(f"{W}p")]
        sect = body.find(f"{W}sectPr")
        width, height, left, right, _, _ = b14_geometry(sect)
        self.assertLess(carlito_width(label) + 12, width - left - right, "one body line")
        tall = int(re.search(r"inline picture (\d+) pt tall", label).group(1))
        self.assertIn("then a 12 pt line", label)
        rels = {r.get("Id"): r.get("Target")
                for r in ElementTree.fromstring(parts["word/_rels/document.xml.rels"])}
        mar = sect.find(f"{W}pgMar")
        for kind in ("header", "footer"):
            (ref,) = [r for r in sect if r.tag == f"{W}{kind}Reference"]
            story = ElementTree.fromstring(parts["word/" + rels[ref.get(f"{REL_ID}id")]])
            first, second = story.findall(f"{W}p")
            (inline,) = first.iter(f"{WP}inline")
            self.assertEqual(b14_text(first), "", "the picture alone")
            self.assertIsNone(first.find(f"{W}pPr/{W}spacing"), "single spacing")
            extent = inline.find(f"{WP}extent")
            self.assertEqual(int(extent.get("cy")) / EMU_PT, tall)
            self.assertGreater(tall, 0.4 * height)
            spacing = second.find(f"{W}pPr/{W}spacing")
            self.assertEqual((spacing.get(f"{W}line"), spacing.get(f"{W}lineRule")),
                             ("240", "exact"))
            self.assertEqual(b14_text(second), f"{kind} line 2")
        # The comment's arithmetic: header and footer, each at least the picture's line and
        # 12 pt, overlap, so they leave the body no room.
        header_end = int(mar.get(f"{W}header")) / 20 + tall + 12
        footer_start = height - int(mar.get(f"{W}footer")) / 20 - tall - 12
        self.assertAlmostEqual(header_end, 447.4)
        self.assertAlmostEqual(footer_start, 394.5)
        self.assertGreater(header_end, footer_start)
        self.assertAlmostEqual(0.4 * height, 336.76)

    def check_paged(self, name, expected_pages=None):
        """The cases of a document of one case to a page: each label's paragraph holds its
        floating objects after an empty line of exact 100 pt; returns the cases' objects and
        the section of each."""
        body = self.body(name)
        paragraphs = body.findall(f"{W}p")
        sections = [p.find(f"{W}pPr/{W}sectPr") for p in paragraphs]
        ends = [i for i, s in enumerate(sections) if s is not None]
        out = {}
        for n, (page, p) in self.cases(name).items():
            index = paragraphs.index(p)
            section = next((sections[i] for i in ends if i >= index), body.find(f"{W}sectPr"))
            spacer = paragraphs[index - 1]
            line = spacer.find(f"{W}pPr/{W}spacing")
            self.assertEqual((b14_text(spacer), line.get(f"{W}line"), line.get(f"{W}lineRule")),
                             ("", "2000", "exact"), f"{name}: case {n}")
            out[n] = (page, p, section)
        if expected_pages is not None:
            self.assertEqual([out[n][0] for n in sorted(out)], expected_pages, name)
        return out

    def assert_wide_margins(self, name, sect):
        # A4 with margins of 72 pt on the left and at the top and of 144 pt on the right and at
        # the bottom, without a gutter: the centre of the margins is not the page's, and the
        # right and bottom margin areas are twice as wide as the left and top ones.
        width, height, left, right, top, bottom = b14_geometry(sect)
        self.assertEqual((width, height), (595.3, 841.9), name)
        self.assertEqual((left, right, top, bottom), (72, 144, 72, 144), name)
        self.assertEqual(sect.find(f"{W}pgMar").get(f"{W}gutter"), "0", name)

    def assert_label_clear(self, name, facts, sect, case):
        # No picture covers the label's line: the label lies 100 pt below the top margin, a
        # line of Calibri 11 pt tall.
        _, _, _, _, top, _ = b14_geometry(sect)
        label_top, label_bottom = top + 100, top + 100 + carlito_line_height(11)
        _, y = b14_frame(facts, sect)
        self.assertTrue(y + facts["size"][1] <= label_top or y >= label_bottom,
                        f"{name}: case {case} covers its label")

    def test_anchor_page(self):
        name = "E-anchor-page"
        cases = self.check_paged(name, [1, 2, 3, 4])
        expected = {1: (300, 400), 2: ("left", "top"), 3: ("center", "center"),
                    4: ("right", "bottom")}
        words = {1: "300 pt from the page's left edge and 400 pt below its top",
                 2: "aligned left and top", 3: "centred across and down",
                 4: "aligned right and bottom"}
        for n, (page, p, sect) in cases.items():
            (anchor,) = p.iter(f"{WP}anchor")
            facts = b14_anchor(anchor)
            self.assertEqual((facts["across"], facts["down"]),
                             (("page", expected[n][0]), ("page", expected[n][1])))
            self.assertIn(words[n], b14_text(p))
            self.assertEqual((facts["size"], facts["wrap"], facts["behind"]),
                             ((96, 48), "wrapNone", False))
            self.assert_label_clear(name, facts, sect, n)
        # The margins of 72 and 144 pt put the centre of the margins off the page's, as the
        # comment's numbers say.
        width, height, left, right, top, bottom = b14_geometry(cases[1][2])
        self.assertEqual((left, top, right, bottom), (72, 72, 144, 144))
        self.assertAlmostEqual(left + (width - left - right - 96) / 2, 213.65)
        self.assertAlmostEqual((width - 96) / 2, 249.65)
        self.assertAlmostEqual(top + (height - top - bottom - 48) / 2, 360.95)
        self.assertAlmostEqual((height - 48) / 2, 396.95)
        self.assertAlmostEqual(width - left - right, 379.3)
        self.assertAlmostEqual(height - top - bottom, 625.9)

    def test_anchor_margin(self):
        name = "E-anchor-margin"
        cases = self.check_paged(name, list(range(1, 13)))
        for n, (page, p, sect) in cases.items():
            self.assert_wide_margins(name, sect)
            label = b14_text(p)
            (anchor,) = p.iter(f"{WP}anchor")
            facts = b14_anchor(anchor)
            (across, x), (down, y) = facts["across"], facts["down"]
            size = re.search(r": (\d+) x (\d+) pt,", label)
            self.assertEqual(facts["size"], (int(size.group(1)), int(size.group(2))), label)
            if n <= 4:
                self.assertEqual((across, down), ("margin", "margin"), label)
                self.assertEqual(facts["size"], (96, 48))
            else:
                self.assertEqual((across, down), ("leftMargin", "topMargin") if n <= 8
                                 else ("rightMargin", "bottomMargin"), label)
                self.assertEqual(facts["size"], (48, 24))
                # A picture of 48 x 24 pt fits the narrower margin of 72 pt, every way it is
                # placed.
                frame = b14_frame(facts, sect)
                width, height, left, right, top, bottom = b14_geometry(sect)
                low = (0, 0) if n <= 8 else (width - right, height - bottom)
                high = (left, top) if n <= 8 else (width, height)
                for axis in (0, 1):
                    self.assertGreaterEqual(frame[axis], low[axis], label)
                    self.assertLessEqual(frame[axis] + facts["size"][axis], high[axis], label)
            if isinstance(x, str):
                if x == "center":
                    self.assertIn("centred", label)
                    self.assertEqual(y, "center", label)
                else:
                    self.assertRegex(label, rf"aligned {x}\b")
                    self.assertRegex(label, rf"\b{y}\b")
            else:
                self.assertRegex(label, rf"\b{x:g} pt across\b.* {y:g} pt down\b")
            if n > 4:
                self.assertIn(f" in {across}", label)
                self.assertIn(f" in {down}", label)
            self.assert_label_clear(name, facts, sect, n)

    def test_anchor_column_character(self):
        name = "E-anchor-column-character"
        line = carlito_line_height(11)
        indents = {}
        for n, (page, label_p) in self.cases(name).items():
            label = b14_text(label_p)
            self.assertIsNotNone(label_p.find(f"{W}pPr/{W}keepNext"))
            paragraphs = self.body(name).findall(f"{W}p")
            p = next(q for q in paragraphs[paragraphs.index(label_p) + 1:]
                     if list(q.iter(f"{WP}anchor")))
            (anchor,) = p.iter(f"{WP}anchor")
            facts = b14_anchor(anchor)
            relative, value = facts["across"]
            if isinstance(value, str):
                words = {"left": "aligned left against", "center": "centred on",
                         "right": "aligned right against"}[value]
                self.assertIn(f"{words} the {relative}", label)
            else:
                self.assertIn(f"{value:g} pt from the {relative}", label)
            # The anchor's run in the middle of the line: words before and after it.
            runs = p.findall(f"{W}r")
            at = next(k for k, r in enumerate(runs) if r.find(f".//{WP}anchor") is not None)
            self.assertTrue(0 < at < len(runs) - 1, label)
            self.assertTrue(b14_text(runs[at - 1]).strip() and b14_text(runs[at + 1]).strip())
            indent = p.find(f"{W}pPr/{W}ind")
            room = rules.TEXT_W / 20 - (0 if indent is None else int(indent.get(f"{W}left")) / 20)
            self.assertLess(carlito_width(b14_text(p)) + 12, room)
            self.assertEqual(indent is not None and indent.get(f"{W}left") == "1440",
                             "indented 72 pt" in label or "indented like case 06's" in label,
                             label)
            indents[n] = None if indent is None else dict(indent.attrib)
            # 18 pt below the paragraph's top: below its one line, within its 60 pt after.
            self.assertEqual(facts["down"], ("paragraph", 18))
            after = int(p.find(f"{W}pPr/{W}spacing").get(f"{W}after")) / 20
            self.assertEqual(after, 60)
            self.assertGreater(18, line)
            self.assertLessEqual(18 + facts["size"][1], line + after)
        self.assertEqual(len(self.cases(name)), 7)
        self.assertEqual(indents[7], indents[6])
        self.assertIsNotNone(indents[6])

    def test_anchor_paragraph_line(self):
        name = "E-anchor-paragraph-line"
        body = self.body(name)
        paragraphs = body.findall(f"{W}p")
        h = carlito_line_height(11)
        cases = self.cases(name)
        for n, (page, label_p) in cases.items():
            label = b14_text(label_p)
            at = paragraphs.index(label_p)
            if n == 10:
                continue
            spacer, p = paragraphs[at + 1], paragraphs[at + 2]
            self.assertEqual(spacer.find(f"{W}pPr/{W}spacing").get(f"{W}line"), "720")
            for kept in (label_p, spacer):
                self.assertIsNotNone(kept.find(f"{W}pPr/{W}keepNext"))
            self.assertIsNotNone(p.find(f"{W}pPr/{W}keepLines"))
            found = b14_lines(p)
            self.assertEqual(len(found), 5, label)
            (line_no,) = [k for k, (_, drawings) in enumerate(found, 1) if drawings]
            self.assertIn("holds the anchor", found[line_no - 1][0])
            anchors = [b14_anchor(a) for a in found[line_no - 1][1]]
            # A label that names a line names the anchor's, and one of a picture placed from
            # the line names it.
            for named in re.findall(r"line (\d)", label):
                self.assertEqual(int(named), line_no, label)
            if any(a["down"][0] == "line" for a in anchors):
                self.assertIn(f"line {line_no}", label)
            before = p.find(f"{W}pPr/{W}spacing").get(f"{W}before")
            self.assertEqual(before, "480" if n == 9 else None, label)
            # The pictures lie to the right of the lines, side by side.
            for text_line, _ in found:
                self.assertLess(carlito_width(text_line), anchors[0]["across"][1])
            first = anchors[0]
            relative, value = first["down"]
            if n == 9:
                self.assertEqual([a["down"] for a in anchors], [("paragraph", 0), ("line", 0)])
                self.assertIn("paragraph with 24 pt before", label)
                self.assertLessEqual(anchors[0]["across"][1] + 96, anchors[1]["across"][1])
            elif isinstance(value, str):
                words = {"top": "aligned top against", "center": "centred on",
                         "bottom": "aligned bottom against"}[value]
                self.assertIn(words + (" its paragraph" if relative == "paragraph"
                                       else " line 3"), label)
            else:
                self.assertIn(f"{value:g} pt below the top of "
                              + ("its paragraph" if relative == "paragraph" else "line 3"),
                              label)
            # No picture reaches the label: the highest one starts at most 36 pt, the empty
            # line, above its paragraph's top (24 pt above it if measured from above the space
            # before), its paragraph's lines h tall.
            space = 24 if n == 9 else 0
            for a in anchors:
                relative, value = a["down"]
                base = 0 if relative == "paragraph" else (line_no - 1) * h
                span = 5 * h if relative == "paragraph" else h
                offset = {"top": 0, "center": (span - 48) / 2, "bottom": span - 48}.get(
                    value, value) if isinstance(value, str) else value
                self.assertGreaterEqual(base + offset - space, -36, label)
        # Case 10: 58 lines of exact 12 pt to a page; the label and 49 filler lines before the
        # paragraph of 20, without widow control, leave 8 of its lines on the page; line 14,
        # the sixth on the next page, holds the anchors.
        page, label_p = cases[10]
        at = paragraphs.index(label_p)
        self.assertIsNotNone(label_p.find(f"{W}pPr/{W}pageBreakBefore"))
        filler, split = paragraphs[at + 1], paragraphs[at + 2]
        for p in (label_p, filler, split):
            spacing = p.find(f"{W}pPr/{W}spacing")
            self.assertEqual((spacing.get(f"{W}line"), spacing.get(f"{W}lineRule")),
                             ("240", "exact"))
        self.assertEqual(split.find(f"{W}pPr/{W}widowControl").get(f"{W}val"), "0")
        before = 1 + len(b14_lines(filler))
        self.assertEqual(before, 50)
        lines = b14_lines(split)
        self.assertEqual(len(lines), 20)
        (holder,) = [k for k, (_, drawings) in enumerate(lines, 1) if drawings]
        per_page = int((rules.PAGE_H - 2 * rules.MARGIN) / 20 // 12)
        self.assertEqual(per_page, 58)
        self.assertEqual(holder, 14)
        self.assertEqual(holder - (per_page - before), 6)
        self.assertEqual([b14_anchor(a)["down"] for a in lines[holder - 1][1]],
                         [("paragraph", 0), ("line", 0)])
        self.assertIn("20 lines over two pages", b14_text(label_p))
        self.assertIn(f"B 0 pt below line {holder}", b14_text(label_p))

    def test_anchor_inside_outside(self):
        documents = {}
        for name, mirrored in (("E-anchor-inside-outside", False),
                               ("E-anchor-inside-outside-mirror", True)):
            parts = self.parts(name)
            settings = ElementTree.fromstring(parts["word/settings.xml"])
            self.assertEqual(settings.find(f"{W}mirrorMargins") is not None, mirrored)
            documents[name] = parts["word/document.xml"]
            cases = self.check_paged(name, list(range(1, 17)))
            for n, (page, p, sect) in cases.items():
                label = b14_text(p)
                parity = "odd" if page % 2 else "even"
                self.assertIn(f"; page {page}, {parity}", label)
                gutter = int(sect.find(f"{W}pgMar").get(f"{W}gutter"))
                self.assertEqual(gutter, 720 if n > 8 else 0, label)
                self.assertEqual(label.endswith(", gutter"), gutter > 0, label)
                (anchor,) = p.iter(f"{WP}anchor")
                facts = b14_anchor(anchor)
                self.assertEqual(facts["size"], (48, 24))
                (across, x), (down, y) = facts["across"], facts["down"]
                if "from insideMargin both ways" in label or "from outsideMargin" in label:
                    kind = "insideMargin" if "insideMargin" in label else "outsideMargin"
                    self.assertEqual((across, x, down, y), (kind, 12, kind, 12), label)
                    self.assertIn("12 pt from", label)
                else:
                    kind = "inside" if "aligned inside" in label else "outside"
                    self.assertEqual((across, x, down, y), ("margin", kind, "margin", kind))
                self.assertEqual(b14_geometry(sect)[2:4], (72 + gutter / 20, 144))
            # Each placement on an odd page and the even page after it, in each section.
            placements = [(b14_anchor(next(p.iter(f"{WP}anchor")))["across"], page % 2,
                           n > 8) for n, (page, p, _) in cases.items()]
            for k in range(0, 16, 2):
                self.assertEqual(placements[k][0], placements[k + 1][0])
                self.assertEqual((placements[k][1], placements[k + 1][1]), (1, 0))
        # The two documents differ only in their settings.
        self.assertEqual(*documents.values())

    def test_anchor_stack(self):
        name = "E-anchor-stack"
        parts = self.parts(name)
        rels = {r.get("Id"): r.get("Target")
                for r in ElementTree.fromstring(parts["word/_rels/document.xml.rels"])}

        def level(rel_id):
            png = parts["word/" + rels[rel_id]]
            at = png.index(b"IDAT")
            (size,) = struct.unpack(">I", png[at - 4:at])
            pixels = zlib.decompress(png[at + 4:at + 4 + size])
            return pixels[1]

        paragraphs = self.body(name).findall(f"{W}p")
        cases = self.cases(name)
        self.assertEqual([page for page, _ in cases.values()], [1, 2, 3, 4, 5, 6])
        for n, (page, label_p) in cases.items():
            label = b14_text(label_p)
            p = paragraphs[paragraphs.index(label_p) + 1]
            facts = [b14_anchor(a) for a in p.iter(f"{WP}anchor")]
            levels = [level(f["picture"]) for f in facts]
            dark = levels.index(min(levels))
            if len(facts) == 2:
                light = 1 - dark
                self.assertLess(levels[dark], levels[light])
                if "written" in label:
                    self.assertIn("written first" if dark == 0 else "written second", label)
                self.assertEqual(facts[dark]["height"] > facts[light]["height"],
                                 "higher relativeHeight" in label, label)
                # The two overlap: the light one 48 pt right of and 24 pt below the dark one.
                (_, xd), (_, yd) = facts[dark]["across"], facts[dark]["down"]
                (_, xl), (_, yl) = facts[light]["across"], facts[light]["down"]
                self.assertEqual((xl - xd, yl - yd), (48, 24))
                self.assertTrue(xl - xd < 96 and yl - yd < 48)
                light_words = "light in front" if not facts[light]["behind"] else "both behind"
                self.assertIn(light_words, label)
            if "both behind" not in label:
                layer = re.search(r"\bdark (behind|in front)", label).group(1)
                self.assertEqual(layer == "behind", facts[dark]["behind"], label)
            else:
                self.assertTrue(all(f["behind"] for f in facts), label)
            shading = p.find(f"{W}pPr/{W}shd")
            self.assertEqual(None if shading is None else shading.get(f"{W}fill"),
                             "D9D9D9" if "shaded D9D9D9" in label else None, label)
            self.assertEqual(len(b14_lines(p)), 5)
            for f in facts:
                self.assertEqual((f["size"], f["wrap"]), ((96, 48), "wrapNone"))

    def test_anchor_in_cell(self):
        name = "E-anchor-in-cell"
        body = self.body(name)
        tables = body.findall(f"{W}tbl")
        cases = self.cases(name)
        self.assertEqual([page for page, _ in cases.values()], [1, 2])
        for (n, (page, label_p)), table in zip(cases.items(), tables):
            label = b14_text(label_p)
            first, second = table.findall(f"{W}tr/{W}tc")
            paragraphs = second.findall(f"{W}p")
            holders = [k for k, p in enumerate(paragraphs) if list(p.iter(f"{WP}anchor"))]
            self.assertEqual(holders, [1], "the second paragraph of cell 2")
            facts = b14_anchor(next(paragraphs[1].iter(f"{WP}anchor")))
            self.assertIn(f"layoutInCell {facts['in_cell']};", label)
            self.assertEqual(facts["in_cell"], "1" if n == 1 else "0")
            self.assertEqual((facts["across"], facts["down"]), (("column", 36), ("paragraph", 12)))
            self.assertIn("36 pt from the column, 12 pt below the paragraph", label)
            self.assertEqual(list(first.iter(f"{WP}anchor")), [])

    def test_anchor_effects(self):
        import math

        name = "E-anchor-effects"
        cases = self.check_paged(name, [1, 2, 3])
        for n, (page, p, sect) in cases.items():
            label = b14_text(p)
            (anchor,) = p.iter(f"{WP}anchor")
            facts = b14_anchor(anchor)
            shadow = anchor.find(f".//{PIC}spPr/{DML}effectLst/{DML}outerShdw")
            if "no shadow" in label:
                self.assertIsNone(anchor.find(f".//{DML}effectLst"))
                self.assertEqual(facts["effects"], (0, 0, 0, 0))
            else:
                # The extent the shadow reaches past the picture: its blur on every side, moved
                # by its distance in its direction (dir in units of 1/60000 degree, clockwise).
                blur = int(shadow.get("blurRad")) / EMU_PT
                dist = int(shadow.get("dist")) / EMU_PT
                angle = math.radians(int(shadow.get("dir")) / 60000)
                dx, dy = dist * math.cos(angle), dist * math.sin(angle)
                reach = (blur - dx, blur - dy, blur + dx, blur + dy)
                for got, want in zip(facts["effects"], reach):
                    self.assertAlmostEqual(got, want)
                self.assertEqual(facts["effects"], (12, 6, 12, 18))
                self.assertIn("effect extent 12, 6, 12 and 18 pt" if n == 1
                              else "the shadow and effect extent of case 01", label)
            if "aligned left and top" in label:
                self.assertEqual((facts["across"], facts["down"]),
                                 (("margin", "left"), ("margin", "top")))
            else:
                self.assertIn("144 pt across and down from the margins", label)
                self.assertEqual((facts["across"], facts["down"]),
                                 (("margin", 144), ("margin", 144)))
            self.assertEqual(facts["size"], (96, 48))
            # The shadow too stays clear of the label, which lies 100 pt below the top margin.
            _, y = b14_frame(facts, sect)
            top, bottom = y - facts["effects"][1], y + 48 + facts["effects"][3]
            label_top = b14_geometry(sect)[4] + 100
            self.assertTrue(bottom <= label_top or top >= label_top + carlito_line_height(11))

    def test_anchor_header(self):
        name = "E-anchor-header"
        parts, body = self.parts(name), self.body(name)
        header = ElementTree.fromstring(parts["word/header1.xml"])
        first, *rest = header
        self.assertEqual([e.tag for e in rest], [f"{W}tbl", f"{W}p"])
        corner, over = [b14_anchor(a) for a in first.iter(f"{WP}anchor")]
        under = b14_anchor(next(body.iter(f"{WP}anchor")))
        labels = {n: b14_text(p) for n, (_, p) in self.cases(name).items()}
        self.assertEqual((corner["across"], corner["down"], corner["size"], corner["behind"]),
                         (("page", 12), ("page", 12), (48, 24), False))
        self.assertIn("12 pt across and down from the page's corner, on 3 pages", labels[1])
        self.assertEqual((over["across"], over["down"], over["size"], over["behind"]),
                         (("page", 300), ("page", 80), (96, 48), False))
        self.assertIn("a header picture in front of the text, at 300 and 80 pt", labels[2])
        self.assertEqual((under["across"], under["down"], under["size"], under["behind"]),
                         (("page", 120), ("page", 24), (96, 48), True))
        self.assertIn("a body picture behind the text, at 120 and 24 pt", labels[3])
        pages = b14_pages(body)
        self.assertEqual(pages[-1][0], 3)
        # Over the body's text: below the top margin, and every body line of each page runs on
        # past the picture's right edge.
        top = b14_geometry(body.find(f"{W}sectPr"))[4]
        self.assertGreaterEqual(80, top)
        lines = [b14_text(p) for _, p in pages if b14_text(p).startswith("Body text")]
        self.assertEqual(len(lines), 18)
        for line in lines:
            self.assertGreater(carlito_width(line), 300 + 96 - 72)
            self.assertLess(carlito_width(line) + 12, rules.TEXT_W / 20)
        for page in (1, 2, 3):
            texts = [b14_text(p) for k, p in pages if k == page]
            self.assertTrue(texts[0].startswith(f"Body text of page {page}, line 1"), page)
        # Under the header's text and its shaded table: the body's picture spans 24 to 72 pt
        # down and 120 to 216 pt across; the header's line, from 35.4 pt down, runs past
        # 216 pt; the table's cells are shaded; and the header (a line, a row of a line and
        # its borders, and 2 pt) ends above the picture's bottom and the body's top margin.
        self.assertGreater(carlito_width(b14_text(first)), 120 + 96 - 72)
        cells = header.findall(f".//{W}tc")
        self.assertEqual({c.find(f"{W}tcPr/{W}shd").get(f"{W}fill") for c in cells}, {"D9D9D9"})
        distance = int(body.find(f"{W}sectPr/{W}pgMar").get(f"{W}header")) / 20
        h = carlito_line_height(11)
        self.assertLess(24, distance)
        header_end = distance + 2 * h + 1 + 2
        self.assertLess(header_end, 24 + 48)
        self.assertLess(header_end, top)
        self.assertLessEqual(24 + 48, top)

    def check_side_by_side(self, where, holder, from_box=False):
        """Picture A (or a box) 0 pt down from the holder's paragraph, B 0 pt down from its
        line, 72 x 36 pt, A at 216 and B at 324 pt from the margin, the holder's text left of
        them."""
        facts = [b14_anchor(a) for a in holder.iter(f"{WP}anchor")]
        downs = [f["down"] for f in facts]
        self.assertEqual(downs, [("paragraph", 0), ("line", 0)], where)
        if from_box:
            self.assertIsNotNone(holder.find(f".//{WPS}txbx"))
            self.assertEqual((facts[0]["across"], facts[0]["size"]), (("margin", 144), (144, 36)))
        else:
            self.assertEqual((facts[0]["across"], facts[0]["size"]), (("margin", 216), (72, 36)))
        self.assertEqual((facts[1]["across"], facts[1]["size"]), (("margin", 324), (72, 36)))
        self.assertLess(carlito_width(b14_text(holder), 10 if "note" in where else 11) + 6,
                        facts[0]["across"][1])

    def test_anchor_paragraph_spacing(self):
        name = "E-anchor-paragraph-spacing"
        parts, body = self.parts(name), self.body(name)
        paragraphs = body.findall(f"{W}p")
        styles = ElementTree.fromstring(parts["word/styles.xml"])
        (spaced,) = [s for s in styles.iter(f"{W}style") if s.get(f"{W}styleId") == "Spaced"]
        self.assertEqual(spaced.find(f"{W}pPr/{W}spacing").get(f"{W}before"), "240")
        self.assertIsNotNone(spaced.find(f"{W}pPr/{W}contextualSpacing"))

        def spacing(p):
            s = p.find(f"{W}pPr/{W}spacing")
            return {} if s is None else {k[len(W):]: v for k, v in s.attrib.items()}

        cases = self.cases(name)
        holders = {}
        for n, (page, label_p) in cases.items():
            at = paragraphs.index(label_p)
            holder = next(p for p in paragraphs[at + 1:] if list(p.iter(f"{WP}anchor")))
            above = paragraphs[paragraphs.index(holder) - 1]
            holders[n] = (page, label_p, above, holder)
            self.assertEqual(b14_text(holder), f"The anchor of case {n:02d}.")
            self.check_side_by_side(f"case {n}", holder, from_box=n >= 4)
        # Case 01: after a paragraph that holds only a page break, at the top of page 2, 12 pt
        # before.
        page, label_p, above, holder = holders[1]
        self.assertEqual((page, b14_pages(body)[paragraphs.index(holder)][0]), (1, 2))
        self.assertEqual([br.get(f"{W}type") for br in above.iter(f"{W}br")], ["page"])
        self.assertEqual(b14_text(above), "")
        self.assertEqual(spacing(holder).get("before"), "240")
        self.assertIn("12 pt before, at the top after a page break", b14_text(label_p))
        # Case 02: two paragraphs of the style, the second holding A and B.
        _, label_p, above, holder = holders[2]
        for p in (above, holder):
            self.assertEqual(p.find(f"{W}pPr/{W}pStyle").get(f"{W}val"), "Spaced")
            self.assertNotIn("before", spacing(p))
        self.assertIn("two paragraphs of a style, 12 pt before, contextual spacing",
                      b14_text(label_p))
        # Cases 03 to 06: the paragraph above, as its text and the label say.
        expected = {3: ({"after": "480"}, {"before": "120", "after": "960"},
                        "24 pt after", "with 6 pt before"),
                    4: ({"after": "40"}, {"after": "960"}, "2 pt after", "after 2 pt after"),
                    5: ({"line": "240", "lineRule": "exact"},
                        {"after": "960", "line": "240", "lineRule": "exact"},
                        "exact 12 pt", "exact 12 pt lines, after such a line"),
                    6: ({"after": "40", "line": "240", "lineRule": "exact"},
                        {"after": "960", "line": "240", "lineRule": "exact"},
                        "exact 12 pt, 2 pt after", "after one with 2 pt after")}
        for n, (above_spacing, holder_spacing, above_words, label_words) in expected.items():
            _, label_p, above, holder = holders[n]
            self.assertEqual(spacing(above), above_spacing, n)
            self.assertEqual(spacing(holder), holder_spacing, n)
            self.assertEqual(b14_text(above), f"Above case {n:02d}: {above_words}.")
            self.assertIn(label_words, b14_text(label_p))
            self.assertLess(carlito_width(b14_text(above)) + 6, 216)
        # The boxes: E-textbox-from-paragraph's, one line centred down, insets 7.2 pt across
        # and 0 down.
        for n in (4, 5, 6):
            body_pr = holders[n][3].find(f".//{WPS}bodyPr")
            self.assertEqual({k: body_pr.get(k) for k in ("anchor", "lIns", "tIns", "rIns",
                                                          "bIns")},
                             {"anchor": "ctr", "lIns": "91440", "tIns": "0", "rIns": "91440",
                              "bIns": "0"})
        self.assertIn("box centred down, 0 pt below its paragraph", b14_text(holders[4][1]))

    def test_anchor_paragraph_header_note(self):
        name = "E-anchor-paragraph-header-note"
        parts, body = self.parts(name), self.body(name)
        header = ElementTree.fromstring(parts["word/header1.xml"])
        first, second = header.findall(f"{W}p")
        self.assertEqual(list(first.iter(f"{WP}anchor")), [])
        self.assertEqual(second.find(f"{W}pPr/{W}spacing").get(f"{W}before"), "240")
        self.check_side_by_side("header", second)
        notes = {n.get(f"{W}id"): n.findall(f"{W}p") for n in ElementTree.fromstring(
            parts["word/footnotes.xml"]).findall(f"{W}footnote")}
        note_1, note_2 = notes["1"], notes["2"]
        self.assertEqual(len(note_1), 2)
        self.assertEqual(list(note_1[0].iter(f"{WP}anchor")), [])
        self.assertEqual(note_1[1].find(f"{W}pPr/{W}spacing").get(f"{W}before"), "240")
        self.check_side_by_side("note 1", note_1[1])
        self.assertEqual(len(note_2), 1)
        self.assertEqual(note_2[0].find(f"{W}pPr/{W}spacing").get(f"{W}before"), "240")
        self.check_side_by_side("note 2", note_2[0])
        labels = self.cases(name)
        refs = {n: [r.get(f"{W}id") for r in p.iter(f"{W}footnoteReference")]
                for n, (_, p) in labels.items()}
        self.assertEqual(refs, {1: [], 2: ["1"], 3: ["2"]})
        self.assertEqual([page for page, _ in labels.values()], [1, 1, 2])
        self.assertIn("the header's second paragraph, 12 pt before", b14_text(labels[1][1]))
        self.assertIn("footnote 1's second paragraph, 12 pt before", b14_text(labels[2][1]))
        self.assertIn("footnote 2's first paragraph, 12 pt before", b14_text(labels[3][1]))
        # The empty line of 48 pt at the top of each page ends below the header's pictures:
        # at most 36 pt down from the line of the header's second paragraph.
        sect = body.find(f"{W}sectPr")
        distance = int(sect.find(f"{W}pgMar").get(f"{W}header")) / 20
        h = carlito_line_height(11)
        pictures_end = distance + h + 12 + 36
        body_top = max(b14_geometry(sect)[4], distance + 2 * h + 12)
        for k, (page, p) in enumerate(b14_pages(body)):
            if k == 0 or p.find(f"{W}pPr/{W}pageBreakBefore") is not None:
                self.assertEqual(p.find(f"{W}pPr/{W}spacing").get(f"{W}line"), "960")
                self.assertEqual(b14_text(p), "")
        self.assertLess(pictures_end, body_top + 48)

    def test_vml_anchor_relative(self):
        name = "E-vml-anchor-relative"
        body = self.body(name)
        self.assertEqual([p for p in body.iter(f"{W}sectPr")], [body.find(f"{W}sectPr")])
        self.assert_wide_margins(name, body.find(f"{W}sectPr"))
        paragraphs = body.findall(f"{W}p")
        cases = self.cases(name)
        self.assertEqual([page for page, _ in cases.values()], [1, 1, 2, 3, 4, 5, 6, 7])
        for n, (page, label_p) in cases.items():
            label = b14_text(label_p)
            holder = label_p if n > 2 else paragraphs[paragraphs.index(label_p) + 1]
            (shape,) = holder.iter(f"{VML}shape")
            css = b14_css(shape)
            self.assertEqual((css["width"], css["height"], css["position"]),
                             ("48pt", "24pt", "absolute"))
            self.assertEqual(shape.find(f"{W10}wrap").get("type"), "none")
            across, down = (css["mso-position-horizontal-relative"],
                            css["mso-position-vertical-relative"])
            x, y = css["margin-left"], css["margin-top"]
            if n == 1:
                self.assertEqual((across, x, down, y), ("text", "36pt", "text", "18pt"))
                self.assertEqual(holder.find(f"{W}pPr/{W}ind").get(f"{W}left"), "1440")
                self.assertIn("36 pt from text across, 18 pt from text down, indented 72 pt",
                              label)
            elif n == 2:
                self.assertEqual((across, x, down, y), ("char", "12pt", "line", "18pt"))
                lines = b14_lines(holder)
                self.assertEqual(len(lines), 5)
                self.assertEqual([k for k, (_, d) in enumerate(lines, 1) if d], [3])
                self.assertIn("12 pt from char across, 18 pt from line down, line 3 of five",
                              label)
            else:
                self.assertEqual((x, y), ("12pt", "12pt"))
                areas = (f"{across} both ways" if across == down else f"{across} and {down}")
                parity = "odd" if page % 2 else "even"
                self.assertIn(f"12 pt from {areas}; page {page}, {parity}", label)
        # Every margin area of #400's item 1, across and down, in the order of the cases.
        areas = [(b14_css(next(p.iter(f"{VML}shape")))["mso-position-horizontal-relative"],
                  b14_css(next(p.iter(f"{VML}shape")))["mso-position-vertical-relative"])
                 for n, (page, p) in cases.items() if n > 2]
        self.assertEqual(areas, [("left-margin-area", "top-margin-area"),
                                 ("right-margin-area", "bottom-margin-area"),
                                 ("inner-margin-area", "inner-margin-area"),
                                 ("inner-margin-area", "inner-margin-area"),
                                 ("outer-margin-area", "outer-margin-area"),
                                 ("outer-margin-area", "outer-margin-area")])
        found = {(b14_css(next(p.iter(f"{VML}shape")))["mso-position-horizontal-relative"],
                  page % 2) for n, (page, p) in cases.items() if n > 2}
        for area in ("inner-margin-area", "outer-margin-area"):
            self.assertTrue({(area, 0), (area, 1)} <= found, area)
        self.assertTrue({"left-margin-area", "right-margin-area"} <= {a for a, _ in found})

    def test_vml_anchor_align(self):
        name = "E-vml-anchor-align"
        cases = self.check_paged(name, list(range(1, 9)))
        for n, (page, p, sect) in cases.items():
            self.assert_wide_margins(name, sect)
            label = b14_text(p)
            (shape,) = p.iter(f"{VML}shape")
            css = b14_css(shape)
            self.assertEqual((css["width"], css["height"]), ("96pt", "48pt"))
            self.assertEqual((css["mso-position-horizontal-relative"],
                              css["mso-position-vertical-relative"]), ("margin", "margin"))
            self.assertEqual((css["margin-left"], css["margin-top"]), ("36pt", "36pt"))
            across = css.get("mso-position-horizontal")
            down = css.get("mso-position-vertical")
            if n == 8:
                self.assertEqual((across, down, css["left"]), (None, None, "36pt"))
                self.assertIn("left and margin-left of 36 pt each, margin-top of 36 pt", label)
                continue
            self.assertNotIn("left", css)
            if across == "center":
                self.assertEqual(down, "center")
                self.assertIn("centred across and down", label)
            elif across in ("inside", "outside"):
                self.assertEqual(down, across)
                parity = "odd" if page % 2 else "even"
                self.assertIn(f"aligned {across} across and down against the margins; page "
                              f"{page}, {parity}", label)
            else:
                self.assertIn(f"aligned {across} and {down} against the margins", label)
        kinds = {(b14_css(next(p.iter(f"{VML}shape"))).get("mso-position-horizontal"),
                  page % 2) for page, p, _ in cases.values()}
        self.assertTrue({("inside", 0), ("inside", 1), ("outside", 0), ("outside", 1)} <= kinds)

    def test_vml_stack(self):
        name = "E-vml-stack"
        parts = self.parts(name)
        paragraphs = self.body(name).findall(f"{W}p")
        targets = {r.get("Id"): r.get("Target")
                   for r in ElementTree.fromstring(parts["word/_rels/document.xml.rels"])
                   if r.get("Type") == IMAGE}
        levels = {}
        for rel_id, target in targets.items():
            png = parts["word/" + target]
            at = png.index(b"IDAT")
            (size,) = struct.unpack(">I", png[at - 4:at])
            levels[rel_id] = zlib.decompress(png[at + 4:at + 4 + size])[1]
        cases = self.cases(name)
        self.assertEqual([page for page, _ in cases.values()], [1, 2, 3])
        for n, (page, label_p) in cases.items():
            label = b14_text(label_p)
            holder = paragraphs[paragraphs.index(label_p) + 1]
            shapes = list(holder.iter(f"{VML}shape"))
            self.assertEqual(len(shapes), 2)
            zs = [int(b14_css(s)["z-index"]) for s in shapes]
            shade = [levels[s.find(f"{VML}imagedata").get(f"{REL_ID}id")] for s in shapes]
            dark = shade.index(min(shade))
            self.assertLess(shade[dark], shade[1 - dark])
            if n == 2:
                self.assertEqual(dark, 1, "written second")
                self.assertEqual(sorted(zs), sorted(int(z) for z in re.findall(
                    r"-?\d{9}", b14_text(cases[1][1]))))
                self.assertIn("the same z-index values, the dark picture written second", label)
            else:
                self.assertEqual(dark, 0, "written first")
                self.assertIn(f"dark of z-index {zs[0]} written first", label)
                self.assertIn(f"light of {zs[1]}", label)
            self.assertEqual(all(z < 0 for z in zs), "behind the text" in label or n == 2)
            self.assertTrue(all(z > 0 for z in zs) or all(z < 0 for z in zs))
            # They overlap: the second's offsets 48 and 24 pt past the first's.
            offsets = sorted((float(b14_css(s)["margin-left"][:-2]),
                              float(b14_css(s)["margin-top"][:-2])) for s in shapes)
            self.assertEqual((offsets[1][0] - offsets[0][0], offsets[1][1] - offsets[0][1]),
                             (48, 24))
            self.assertEqual(len(b14_lines(holder)), 5)

    def test_vml_crop_rotate_and_its_picture(self):
        name = "E-vml-crop-rotate"
        parts, body = self.parts(name), self.body(name)
        paragraphs = body.findall(f"{W}p")
        crops = ("cropleft", "croptop", "cropright", "cropbottom")
        for n, (page, label_p) in self.cases(name).items():
            label = b14_text(label_p)
            spacer = paragraphs[paragraphs.index(label_p) + 1]
            holder = paragraphs[paragraphs.index(label_p) + 2]
            (shape,) = holder.iter(f"{VML}shape")
            css = b14_css(shape)
            data = shape.find(f"{VML}imagedata")
            written = {k: data.get(k) for k in crops if data.get(k) is not None}
            turned = {k: v for k, v in css.items() if k in ("rotation", "flip")}
            if n == 1:
                self.assertEqual((written, turned), ({}, {}))
            elif written:
                ((side, value),) = written.items()
                self.assertEqual(turned, {})
                self.assertIn(f'{side} "{value}"', label)
                fraction = int(value[:-1]) / 65536 if value.endswith("f") else float(value)
                self.assertAlmostEqual(abs(fraction), 0.1, places=4)
                self.assertEqual(fraction < 0, "negative" in label)
            else:
                ((key, value),) = turned.items()
                self.assertRegex(label.split(": ", 1)[1], rf"^{key}:{re.escape(value)}(,|$)")
                if key == "rotation":
                    degrees = (int(value[:-2]) / 65536 if value.endswith("fd") else int(value))
                    self.assertEqual(degrees, 90)
            # A picture turned by 90 degrees reaches (144 - 72) / 2 = 36 pt above and below
            # its frame: the empty line above and the space after leave room for it.
            width, height = float(css["width"][:-2]), float(css["height"][:-2])
            reach = (width - height) / 2
            self.assertEqual(reach, 36)
            self.assertLessEqual(reach, int(spacer.find(f"{W}pPr/{W}spacing").get(f"{W}line")) / 20)
            after = int(holder.find(f"{W}pPr/{W}spacing").get(f"{W}after")) / 20
            self.assertLessEqual(height + reach, carlito_line_height(11) + after)
            self.assertEqual((css["margin-left"], css["margin-top"]), ("144pt", "0pt"))
        # The picture: 200 x 100 px of 10 x 10 cells of 20 x 10 px, a chequerboard but for the
        # 2 x 2 cells of the top left corner, all dark: a tenth of the picture is one column or
        # row of cells.
        png = parts["word/media/image1.png"]
        width, height = struct.unpack(">II", png[16:24])
        self.assertEqual((width, height), (200, 100))
        at = png.index(b"IDAT")
        (size,) = struct.unpack(">I", png[at - 4:at])
        pixels = zlib.decompress(png[at + 4:at + 4 + size])
        rows = [pixels[r * (width + 1) + 1:(r + 1) * (width + 1)] for r in range(height)]
        for row in range(10):
            for column in range(10):
                block = {rows[row * 10 + y][column * 20 + x] for y in range(10) for x in range(20)}
                dark = (row < 2 and column < 2) or (row + column) % 2 == 0
                self.assertEqual(block, {0} if dark else {255}, (row, column))
        self.assertEqual(width / 10, 20)

    def test_vml_px_effects(self):
        name = "E-vml-px-effects"
        paragraphs = self.body(name).findall(f"{W}p")
        for n, (page, label_p) in self.cases(name).items():
            label = b14_text(label_p)
            holder = paragraphs[paragraphs.index(label_p) + 1]
            (shape,) = holder.iter(f"{VML}shape")
            css = b14_css(shape)
            self.assertNotIn("position", css, "inline")
            data = shape.find(f"{VML}imagedata")
            if n == 1:
                self.assertEqual((css["width"], css["height"]), ("96px", "48px"))
                # At 96 pixels to the inch, the size of case 02.
                self.assertEqual((96 * 72 / 96, 48 * 72 / 96), (72, 36))
            elif n == 2:
                self.assertEqual((css["width"], css["height"]), ("72pt", "36pt"))
            else:
                self.assertEqual((css["width"], css["height"]), ("144pt", "72pt"))
            washed = (data.get("gain"), data.get("blacklevel"))
            if "washed out" in label:
                self.assertEqual(washed, ("19661f", "22938f"))
                self.assertAlmostEqual(19661 / 65536, 0.3, places=4)
                self.assertAlmostEqual(22938 / 65536, 0.35, places=4)
            else:
                self.assertEqual(washed, (None, None))
            stroke = shape.find(f"{VML}stroke")
            if "an outline" in label:
                self.assertEqual(shape.get("stroked"), "t")
                self.assertEqual((stroke.get("weight"), stroke.get("color")), ("3pt", "black"))
            else:
                self.assertIsNone(stroke)
                self.assertIsNone(shape.get("stroked"))
            self.assertEqual(shape.get("type") is None, "without a type" in label, label)

    def test_textbox_vml_inline_shapes(self):
        name = "E-textbox-vml-inline-shapes"
        paragraphs = self.body(name).findall(f"{W}p")
        fonts = {"Calibri": CARLITO, "Times New Roman": LIBERATION_SERIF,
                 "Noto Sans": NOTO_TRUETYPE}
        kinds = []
        for n, (page, label_p) in self.cases(name).items():
            label = b14_text(label_p)
            holder = paragraphs[paragraphs.index(label_p) + 1]
            after = paragraphs[paragraphs.index(label_p) + 2]
            self.assertTrue(b14_text(after).startswith("The paragraph after"))
            self.assertIsNone(holder.find(f"{W}pPr/{W}spacing"), "0 pt before")
            (pict,) = holder.iter(f"{W}pict")
            (box,) = [e for e in pict if e.tag != f"{VML}shapetype"]
            # A label naming a v:rect or a v:roundrect holds that element; every other one a
            # v:shape of the text box type.
            kind = box.tag[len(VML):]
            named = re.findall(r"\ba (v:rect|v:roundrect)\b", label)
            self.assertEqual(kind, named[0][len("v:"):] if named else "shape", label)
            self.assertEqual(box.get("type"), "#_x0000_t202" if kind == "shape" else None, label)
            kinds.append(kind)
            css = b14_css(box)
            size = re.search(r"(\d+) x (\d+) pt", label)
            if size:
                self.assertEqual((css["width"], css["height"]),
                                 (f"{size.group(1)}pt", f"{size.group(2)}pt"), label)
            else:
                self.assertEqual((css["width"], css["height"]), ("216pt", "90pt"), label)
            self.assertEqual(box.get("strokeweight"), ".5pt")
            textbox = box.find(f"{VML}textbox")
            content = textbox.findall(f"{W}txbxContent/{W}p")
            if "mso-fit-shape-to-text" in label:
                self.assertEqual(textbox.get("style"), "mso-fit-shape-to-text:t")
                self.assertEqual(len(content), 3)
                self.assertIn("for three lines", label)
            else:
                self.assertIsNone(textbox.get("style"))
                self.assertEqual(len(content), 1)
            # The runs of the line, the box's run and the paragraph mark: the case's font and
            # size, as the label says.
            # Calibri 11 pt where the label names none, as the document's purpose says.
            match = re.search(r"the line in (Calibri|Times New Roman|Noto Sans) (\d+) pt", label)
            font, points = (match.group(1), int(match.group(2))) if match else ("Calibri", 11)
            self.assertEqual(match is None, n >= 7, label)
            properties = [r.find(f"{W}rPr") for r in holder.findall(f"{W}r")]
            properties.append(holder.find(f"{W}pPr/{W}rPr"))
            self.assertEqual(len(properties), 4)
            for r_pr in properties:
                self.assertEqual(r_pr.find(f"{W}sz").get(f"{W}val"), str(2 * points), label)
                named = r_pr.find(f"{W}rFonts")
                self.assertEqual(font, "Calibri" if named is None else named.get(f"{W}ascii"))
            # The text before and after the box shares its line.
            line = b14_text(holder)
            self.assertEqual(line, f"Case {n:02d}  after the box.")
            self.assertLess(carlito_width(line, points, fonts[font]) + float(css["width"][:-2])
                            + 12, rules.TEXT_W / 20, label)
        self.assertEqual(len(self.cases(name)), 9)
        self.assertEqual(kinds, ["shape"] * 7 + ["rect", "roundrect"])

    def test_hidden_drawings(self):
        name = "E-hidden-drawings"
        paragraphs = self.body(name).findall(f"{W}p")
        cases = self.cases(name)
        self.assertEqual(len(cases), 13)
        pages = []
        for n, (page, label_p) in cases.items():
            label = b14_text(label_p)
            holder = paragraphs[paragraphs.index(label_p) + 1]
            pages.append(page)
            if "VML" in label:
                (shape,) = holder.iter(f"{VML}shape")
                self.assertEqual(b14_css(shape)["visibility"], "hidden")
                self.assertEqual(shape.find(f"{W10}wrap").get("type"), "square")
                self.assertIn("visibility:hidden, wrapped square", label)
                continue
            inline = list(holder.iter(f"{WP}inline"))
            floating = list(holder.iter(f"{WP}anchor"))
            (drawing,) = inline + floating
            self.assertEqual(bool(inline), "inline" in label, label)
            self.assertEqual(bool(floating), "floating" in label, label)
            hidden = drawing.find(f"{WP}docPr").get("hidden")
            self.assertEqual(hidden, "1" if "docPr hidden" in label else None, label)
            # wp:anchor's own hidden attribute, which MS-OI29500 2.1.1350 a says Word does not
            # support, only in the case that says so, and never with docPr's.
            anchor_hidden = drawing.get("hidden") if floating else None
            self.assertEqual(anchor_hidden, "1" if "hidden on wp:anchor" in label else None,
                             label)
            self.assertFalse(hidden and anchor_hidden, label)
            self.assertEqual("visible" in label, hidden is None and anchor_hidden is None, label)
            text_box = drawing.find(f".//{WPS}txbx") is not None
            shape = drawing.find(f".//{WPS}wsp") is not None
            picture = drawing.find(f".//{PIC}pic") is not None
            self.assertEqual((picture, text_box, shape and not text_box),
                             ("picture" in label, "text box" in label,
                              "shape without text" in label), label)
            if floating:
                facts = b14_anchor(drawing)
                self.assertEqual(facts["wrap"], "wrapSquare" if "wrapped square" in label
                                 else "wrapNone", label)
                self.assertTrue("wrapped square" in label or "not wrapped" in label, label)
                self.assertEqual((facts["across"], facts["down"], facts["size"]),
                                 (("margin", 0), ("paragraph", 0), (144, 72)))
                self.assertGreater(carlito_width(b14_text(holder)), 4 * rules.TEXT_W / 20,
                                   "text to wrap")
            else:
                self.assertEqual(int(drawing.find(f"{WP}extent").get("cx")) / EMU_PT, 72)
                self.assertTrue(b14_text(holder).endswith(" after it."))
        # The inline cases on page 1, then two floating cases to a page.
        self.assertEqual(pages, [1, 1, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6])


A = "{http://schemas.openxmlformats.org/drawingml/2006/main}"
R_ID = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}id"


class BatchFourteen(unittest.TestCase):
    """Helpers of the batch 14 tests of part C: lists, styles and unusual values."""

    @classmethod
    def setUpClass(cls):
        cls.docs = {doc.name: doc for doc in rules.documents()}

    def parts(self, name):
        return dict(rules.parts(self.docs[name]))

    def xml(self, name, part_name):
        return ElementTree.fromstring(self.parts(name)[part_name])

    def body(self, name):
        return self.xml(name, "word/document.xml").find(f"{W}body")

    @staticmethod
    def texts(element):
        return "".join(t.text or "" for t in element.iter(f"{W}t"))

    @staticmethod
    def val(element, path):
        found = element.find(path)
        return None if found is None else found.get(f"{W}val")

    def styles(self, name):
        """Every w:style of the document, in order."""
        return list(self.xml(name, "word/styles.xml").iter(f"{W}style"))

    def style(self, name, style_id):
        (found,) = [s for s in self.styles(name) if s.get(f"{W}styleId") == style_id]
        return found

    def numbering(self, name):
        """{numId: {ilvl: w:lvl}} of the document's list instances."""
        root = self.xml(name, "word/numbering.xml")
        abstracts = {a.get(f"{W}abstractNumId"): {int(lvl.get(f"{W}ilvl")): lvl
                                                  for lvl in a.iter(f"{W}lvl")}
                     for a in root.iter(f"{W}abstractNum")}
        return {int(n.get(f"{W}numId")): abstracts[self.val(n, f"{W}abstractNumId")]
                for n in root.iter(f"{W}num")}

    def items(self, name, root=None):
        """(text, numId, ilvl, w:p) of every paragraph with a list of its own."""
        root = self.body(name) if root is None else root
        found = []
        for p in root.iter(f"{W}p"):
            num_pr = p.find(f"{W}pPr/{W}numPr")
            if num_pr is not None:
                found.append((self.texts(p), int(self.val(num_pr, f"{W}numId")),
                              int(self.val(num_pr, f"{W}ilvl")), p))
        return found

    def settings(self, name):
        return self.xml(name, "word/settings.xml")

    def compat_values(self, name):
        return {s.get(f"{W}name"): s.get(f"{W}val")
                for s in self.settings(name).iter(f"{W}compatSetting")}

    @staticmethod
    def wrapped_lines(text_, width, size=11):
        """The lines of text_ broken first fit at its spaces in Carlito at size points, in a
        column width points wide: a lower bound on Word's count, which keeps a space at a
        line's end out of its width."""
        lines_, line = [], ""
        for word in text_.split(" "):
            trial = f"{line} {word}" if line else word
            if line and carlito_width(trial, size) > width:
                lines_.append(line)
                line = word
            else:
                line = trial
        return lines_ + [line]


class BatchFourteenLists(BatchFourteen):
    def test_list_header_numbers_one_instance_in_the_header_and_the_body(self):
        name = "E-list-header"
        instances = self.numbering(name)
        self.assertEqual(list(instances), [1])
        self.assertEqual((self.val(instances[1][0], f"{W}numFmt"),
                          self.val(instances[1][0], f"{W}lvlText")), ("decimal", "%1."))
        body = self.body(name)
        (reference,) = body.iter(f"{W}headerReference")
        self.assertEqual(reference.get(f"{W}type"), "default")
        self.assertEqual(len(list(body.iter(f"{W}sectPr"))), 1, "one section for both pages")
        rels = self.xml(name, "word/_rels/document.xml.rels")
        target = {r.get("Id"): r.get("Target") for r in rels}[reference.get(R_ID)]
        header = self.xml(name, f"word/{target}")
        header_items = self.items(name, header)
        self.assertEqual(len(header_items), 2)
        self.assertEqual(len(header.findall(f"{W}p")), 2, "nothing else in the header")
        for (text, num_id, ilvl, _), which in zip(header_items, ("first", "second")):
            self.assertEqual((num_id, ilvl), (1, 0))
            self.assertIn(f"the {which} of two (num 1, level 0)", text)
        body_items = self.items(name)
        self.assertEqual(len(body_items), 3)
        label = self.texts(body.find(f"{W}p"))
        self.assertIn("three items of num 1", label)
        self.assertIn("two on page 1 and one on page 2", label)
        self.assertIn("two items of the same num 1", label)
        for (text, num_id, ilvl, p), (which, page) in zip(
                body_items, (("first", 1), ("second", 1), ("third", 2))):
            self.assertEqual((num_id, ilvl), (1, 0))
            self.assertIn(f"the {which} of three, on page {page}", text)
            self.assertEqual(p.find(f"{W}pPr/{W}pageBreakBefore") is not None, page == 2, text)
        self.assertEqual(len(list(body.iter(f"{W}br"))), 0, "the only page break is case 3's")

    def check_sequence(self, label, earlier):
        """The claims a label makes about the items of its list before it, (ilvl, ...)."""
        if "its first item" in label:
            self.assertEqual(earlier, [], label)
        match = re.search(r"none at levels? ([\d and]+)", label)
        if match:
            for ilvl in map(int, re.findall(r"\d", match.group(1))):
                self.assertNotIn(ilvl, earlier, label)
        match = re.search(r"after (?:the item|items) at levels? ([\d and]+)", label)
        if match:
            levels = list(map(int, re.findall(r"\d", match.group(1))))
            self.assertEqual(earlier[-len(levels):], levels, label)

    def test_list_own_level_items_are_where_their_labels_say(self):
        name = "E-list-own-level"
        instances = self.numbering(name)
        self.assertEqual(sorted(instances), [1, 2])
        self.assertIsNot(instances[1], instances[2], "two definitions")
        shown = {0: ("decimal", "%1.", "1."), 1: ("upperLetter", "%2.", "A."),
                 2: ("lowerLetter", "%3)", "a)")}
        for levels in instances.values():
            for ilvl, (fmt, template, _) in shown.items():
                lvl = levels[ilvl]
                self.assertEqual((self.val(lvl, f"{W}numFmt"), self.val(lvl, f"{W}lvlText")),
                                 (fmt, template))
                # Each level shows only its own number.
                self.assertEqual(re.findall(r"%(\d)", template), [str(ilvl + 1)])
        items = self.items(name)
        self.assertEqual(len(items), 7)
        before = {1: [], 2: []}
        for n, (label, num_id, ilvl, _) in enumerate(items, 1):
            self.assertTrue(label.startswith(f"Case {n:02d}: list {'AB'[num_id - 1]}, "
                                             f"level {ilvl}, "), label)
            self.check_sequence(label, before[num_id])
            before[num_id].append(ilvl)
        self.assertEqual(before, {1: [2, 1, 0, 1], 2: [2, 0, 1]})
        self.assertIn("(1., A. and a))", self.docs[name].purpose)

    def test_long_templates_have_the_length_their_labels_name(self):
        for name, length in (("E-list-long-template", 300), ("E-list-long-template-600", 600)):
            (levels,) = self.numbering(name).values()
            template = self.val(levels[0], f"{W}lvlText")
            self.assertEqual(len(template), length, name)
            self.assertGreater(length, 256, "longer than the layout's cut")
            self.assertTrue(template.startswith("%1. "), name)
            self.assertEqual(template.count("%"), 1, "no other placeholder")
            # Every block of ten ends in the count of characters up to its end.
            for end in range(10, length + 1, 10):
                self.assertEqual(template[end - 4:end], f"{end:04d}", f"{name} at {end}")
            self.assertIn(f"w:lvlText has {length} characters", self.items(name)[0][0])
            self.assertIn(f"w:lvlText has {length} characters", self.docs[name].purpose)

    def test_template_controls_hold_the_characters_their_labels_name(self):
        name = "E-list-template-controls"
        instances = self.numbering(name)
        items = self.items(name)
        self.assertEqual(len(items), 7)
        self.assertEqual(len({id(levels) for levels in instances.values()}), 7, "a list each")
        raw = self.parts(name)["word/numbering.xml"].decode()
        for text_ in re.findall(r'<w:lvlText w:val="([^"]*)"', raw):
            # A literal tab or line feed would be read as a space (XML 1.0 §3.3.3).
            self.assertNotRegex(text_, "[\t\n\r]")
        names = {"U+0009, a tab": None, "U+000A, a line feed": None,
                 "U+2028, the line separator": "LINE SEPARATOR",
                 "U+2029, the paragraph separator": "PARAGRAPH SEPARATOR"}
        seen = []
        for n, (label, num_id, ilvl, _) in enumerate(items, 1):
            self.assertTrue(label.startswith(f"Case {n:02d}: the label [%1] with "), label)
            template = self.val(instances[num_id][0], f"{W}lvlText")
            self.assertTrue(template.startswith("[%1") and template.endswith("]"), template)
            middle = template[3:-1]
            said = next((k for k in names if k in label), None)
            if said is not None:
                (character,) = middle
                code = int(said[2:6], 16)
                self.assertEqual(ord(character), code, label)
                if names[said] is None:
                    self.assertEqual(unicodedata.category(character), "Cc", label)
                else:
                    self.assertEqual(unicodedata.name(character), names[said], label)
                seen.append(code)
            elif "U+000B" in label:
                self.assertIn("_x000B_", label)
                self.assertEqual(middle, "_x000B_")
                self.assertEqual(int(middle[2:6], 16), 0x0B)
                seen.append(0x0B)
            elif "U+007F" in label:
                self.assertEqual(middle, "\x7f")
                self.assertEqual(unicodedata.category(middle), "Cc")
                seen.append(0x7F)
            else:
                self.assertIn("no other character", label)
                self.assertEqual(middle, "")
        self.assertEqual(seen, [0x09, 0x0A, 0x0B, 0x2028, 0x2029, 0x7F])
        purpose = self.docs[name].purpose
        for said in ("a tab", "a line feed", "U+000B (as _x000B_)", "U+2028", "U+2029",
                     "U+007F"):
            self.assertIn(said, purpose)

    def test_deeper_template_items_are_where_their_labels_say(self):
        name = "E-list-template-deeper"
        instances = self.numbering(name)
        items = self.items(name)
        self.assertEqual(len(items), 5)
        before = {1: [], 2: []}
        for n, (label, num_id, ilvl, _) in enumerate(items, 1):
            template = self.val(instances[num_id][ilvl], f"{W}lvlText")
            self.assertTrue(label.startswith(f"Case {n:02d}: list {'AB'[num_id - 1]}, level "
                                             f"{ilvl} ({template})"), label)
            if "with no item at level 0 before it" in label:
                self.assertNotIn(0, before[num_id], label)
            if label.endswith("after it"):
                self.assertEqual(len(before[num_id]), 1, label)
            if "after the item at level 0" in label:
                self.assertEqual(before[num_id][-1], 0, label)
            before[num_id].append(ilvl)
        # List A's level 1 shows level 0 and the deeper level 2; list B's only 0 and 1.
        shown = {num_id: [int(d) for d in re.findall(r"%(\d)", self.val(
            levels[1], f"{W}lvlText"))] for num_id, levels in instances.items()}
        self.assertEqual(shown, {1: [1, 3], 2: [1, 2]})
        self.assertIn(2, instances[1], "list A has the level its template shows")
        formats = [self.val(instances[1][k], f"{W}numFmt") for k in range(3)]
        self.assertEqual(len(set(formats)), 3, "each level counts in a format of its own")

    def test_label_mark_style_cases_have_the_style_their_labels_name(self):
        name = "E-list-label-mark-style"
        instances = self.numbering(name)
        properties = {
            "colour FF0000": [("color", {f"{W}val": "FF0000"})],
            "bold": [("b", {}), ("bCs", {})], "italic": [("i", {}), ("iCs", {})],
            "capitals": [("caps", {})], "struck through": [("strike", {})],
            "w:rFonts w:ascii Times New Roman": [("rFonts", {f"{W}ascii": "Times New Roman"})],
            "single underline": [("u", {f"{W}val": "single"})],
        }
        items = self.items(name)
        self.assertEqual(len(items), 9)
        self.assertEqual(len({num_id for _, num_id, _, _ in items}), 9, "a list each")
        for n, (label, num_id, ilvl, p) in enumerate(items, 1):
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            mark = p.find(f"{W}pPr/{W}rPr")
            mark_style = self.val(p, f"{W}pPr/{W}rPr/{W}rStyle")
            match = re.search(r"mark style (\w+)(?: \(([^)]+)\))?", label)
            if match:
                self.assertEqual(mark_style, match.group(1), label)
                style_ = self.style(name, mark_style)
                self.assertEqual(style_.get(f"{W}type"), "character")
                said = match.group(2) or "colour FF0000"
                self.assertEqual(said == "colour FF0000", mark_style == "MarkRed", label)
                self.assertEqual([(c.tag, dict(c.attrib)) for c in style_.find(f"{W}rPr")],
                                 [(f"{W}{tag}", attributes) for tag, attributes in
                                  properties[said]], label)
            else:
                self.assertIn("no mark style", label)
                self.assertIsNone(mark_style, label)
            fmt = self.val(instances[num_id][0], f"{W}numFmt")
            self.assertEqual(fmt, "lowerLetter" if "capitals" in label else "decimal", label)
            direct = [c for c in (mark if mark is not None else []) if c.tag != f"{W}rStyle"]
            if "a double underline on the mark" in label:
                self.assertEqual([(c.tag, c.get(f"{W}val")) for c in direct],
                                 [(f"{W}u", "double")])
            else:
                self.assertEqual(direct, [], label)
            p_style = self.val(p, f"{W}pPr/{W}pStyle")
            if "paragraph style ParaUnderline (single underline)" in label:
                self.assertEqual(p_style, "ParaUnderline")
                style_ = self.style(name, "ParaUnderline")
                self.assertEqual(self.val(style_, f"{W}basedOn"), "Normal")
                (only,) = style_.find(f"{W}rPr")
                self.assertEqual((only.tag, only.get(f"{W}val")), (f"{W}u", "single"))
            else:
                self.assertIsNone(p_style, label)
        self.assertNotIn(b"highlight", self.parts(name)["word/styles.xml"])

    def test_label_mark_highlight_cases_have_the_highlight_their_labels_name(self):
        name = "E-list-label-mark-highlight"
        style_ = self.style(name, "MarkHighlight")
        self.assertEqual(style_.get(f"{W}type"), "character")
        (only,) = style_.find(f"{W}rPr")
        self.assertEqual((only.tag, only.get(f"{W}val")), (f"{W}highlight", "yellow"))
        items = self.items(name)
        self.assertEqual(len(items), 2)
        (first,) = items[0][3].find(f"{W}pPr/{W}rPr")
        self.assertIn("mark style MarkHighlight (highlight yellow)", items[0][0])
        self.assertEqual((first.tag, first.get(f"{W}val")), (f"{W}rStyle", "MarkHighlight"))
        (second,) = items[1][3].find(f"{W}pPr/{W}rPr")
        self.assertIn("a highlight yellow on the mark itself", items[1][0])
        self.assertEqual((second.tag, second.get(f"{W}val")), (f"{W}highlight", "yellow"))

    def test_high_levels_are_the_ones_their_labels_name(self):
        name = "E-value-ilvl-high"
        (levels,) = self.numbering(name).values()
        self.assertEqual(sorted(levels), list(range(9)), "nine levels")
        for ilvl, lvl in levels.items():
            # Each label names its level: L<level>-<number>.
            self.assertEqual(self.val(lvl, f"{W}lvlText"), f"L{ilvl}-%{ilvl + 1}")
        items = self.items(name)
        self.assertEqual([ilvl for _, _, ilvl, _ in items], [0, 9, 12, 8, 0])
        for n, (label, _, ilvl, _) in enumerate(items, 1):
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            if "w:ilvl of" in label:
                self.assertIn(f"a direct w:ilvl of {ilvl}", label)
                self.assertGreater(ilvl, max(levels))
            else:
                self.assertIn(f"level {ilvl}", label)
                self.assertIn(ilvl, levels)
        self.assertIn("past the last of the nine levels", items[1][0])


class BatchFourteenParagraphs(BatchFourteen):
    def case_paragraphs(self, name):
        return [p for p in self.body(name).findall(f"{W}p")
                if self.texts(p).startswith("Case ")]

    def test_paragraph_units_normal_cases_have_the_sizes_their_labels_name(self):
        name = "E-paragraph-units-normal"
        styles = self.xml(name, "word/styles.xml")
        defaults = styles.find(f"{W}docDefaults/{W}rPrDefault/{W}rPr")
        self.assertEqual(self.val(defaults, f"{W}sz"), "22", "document defaults of 11 pt")
        normal = self.style(name, "Normal")
        self.assertEqual([c.tag for c in normal.find(f"{W}rPr")], [f"{W}sz", f"{W}szCs"])
        self.assertEqual(self.val(normal, f"{W}rPr/{W}sz"), "32", "Normal 16 pt")
        self.assertIsNone(normal.find(f"{W}pPr"))
        paragraphs = self.body(name).findall(f"{W}p")
        self.assertEqual(len(paragraphs), 10)
        previous = None
        for n, (label_p, p) in enumerate(zip(paragraphs[::2], paragraphs[1::2]), 1):
            # The label is a paragraph of its own, in Normal, above the case.
            label = self.texts(label_p)
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            self.assertIsNone(label_p.find(f"{W}pPr"), label)
            self.assertIsNone(label_p.find(f"{W}r/{W}rPr"), label)
            ind = p.find(f"{W}pPr/{W}ind")
            self.assertEqual(dict(ind.attrib), {f"{W}leftChars": "200"}, label)
            self.assertIn("leftChars 200", label)
            self.assertEqual(p.find(f"{W}pPr/{W}shd").get(f"{W}fill"), rules.GREY)
            (sample_run,) = p.findall(f"{W}r")
            sample = self.texts(sample_run)
            sizes = []
            for props in (sample_run.find(f"{W}rPr"), p.find(f"{W}pPr/{W}rPr")):
                sized = props is not None and props.find(f"{W}sz") is not None
                sizes.append((int(self.val(props, f"{W}sz")) / 2,
                              int(self.val(props, f"{W}szCs")) / 2) if sized else None)
            self.assertEqual(len(set(sizes)), 1, "run and mark alike")
            fonts = sample_run.find(f"{W}rPr/{W}rFonts")
            match = re.search(r"w:sz (\d+) pt and w:szCs (\d+) pt", label)
            if "no other size" in label:
                self.assertEqual(sizes[0], None)
            elif "the same sizes" in label:
                self.assertEqual(sizes[0], previous, label)
            else:
                self.assertEqual(sizes[0], (float(match.group(1)), float(match.group(2))))
                self.assertNotEqual(*sizes[0])
            if "East Asian text" in label:
                self.assertTrue(all(unicodedata.east_asian_width(c) in "WF" for c in sample),
                                sample)
                self.assertTrue(any(unicodedata.name(c).startswith("CJK UNIFIED") for c in sample))
                self.assertIn("East Asian text in SimSun", label)
                self.assertEqual(dict(fonts.attrib), {f"{W}eastAsia": "SimSun"})
            else:
                self.assertTrue(sample.isascii(), label)
                self.assertIsNone(fonts, label)
            previous = sizes[0]

    def test_paragraph_units_grids_have_the_grids_their_labels_name(self):
        name = "E-paragraph-units-grids"
        body = self.body(name)
        sections, current = [], []
        for p in body.findall(f"{W}p"):
            if self.texts(p).startswith("Case "):
                current.append(p)
            sect = p.find(f"{W}pPr/{W}sectPr")
            if sect is not None:
                sections.append((sect, current))
                current = []
        sections.append((body.find(f"{W}sectPr"), current))
        self.assertEqual(len(sections), 4)
        types = []
        n = 0
        for sect, cases in sections:
            grid = sect.find(f"{W}docGrid")
            self.assertIn(self.val(sect, f"{W}type"), (None, "nextPage"), "a page each")
            self.assertEqual(int(grid.get(f"{W}linePitch")) / 20, 18)
            types.append(grid.get(f"{W}type"))
            self.assertEqual(len(cases), 2)
            for p in cases:
                n += 1
                label = self.texts(p)
                self.assertTrue(label.startswith(f"Case {n:02d}: grid {types[-1]}, pitch 18 pt; "),
                                label)
                space = {k.replace(W, ""): v for k, v in p.find(f"{W}pPr/{W}spacing").attrib.items()}
                if "beforeLines 100 beside 6 pt before" in label:
                    self.assertEqual(space, {"before": "120", "beforeLines": "100"})
                else:
                    self.assertIn("6 pt before only", label)
                    self.assertEqual(space, {"before": "120"})
        self.assertEqual(types, ["lines", "linesAndChars", "snapToChars", "default"])

    def test_negative_line_cases_have_the_spacing_their_labels_name(self):
        name = "E-value-line-negative"
        cases = self.case_paragraphs(name)
        found = []
        for n, p in enumerate(cases, 1):
            label = self.texts(p)
            space = p.find(f"{W}pPr/{W}spacing")
            value, rule = int(space.get(f"{W}line")), space.get(f"{W}lineRule")
            self.assertTrue(label.startswith(f"Case {n:02d}: w:spacing w:line {value} "
                                             f"w:lineRule {rule}: "), label)
            self.assertLess(value, 0)
            self.assertEqual(len(space.attrib), 2)
            # "a third line": the text takes three lines of Calibri 11 pt at least.
            self.assertGreaterEqual(len(self.wrapped_lines(label, rules.TEXT_W / 20)), 3,
                                    label)
            found.append((value, rule))
        self.assertEqual(found, [(-240, "auto"), (-480, "auto"), (-480, "exact"),
                                 (-480, "atLeast")])

    def test_outline_level_cases_have_the_levels_their_labels_name(self):
        name = "E-value-outline-level"
        found = []
        for n, p in enumerate(self.case_paragraphs(name), 1):
            value = int(self.val(p, f"{W}pPr/{W}outlineLvl"))
            self.assertEqual(self.texts(p), f"Case {n:02d}: w:outlineLvl {value}")
            self.assertEqual(len(p.find(f"{W}pPr")), 1)
            found.append(value)
        self.assertEqual(found, [0, -3, -2, 25, 9])
        outside = [v for v in found if not 0 <= v <= 9]
        self.assertEqual(outside, [-3, -2, 25], self.docs[name].purpose)

    def test_negative_hanging_cases_have_the_indents_their_labels_name(self):
        name = "E-value-hanging-negative"
        expected = {"left 72 pt, w:hanging -18 pt": {"left": "1440", "hanging": "-360"},
                    "left 72 pt, w:hanging 18 pt": {"left": "1440", "hanging": "360"},
                    "left 72 pt, w:firstLine 18 pt": {"left": "1440", "firstLine": "360"}}
        cases = self.case_paragraphs(name)
        self.assertEqual(len(cases), 3)
        for n, (p, (said, ind)) in enumerate(zip(cases, expected.items()), 1):
            label = self.texts(p)
            self.assertTrue(label.startswith(f"Case {n:02d}: {said}"), label)
            found = {k.replace(W, ""): v for k, v in p.find(f"{W}pPr/{W}ind").attrib.items()}
            self.assertEqual(found, ind)
            # Two lines at least, at the 72 pt indent (54 pt for the first line of case 01, 90
            # pt for case 03's), so that the hanging indent shows.
            self.assertGreaterEqual(len(self.wrapped_lines(label, rules.TEXT_W / 20 - 54)), 2,
                                    label)

    def test_spacing_minus_one_cases_have_the_spacing_their_labels_name(self):
        name = "E-value-spacing-minus-one"
        spaced = self.style(name, "Spaced")
        self.assertEqual(self.val(spaced, f"{W}basedOn"), "Normal")
        space = spaced.find(f"{W}pPr/{W}spacing")
        self.assertEqual((int(space.get(f"{W}before")) / 20, int(space.get(f"{W}after")) / 20),
                         (24, 24))
        paragraphs = self.body(name).findall(f"{W}p")
        expected = [{"before": "-1"}, {"after": "-1"}, {"before": "-1", "after": "-1"},
                    {"before": "0", "after": "0"}, None]
        cases = [(k, p) for k, p in enumerate(paragraphs) if self.texts(p).startswith("Case ")]
        self.assertEqual(len(cases), 5)
        for n, ((k, p), spacing_) in enumerate(zip(cases, expected), 1):
            label = self.texts(p)
            self.assertTrue(label.startswith(f"Case {n:02d}: style Spaced (24 pt before and "
                                             "after); "), label)
            self.assertEqual(self.val(p, f"{W}pPr/{W}pStyle"), "Spaced")
            found = p.find(f"{W}pPr/{W}spacing")
            if spacing_ is None:
                self.assertIsNone(found)
                self.assertIn("no spacing of its own", label)
                continue
            attributes = {a.replace(W, ""): v for a, v in found.attrib.items()}
            self.assertEqual(attributes, spacing_)
            said = " and ".join(f"w:{a} {v}" for a, v in spacing_.items())
            self.assertIn(said, label)
            # Its neighbours are in Normal with no spacing of their own.
            for neighbour in (paragraphs[k - 1], paragraphs[k + 1]):
                self.assertIsNone(neighbour.find(f"{W}pPr"), self.texts(neighbour))

    def test_short_theme_tint_cases_have_the_values_their_labels_name(self):
        name = "E-value-theme-tint-short"
        theme = self.xml(name, "word/theme/theme1.xml")
        accent1 = theme.find(f".//{A}accent1/{A}srgbClr").get("val")
        self.assertEqual(accent1, "336699")
        cases = self.case_paragraphs(name)
        self.assertEqual(len(cases), 6)
        digits = []
        for n, p in enumerate(cases, 1):
            label = self.texts(p)
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            (props,) = p.findall(f"{W}r")[1].find(f"{W}rPr")
            attribute, value = re.search(r'w:(\w+) "(\w+)"', label).groups()
            if label.startswith(f"Case {n:02d}: text in accent1"):
                self.assertEqual(props.tag, f"{W}color")
                self.assertEqual((props.get(f"{W}themeColor"), props.get(f"{W}val")),
                                 ("accent1", "000000"))
            else:
                self.assertIn("run shading in accent1", label)
                self.assertEqual(props.tag, f"{W}shd")
                self.assertEqual((props.get(f"{W}themeFill"), props.get(f"{W}fill")),
                                 ("accent1", "000000"))
            self.assertEqual(props.get(f"{W}{attribute}"), value, label)
            themed = {a for a in props.attrib if "Tint" in a or "Shade" in a}
            self.assertEqual(themed, {f"{W}{attribute}"}, label)
            self.assertEqual(len(value) == 2, "(to compare)" in label, label)
            digits.append(len(value))
        self.assertEqual(digits, [1, 2, 1, 2, 1, 2])

    def test_char_scale_cases_have_the_scales_their_labels_name(self):
        name = "E-char-scale-zero"
        cases = self.case_paragraphs(name)
        scales = []
        for n, p in enumerate(cases, 1):
            label, sample, after = [self.texts(r) for r in p.findall(f"{W}r")]
            scale = int(self.val(p.findall(f"{W}r")[1], f"{W}rPr/{W}w"))
            self.assertEqual(label, f"Case {n:02d}: w:w {scale}, between the bars: |")
            self.assertEqual((sample, after), ("Hamburgefontsiv Hamburgefontsiv", "|"))
            self.assertIsNone(p.findall(f"{W}r")[0].find(f"{W}rPr"))
            self.assertIsNone(p.findall(f"{W}r")[2].find(f"{W}rPr"))
            # One line even at 100 %.
            self.assertLess(carlito_width(label + sample + after), rules.TEXT_W / 20)
            scales.append(scale)
        self.assertEqual(scales, [0, 100, 1])

    def test_hash_colour_cases_have_the_values_their_labels_name(self):
        name = "E-color-hash"
        cases = self.case_paragraphs(name)
        self.assertEqual(len(cases), 5)
        for n, p in enumerate(cases, 1):
            label = self.texts(p.findall(f"{W}r")[0])
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            (value,) = re.findall(r'"(#?[0-9A-F]{6})"', label)
            sample = p.findall(f"{W}r")[1]
            if "paragraph shading" in label:
                element, attribute = p.find(f"{W}pPr/{W}shd"), "fill"
                self.assertIsNone(sample.find(f"{W}rPr"))
            elif "run shading" in label:
                element, attribute = sample.find(f"{W}rPr/{W}shd"), "fill"
            elif label.startswith(f"Case {n:02d}: w:u"):
                element, attribute = sample.find(f"{W}rPr/{W}u"), "color"
                self.assertIn("on black text", label)
                self.assertIsNone(sample.find(f"{W}rPr/{W}color"))
            else:
                element, attribute = sample.find(f"{W}rPr/{W}color"), "val"
            self.assertEqual(element.get(f"{W}{attribute}"), value, label)
            self.assertEqual(value.startswith("#"), "(to compare)" not in label, label)

    def leader_font_codes(self, path, platform, encoding):
        """The characters a cmap subtable of format 4 maps to a glyph, or None without it."""
        data = path.read_bytes()
        (count,) = struct.unpack(">H", data[4:6])
        tables = {}
        for i in range(count):
            tag, _, offset, length = struct.unpack(">4sIII", data[12 + 16 * i: 28 + 16 * i])
            tables[tag] = data[offset: offset + length]
        cmap = tables[b"cmap"]
        (subtables,) = struct.unpack(">H", cmap[2:4])
        for i in range(subtables):
            found = struct.unpack(">HHI", cmap[4 + 8 * i: 12 + 8 * i])
            if found[:2] == (platform, encoding):
                sub = cmap[found[2]:]
                break
        else:
            return None
        self.assertEqual(struct.unpack(">H", sub[0:2]), (4,))
        segments = struct.unpack(">H", sub[6:8])[0] // 2
        ends = struct.unpack(f">{segments}H", sub[14: 14 + 2 * segments])
        at = 16 + 2 * segments
        starts = struct.unpack(f">{segments}H", sub[at: at + 2 * segments])
        deltas = struct.unpack(f">{segments}h", sub[at + 2 * segments: at + 4 * segments])
        ranges_at = at + 4 * segments
        ranges = struct.unpack(f">{segments}H", sub[ranges_at: ranges_at + 2 * segments])
        codes = set()
        for k in range(segments):
            for code in range(starts[k], ends[k] + 1):
                if ranges[k] == 0:
                    glyph = (code + deltas[k]) & 0xFFFF
                else:
                    place = ranges_at + 2 * k + ranges[k] + 2 * (code - starts[k])
                    (glyph,) = struct.unpack(">H", sub[place: place + 2])
                if glyph and code != 0xFFFF:
                    codes.add(code)
        return codes

    def test_leader_cases_have_the_fonts_and_leaders_their_labels_name(self):
        name = "E-tab-leader-no-underscore"
        font = Path(__file__).resolve().parents[2] / (
            "tests/fonts-word/test-symbol/OfficewrightTestSymbol-Regular.ttf")
        symbol = self.leader_font_codes(font, 3, 0)
        self.assertIsNone(self.leader_font_codes(font, 3, 1), "no Unicode character map")
        self.assertNotIn(ord("_"), symbol)
        self.assertIn(0xF000 + ord("_"), symbol, "the symbol code F05F")
        for leader in ".-·":
            self.assertNotIn(ord(leader), symbol)
            self.assertIn(0xF000 + ord(leader), symbol)
        self.assertIn("(only the symbol code F05F)", self.docs[name].purpose)
        self.assertIn("dot, hyphen and middle dot leaders in it, and underscore and heavy "
                      "leaders in Calibri", self.docs[name].purpose)
        paragraphs = self.body(name).findall(f"{W}p")
        self.assertEqual(len(paragraphs), 14)
        found = []
        for n, (label_p, case) in enumerate(zip(paragraphs[::2], paragraphs[1::2]), 1):
            label = self.texts(label_p)
            match = re.fullmatch(rf"Case {n:02d}: (\w+) leader to a left stop at 216 pt, the tab "
                                 r"in (.+?)( \(to compare\))?:", label)
            self.assertIsNotNone(match, label)
            leader, family = match.group(1), match.group(2)
            (stop,) = case.find(f"{W}pPr/{W}tabs")
            self.assertEqual((stop.get(f"{W}val"), int(stop.get(f"{W}pos")) / 20,
                              stop.get(f"{W}leader")), ("left", 216, leader))
            tab_run_, after = case.findall(f"{W}r")
            self.assertIsNotNone(tab_run_.find(f"{W}tab"))
            fonts = tab_run_.find(f"{W}rPr/{W}rFonts")
            self.assertEqual({fonts.get(f"{W}{slot}") for slot in ("ascii", "hAnsi", "eastAsia",
                                                                   "cs")}, {family})
            self.assertEqual(self.texts(after), "x")
            self.assertEqual(match.group(3) is None,
                             leader in ("underscore", "heavy") and family == rules.SYMBOL_FONT)
            found.append((leader, family))
        self.assertEqual(found, [("underscore", "Officewright Test Symbol"),
                                 ("heavy", "Officewright Test Symbol"),
                                 ("dot", "Officewright Test Symbol"),
                                 ("hyphen", "Officewright Test Symbol"),
                                 ("middleDot", "Officewright Test Symbol"),
                                 ("underscore", "Calibri"), ("heavy", "Calibri")])


class BatchFourteenTableStyles(BatchFourteen):
    def big(self, name):
        big = self.style(name, "Big")
        self.assertEqual(big.get(f"{W}type"), "table")
        self.assertEqual(self.val(big, f"{W}pPr/{W}jc"), "center")
        self.assertEqual(int(self.val(big, f"{W}rPr/{W}sz")) / 2, 14)

    def cells(self, name):
        (table,) = self.body(name).iter(f"{W}tbl")
        self.assertEqual(self.val(table, f"{W}tblPr/{W}tblStyle"), "Big")
        return [p for tc in table.iter(f"{W}tc") for p in tc.findall(f"{W}p")]

    def test_based_on_normal_cases_have_the_styles_their_labels_name(self):
        name = "E-table-style-based-on-normal-14"
        self.big(name)
        self.assertEqual(self.compat_values(name), {"compatibilityMode": "14"})
        normal = self.style(name, "Normal")
        self.assertEqual((self.val(normal, f"{W}pPr/{W}jc"),
                          int(self.val(normal, f"{W}rPr/{W}sz")) / 2), ("left", 12))
        inherit, own = self.style(name, "BasedInherit"), self.style(name, "BasedOwn")
        for based in (inherit, own):
            self.assertEqual(self.val(based, f"{W}basedOn"), "Normal")
            self.assertIsNone(based.find(f"{W}pPr"))
        self.assertIsNone(inherit.find(f"{W}rPr"), "inherits the 12 pt")
        self.assertEqual([(c.tag, c.get(f"{W}val")) for c in own.find(f"{W}rPr")],
                         [(f"{W}sz", "24"), (f"{W}szCs", "24")])
        for p in self.cells(name):
            said = self.texts(p).split(": ")[1]
            self.assertEqual(self.val(p, f"{W}pPr/{W}pStyle"),
                             None if said == "Normal" else said)
        label = self.texts(self.body(name).find(f"{W}p"))
        self.assertIn("Normal 12 pt, left", label)
        self.assertIn("(14 pt, centred)", label)

    def test_no_defaults_case_has_no_run_defaults(self):
        name = "E-table-style-no-defaults"
        self.big(name)
        styles = self.xml(name, "word/styles.xml")
        self.assertIsNone(styles.find(f"{W}docDefaults/{W}rPrDefault"))
        normal = self.style(name, "Normal")
        self.assertEqual([(c.tag, c.get(f"{W}val")) for c in normal.find(f"{W}rPr")],
                         [(f"{W}sz", "22"), (f"{W}szCs", "22")])
        self.assertIsNone(normal.find(f"{W}pPr"))
        # Normal's size is the one Word gives text when the document defaults give none:
        # case 01 of E-defaults-empty's reference, which has no w:rPrDefault either.
        reference = json.loads((Path(__file__).resolve().parents[1] / "corpus/reference/"
                                "word-rules/E-defaults-empty/formatting.json").read_text())
        (story,) = [s for s in reference["stories"] if s["story"] == "main"]
        first = story["paragraphs"][0]
        self.assertTrue(first["text"].startswith("Case 01: no font or size anywhere"))
        self.assertEqual(int(self.val(normal, f"{W}rPr/{W}sz")) / 2,
                         reference["fonts"][first["font"]]["size"])
        empty = self.xml("E-defaults-empty", "word/styles.xml")
        self.assertIsNone(empty.find(f"{W}docDefaults/{W}rPrDefault"))
        self.assertIn("w:sz 22 (11 pt, the size Word gives text without them)",
                      self.texts(self.body(name).find(f"{W}p")))
        self.assertEqual(self.compat_values(name)["overrideTableStyleFontSizeAndJustification"],
                         "1")
        self.assertEqual(len(self.cells(name)), 4)

    def test_normal_both_cases_wrap_and_have_the_justification_their_labels_name(self):
        name = "E-table-style-normal-both"
        self.big(name)
        styles = self.xml(name, "word/styles.xml")
        self.assertEqual(self.val(styles, f"{W}docDefaults/{W}rPrDefault/{W}rPr/{W}sz"), "24")
        self.assertIsNone(styles.find(f"{W}docDefaults/{W}pPrDefault/{W}pPr/{W}jc"))
        normal = self.style(name, "Normal")
        self.assertEqual((self.val(normal, f"{W}pPr/{W}jc"),
                          int(self.val(normal, f"{W}rPr/{W}sz")) / 2), ("both", 11))
        self.assertEqual(self.compat_values(name)["overrideTableStyleFontSizeAndJustification"],
                         "1")
        # Every cell's text runs over a line at 11 pt, so that justification shows.
        cell_width = (rules.TEXT_W // 2 - 2 * 108) / 20
        cells = self.cells(name)
        self.assertEqual(len(cells), 4)
        for p in cells:
            self.assertGreaterEqual(len(self.wrapped_lines(self.texts(p), cell_width)), 2)
        after = self.body(name).findall(f"{W}p")[-1]
        self.assertGreaterEqual(len(self.wrapped_lines(self.texts(after), rules.TEXT_W / 20)),
                                2)


class BatchFourteenStyles(BatchFourteen):
    def case_paragraphs(self, name):
        return [p for p in self.body(name).findall(f"{W}p")
                if self.texts(p).startswith("Case ")]

    def assert_centred_bold(self, style_):
        self.assertEqual(self.val(style_, f"{W}pPr/{W}jc"), "center")
        self.assertEqual([c.tag for c in style_.find(f"{W}rPr")], [f"{W}b", f"{W}bCs"])

    def test_style_id_case_cases_name_ids_in_the_cases_their_labels_say(self):
        name = "E-style-id-case"
        ids = {s.get(f"{W}styleId"): s for s in self.styles(name)}
        self.assert_centred_bold(ids["MixedCase"])
        self.assertEqual([c.tag for c in ids["BasedOnLower"].find(f"{W}rPr")],
                         [f"{W}i", f"{W}iCs"])
        self.assertEqual(self.val(ids["LinkedPara"], f"{W}pPr/{W}jc"), "right")
        self.assertEqual(self.val(ids["LinkedChar"], f"{W}rPr/{W}u"), "single")
        self.assertEqual(ids["LinkedChar"].get(f"{W}type"), "character")
        named = set()
        for n, p in enumerate(self.case_paragraphs(name), 1):
            label = self.texts(p)
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            (value,) = re.findall(r'(?:w:pStyle|w:rStyle) "(\w+)"', label)
            kind = "rStyle" if "w:rStyle" in label else "pStyle"
            found = p.find(f".//{W}{kind}")
            self.assertEqual(found.get(f"{W}val"), value, label)
            named.add(value)
            if "as written" in label:
                self.assertIn(value, ids)
            target = re.search(r"the id ([A-Z]\w*)", label)
            if target:
                target = target.group(1)
                if "in lower case" in label:
                    self.assertEqual(value, target.lower())
                elif "in capitals" in label:
                    self.assertEqual(value, target.upper())
                self.assertEqual(value == target, "as written" in label, label)
                self.assertIn(target, ids)
        self.assertEqual(self.val(ids["BasedOnLower"], f"{W}basedOn"), "mixedcase")
        self.assertEqual(self.val(ids["LinkedPara"], f"{W}link"), "linkedchar")
        self.assertEqual(self.val(ids["LinkedChar"], f"{W}link"), "linkedpara")
        named |= {"mixedcase", "linkedchar", "linkedpara"}
        # The values in another case match an id only without regard to case, and no name.
        for value in named:
            matches = [i for i in ids if i.lower() == value.lower()]
            self.assertEqual(len(matches), 1, value)
            for style_ in ids.values():
                style_name = self.val(style_, f"{W}name")
                self.assertNotEqual(style_name.replace(" ", "").lower(), value.lower())
        self.assertEqual({v for v in named if v not in ids},
                         {"mixedcase", "MIXEDCASE", "linkedchar", "linkedpara"})

    def test_repeated_ids_are_the_styles_the_labels_name(self):
        name = "E-style-id-repeated"
        by_id = {}
        for style_ in self.styles(name):
            by_id.setdefault(style_.get(f"{W}styleId"), []).append(style_)
        first, second = by_id["Twice"]
        self.assertEqual([self.val(s, f"{W}name") for s in (first, second)],
                         ["Twice first", "Twice second"])
        self.assert_centred_bold(first)
        self.assertEqual(self.val(second, f"{W}pPr/{W}jc"), "right")
        self.assertEqual([c.tag for c in second.find(f"{W}rPr")], [f"{W}i", f"{W}iCs"])
        first, second = by_id[""]
        self.assertEqual([self.val(s, f"{W}name") for s in (first, second)],
                         ["Empty id first", "Empty id second"])
        self.assertEqual([int(s.find(f"{W}pPr/{W}spacing").get(f"{W}before")) / 20
                          for s in (first, second)], [12, 24])
        self.assertEqual((self.val(first, f"{W}rPr/{W}u"), first.find(f"{W}rPr/{W}strike")),
                         ("single", None))
        self.assertEqual([c.tag for c in second.find(f"{W}rPr")], [f"{W}strike"])
        cases = self.case_paragraphs(name)
        self.assertEqual([self.val(p, f"{W}pPr/{W}pStyle") for p in cases], ["Twice", "", None])
        self.assertIn('w:pStyle "Twice"', self.texts(cases[0]))
        self.assertIn('w:pStyle ""', self.texts(cases[1]))

    def test_missing_id_cases_name_the_style_by_its_name(self):
        name = "E-style-id-missing"
        without = [s for s in self.styles(name) if s.get(f"{W}styleId") is None]
        (style_,) = without
        self.assertEqual(self.val(style_, f"{W}name"), "No Id Style")
        self.assert_centred_bold(style_)
        ids = {s.get(f"{W}styleId") for s in self.styles(name)}
        cases = self.case_paragraphs(name)
        values = [self.val(p, f"{W}pPr/{W}pStyle") for p in cases]
        self.assertEqual(values, ["NoIdStyle", "No Id Style", None])
        self.assertEqual(values[0], "No Id Style".replace(" ", ""))
        for value, p in zip(values[:2], cases):
            self.assertIn(f'w:pStyle "{value}"', self.texts(p))
            self.assertNotIn(value, ids)

    def test_unknown_type_cases_name_the_style_of_no_type(self):
        name = "E-style-type-unknown"
        style_ = self.style(name, "UnknownType")
        self.assertEqual(style_.get(f"{W}type"), "unknown")
        self.assertNotIn("unknown", ("paragraph", "character", "table", "numbering"))
        self.assert_centred_bold(style_)
        cases = self.case_paragraphs(name)
        self.assertEqual([(self.val(p, f"{W}pPr/{W}pStyle"), self.val(p, f".//{W}rStyle"))
                          for p in cases],
                         [("UnknownType", None), (None, "UnknownType"), (None, None)])
        self.assertIn('w:type "unknown"', self.texts(cases[0]))


class BatchFourteenSectionsAndWhitespace(BatchFourteen):
    def test_sectpr_in_cell_has_the_page_sizes_its_labels_name(self):
        name = "E-sectpr-in-cell"
        body = self.body(name)
        (table,) = body.iter(f"{W}tbl")
        (inner,) = table.iter(f"{W}sectPr")
        size = inner.find(f"{W}pgSz")
        width, height = int(size.get(f"{W}w")) / 20, int(size.get(f"{W}h")) / 20
        self.assertEqual((width, height), (792, 612), "Letter in landscape")
        first_cell = table.find(f"{W}tr/{W}tc/{W}p")
        self.assertIs(first_cell.find(f"{W}pPr/{W}sectPr"), inner)
        self.assertIn("792 x 612 pt", self.texts(first_cell))
        last = body.find(f"{W}sectPr").find(f"{W}pgSz")
        self.assertEqual((int(last.get(f"{W}w")), int(last.get(f"{W}h"))), (11906, 16838))
        self.assertEqual(len(list(body.iter(f"{W}sectPr"))), 2, "no other section break")
        self.assertIn("(792 x 612 pt); the last section is A4 portrait",
                      self.texts(body.find(f"{W}p")))

    def positions(self, name):
        """Per case: its label, and (relativeFrom, kind, text) across and down."""
        found = []
        for p in self.body(name).findall(f"{W}p"):
            anchor = p.find(f".//{WP}anchor")
            if anchor is None:
                continue
            self.assertEqual(int(anchor.find(f"{WP}extent").get("cx")) / 12700, 72)
            self.assertEqual(int(anchor.find(f"{WP}extent").get("cy")) / 12700, 72)
            places = []
            for axis in ("positionH", "positionV"):
                position = anchor.find(f"{WP}{axis}")
                (child,) = position
                places.append((position.get("relativeFrom"), child.tag.replace(WP, ""),
                               child.text))
            found.append((self.texts(p), p, places))
        return found

    def test_pos_offset_cases_have_the_whitespace_their_labels_name(self):
        name = "E-whitespace-pos-offset"
        cases = self.positions(name)
        self.assertEqual(len(cases), 2)
        for n, (label, p, places) in enumerate(cases, 1):
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            self.assertEqual(p.find(f"{W}pPr/{W}pageBreakBefore") is not None, n == 2)
            values = []
            for relative, kind, value in places:
                self.assertEqual((relative, kind), ("page", "posOffset"))
                if n == 1:
                    self.assertEqual(value[0], "\n", "a line feed before the number")
                    self.assertTrue(value[1:].isdigit())
                else:
                    self.assertTrue(value.isdigit())
                values.append(int(value) / 12700)
            self.assertEqual(values, [144, 216])
        self.assertIn("144 pt from the page's left edge and 216 pt from its top", cases[0][0])

    def test_align_cases_have_the_whitespace_their_labels_name(self):
        name = "E-whitespace-align"
        cases = self.positions(name)
        expected = [
            [("margin", "align", "\ncenter\n"), ("page", "posOffset", str(216 * 12700))],
            [("page", "posOffset", str(144 * 12700)), ("margin", "align", "\nbottom\n")],
            [("margin", "align", "center"), ("margin", "align", "bottom")],
        ]
        self.assertEqual([places for _, _, places in cases], expected)
        for n, (label, p, _) in enumerate(cases, 1):
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            self.assertEqual(p.find(f"{W}pPr/{W}pageBreakBefore") is not None, n > 1)
        self.assertIn("a line feed before and after center", cases[0][0])
        self.assertIn("216 pt from the page's top", cases[0][0])
        self.assertIn("a line feed before and after bottom", cases[1][0])
        self.assertIn("144 pt from the page's left edge", cases[1][0])
        self.assertIn("no whitespace in wp:align", cases[2][0])

    def test_compat_values_have_a_space_around_them(self):
        name = "E-whitespace-compat"
        values = self.compat_values(name)
        self.assertEqual(values, {
            "compatibilityMode": " 15 ", "overrideTableStyleFontSizeAndJustification": " 1 ",
            "enableOpenTypeFeatures": " 1 ", "doNotFlipMirrorIndents": " 1 ",
            "differentiateMultirowTableHeaders": " 1 "})
        compat = self.settings(name).find(f"{W}compat")
        self.assertEqual([c.tag for c in compat if c.tag != f"{W}compatSetting"],
                         [f"{W}doNotUseIndentAsNumberingTabStop"])
        (item,) = self.items(name)
        lvl = self.numbering(name)[item[1]][item[2]]
        ind = lvl.find(f"{W}pPr/{W}ind")
        self.assertEqual((int(ind.get(f"{W}left")) / 20, int(ind.get(f"{W}hanging")) / 20,
                          self.val(lvl, f"{W}suff")), (50, 50, "tab"))
        self.assertIn("level indent left 50 pt, hanging 50 pt", item[0])
        paragraphs = self.body(name).findall(f"{W}p")
        (ligatures,) = [p for p in paragraphs if p.find(f".//{W14}ligatures") is not None]
        self.assertEqual(ligatures.find(f".//{W14}ligatures").get(f"{W14}val"), "standard")
        styles = self.xml(name, "word/styles.xml")
        defaults = styles.find(f"{W}docDefaults")
        self.assertEqual((self.val(defaults, f"{W}rPrDefault/{W}rPr/{W}sz"),
                          self.val(defaults, f"{W}pPrDefault/{W}pPr/{W}jc")), ("24", "right"))
        normal = self.style(name, "Normal")
        self.assertEqual((self.val(normal, f"{W}pPr/{W}jc"), self.val(normal, f"{W}rPr/{W}sz")),
                         ("left", "22"))
        self.assertEqual(defaults.find(f"{W}rPrDefault/{W}rPr/{W}rFonts").get(f"{W}ascii"),
                         "Calibri")
        self.assertIsNone(ligatures.find(f".//{W}sz"), "Normal's 11 pt")
        big = self.style(name, "Big")
        self.assertEqual((self.val(big, f"{W}pPr/{W}jc"), self.val(big, f"{W}rPr/{W}sz")),
                         ("center", "28"))


class BatchFourteenThemeFonts(BatchFourteen):
    SCRIPTS = {"my-MM": "Mymr", "jv-ID": "Java", "nqo": "Nkoo", "sat": "Olck",
               "zgh-Tfng": "Tfng"}

    def test_theme_font_languages_select_a_script_the_theme_gives_a_font(self):
        names = sorted(n for n in self.docs if n.startswith("E-theme-font-lang-"))
        self.assertEqual(len(names), 5)
        tags = []
        for name in names:
            settings = self.settings(name)
            lang = settings.find(f"{W}themeFontLang")
            values = {lang.get(f"{W}{k}") for k in ("val", "eastAsia", "bidi")}
            (tag,) = values
            tags.append(tag)
            self.assertIn(f"gives {tag} (", self.docs[name].purpose)
            self.assertTrue(name.endswith("-" + tag.split("-")[0]), name)
            theme = self.xml(name, "word/theme/theme1.xml")
            typefaces = set()
            for kind in ("major", "minor"):
                collection = theme.find(f".//{A}{kind}Font")
                if kind == "minor":
                    self.assertEqual([collection.find(f"{A}{slot}").get("typeface")
                                      for slot in ("latin", "ea", "cs")],
                                     ["Verdana", "", "Tahoma"])
                scripts = {f.get("script"): f.get("typeface") for f in collection.iter(f"{A}font")}
                self.assertEqual(scripts, {"Mymr": "Constantia", "Java": "Cambria",
                                           "Nkoo": "Consolas", "Olck": "Trebuchet MS",
                                           "Tfng": "Noto Sans"})
                typefaces |= set(scripts.values())
                self.assertIn(self.SCRIPTS[tag], scripts)
            # Each script's font differs from the others, from the theme's own and the
            # labels' fonts, and from the fonts Word commonly falls back to for an empty slot,
            # so the font of a case names the script.
            self.assertEqual(len(typefaces), 5)
            self.assertFalse(typefaces & {"Verdana", "Tahoma", "Georgia", "Calibri",
                                          "Times New Roman", "Arial", "Courier New",
                                          "Segoe UI", "MS Mincho", "SimSun", "MS Gothic",
                                          "Yu Mincho", "Yu Gothic"})
            self.assertIn("in the theme's Latin, East Asian and complex script slots",
                          self.docs[name].purpose)
            fonts = ", ".join(f"{s} {t}" for s, t in scripts.items())
            self.assertIn(fonts, self.docs[name].purpose)
            self.check_cases(name, tag)
        self.assertEqual(sorted(tags), sorted(self.SCRIPTS))

    def check_cases(self, name, tag):
        paragraphs = self.body(name).findall(f"{W}p")
        self.assertEqual(len(paragraphs), 12)
        slots, runs = [], []
        for n, (label_p, case) in enumerate(zip(paragraphs[::2], paragraphs[1::2]), 1):
            label = self.texts(label_p)
            self.assertTrue(label.startswith(f"Case {n:02d}: "), label)
            (r,) = case.findall(f"{W}r")
            props = r.find(f"{W}rPr")
            lang = props.find(f"{W}lang")
            if n > 3:
                # The text of case n - 3, in the language of w:themeFontLang.
                self.assertEqual(label, f"Case {n:02d}: case 0{n - 3}'s text with w:lang {tag} "
                                        "on the run:")
                self.assertEqual({lang.get(f"{W}{k}") for k in ("val", "eastAsia", "bidi")},
                                 {tag})
                props.remove(lang)
                self.assertEqual(ElementTree.tostring(r), runs[n - 4])
                continue
            self.assertIsNone(lang)
            runs.append(ElementTree.tostring(r))
            fonts = {k.replace(W, ""): v for k, v in props.find(f"{W}rFonts").attrib.items()}
            sample = self.texts(r)
            toggles = [c.tag for c in props if c.tag != f"{W}rFonts"]
            if "minorEastAsia" in label:
                self.assertEqual(fonts, {"eastAsiaTheme": "minorEastAsia", "hint": "eastAsia"})
                self.assertIn("w:hint eastAsia", label)
                letters = [c for c in sample if not c.isspace()]
                self.assertTrue(all(unicodedata.name(c).split()[0] in ("GREEK", "CYRILLIC")
                                    for c in letters), sample)
                self.assertEqual({unicodedata.name(c).split()[0] for c in letters},
                                 {"GREEK", "CYRILLIC"})
                self.assertEqual(toggles, [])
                slots.append("ea")
            elif "minorBidi" in label:
                self.assertEqual(fonts, {"cstheme": "minorBidi"})
                self.assertEqual(toggles, [f"{W}cs"])
                self.assertIn("with w:cs", label)
                self.assertTrue(sample.isascii() and sample.isalpha())
                slots.append("cs")
            else:
                self.assertIn("minorHAnsi", label)
                self.assertEqual(fonts, {"asciiTheme": "minorHAnsi", "hAnsiTheme": "minorHAnsi"})
                self.assertEqual(toggles, [])
                self.assertTrue(sample.isascii() and sample.isalpha())
                slots.append("latin")
        self.assertEqual(slots, ["latin", "ea", "cs"])


class BatchFourteenIsolation(BatchFourteen):
    """Markup Word may refuse stays in the document about it (the brief's group run last)."""

    def odd_markup(self, name):
        parts = self.parts(name)
        found = set()
        stories = [n for n in parts if n.startswith("word/") and n.endswith(".xml")
                   and n.split("/")[1].startswith(("document", "header", "footer", "footnotes",
                                                   "endnotes", "styles", "numbering"))]
        for part_name in stories:
            root = ElementTree.fromstring(parts[part_name])
            for element in root.iter():
                tag = element.tag
                for attribute, value in element.attrib.items():
                    local = attribute.replace(W, "")
                    # w:color's w:val, w:u's w:color, w:shd's w:color and w:fill (VML's
                    # colours are #RRGGBB by its own schema).
                    colour = local in ("color", "fill") or (tag == f"{W}color" and local == "val")
                    if tag.startswith(W) and colour and value.startswith("#"):
                        found.add("hash colour")
                    if tag.startswith(W) and local in ("themeTint", "themeShade",
                                                       "themeFillTint", "themeFillShade") \
                            and len(value) != 2:
                        found.add("short tint")
                if tag == f"{W}w" and int(element.get(f"{W}val")) < 1:
                    found.add("scale below 1")
                if tag == f"{W}ind" and int(element.get(f"{W}hanging", "0")) < 0:
                    found.add("negative hanging")
                if tag == f"{W}spacing":
                    for side in ("before", "after"):
                        if int(element.get(f"{W}{side}", "0")) < 0:
                            found.add("negative spacing")
                    if int(element.get(f"{W}line", "0")) < 0:
                        found.add("negative line")
                if tag == f"{W}outlineLvl" and not 0 <= int(element.get(f"{W}val")) <= 9:
                    found.add("outline level")
                if (tag == f"{W}ilvl" and part_name == "word/document.xml"
                        and int(element.get(f"{W}val")) > 8):
                    found.add("high level")
                if tag == f"{W}lvlText":
                    value = element.get(f"{W}val")
                    if len(value) > 255:
                        found.add("long template")
                    if any(unicodedata.category(c) in ("Cc", "Zl", "Zp") for c in value) \
                            or "_x" in value:
                        found.add("template control")
                if tag in (f"{WP}posOffset", f"{WP}align") and element.text != \
                        element.text.strip():
                    found.add(f"whitespace {tag.replace(WP, '')}")
            for tc in root.iter(f"{W}tc"):
                if tc.find(f".//{W}sectPr") is not None:
                    found.add("section in a cell")
            if part_name == "word/styles.xml":
                styles = list(root.iter(f"{W}style"))
                ids = [s.get(f"{W}styleId") for s in styles]
                if None in ids:
                    found.add("style without id")
                if len([i for i in ids if i is not None]) != len({i for i in ids if i is not None}):
                    found.add("repeated style id")
                if any(s.get(f"{W}type") not in ("paragraph", "character", "table", "numbering")
                       for s in styles):
                    found.add("unknown style type")
                if any(s.find(f"{W}rPr/{W}highlight") is not None for s in styles):
                    found.add("highlight in a style")
        settings = ElementTree.fromstring(parts["word/settings.xml"])
        for setting in settings.iter(f"{W}compatSetting"):
            if setting.get(f"{W}val") != setting.get(f"{W}val").strip():
                found.add("whitespace compatSetting")
        return found

    def test_markup_word_may_refuse_stays_in_its_own_document(self):
        expected = {
            "E-color-hash": {"hash colour"},
            "E-value-theme-tint-short": {"short tint"},
            "E-char-scale-zero": {"scale below 1"},
            "E-value-hanging-negative": {"negative hanging"},
            "E-value-spacing-minus-one": {"negative spacing"},
            "E-value-line-negative": {"negative line"},
            "E-value-outline-level": {"outline level"},
            "E-value-ilvl-high": {"high level"},
            "E-list-long-template": {"long template"},
            "E-list-long-template-600": {"long template"},
            "E-list-template-controls": {"template control"},
            "E-whitespace-pos-offset": {"whitespace posOffset"},
            "E-whitespace-align": {"whitespace align"},
            "E-whitespace-compat": {"whitespace compatSetting"},
            "E-sectpr-in-cell": {"section in a cell"},
            "E-style-id-missing": {"style without id"},
            "E-style-id-repeated": {"repeated style id"},
            "E-style-type-unknown": {"unknown style type"},
            "E-list-label-mark-highlight": {"highlight in a style"},
        }
        for name in self.docs:
            self.assertEqual(self.odd_markup(name), expected.get(name, set()), name)


if __name__ == "__main__":
    unittest.main()
