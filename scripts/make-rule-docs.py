#!/usr/bin/env python3
"""Writes the Word rule documents of the test corpus into tests/corpus/word-rules/.

Each document isolates one layout rule that ECMA-376 and [MS-OI29500] leave open, so that
a reference render made by Microsoft Word (scripts/windows/make-reference.ps1) shows what
Word does. The markup is written from ECMA-376 Part 1 and [MS-DOCX]; nothing is copied
from documents made by other software.

    make-rule-docs.py           write every document
    make-rule-docs.py --check   fail if a committed document is missing, stale or unknown
    make-rule-docs.py --list    print the SOURCES.md row of every document

A rule that depends on a document-wide setting (the compatibility mode, a w:compat flag)
gets one document per setting, named after it. Every case inside a document is labelled
in its own text, so the reference PDF says which case each line belongs to.

The ZIP entries are stored uncompressed with fixed timestamps, so the bytes depend
neither on the platform's zlib nor on the time the script ran.
"""

from __future__ import annotations

import argparse
import re
import sys
import zipfile
from dataclasses import dataclass
from io import BytesIO
from pathlib import Path
from xml.dom import minidom
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "tests" / "corpus" / "word-rules"
SOURCES = ROOT / "tests" / "corpus" / "SOURCES.md"

W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R_NS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
WORD_URI = "http://schemas.microsoft.com/office/word"
W14_NS = "http://schemas.microsoft.com/office/word/2010/wordml"
MC_NS = "http://schemas.openxmlformats.org/markup-compatibility/2006"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
CT_W = "application/vnd.openxmlformats-officedocument.wordprocessingml"

# A4 with 1 inch margins: the text column is 9026 twips (451.3 pt) wide and the body
# 13958 twips (697.9 pt) tall.
PAGE_W, PAGE_H, MARGIN = 11906, 16838, 1440
TEXT_W = PAGE_W - 2 * MARGIN

GREY = "D9D9D9"
DARK_GREY = "BFBFBF"


# Markup helpers. Element order inside pPr, rPr, tblPr and tcPr follows the sequences of
# ECMA-376 Part 1 (transitional schema); Word rejects documents that break them.


def attr(value: object) -> str:
    return escape(str(value), {'"': "&quot;"})


def twips(points: float) -> int:
    value = points * 20
    if value != int(value):
        raise ValueError(f"{points} pt is not a whole number of twips")
    return int(value)


def half_points(points: float) -> int:
    value = points * 2
    if value != int(value):
        raise ValueError(f"{points} pt is not a whole number of half-points")
    return int(value)


def rpr(font: str | None = None, size: float | None = None, toggles: str = "") -> str:
    """Run properties; toggles (w:b, w:i, w:smallCaps ...) go between w:rFonts and w:sz,
    in the order of the schema."""
    parts = []
    if font:
        f = attr(font)
        parts.append(f'<w:rFonts w:ascii="{f}" w:hAnsi="{f}" w:eastAsia="{f}" w:cs="{f}"/>')
    if toggles:
        parts.append(toggles)
    if size is not None:
        hp = half_points(size)
        parts.append(f'<w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/>')
    return f"<w:rPr>{''.join(parts)}</w:rPr>" if parts else ""


def text(value: str) -> str:
    return f'<w:t xml:space="preserve">{escape(value)}</w:t>'


def run(value: str, font: str | None = None, size: float | None = None, toggles: str = "") -> str:
    return f"<w:r>{rpr(font, size, toggles)}{text(value)}</w:r>"


def lines(values: list[str], font: str | None = None, size: float | None = None) -> str:
    """One run whose lines are separated by line breaks (w:br)."""
    inner = "<w:br/>".join(text(v) for v in values)
    return f"<w:r>{rpr(font, size)}{inner}</w:r>"


def line_break_run() -> str:
    return "<w:r><w:br/></w:r>"


def tab(font: str | None = None, size: float | None = None) -> str:
    return f"<w:r>{rpr(font, size)}<w:tab/></w:r>"


def page_break_run() -> str:
    return '<w:r><w:br w:type="page"/></w:r>'


def spacing(
    before: float | None = None,
    after: float | None = None,
    line: tuple[str, float] | None = None,
    before_auto: bool = False,
    after_auto: bool = False,
    before_lines: int | None = None,
) -> str:
    """line is ("exact" | "atLeast", points) or ("auto", w:line, where 240 is one line);
    before_lines is in hundredths of a line."""
    a = []
    if before is not None:
        a.append(f'w:before="{twips(before)}"')
    if before_lines is not None:
        a.append(f'w:beforeLines="{before_lines}"')
    if before_auto:
        a.append('w:beforeAutospacing="1"')
    if after is not None:
        a.append(f'w:after="{twips(after)}"')
    if after_auto:
        a.append('w:afterAutospacing="1"')
    if line is not None:
        rule, value = line
        amount = int(value) if rule == "auto" else twips(value)
        a.append(f'w:line="{amount}" w:lineRule="{rule}"')
    return f"<w:spacing {' '.join(a)}/>"


def ppr(
    *,
    p_style: str | None = None,
    page_break_before: bool = False,
    widow_control: bool | None = None,
    list_item: tuple[int, int] | None = None,
    num_pr: str | None = None,
    shading: str | None = None,
    tabs: list[tuple[str, int]] | None = None,
    space: str | None = None,
    ind: dict[str, int] | None = None,
    jc: str | None = None,
    mark: str | None = None,
    sect: str | None = None,
) -> str:
    """mark is the content of the paragraph mark's w:rPr."""
    parts = []
    if p_style:
        parts.append(f'<w:pStyle w:val="{p_style}"/>')
    if page_break_before:
        parts.append("<w:pageBreakBefore/>")
    if widow_control is not None:
        parts.append("<w:widowControl/>" if widow_control else '<w:widowControl w:val="0"/>')
    if list_item is not None:
        num_id, ilvl = list_item
        parts.append(f'<w:numPr><w:ilvl w:val="{ilvl}"/><w:numId w:val="{num_id}"/></w:numPr>')
    if num_pr is not None:
        parts.append(f"<w:numPr>{num_pr}</w:numPr>")
    if shading:
        parts.append(f'<w:shd w:val="clear" w:color="auto" w:fill="{shading}"/>')
    if tabs:
        stops = "".join(f'<w:tab w:val="{kind}" w:pos="{pos}"/>' for kind, pos in tabs)
        parts.append(f"<w:tabs>{stops}</w:tabs>")
    if space:
        parts.append(space)
    if ind:
        parts.append("<w:ind " + " ".join(f'w:{k}="{v}"' for k, v in ind.items()) + "/>")
    if jc:
        parts.append(f'<w:jc w:val="{jc}"/>')
    if mark:
        parts.append(f"<w:rPr>{mark}</w:rPr>")
    if sect:
        parts.append(sect)
    return f"<w:pPr>{''.join(parts)}</w:pPr>" if parts else ""


def para(content: str = "", **props) -> str:
    return f"<w:p>{ppr(**props)}{content}</w:p>"


def sect_pr() -> str:
    """A4 portrait, 1 inch margins, header and footer 0.5 inch from the edge, one column."""
    return (
        "<w:sectPr>"
        f'<w:pgSz w:w="{PAGE_W}" w:h="{PAGE_H}"/>'
        f'<w:pgMar w:top="{MARGIN}" w:right="{MARGIN}" w:bottom="{MARGIN}" w:left="{MARGIN}"'
        ' w:header="708" w:footer="708" w:gutter="0"/>'
        '<w:cols w:space="708"/>'
        '<w:docGrid w:linePitch="360"/>'
        "</w:sectPr>"
    )


def border(side: str) -> str:
    return f'<w:{side} w:val="single" w:sz="4" w:space="0" w:color="000000"/>'


def table(
    cells: list[str],
    grid: list[int],
    *,
    ind: int | None = 0,
    jc: str | None = None,
    margins: tuple[int, int] | None = (108, 108),
    first_cell_left: int | None = None,
) -> str:
    """A one-row table with fixed layout, single 0.5 pt borders and the given grid.

    cells holds the paragraphs of each cell. ind None leaves tblInd out, so the table
    style's value applies; margins None leaves tblCellMar out. first_cell_left gives the
    first cell its own left margin (tcMar).
    """
    if len(cells) != len(grid):
        raise ValueError("one cell per grid column")
    tbl_pr = [f'<w:tblW w:w="{sum(grid)}" w:type="dxa"/>']
    if jc:
        tbl_pr.append(f'<w:jc w:val="{jc}"/>')
    if ind is not None:
        tbl_pr.append(f'<w:tblInd w:w="{ind}" w:type="dxa"/>')
    sides = ("top", "left", "bottom", "right", "insideH", "insideV")
    tbl_pr.append("<w:tblBorders>" + "".join(border(s) for s in sides) + "</w:tblBorders>")
    tbl_pr.append('<w:tblLayout w:type="fixed"/>')
    if margins is not None:
        left, right = margins
        tbl_pr.append(
            "<w:tblCellMar>"
            f'<w:left w:w="{left}" w:type="dxa"/><w:right w:w="{right}" w:type="dxa"/>'
            "</w:tblCellMar>"
        )
    grid_xml = "".join(f'<w:gridCol w:w="{w}"/>' for w in grid)
    row = []
    for i, (content, width) in enumerate(zip(cells, grid)):
        tc_pr = f'<w:tcW w:w="{width}" w:type="dxa"/>'
        if i == 0 and first_cell_left is not None:
            tc_pr += f'<w:tcMar><w:left w:w="{first_cell_left}" w:type="dxa"/></w:tcMar>'
        row.append(f"<w:tc><w:tcPr>{tc_pr}</w:tcPr>{content}</w:tc>")
    return (
        f"<w:tbl><w:tblPr>{''.join(tbl_pr)}</w:tblPr><w:tblGrid>{grid_xml}</w:tblGrid>"
        f"<w:tr>{''.join(row)}</w:tr></w:tbl>"
    )


def level(
    ilvl: int,
    fmt: str,
    template: str,
    *,
    left: int,
    hanging: int,
    suffix: str | None = None,
    tab_stop: int | None = None,
    p_style: str | None = None,
    label: str = "",
) -> str:
    """label is the content of the level's w:rPr, the run properties of its label."""
    suff = f'<w:suff w:val="{suffix}"/>' if suffix else ""
    tabs = f'<w:tabs><w:tab w:val="num" w:pos="{tab_stop}"/></w:tabs>' if tab_stop else ""
    linked = f'<w:pStyle w:val="{p_style}"/>' if p_style else ""
    return (
        f'<w:lvl w:ilvl="{ilvl}"><w:start w:val="1"/><w:numFmt w:val="{fmt}"/>{linked}{suff}'
        f'<w:lvlText w:val="{attr(template)}"/><w:lvlJc w:val="left"/>'
        f'<w:pPr>{tabs}<w:ind w:left="{left}" w:hanging="{hanging}"/></w:pPr>'
        + (f"<w:rPr>{label}</w:rPr>" if label else "")
        + "</w:lvl>"
    )


def abstract_num(num_id: int, nsid: str, levels: list[str]) -> str:
    kind = "singleLevel" if len(levels) == 1 else "multilevel"
    return (
        f'<w:abstractNum w:abstractNumId="{num_id}"><w:nsid w:val="{nsid}"/>'
        f'<w:multiLevelType w:val="{kind}"/>{"".join(levels)}</w:abstractNum>'
    )


def num(
    num_id: int, abstract: int, start_override: int | None = None, override_level: str = ""
) -> str:
    """override_level is a w:lvl that takes the place of level 0 of the definition."""
    override = ""
    if start_override is not None or override_level:
        start = ""
        if start_override is not None:
            start = f'<w:startOverride w:val="{start_override}"/>'
        override = f'<w:lvlOverride w:ilvl="0">{start}{override_level}</w:lvlOverride>'

    return f'<w:num w:numId="{num_id}"><w:abstractNumId w:val="{abstract}"/>{override}</w:num>'


# Documents.


@dataclass(frozen=True)
class Doc:
    name: str
    purpose: str
    body: str
    mode: int = 15
    compat_flags: tuple[str, ...] = ()
    numbering: str | None = None
    # Style definitions added to styles.xml after the built-in ones (styles), or before
    # Normal, ahead of the built-in ones (styles_before_normal).
    styles: str = ""
    styles_before_normal: str = ""
    # Whether Normal is marked w:default.
    normal_default: bool = True
    # Whether the document defaults give run properties (Calibri 11 pt), and properties
    # added to them.
    run_defaults: bool = True
    run_defaults_extra: str = ""
    # The attributes of the document defaults' w:rFonts.
    run_default_fonts: str = (
        'w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Calibri"'
    )
    # Whether the document has a theme part (theme_xml) and a colour mapping for it.
    theme: bool = False
    # Entries of the colour mapping that differ from the identity, such as
    # (("t2", "accent3"),).
    color_mapping: tuple[tuple[str, str], ...] = ()
    # Values other than 1 of the compatibility settings Word writes in mode 15
    # (MODE_15_SETTINGS), such as (("enableOpenTypeFeatures", 0),); None leaves one out.
    compat_values: tuple[tuple[str, int | None], ...] = ()


def line_fonts_body(fonts: list[str]) -> str:
    """One page per font with a shaded paragraph of ten lines at 11 pt and one at 12 pt."""
    body = []
    for i, font in enumerate(fonts):
        for size, fill in ((11, GREY), (12, DARK_GREY)):
            values = [f"{font} {size} pt, line {n:02d}: Hamburgefonts gjpqy" for n in range(1, 11)]
            body.append(
                para(
                    lines(values, font, size),
                    page_break_before=i > 0 and size == 11,
                    shading=fill,
                )
            )
    return "".join(body)


def line_fonts() -> Doc:
    fonts = [
        "Calibri",
        "Cambria",
        "Times New Roman",
        "Arial",
        "Courier New",
        "Verdana",
        "Georgia",
        "Segoe UI",
        "Aptos",
        "Cascadia Mono",
        "Liberation Sans",
        "Noto Sans",
    ]
    return Doc(
        "E-line-fonts",
        "Single line height per font: one page per font with a shaded paragraph of ten "
        "lines at 11 pt and one at 12 pt; Aptos, Cascadia Mono and Noto Sans set "
        "USE_TYPO_METRICS",
        line_fonts_body(fonts),
    )


def line_fonts_truetype() -> Doc:
    # Word ignored USE_TYPO_METRICS for the CFF build of Noto Sans (installed for the user)
    # and honoured it for Aptos and Cascadia Mono (TrueType outlines). The TrueType build of
    # the same Noto Sans version has the same metrics and names, so rendered with it
    # installed instead, this document tells the outline format from the installation.
    return Doc(
        "E-line-fonts-truetype",
        "Single line height as in E-line-fonts, for Noto Sans rendered with the TrueType "
        "build of Noto Sans 2.015 installed for the user instead of the CFF build (same "
        "metrics and names; USE_TYPO_METRICS set), and for Aptos (flag set, TrueType) and "
        "Liberation Sans (flag clear) as controls",
        line_fonts_body(["Noto Sans", "Aptos", "Liberation Sans"]),
    )


# Liberation Sans and Carlito have no smcp feature (Carlito has c2sc only); Noto Sans has one,
# so its rows show whether Word uses the font's small capitals or synthesises them.
SMALL_CAPS_FONTS = ["Liberation Sans", "Carlito", "Noto Sans"]
# Sizes whose 80 % falls between half-points (7.2, 8.4, 8.8, 10.4, 16.8, 57.6 pt) and on one
# (32 pt), so that the reference shows whether and how Word rounds the reduced size.
SMALL_CAPS_SIZES = [9, 10.5, 11, 13, 21, 40, 72]


def small_caps(mode: int) -> Doc:
    if mode == 12:
        setting = "no compatibilityMode setting, so Word's default, mode 12"
    else:
        setting = f"compatibility mode {mode}"
    body = []
    for font in SMALL_CAPS_FONTS:
        for size in SMALL_CAPS_SIZES:
            for label, word, toggles in (
                ("small caps", "hamburg", "<w:smallCaps/>"),
                ("capitals", "HAMBURG", ""),
                ("mixed small caps", "Hamburg", "<w:smallCaps/>"),
            ):
                body.append(
                    para(
                        run(f"{font} {size} pt, {label}:", font, 10)
                        + line_break_run()
                        + run(word, font, size, toggles)
                    )
                )
    return Doc(
        f"E-smallcaps-{mode}",
        "Small capitals: per font (Liberation Sans and Carlito, without an smcp feature, and "
        "Noto Sans, with one, rendered with its TrueType build installed as for "
        "E-line-fonts-truetype) and size (9 to 72 pt), a paragraph with 'hamburg' in small "
        "caps, one with 'HAMBURG' typed in capitals and one with 'Hamburg' in small caps; "
        "their widths and heights give the size Word reduces small capitals to; "
        f"{setting}",
        "".join(body),
        mode=mode,
    )


def italic_missing() -> Doc:
    # Issue #82: a family with Bold Italic but no Italic face. Rendered with Noto Sans
    # installed as Regular, Bold and Bold Italic only; Liberation Sans, installed complete,
    # shows each face as it is.
    body = []
    for font in ("Noto Sans", "Liberation Sans"):
        for size in (11, 20):
            for label, toggles in (
                ("regular", ""),
                ("italic", "<w:i/><w:iCs/>"),
                ("bold", "<w:b/><w:bCs/>"),
                ("bold italic", "<w:b/><w:bCs/><w:i/><w:iCs/>"),
            ):
                body.append(
                    para(run(f"{font} {size} pt {label}: Hamburgefonts gjpqy", font, size, toggles))
                )
    return Doc(
        "E-italic-missing",
        "Italic text in a family without an Italic face (issue #82): Noto Sans, rendered "
        "with only its Regular, Bold and Bold Italic faces installed, and Liberation Sans, "
        "installed complete, at 11 and 20 pt in regular, italic, bold and bold italic; one "
        "paragraph per case",
        "".join(body),
    )


def line_rule_doc(name: str, purpose: str, rule: str, values: list[float]) -> Doc:
    """Shaded test paragraphs of four lines, each between single-spaced reference lines."""

    def describe(value: float) -> str:
        if rule == "auto":
            return f"multiple {value / 240:g} lines (w:line {value:g})"
        return f"{rule} {value:g} pt"

    body = [para(run("Reference line above the first case, single line spacing."))]
    for value in values:
        label = describe(value)
        values_text = [f"{label}, line {n}: Hamburgefonts gjpqy" for n in range(1, 5)]
        body.append(para(lines(values_text), shading=GREY, space=spacing(line=(rule, value))))
        body.append(para(run(f"Reference line after {label}.")))
    mixed_value = values[len(values) // 2]
    mixed = (
        run(f"{describe(mixed_value)} with a ")
        + run("28 pt", size=28)
        + run(" run, line 1")
        + lines(["", "line 2: Hamburgefonts gjpqy", "line 3: Hamburgefonts gjpqy"])
    )
    body.append(para(mixed, shading=GREY, space=spacing(line=(rule, mixed_value))))
    body.append(para(run("Reference line after the mixed case.")))
    return Doc(name, purpose, "".join(body))


def line_exact() -> Doc:
    return line_rule_doc(
        "E-line-exact",
        "Exact line spacing (8, 12, 20, 30 pt) in Calibri 11: where the baseline sits in "
        "the line box and whether glyphs are clipped; one case mixes in a 28 pt run",
        "exact",
        [8, 12, 20, 30],
    )


def line_at_least() -> Doc:
    return line_rule_doc(
        "E-line-at-least",
        "At-least line spacing (6, 12, 14, 20, 30 pt) in Calibri 11: when the minimum "
        "applies and where the extra space goes; one case mixes in a 28 pt run",
        "atLeast",
        [6, 12, 14, 20, 30],
    )


def line_multiple() -> Doc:
    return line_rule_doc(
        "E-line-multiple",
        "Multiple line spacing (0.5 to 3 lines, including 1.08 and 1.15) in Calibri 11: "
        "line height and baseline position; one case mixes in a 28 pt run",
        "auto",
        [120, 200, 240, 259, 276, 360, 480, 720],
    )


def advance_rounding(mode: int) -> Doc:
    body = []
    for font in ("Calibri", "Times New Roman"):
        for size in (10, 10.5, 11, 12):
            for char, count in (("i", 100), (".", 100), ("m", 30), ("0", 50)):
                label = f" {font} {size:g} pt, {count} x '{char}'"
                body.append(para(run(char * count, font, size) + run(label, "Calibri", 8)))
    if mode == 12:
        setting = "no compatibilityMode setting, so Word's default, mode 12"
    else:
        setting = f"compatibility mode {mode}"
    return Doc(
        f"E-advance-rounding-{mode}",
        "Glyph advance rounding: lines of 100 'i', 100 '.', 30 'm' and 50 '0' in Calibri "
        f"and Times New Roman at 10, 10.5, 11 and 12 pt; {setting}",
        "".join(body),
        mode=mode,
    )


def space_before_top() -> Doc:
    exact = ("exact", 20)

    def case(label: str, before: float = 24, **props) -> str:
        return para(run(label), shading=GREY, space=spacing(before=before, line=exact), **props)

    def case_in_table(label: str) -> str:
        return table([case(label)], [TEXT_W])

    # Cases 2, 6 and 7 rely on mode 15 keeping the paragraph mark that follows a page break
    # on the page of the break; in earlier modes (w:splitPgBreakAndParaMark) an empty
    # paragraph would start the next page. A variant in another mode must say so.
    filler = [f"Filler line {n}" for n in range(1, 34)]
    body = [
        case("Case 1: first paragraph of the document, space before 24 pt"),
        para(page_break_run()),
        case("Case 2: after a paragraph that holds only a page break"),
        case("Case 3: page break before, space before 24 pt", page_break_before=True),
        case("Control: page break before, space before 0", before=0, page_break_before=True),
        # 33 lines of 20 pt leave 17.9 pt of the body: case 4 cannot fit and flows.
        para(lines(filler), widow_control=False, space=spacing(line=exact)),
        case("Case 4: flowed to a new page; the last paragraph of section 1", sect=sect_pr()),
        case("Case 5: first paragraph of section 2, which starts on a new page"),
        para(run("A paragraph with text that ends in a page break.") + page_break_run()),
        case("Case 6: after a page break that ends a paragraph with text"),
        para(page_break_run()),
        case_in_table("Case 7: first paragraph of a table cell at the top of a page"),
        para(run("Reference paragraph between the two tables, single line spacing.")),
        case_in_table("Case 8: first paragraph of a table cell in the middle of a page"),
        para(run("End of the document.")),
    ]
    return Doc(
        "E-space-before-top",
        "Space before (24 pt) of a paragraph at the top of a page: document start, after "
        "page breaks, page break before, flowed, section start, in table cells; shaded "
        "20 pt exact lines mark the paragraph boxes",
        "".join(body),
    )


def autospacing(no_html: bool) -> Doc:
    exact = ("exact", 20)

    def p(label: str, fill: str, size: float | None = None, **space) -> str:
        return para(run(label, size=size), shading=fill, space=spacing(line=exact, **space))

    def plain(fill: str) -> str:
        return p("Plain paragraph, no spacing", fill, before=0, after=0)

    body = [
        plain(GREY),
        p("Auto spacing before and after", DARK_GREY, before_auto=True, after_auto=True),
        plain(GREY),
        p("Auto spacing before only", DARK_GREY, before_auto=True, after=0),
        plain(GREY),
        p("Auto spacing after only", DARK_GREY, before=0, after_auto=True),
        plain(GREY),
        p("Two paragraphs in a row, auto before and after: first", DARK_GREY,
          before_auto=True, after_auto=True),
        p("Two paragraphs in a row, auto before and after: second", GREY,
          before_auto=True, after_auto=True),
        plain(DARK_GREY),
        p("Auto spacing before and after, with explicit before 24 and after 24 as well",
          GREY, before=24, after=24, before_auto=True, after_auto=True),
        plain(DARK_GREY),
        p("Auto spacing before and after in 20 pt text", GREY, size=20,
          before_auto=True, after_auto=True),
        plain(DARK_GREY),
    ]
    flags = ("doNotUseHTMLParagraphAutoSpacing",) if no_html else ()
    variant = "with" if no_html else "without"
    return Doc(
        "E-autospacing-nohtml" if no_html else "E-autospacing",
        "Automatic paragraph spacing (beforeAutospacing, afterAutospacing) "
        f"{variant} the compat flag doNotUseHTMLParagraphAutoSpacing; shaded 20 pt "
        "exact lines mark the paragraph boxes",
        "".join(body),
        compat_flags=flags,
    )


def table_indent(mode: int) -> Doc:
    grid = [2000, 3000]

    def cells() -> list[str]:
        return [para(run("Cell text")), para(run("Second cell"))]

    cases = [
        ("Table A: tblInd 0, cell margins 108 twips", {}),
        ("Table B: tblInd 720 twips", {"ind": 720}),
        ("Table C: tblInd 0, cell margins 0", {"margins": (0, 0)}),
        ("Table D: tblInd 0, cell margins 360 twips", {"margins": (360, 360)}),
        ("Table E: no tblInd, so the table style's 0 applies", {"ind": None}),
        ("Table F: tblInd -360 twips", {"ind": -360}),
        ("Table G: centred, tblInd 720 twips", {"ind": 720, "jc": "center"}),
        ("Table H: tblInd 0, first cell's own left margin 360 twips", {"first_cell_left": 360}),
    ]
    body = [para(run("Reference paragraph: it starts at the left margin."))]
    for caption, options in cases:
        body.append(para(run(caption), space=spacing(before=12)))
        body.append(table(cells(), grid, **options))
    body.append(para(run("End of the document."), space=spacing(before=12)))
    return Doc(
        f"E-table-indent-{mode}",
        "Horizontal position of table borders and cell text against tblInd, cell margins "
        f"and alignment, in compatibility mode {mode}",
        "".join(body),
        mode=mode,
    )


def list_shared_abstract() -> Doc:
    numbering = (
        abstract_num(
            0,
            "5C3A0001",
            [
                level(0, "decimal", "%1.", left=720, hanging=360),
                level(1, "decimal", "%1.%2.", left=1440, hanging=720),
            ],
        )
        + abstract_num(1, "5C3A0002", [level(0, "lowerLetter", "%1)", left=720, hanging=360)])
        + num(1, 0)
        + num(2, 0)
        + num(3, 0, start_override=1)
        + num(4, 1)
        + num(5, 0, start_override=10)
    )
    items = [
        (1, 0, "num 1, level 0, first item"),
        (1, 1, "num 1, level 1"),
        (1, 0, "num 1, level 0"),
        (2, 0, "num 2 (same abstract definition as num 1), level 0"),
        (2, 1, "num 2, level 1"),
        (1, 0, "num 1, level 0, after num 2"),
        (4, 0, "num 4 (another abstract definition), level 0"),
        (1, 0, "num 1, level 0, after num 4"),
        (3, 0, "num 3 (same abstract, startOverride 1), level 0"),
        (3, 0, "num 3, level 0"),
        (1, 0, "num 1, level 0, after num 3"),
        (5, 0, "num 5 (same abstract, startOverride 10), level 0"),
        (1, 1, "num 1, level 1, after num 5"),
        (2, 0, "num 2, level 0, last item"),
    ]
    body = "".join(para(run(label), list_item=(num_id, ilvl)) for num_id, ilvl, label in items)
    return Doc(
        "E-list-shared-abstract",
        "List numbering across w:num instances that share one w:abstractNum, with and "
        "without startOverride, interleaved with a list of another abstract definition",
        body,
        numbering=numbering,
    )


def tab_hanging(no_indent_tab: bool, mode: int = 15) -> Doc:
    hanging = {"left": 1000, "hanging": 1000}

    def single(abstract: int, nsid: str, template: str, suffix: str, stop: int = 0) -> str:
        lvl = level(0, "decimal", template, **hanging, suffix=suffix, tab_stop=stop or None)
        return abstract_num(abstract, nsid, [lvl])

    numbering = (
        single(0, "5C3A0011", "%1.", "tab")
        + single(1, "5C3A0012", "%1.", "tab", stop=1440)
        + single(2, "5C3A0013", "%1.", "space")
        + single(3, "5C3A0014", "Paragraph %1:", "tab")
        + num(1, 0)
        + num(2, 1)
        + num(3, 2)
        + num(4, 3)
    )
    rest = (
        " The text after the tab shows where the tab ended, and the wrapped second line "
        "shows where the hanging indent puts the text of the following lines."
    )

    def typed(case: str, label: str, **props) -> str:
        return para(run(label) + tab() + run(case + rest), **props)

    def listed(case: str, num_id: int) -> str:
        return para(run(case + rest), list_item=(num_id, 0))

    body = [
        typed("Case A: typed label, hanging indent 50 pt, no custom tab stops.", "1.1",
              ind=hanging),
        listed("Case B: list label, level indent left 50 pt, hanging 50 pt, tab suffix.", 1),
        typed("Case C: typed label wider than the 50 pt hanging indent.", "Clause 12.3.4",
              ind=hanging),
        typed("Case D: typed label, hanging indent 50 pt, custom tab stop at 75 pt.", "1.1",
              ind=hanging, tabs=[("left", 1500)]),
        typed("Case E: typed label, first-line indent 25 pt, no hanging indent.", "1.1",
              ind={"left": 0, "firstLine": 500}),
        typed("Case F: typed label, left indent 100 pt, hanging 50 pt.", "1.1",
              ind={"left": 2000, "hanging": 1000}),
        listed("Case G: list label with a level tab stop of type num at 72 pt.", 2),
        listed("Case H: list label with a space suffix.", 3),
        listed("Case I: list label wider than the 50 pt hanging indent.", 4),
    ]
    flags = ("doNotUseIndentAsNumberingTabStop",) if no_indent_tab else ()
    variant = "with" if no_indent_tab else "without"
    name = "E-tab-hanging-noindenttab" if no_indent_tab else "E-tab-hanging"
    # Word 16 drops the flag from a document in mode 15 when it saves it and renders as if
    # it were absent (the references of #13). In mode 14 it keeps the flag and applies it
    # (#18, references of #50): the re-saved E-tab-hanging-noindenttab-14 still has it, and
    # the text after the list label moves from 122.06 pt to 108.02 pt in case B and to
    # 144.02 pt in case G (reference/word-rules/E-tab-hanging-noindenttab-14/geometry.json).
    where = ""
    if mode != 15:
        name += f"-{mode}"
        where = f", in compatibility mode {mode}"
    return Doc(
        name,
        "Where a tab after a label goes when the paragraph has a hanging indent, for typed "
        f"labels and list labels, {variant} the compat flag doNotUseIndentAsNumberingTabStop "
        f"(default tab stops every 36 pt){where}",
        "".join(body),
        compat_flags=flags,
        numbering=numbering,
        mode=mode,
    )


# Styles and themes: what Word does where the style hierarchy, theme fonts and theme
# colours meet, which ECMA-376 §17.7 and [MS-OI29500] §17.7 describe differently.


def style(kind: str, style_id: str, rpr_xml: str, based_on: str | None = None) -> str:
    base = f'<w:basedOn w:val="{based_on}"/>' if based_on else ""
    rpr = f"<w:rPr>{rpr_xml}</w:rPr>" if rpr_xml else ""
    return (
        f'<w:style w:type="{kind}" w:customStyle="1" w:styleId="{style_id}">'
        f'<w:name w:val="{style_id}"/>{base}{rpr}</w:style>'
    )


def styled_para(label: str, p_style: str | None, r_style: str | None, direct: str) -> str:
    """A paragraph of one run whose text names its case."""
    ppr_xml = f'<w:pPr><w:pStyle w:val="{p_style}"/></w:pPr>' if p_style else ""
    rpr_parts = (f'<w:rStyle w:val="{r_style}"/>' if r_style else "") + direct
    rpr_xml = f"<w:rPr>{rpr_parts}</w:rPr>" if rpr_parts else ""
    return f"<w:p>{ppr_xml}<w:r>{rpr_xml}{text(label)}</w:r></w:p>"


TOGGLE_STYLES = "".join(
    [
        style("paragraph", "ParaBold", "<w:b/>", "Normal"),
        style("paragraph", "ParaBoldAgain", "<w:b/>", "ParaBold"),
        style("paragraph", "ParaBoldOff", '<w:b w:val="0"/>', "ParaBold"),
        # Inherit bold on or off without a w:b of their own (ECMA-376 §17.7.3,
        # MS-OI29500 §17.7.3(a) and (c)): with bold in the document defaults, the ones that
        # inherit off tell whether Word counts the levels of a basedOn chain one by one.
        style("paragraph", "ParaBoldInherit", "", "ParaBold"),
        style("paragraph", "ParaBoldOffInherit", "", "ParaBoldOff"),
        style("character", "CharBold", "<w:b/>"),
        style("character", "CharBoldAgain", "<w:b/>", "CharBold"),
        style("character", "CharBoldOff", '<w:b w:val="0"/>', "CharBold"),
        style("character", "CharBoldInherit", "", "CharBold"),
        style("character", "CharBoldOffInherit", "", "CharBoldOff"),
    ]
)

# (paragraph style, character style, direct run properties) of each toggle case.
TOGGLE_CASES = [
    (None, None, ""),
    ("ParaBold", None, ""),
    ("ParaBoldAgain", None, ""),
    ("ParaBoldOff", None, ""),
    (None, "CharBold", ""),
    (None, "CharBoldAgain", ""),
    (None, "CharBoldOff", ""),
    ("ParaBold", "CharBold", ""),
    ("ParaBold", "CharBoldAgain", ""),
    ("ParaBold", "CharBoldOff", ""),
    ("ParaBold", "CharBold", "<w:b/>"),
    ("ParaBold", "CharBold", '<w:b w:val="0"/>'),
    ("ParaBold", None, '<w:b w:val="0"/>'),
    (None, None, "<w:b/>"),
    (None, "CharBold", '<w:b w:val="0"/>'),
    ("ParaBoldInherit", None, ""),
    (None, "CharBoldInherit", ""),
    ("ParaBold", "CharBoldInherit", ""),
    ("ParaBoldOffInherit", None, ""),
    (None, "CharBoldOffInherit", ""),
]


def style_toggles(defaults_bold: bool) -> Doc:
    body = []
    for n, (p_style, r_style, direct) in enumerate(TOGGLE_CASES, 1):
        named = {"<w:b/>": "b on", '<w:b w:val="0"/>': "b off", "": "none"}[direct]
        label = (
            f"Case {n:02d}: paragraph style {p_style or 'Normal'}, character style "
            f"{r_style or 'none'}, direct {named}"
        )
        body.append(styled_para(label, p_style, r_style, direct))
    suffix = "-defaults" if defaults_bold else ""
    where = "with" if defaults_bold else "without"
    return Doc(
        f"E-style-toggles{suffix}",
        "Bold as a toggle property across the style hierarchy, " + where + " bold in the "
        "document defaults: paragraph styles and character styles based on bold ones that "
        "set it on or off or inherit it, with and without direct formatting; one case per "
        "paragraph",
        "".join(body),
        styles=TOGGLE_STYLES,
        run_defaults_extra="<w:b/>" if defaults_bold else "",
    )


def defaults_empty() -> Doc:
    sized = style("paragraph", "ParaSized", '<w:sz w:val="28"/><w:szCs w:val="28"/>', "Normal")
    named = style(
        "paragraph",
        "ParaNamed",
        '<w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:eastAsia="Arial" w:cs="Arial"/>',
        "Normal",
    )
    body = "".join(
        [
            styled_para("Case 01: no font or size anywhere in the hierarchy", None, None, ""),
            styled_para(
                "Case 02: a paragraph style gives only the size, 14 pt", "ParaSized", None, ""
            ),
            styled_para(
                "Case 03: a paragraph style gives only the font, Arial", "ParaNamed", None, ""
            ),
        ]
    )
    return Doc(
        "E-defaults-empty",
        "The font and size Word applies when neither the document defaults nor any style "
        "gives them, alone and with a style that gives only one of the two",
        body,
        styles=sized + named,
        run_defaults=False,
    )


# The colours of the theme of these documents, chosen to be told apart in a render.
THEME_COLORS = {
    "dk1": '<a:sysClr val="windowText" lastClr="000000"/>',
    "lt1": '<a:sysClr val="window" lastClr="FFFFFF"/>',
    "dk2": '<a:srgbClr val="223344"/>',
    "lt2": '<a:srgbClr val="EEDDCC"/>',
    "accent1": '<a:srgbClr val="336699"/>',
    "accent2": '<a:srgbClr val="CC6633"/>',
    "accent3": '<a:srgbClr val="669933"/>',
    "accent4": '<a:srgbClr val="993366"/>',
    "accent5": '<a:srgbClr val="339999"/>',
    "accent6": '<a:srgbClr val="996633"/>',
    "hlink": '<a:srgbClr val="0066CC"/>',
    "folHlink": '<a:srgbClr val="663399"/>',
}
THEME_FONTS = {"major": "Georgia", "minor": "Verdana", "cs": "Tahoma"}
# The theme font each ST_Theme value of these documents stands for.
THEME_SLOTS = {
    "majorAscii": "major",
    "majorHAnsi": "major",
    "minorAscii": "minor",
    "minorHAnsi": "minor",
    "majorBidi": "cs",
    "minorBidi": "cs",
}


def theme_colors() -> Doc:
    cases = [
        ("accent1", "000000", None, None),
        ("accent1", "000000", "99", None),
        ("accent1", "000000", None, "BF"),
        ("accent1", "000000", "99", "BF"),
        ("accent2", "000000", "33", None),
        ("accent2", "000000", None, "80"),
        ("text1", "FF0000", None, None),
        ("text2", "FF0000", None, None),
        ("background2", "FF0000", None, "80"),
        ("dark2", "FF0000", None, None),
        ("hyperlink", "FF0000", None, None),
        ("followedHyperlink", "FF0000", "66", None),
    ]
    body = []
    for n, (theme, value, tint, shade) in enumerate(cases, 1):
        extra = (f' w:themeTint="{tint}"' if tint else "") + (
            f' w:themeShade="{shade}"' if shade else ""
        )
        label = (
            f"Case {n:02d}: themeColor {theme}, val {value}, tint {tint or 'none'}, "
            f"shade {shade or 'none'}"
        )
        direct = f'<w:color w:val="{value}" w:themeColor="{theme}"{extra}/>'
        body.append(styled_para(label, None, None, direct))
    return Doc(
        "E-theme-colors",
        "Theme colours of runs as Word resolves them: through a colour mapping that maps "
        "text 2 and background 2 to accents 3 and 4, with themeTint and themeShade alone "
        "and together, against a w:val that disagrees",
        "".join(body),
        theme=True,
        # Not the identity, so that the render tells a colour resolved through the
        # mapping from one resolved by its name.
        color_mapping=(("bg2", "accent4"), ("t2", "accent3")),
    )


def theme_fonts() -> Doc:
    def fonts(attrs: str) -> str:
        return f"<w:rFonts {attrs}/>"

    major = 'w:asciiTheme="majorHAnsi" w:hAnsiTheme="majorHAnsi"'
    minor = 'w:asciiTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi"'
    arial = 'w:ascii="Arial" w:hAnsi="Arial"'
    styles = "".join(
        [
            style("paragraph", "ParaMajor", fonts(major), "Normal"),
            style("paragraph", "ParaArial", fonts(arial), "Normal"),
            style("paragraph", "ParaBoth", fonts(f"{arial} {major}"), "Normal"),
            style("character", "CharMinor", fonts(minor)),
        ]
    )
    courier = fonts('w:ascii="Courier New" w:hAnsi="Courier New"')
    cases = [
        ("the document defaults' theme font, minorHAnsi", None, None, ""),
        ("a paragraph style's theme font, majorHAnsi", "ParaMajor", None, ""),
        ("a paragraph style's font, Arial", "ParaArial", None, ""),
        ("a font, then a character style's theme font", "ParaArial", "CharMinor", ""),
        ("a theme font, then a direct font, Courier New", "ParaMajor", None, courier),
        ("a font and a theme font in one w:rFonts", "ParaBoth", None, ""),
    ]
    body = "".join(
        styled_para(f"Case {n:02d}: {label}", p_style, r_style, direct)
        for n, (label, p_style, r_style, direct) in enumerate(cases, 1)
    )
    return Doc(
        "E-theme-fonts",
        "Theme fonts across the style hierarchy: whether a font or a theme font written at a "
        "later level replaces the other, and which wins when one w:rFonts gives both",
        body,
        styles=styles,
        # Calibri stays for East Asian and complex script text.
        run_default_fonts=f'{minor} w:eastAsia="Calibri" w:cs="Calibri"',
        theme=True,
    )


# Paragraph formatting across the style hierarchy: what ECMA-376 §17.3.1 and §17.9 leave
# open or [MS-OI29500] describes only in prose.


def para_style(style_id: str, ppr_xml: str, *, default: bool = False) -> str:
    """A paragraph style with these paragraph properties (the children of w:pPr)."""
    flag = ' w:default="1"' if default else ""
    return (
        f'<w:style w:type="paragraph"{flag} w:customStyle="1" w:styleId="{style_id}">'
        f'<w:name w:val="{style_id}"/><w:basedOn w:val="Normal"/>'
        f"<w:pPr>{ppr_xml}</w:pPr></w:style>"
    )


def list_level_inherit() -> Doc:
    def levels(fmt: str, templates: list[str], linked: str | None = None) -> list[str]:
        return [
            level(
                ilvl,
                fmt,
                template,
                left=720 * (ilvl + 1),
                hanging=360,
                p_style=linked if ilvl == 2 else None,
            )
            for ilvl, template in enumerate(templates)
        ]

    numbering = (
        abstract_num(0, "5C3A0021", levels("decimal", ["%1.", "%1.%2.", "%1.%2.%3."]))
        + abstract_num(1, "5C3A0022", levels("lowerLetter", ["%1)", "(%2)", "[%3]"], "Linked"))
        + num(1, 0)
        + num(2, 1)
    )
    styles = "".join(
        [
            para_style(
                "StyleLevel1", '<w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr>'
            ),
            para_style("StyleNoLevel", '<w:numPr><w:numId w:val="1"/></w:numPr>'),
            # Level 2 of definition 1 names this style; its own w:ilvl says 0. ECMA-376
            # §17.3.1.19 takes the level from w:pStyle, MS-OI29500 §17.3.1.19(a) says Word
            # does not ignore the w:ilvl.
            para_style("Linked", '<w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr>'),
        ]
    )
    cases = [
        ("StyleLevel1", None, "the style gives instance 1 and level 1"),
        ("StyleLevel1", '<w:numId w:val="1"/>', "the style gives level 1; direct numId 1, no ilvl"),
        (
            "StyleLevel1",
            '<w:ilvl w:val="2"/>',
            "the style gives instance 1; direct ilvl 2, no numId",
        ),
        ("Linked", None, "the style gives instance 2 and level 0; level 2 names the style"),
        ("StyleNoLevel", None, "the style gives instance 1 and no level"),
        (None, '<w:numId w:val="1"/>', "no style; direct numId 1, no ilvl"),
        ("StyleLevel1", '<w:numId w:val="0"/>', "the style gives instance 1; direct numId 0"),
        (
            "StyleLevel1",
            '<w:numId w:val="2"/>',
            "the style gives instance 1 and level 1; direct numId 2, no ilvl",
        ),
    ]
    body = "".join(
        para(run(f"Case {n:02d}: {label}"), p_style=style, num_pr=direct)
        for n, (style, direct, label) in enumerate(cases, 1)
    )
    return Doc(
        "E-list-level-inherit",
        "Which list level a paragraph takes when its style and its direct formatting each "
        "give part of w:numPr, also with another instance, when a level's w:pStyle names the "
        "style, and when direct formatting gives instance 0",
        body,
        numbering=numbering,
        styles=styles,
    )


def paragraph_units() -> Doc:
    styles = "".join(
        [
            para_style("CharsIndent", '<w:ind w:leftChars="200"/>'),
            para_style("LinesBefore", '<w:spacing w:beforeLines="100"/>'),
        ]
    )

    def case(n: int, label: str, style: str | None, **props) -> str:
        return para(run(f"Case {n:02d}: {label}"), p_style=style, shading=GREY, **props)

    body = "".join(
        [
            case(1, "a style indents 2 characters (leftChars 200)", "CharsIndent"),
            case(2, "the same style; direct left indent 72 pt", "CharsIndent",
                 ind={"left": 1440}),
            case(3, "the same style; direct leftChars 0 and left indent 72 pt", "CharsIndent",
                 ind={"leftChars": 0, "left": 1440}),
            case(4, "no style; one w:ind gives left 50 pt and leftChars 300", None,
                 ind={"left": 1000, "leftChars": 300}),
            case(5, "a style gives 1 line before (beforeLines 100)", "LinesBefore"),
            case(6, "the same style; direct 24 pt before", "LinesBefore",
                 space=spacing(before=24)),
            case(7, "the same style; direct beforeLines 0 and 24 pt before", "LinesBefore",
                 space=spacing(before=24, before_lines=0)),
            case(8, "no style; one w:spacing gives 24 pt and beforeLines 50", None,
                 space=spacing(before=24, before_lines=50)),
        ]
    )
    return Doc(
        "E-paragraph-units",
        "Indents in character units and space before in line units against points, at one "
        "level and across the style hierarchy, and what a value of 0 in those units does",
        body,
        styles=styles,
    )


def default_paragraph_style() -> Doc:
    # Normal is not marked w:default, so that Word's choice tells the first default
    # style, the last one and the style named Normal apart.
    first = para_style(
        "FirstDefault",
        '<w:spacing w:after="240"/><w:jc w:val="right"/>',
        default=True,
    )
    last = para_style(
        "LastDefault",
        '<w:spacing w:after="480"/><w:jc w:val="center"/>',
        default=True,
    )
    body = "".join(
        [
            para(run("Case 01: a paragraph that names no style")),
            para(run("Case 02: a paragraph that names Normal"), p_style="Normal"),
            para(run("Case 03: a paragraph that names FirstDefault"), p_style="FirstDefault"),
            para(run("Case 04: a paragraph that names LastDefault"), p_style="LastDefault"),
            para(run("Case 05: a paragraph that names no style, after the others")),
        ]
    )
    return Doc(
        "E-default-paragraph-style",
        "Which style a paragraph without w:pStyle takes when two paragraph styles are marked "
        "w:default and Normal is not: FirstDefault (right-aligned, 12 pt after), written "
        "before Normal, LastDefault (centred, 24 pt after), written last, or Normal "
        "(left-aligned, no space after)",
        body,
        styles=last,
        styles_before_normal=first,
        normal_default=False,
    )


# Run properties of list labels (#162). Each case is a list of its own, so every label
# reads "1.", and every difference in formatting the label might take is visible: a size
# of 20 pt makes the line taller in geometry.json, the rest shows in the rasters.
BOLD = "<w:b/><w:bCs/>"
NOT_BOLD = '<w:b w:val="0"/><w:bCs w:val="0"/>'
ITALIC = "<w:i/><w:iCs/>"
NOT_ITALIC = '<w:i w:val="0"/><w:iCs w:val="0"/>'
SIZE_20 = '<w:sz w:val="40"/><w:szCs w:val="40"/>'


def label_level(label: str = "", p_style: str | None = None) -> str:
    return level(0, "decimal", "%1.", left=720, hanging=360, p_style=p_style, label=label)


def label_lists(cases: list[tuple[str, dict, str, str]]) -> tuple[str, str]:
    """The numbering part and the body: per case its label, its paragraph properties
    (without the list), its definition's level 0 and the override level of its instance
    ("" for none)."""
    definitions, instances, body = [], [], []
    for n, (label, props, definition, override) in enumerate(cases, 1):
        definitions.append(abstract_num(n - 1, f"5C3A01{n:02X}", [definition]))
        instances.append(num(n, n - 1, override_level=override))
        body.append(para(run(f"Case {n:02d}: {label}"), list_item=(n, 0), **props))
    return "".join(definitions) + "".join(instances), "".join(body)


def list_label_format() -> Doc:
    styles = "".join(
        [
            style("paragraph", "BoldStyle", BOLD, based_on="Normal"),
            style("character", "MarkCharStyle", SIZE_20 + '<w:u w:val="single"/>'),
            style("paragraph", "LabelParaStyle", '<w:color w:val="FF0000"/>' + SIZE_20,
                  based_on="Normal"),
        ]
    )
    cases = [
        ("mark italic; level not italic", {"mark": ITALIC}, label_level(NOT_ITALIC), ""),
        ("style BoldStyle (bold); level not bold", {"p_style": "BoldStyle"},
         label_level(NOT_BOLD), ""),
        ("style BoldStyle (bold); level bold", {"p_style": "BoldStyle"}, label_level(BOLD), ""),
        ("definition's level bold, 20 pt; override level italic", {},
         label_level(BOLD + SIZE_20), label_level(ITALIC)),
        ("mark style MarkCharStyle (20 pt, underlined)",
         {"mark": '<w:rStyle w:val="MarkCharStyle"/>'}, label_level(), ""),
        ("level style LabelParaStyle (red, 20 pt)", {}, label_level(p_style="LabelParaStyle"),
         ""),
        ("mark bold; level 20 pt", {"mark": BOLD}, label_level(SIZE_20), ""),
    ]
    numbering, body = label_lists(cases)
    return Doc(
        "E-list-label-format",
        "The run properties of a list label: the paragraph mark's, its character style's and "
        "its paragraph style's under a level's own, whether a level's on/off values set or "
        "toggle, an override level's against its definition's, and a level's linked style",
        body,
        numbering=numbering,
        styles=styles,
    )


def list_label_rstyle() -> Doc:
    # MS-OI29500 §17.9.24(a): Word does not allow w:rStyle and w:highlight in a level's run
    # properties. A document of their own, so that a file Word refuses costs no other case.
    styles = style("character", "LabelCharStyle", SIZE_20)
    cases = [
        ("level character style LabelCharStyle (20 pt)", {},
         label_level('<w:rStyle w:val="LabelCharStyle"/>'), ""),
        ("level highlight yellow", {}, label_level('<w:highlight w:val="yellow"/>'), ""),
    ]
    numbering, body = label_lists(cases)
    return Doc(
        "E-list-label-rstyle",
        "Whether Word opens a list level whose run properties name a character style or a "
        "highlight, which MS-OI29500 says it does not allow, and whether the label shows them",
        body,
        numbering=numbering,
        styles=styles,
    )


# OpenType features (#184): ligatures and kerning against enableOpenTypeFeatures.
OPEN_TYPE_TEXT = "office affine fluffy AVATAR Type"
LIGATURES = '<w14:ligatures w14:val="standard"/>'
KERNING = '<w:kern w:val="2"/>'
OPEN_TYPE_CASES = [
    ("plain", ""),
    ("standard ligatures", LIGATURES),
    ("kerning from 1 pt", KERNING),
    ("standard ligatures and kerning from 1 pt", KERNING + LIGATURES),
]


def open_type(name: str, mode: int, setting: int | None) -> Doc:
    said = {1: "on", 0: "off", None: "not written"}[setting]
    body = "".join(
        para(run(f"Case {n:02d}: {label}"), space=spacing(after=6))
        + para(
            # Word 2010's w14 run properties follow every w: one, as Word writes them.
            f"<w:r>{f'<w:rPr>{props}</w:rPr>' if props else ''}{text(OPEN_TYPE_TEXT)}</w:r>",
            space=spacing(after=12),
        )
        for n, (label, props) in enumerate(OPEN_TYPE_CASES, 1)
    )
    return Doc(
        name,
        f"Ligatures (w14:ligatures) and kerning (w:kern) in Calibri 11 pt, in compatibility "
        f"mode {mode} with the setting enableOpenTypeFeatures {said}: the rasters show the "
        "ligatures, the word boxes the kerning of AVATAR",
        body,
        mode=mode,
        compat_values=(("enableOpenTypeFeatures", setting),) if mode == 15 else (),
    )


def default_paragraph_style_after_normal() -> Doc:
    # E-default-paragraph-style showed that of two default styles, neither of them Normal,
    # Word takes the last. Here Normal is marked default and is not the last (#150).
    later = para_style(
        "LaterDefault",
        '<w:spacing w:after="480"/><w:jc w:val="center"/>',
        default=True,
    )
    body = "".join(
        [
            para(run("Case 01: a paragraph that names no style")),
            para(run("Case 02: a paragraph that names Normal"), p_style="Normal"),
            para(run("Case 03: a paragraph that names LaterDefault"), p_style="LaterDefault"),
            para(run("Case 04: a paragraph that names no style, after the others")),
        ]
    )
    return Doc(
        "E-default-paragraph-style-after-normal",
        "Which style a paragraph without w:pStyle takes when Normal (left-aligned, no space "
        "after) is marked w:default and LaterDefault (centred, 24 pt after), written after it, "
        "is marked w:default too",
        body,
        styles=later,
    )


def documents() -> list[Doc]:
    return [
        line_fonts(),
        line_exact(),
        line_at_least(),
        line_multiple(),
        advance_rounding(12),
        advance_rounding(14),
        advance_rounding(15),
        space_before_top(),
        autospacing(False),
        autospacing(True),
        table_indent(14),
        table_indent(15),
        list_shared_abstract(),
        tab_hanging(False),
        tab_hanging(True),
        style_toggles(False),
        style_toggles(True),
        defaults_empty(),
        theme_colors(),
        theme_fonts(),
        tab_hanging(False, 14),
        tab_hanging(True, 14),
        line_fonts_truetype(),
        small_caps(12),
        small_caps(14),
        small_caps(15),
        italic_missing(),
        list_level_inherit(),
        paragraph_units(),
        default_paragraph_style(),
        list_label_format(),
        list_label_rstyle(),
        open_type("E-opentype-on", 15, 1),
        open_type("E-opentype-off", 15, 0),
        open_type("E-opentype-none", 15, None),
        open_type("E-opentype-12", 12, None),
        default_paragraph_style_after_normal(),
    ]


# Package parts.


def xml_part(root: str) -> bytes:
    return ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n' + root).encode()


def styles_xml(doc: Doc) -> str:
    hidden = '<w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/>'
    run_defaults = ""
    if doc.run_defaults:
        # Other properties follow w:rFonts, as the order of CT_RPr asks.
        run_defaults = (
            "<w:rPrDefault><w:rPr>"
            f"<w:rFonts {doc.run_default_fonts}/>{doc.run_defaults_extra}"
            '<w:sz w:val="22"/><w:szCs w:val="22"/>'
            '<w:lang w:val="en-US" w:eastAsia="en-US" w:bidi="ar-SA"/>'
            "</w:rPr></w:rPrDefault>"
        )
    normal_default = ' w:default="1"' if doc.normal_default else ""
    return (
        f'<w:styles xmlns:w="{W_NS}">'
        "<w:docDefaults>"
        f"{run_defaults}"
        "<w:pPrDefault><w:pPr><w:widowControl/>"
        '<w:spacing w:after="0" w:line="240" w:lineRule="auto"/>'
        "</w:pPr></w:pPrDefault>"
        "</w:docDefaults>"
        f"{doc.styles_before_normal}"
        f'<w:style w:type="paragraph"{normal_default} w:styleId="Normal">'
        '<w:name w:val="Normal"/><w:qFormat/></w:style>'
        '<w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont">'
        '<w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/>'
        "<w:semiHidden/><w:unhideWhenUsed/></w:style>"
        '<w:style w:type="table" w:default="1" w:styleId="TableNormal">'
        f'<w:name w:val="Normal Table"/>{hidden}<w:tblPr>'
        '<w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar>'
        '<w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/>'
        '<w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/>'
        "</w:tblCellMar></w:tblPr></w:style>"
        '<w:style w:type="numbering" w:default="1" w:styleId="NoList">'
        f'<w:name w:val="No List"/>{hidden}</w:style>'
        f"{doc.styles}"
        "</w:styles>"
    )


A_NS = "http://schemas.openxmlformats.org/drawingml/2006/main"


def theme_xml() -> str:
    """A theme of this repository: the colours and fonts of THEME_COLORS and THEME_FONTS,
    and the smallest format scheme the schema allows (three styles in each list)."""
    colors = "".join(f"<a:{name}>{color}</a:{name}>" for name, color in THEME_COLORS.items())

    def collection(kind: str) -> str:
        latin = THEME_FONTS[kind]
        return (
            f'<a:{kind}Font><a:latin typeface="{latin}"/><a:ea typeface=""/>'
            f'<a:cs typeface="{THEME_FONTS["cs"]}"/></a:{kind}Font>'
        )

    fill = '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
    line = f'<a:ln w="6350">{fill}</a:ln>'
    effect = "<a:effectStyle><a:effectLst/></a:effectStyle>"
    fmt = (
        '<a:fmtScheme name="Officewright">'
        f"<a:fillStyleLst>{fill * 3}</a:fillStyleLst>"
        f"<a:lnStyleLst>{line * 3}</a:lnStyleLst>"
        f"<a:effectStyleLst>{effect * 3}</a:effectStyleLst>"
        f"<a:bgFillStyleLst>{fill * 3}</a:bgFillStyleLst>"
        "</a:fmtScheme>"
    )
    return (
        f'<a:theme xmlns:a="{A_NS}" name="Officewright">'
        f'<a:themeElements><a:clrScheme name="Officewright">{colors}</a:clrScheme>'
        f'<a:fontScheme name="Officewright">{collection("major")}{collection("minor")}'
        f"</a:fontScheme>{fmt}</a:themeElements></a:theme>"
    )


def compat_setting(name: str, value: int) -> str:
    return f'<w:compatSetting w:name="{name}" w:uri="{WORD_URI}" w:val="{value}"/>'


# The colour mapping Word writes, in its order: each theme colour stands for itself.
IDENTITY_MAPPING = (
    ("bg1", "light1"),
    ("t1", "dark1"),
    ("bg2", "light2"),
    ("t2", "dark2"),
    ("accent1", "accent1"),
    ("accent2", "accent2"),
    ("accent3", "accent3"),
    ("accent4", "accent4"),
    ("accent5", "accent5"),
    ("accent6", "accent6"),
    ("hyperlink", "hyperlink"),
    ("followedHyperlink", "followedHyperlink"),
)


# The settings Word writes into a new document in mode 15 ([MS-DOCX] 2.3.1 to 2.3.4), each
# with the value 1.
MODE_15_SETTINGS = (
    "overrideTableStyleFontSizeAndJustification",
    "enableOpenTypeFeatures",
    "doNotFlipMirrorIndents",
    "differentiateMultirowTableHeaders",
)


def settings_xml(doc: Doc) -> str:
    flags = "".join(f"<w:{flag}/>" for flag in doc.compat_flags)
    mapping = ""
    if doc.theme:
        entries = dict(IDENTITY_MAPPING)
        values = {value for _, value in IDENTITY_MAPPING}
        for name, value in doc.color_mapping:
            if name not in entries:
                raise ValueError(f"{doc.name}: w:clrSchemeMapping has no attribute {name}")
            # The values of IDENTITY_MAPPING are those of ST_WmlColorSchemeIndex.
            if value not in values:
                raise ValueError(f"{doc.name}: {value} is not a theme colour to map {name} to")
        entries.update(doc.color_mapping)
        attributes = " ".join(f'w:{name}="{value}"' for name, value in entries.items())
        mapping = f"<w:clrSchemeMapping {attributes}/>"
    settings = ""
    if doc.mode != 12:
        settings = compat_setting("compatibilityMode", doc.mode)
    values = dict(doc.compat_values)
    for name in values:
        if name not in MODE_15_SETTINGS or doc.mode != 15:
            raise ValueError(f"{doc.name}: {name} is not a setting Word writes in this mode")
    if doc.mode == 15:
        for name in MODE_15_SETTINGS:
            value = values.get(name, 1)
            if value is not None:
                settings += compat_setting(name, value)
    return (
        f'<w:settings xmlns:w="{W_NS}">'
        '<w:zoom w:percent="100"/>'
        '<w:defaultTabStop w:val="720"/>'
        '<w:characterSpacingControl w:val="doNotCompress"/>'
        f"<w:compat>{flags}{settings}</w:compat>"
        # The languages and number format are set, so that a re-saved copy that names the
        # language or locale of the machine Word ran on stands out (corpus-import.py).
        '<w:themeFontLang w:val="en-US" w:eastAsia="en-US" w:bidi="ar-SA"/>'
        f"{mapping}"
        '<w:decimalSymbol w:val="."/>'
        '<w:listSeparator w:val=","/>'
        "</w:settings>"
    )


def font_table_xml(fonts: list[str]) -> str:
    entries = "".join(f'<w:font w:name="{attr(f)}"/>' for f in fonts)
    return f'<w:fonts xmlns:w="{W_NS}">{entries}</w:fonts>'


def core_xml(doc: Doc) -> str:
    return (
        '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/'
        'metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" '
        'xmlns:dcterms="http://purl.org/dc/terms/" '
        'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
        f"<dc:title>{escape(doc.name)}</dc:title>"
        f"<dc:description>{escape(doc.purpose)}</dc:description>"
        "<dc:creator>Officewright</dc:creator>"
        "<cp:lastModifiedBy>Officewright</cp:lastModifiedBy>"
        '<dcterms:created xsi:type="dcterms:W3CDTF">2026-10-12T00:00:00Z</dcterms:created>'
        '<dcterms:modified xsi:type="dcterms:W3CDTF">2026-10-12T00:00:00Z</dcterms:modified>'
        "</cp:coreProperties>"
    )


def relationships(rels: list[tuple[str, str]]) -> str:
    entries = "".join(
        f'<Relationship Id="rId{i}" Type="{REL}/{kind}" Target="{target}"/>'
        for i, (kind, target) in enumerate(rels, 1)
    )
    ns = "http://schemas.openxmlformats.org/package/2006/relationships"
    return f'<Relationships xmlns="{ns}">{entries}</Relationships>'


def parts(doc: Doc) -> list[tuple[str, bytes]]:
    """The package parts of a document, in the order they are stored."""
    namespaces = f'xmlns:w="{W_NS}" xmlns:r="{R_NS}"'
    if "<w14:" in doc.body:
        # Word 2010's run properties, which an application that does not know them ignores
        # (ECMA-376 Part 3, mc:Ignorable).
        namespaces += f' xmlns:w14="{W14_NS}" xmlns:mc="{MC_NS}" mc:Ignorable="w14"'
    document = (
        f"<w:document {namespaces}>"
        f"<w:body>{doc.body}{sect_pr()}</w:body></w:document>"
    )
    numbering = None
    if doc.numbering:
        numbering = f'<w:numbering xmlns:w="{W_NS}">{doc.numbering}</w:numbering>'

    used = document + styles_xml(doc) + (numbering or "")
    # The fonts every w:rFonts names, directly or through a theme attribute.
    fonts = set()
    for attributes in re.findall(r"<w:rFonts ([^>]*)/>", used):
        fonts.update(re.findall(r'w:(?:ascii|hAnsi|eastAsia|cs)="([^"]+)"', attributes))
        for value in re.findall(r'w:(?:ascii|hAnsi|eastAsia|cs)Theme="([^"]+)"', attributes):
            fonts.add(THEME_FONTS[THEME_SLOTS[value]])
    fonts = sorted(fonts)

    word_rels = [
        ("styles", "styles.xml"),
        ("settings", "settings.xml"),
        ("fontTable", "fontTable.xml"),
    ]
    overrides = [
        ("/word/document.xml", f"{CT_W}.document.main+xml"),
        ("/word/styles.xml", f"{CT_W}.styles+xml"),
        ("/word/settings.xml", f"{CT_W}.settings+xml"),
        ("/word/fontTable.xml", f"{CT_W}.fontTable+xml"),
        ("/docProps/core.xml", "application/vnd.openxmlformats-package.core-properties+xml"),
    ]
    if numbering:
        word_rels.append(("numbering", "numbering.xml"))
        overrides.append(("/word/numbering.xml", f"{CT_W}.numbering+xml"))
    if doc.theme:
        word_rels.append(("theme", "theme/theme1.xml"))
        overrides.append(
            ("/word/theme/theme1.xml", "application/vnd.openxmlformats-officedocument.theme+xml")
        )
    content_types = (
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.'
        'relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        + "".join(f'<Override PartName="{p}" ContentType="{t}"/>' for p, t in overrides)
        + "</Types>"
    )
    root_rels = (
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        f'<Relationship Id="rId1" Type="{REL}/officeDocument" Target="word/document.xml"/>'
        '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/'
        'relationships/metadata/core-properties" Target="docProps/core.xml"/>'
        "</Relationships>"
    )
    result = [
        ("[Content_Types].xml", xml_part(content_types)),
        ("_rels/.rels", xml_part(root_rels)),
        ("docProps/core.xml", xml_part(core_xml(doc))),
        ("word/document.xml", xml_part(document)),
        ("word/styles.xml", xml_part(styles_xml(doc))),
        ("word/settings.xml", xml_part(settings_xml(doc))),
        ("word/fontTable.xml", xml_part(font_table_xml(fonts))),
    ]
    if numbering:
        result.append(("word/numbering.xml", xml_part(numbering)))
    if doc.theme:
        result.append(("word/theme/theme1.xml", xml_part(theme_xml())))
    result.append(("word/_rels/document.xml.rels", xml_part(relationships(word_rels))))
    for name, data in result:
        try:
            minidom.parseString(data)
        except Exception as error:  # any parse failure is a bug in this script
            raise ValueError(f"{doc.name}: {name} is not well-formed XML: {error}") from error
    return result


def package(doc: Doc) -> bytes:
    out = BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as archive:
        for name, data in parts(doc):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 0
            info.external_attr = 0
            archive.writestr(info, data, zipfile.ZIP_STORED)
    return out.getvalue()


def stored_parts(path: Path) -> list[tuple[str, bytes]]:
    with zipfile.ZipFile(path) as archive:
        return [(info.filename, archive.read(info)) for info in archive.infolist()]


def sources_row(doc: Doc) -> str:
    return (
        f"| word-rules/{doc.name}.docx | Written by scripts/make-rule-docs.py "
        f"| BUSL-1.1 (this repository) | {doc.purpose} |"
    )


def check(docs: list[Doc]) -> list[str]:
    """What differs between the committed documents and the ones this script writes."""
    problems = []
    expected = {f"{doc.name}.docx": doc for doc in docs}
    present = {p.name for p in OUT_DIR.glob("*")} if OUT_DIR.is_dir() else set()
    for name in sorted(present - set(expected)):
        problems.append(f"tests/corpus/word-rules/{name} is not written by this script")
    lines = SOURCES.read_text(encoding="utf-8").splitlines()
    rows = [number for number, line in enumerate(lines) if line.startswith("| word-rules/")]
    # The rows are table rows: one block of table lines, right under the table's header.
    if rows and not all(
        line.startswith("|") for line in lines[rows[0] - 1 : rows[-1] + 1]
    ):
        problems.append(
            "the word-rules rows of tests/corpus/SOURCES.md are not one block of its table"
        )
    for name, doc in expected.items():
        path = OUT_DIR / name
        if name not in present:
            problems.append(f"tests/corpus/word-rules/{name} is missing")
        elif path.read_bytes() != package(doc):
            # The bytes are what Word renders and corpus-import.py checks by SHA-256.
            committed = f"tests/corpus/word-rules/{name}"
            if stored_parts(path) != parts(doc):
                problems.append(f"{committed} differs from what this script writes")
            else:
                problems.append(f"{committed} has the parts this script writes in other ZIP bytes")
        if not any(lines[number].startswith(f"| word-rules/{name} |") for number in rows):
            problems.append(f"tests/corpus/SOURCES.md has no row for word-rules/{name}")
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="compare instead of writing")
    mode.add_argument("--list", action="store_true", help="print the SOURCES.md rows")
    args = parser.parse_args()
    docs = documents()
    if args.list:
        for doc in docs:
            print(sources_row(doc))
        return 0
    if args.check:
        problems = check(docs)
        for problem in problems:
            print(f"make-rule-docs.py: {problem}", file=sys.stderr)
        if problems:
            print(
                "make-rule-docs.py: run scripts/make-rule-docs.py and commit the result",
                file=sys.stderr,
            )
            return 1
        print(f"make-rule-docs.py: {len(docs)} documents are up to date")
        return 0
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for doc in docs:
        (OUT_DIR / f"{doc.name}.docx").write_bytes(package(doc))
    print(f"make-rule-docs.py: wrote {len(docs)} documents to {OUT_DIR.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
