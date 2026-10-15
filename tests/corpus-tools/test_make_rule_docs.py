"""Checks scripts/make-rule-docs.py and the documents it committed to tests/corpus/word-rules/.

The documents' markup is checked against the ECMA-376 schemas by hand (see the script's
pull request); these tests keep the committed files, the generator and SOURCES.md in step
and check the package structure that Word needs to open a file at all.
"""

import dataclasses
import importlib.util
import re
import shutil
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from xml.etree import ElementTree

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"
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

    def test_font_table_lists_every_font_the_document_names(self):
        for doc in self.docs:
            parts = rules.parts(doc)
            table = {f.get(f"{W}name") for f in part(parts, "word/fontTable.xml")}
            named = set()
            for name in ("word/document.xml", "word/styles.xml"):
                for fonts in part(parts, name).iter(f"{W}rFonts"):
                    for attribute, value in fonts.attrib.items():
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
            # CT_Tbl 17.4.38 and CT_Row 17.4.79.
            "tbl": ["bookmarkStart", "bookmarkEnd", "tblPr", "tblGrid", "tr"],
            "tr": ["tblPrEx", "trPr", "tc"],
        }
        for doc in self.docs:
            parts = rules.parts(doc)
            for name, _ in parts:
                if name not in ("word/document.xml", "word/styles.xml", "word/numbering.xml"):
                    continue
                root = part(parts, name)
                for kind, order in orders.items():
                    for element in root.iter(f"{W}{kind}"):
                        tags = [child.tag.replace(W, "") for child in element]
                        for tag in tags:
                            self.assertIn(tag, order, f"{doc.name}: {kind} {tags}")
                        places = [order.index(tag) for tag in tags]
                        self.assertEqual(places, sorted(places), f"{doc.name}: {tags}")
                        # Only a style's conditional formats, a table's rows and a row's
                        # cells may come more than once.
                        repeated = {tag for tag in tags if tags.count(tag) > 1}
                        self.assertLessEqual(
                            repeated, {"tblStylePr", "tr", "tc"}, f"{doc.name}: {tags}"
                        )

    def test_properties_come_first_in_their_element(self):
        # w:pPr, w:rPr, w:tcPr and w:trPr are the first child of their paragraph, run, cell
        # or row (CT_P 17.3.1.22, CT_R 17.3.2.25, CT_Tc 17.4.66, CT_Row 17.4.79), after a
        # row's w:tblPrEx at most.
        for doc in self.docs:
            for name, _ in rules.parts(doc):
                if not name.startswith(("word/document.xml", "word/header", "word/footer")):
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
            for name in ("word/document.xml", "word/styles.xml", "word/numbering.xml"):
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


if __name__ == "__main__":
    unittest.main()
