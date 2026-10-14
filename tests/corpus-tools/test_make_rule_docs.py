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
                for tag in ("pStyle", "rStyle", "basedOn", "numStyleLink", "styleLink")
                for n in root.iter(f"{W}{tag}")
            }
            self.assertLessEqual(named, defined, doc.name)

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
                        self.assertEqual(places, sorted(set(places)), f"{doc.name}: {tags}")

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
        for doc in self.docs:
            parts = rules.parts(doc)
            for name in ("word/document.xml", "word/styles.xml"):
                for properties in part(parts, name).iter(f"{W}rPr"):
                    tags = [child.tag.replace(W, "") for child in properties]
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

    def test_refuses_a_colour_mapping_attribute_that_does_not_exist(self):
        doc = next(doc for doc in self.docs if doc.color_mapping)
        mistyped = dataclasses.replace(doc, color_mapping=(("t3", "accent3"),))
        with self.assertRaisesRegex(ValueError, "no attribute t3"):
            rules.settings_xml(mistyped)


if __name__ == "__main__":
    unittest.main()
