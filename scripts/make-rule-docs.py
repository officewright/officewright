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
import struct
import sys
import zipfile
import zlib
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
# VML and its Office and Word extensions, for pictures in w:pict (ECMA-376 Part 4, §14).
V_NS = "urn:schemas-microsoft-com:vml"
O_NS = "urn:schemas-microsoft-com:office:office"
W10_NS = "urn:schemas-microsoft-com:office:word"
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
    keep_next: bool = False,
    keep_lines: bool = False,
    page_break_before: bool = False,
    widow_control: bool | None = None,
    list_item: tuple[int, int] | None = None,
    num_pr: str | None = None,
    borders: str | None = None,
    shading: str | None = None,
    tabs: list[tuple[str, int] | tuple[str, int, str]] | None = None,
    space: str | None = None,
    ind: dict[str, int] | None = None,
    jc: str | None = None,
    mark: str | None = None,
    sect: str | None = None,
) -> str:
    """mark is the content of the paragraph mark's w:rPr; borders is a w:pBdr."""
    parts = []
    if p_style:
        parts.append(f'<w:pStyle w:val="{p_style}"/>')
    if keep_next:
        parts.append("<w:keepNext/>")
    if keep_lines:
        parts.append("<w:keepLines/>")
    if page_break_before:
        parts.append("<w:pageBreakBefore/>")
    if widow_control is not None:
        parts.append("<w:widowControl/>" if widow_control else '<w:widowControl w:val="0"/>')
    if list_item is not None:
        num_id, ilvl = list_item
        parts.append(f'<w:numPr><w:ilvl w:val="{ilvl}"/><w:numId w:val="{num_id}"/></w:numPr>')
    if num_pr is not None:
        parts.append(f"<w:numPr>{num_pr}</w:numPr>")
    if borders:
        parts.append(borders)
    if shading:
        parts.append(f'<w:shd w:val="clear" w:color="auto" w:fill="{shading}"/>')
    if tabs:
        # A stop is (w:val, w:pos) or (w:val, w:pos, w:leader).
        stops = "".join(
            f'<w:tab w:val="{stop[0]}"'
            + (f' w:leader="{stop[2]}"' if len(stop) > 2 else "")
            + f' w:pos="{stop[1]}"/>'
            for stop in tabs
        )
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


def section(
    *,
    kind: str | None = None,
    refs: tuple[tuple[str, str, str], ...] = (),
    page_start: int | None = None,
    margins: tuple[int, int] = (MARGIN, MARGIN),
    page_format: str | None = None,
    title_page: bool = False,
    size: tuple[int, int] = (PAGE_W, PAGE_H),
    sides: tuple[int, int] = (MARGIN, MARGIN),
    header_distance: int = 708,
) -> str:
    """A section's properties: A4 portrait as sect_pr(), with a start type (w:type), header
    and footer references as (headerReference or footerReference, w:type, part name), a
    first page number and the top and bottom margins (negative for a fixed body edge); the
    format of its page numbers (w:pgNumType w:fmt) and a title page (w:titlePg); another
    page size, left and right margins and distance of the header from the edge, in twips."""
    references = "".join(
        f'<w:{element} w:type="{ref_type}" r:id="{part_id(name)}"/>'
        for element, ref_type, name in refs
    )
    start = f'<w:type w:val="{kind}"/>' if kind else ""
    numbers = f'<w:pgNumType w:start="{page_start}"/>' if page_start is not None else ""
    if page_format is not None:
        first = f' w:start="{page_start}"' if page_start is not None else ""
        numbers = f'<w:pgNumType w:fmt="{page_format}"{first}/>'
    title = "<w:titlePg/>" if title_page else ""
    top, bottom = margins
    left, right = sides
    return (
        f"<w:sectPr>{references}{start}"
        f'<w:pgSz w:w="{size[0]}" w:h="{size[1]}"/>'
        f'<w:pgMar w:top="{top}" w:right="{right}" w:bottom="{bottom}" w:left="{left}"'
        f' w:header="{header_distance}" w:footer="708" w:gutter="0"/>'
        f'{numbers}<w:cols w:space="708"/>{title}'
        '<w:docGrid w:linePitch="360"/>'
        "</w:sectPr>"
    )


def part_id(name: str) -> str:
    """The relationship id of a header or footer part: rIdheader1 for header1.xml."""
    return "rId" + name.rsplit(".", 1)[0]


def header_part(name: str, paragraphs: str) -> tuple[str, str]:
    """A header (header*.xml) or footer (footer*.xml) part holding these paragraphs."""
    root = "hdr" if name.startswith("header") else "ftr"
    return name, f'<w:{root} xmlns:w="{W_NS}" xmlns:r="{R_NS}">{paragraphs}</w:{root}>'


def page_field() -> str:
    """The current page number, as a simple field."""
    return '<w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple>'


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
    # Whether Normal is marked w:default, and the w:pPr and w:rPr of Normal.
    normal_default: bool = True
    normal_props: str = ""
    # Whether the document defaults give run properties (the fonts of run_default_fonts at
    # run_default_size), and properties added to them.
    run_defaults: bool = True
    run_defaults_extra: str = ""
    # Properties added to the document defaults' w:pPr after their w:spacing, such as a w:jc,
    # and a w:numPr for them, which goes after their w:widowControl.
    paragraph_defaults_extra: str = ""
    paragraph_defaults_list: str = ""
    # The size of the document defaults' runs, in points.
    run_default_size: float = 11
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
    # Header and footer parts, as (part name under word/, root element), such as
    # header_part("header1.xml", ...); sections name them through section().
    headers: tuple[tuple[str, str], ...] = ()
    # The last section's w:sectPr, when it is not sect_pr()'s.
    last_section: str = ""
    # Pictures under word/media/, as (file name, PNG bytes); the body names each by the
    # relationship id media_id() gives it.
    media: tuple[tuple[str, bytes], ...] = ()


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
    # the text after the list label starts at 108.02 pt in case B and at 144.02 pt in case G
    # (36 pt and 72 pt past the margin), where E-tab-hanging-14 has it at 122.06 pt in both.
    # Each x is from the left page edge, the margin at 72.02 pt, in
    # tests/corpus/reference/word-rules/E-tab-hanging-noindenttab-14/geometry.json.
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
# reads "1.", and every difference in formatting the label might take is visible: the
# label's word box in geometry.json shows whether it took a size of 20 pt (where only the
# label could take it, the line grows taller too), the rest shows in the rasters.
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


def linked_style(style_id: str, instance: int, rpr_xml: str) -> str:
    """A paragraph style that names a list instance back (w:numPr), as Word writes the style
    a list level links to with its w:pStyle."""
    return (
        f'<w:style w:type="paragraph" w:customStyle="1" w:styleId="{style_id}">'
        f'<w:name w:val="{style_id}"/><w:basedOn w:val="Normal"/>'
        f'<w:pPr><w:numPr><w:numId w:val="{instance}"/></w:numPr></w:pPr>'
        f"<w:rPr>{rpr_xml}</w:rPr></w:style>"
    )


def list_label_format() -> Doc:
    red_20 = '<w:color w:val="FF0000"/>' + SIZE_20
    styles = "".join(
        [
            style("paragraph", "BoldStyle", BOLD, based_on="Normal"),
            style("character", "MarkCharStyle", SIZE_20 + '<w:u w:val="single"/>'),
            # Case 06's level links to it, and it names case 06's instance back.
            linked_style("LabelParaStyle", 6, red_20),
            # Case 08's level links to it, but it names no instance.
            style("paragraph", "LabelParaOneWay", red_20, based_on="Normal"),
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
        ("level style LabelParaStyle (red, 20 pt), linked both ways", {},
         label_level(p_style="LabelParaStyle"), ""),
        ("mark bold; level 20 pt", {"mark": BOLD}, label_level(SIZE_20), ""),
        ("level style LabelParaOneWay (red, 20 pt), linked one way", {},
         label_level(p_style="LabelParaOneWay"), ""),
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


# Table styles (#172). Each conditional format of a table style gives its cells a text
# colour of its own and a grey of its own: formatting.json records the colour of every
# cell paragraph's first character, so it names the format that won in each cell, and the
# rasters show the greys.
CONDITIONS = [
    # (w:type of w:tblStylePr, text colour, cell fill), in the order of ST_TblStyleOverrideType
    ("firstRow", "C00000", "F2F2F2"),
    ("lastRow", "00B050", "E6E6E6"),
    ("firstCol", "0070C0", "D9D9D9"),
    ("lastCol", "7030A0", "CCCCCC"),
    ("band1Vert", "FFC000", "BFBFBF"),
    ("band2Vert", "00B0F0", "B3B3B3"),
    ("band1Horz", "FF0000", "A6A6A6"),
    ("band2Horz", "92D050", "999999"),
    ("neCell", "002060", "8C8C8C"),
    ("nwCell", "833C0B", "808080"),
    ("seCell", "375623", "737373"),
    ("swCell", "C55A11", "666666"),
]
ROW_BANDS = [c for c in CONDITIONS if c[0] in ("band1Horz", "band2Horz")]
COL_BANDS = [c for c in CONDITIONS if c[0] in ("band1Vert", "band2Vert")]


def look(first_row: int, last_row: int, first_col: int, last_col: int, rows: int, cols: int):
    """The attributes of w:tblLook; rows and cols say whether row and column bands are on."""
    return (
        f'w:firstRow="{first_row}" w:lastRow="{last_row}" w:firstColumn="{first_col}" '
        f'w:lastColumn="{last_col}" w:noHBand="{1 - rows}" w:noVBand="{1 - cols}"'
    )


def table_style(
    style_id: str,
    conditions: list[tuple[str, str, str]],
    *,
    row_band: int | None = None,
    col_band: int | None = None,
    width: str = "",
    layout: str = "",
    whole: str = "",
    cell_margins: str = "",
    cell: str = "",
    default: bool = False,
) -> str:
    """A table style with single borders whose conditional formats give a text colour and a
    cell fill each. width, layout and cell_margins go into its w:tblPr in their places
    (w:tblW before the borders, w:tblLayout and then w:tblCellMar after them); whole is the
    w:pPr and w:rPr of the whole table, and cell its w:tcPr. default marks the style
    w:default."""
    bands = ""
    if row_band is not None:
        bands += f'<w:tblStyleRowBandSize w:val="{row_band}"/>'
    if col_band is not None:
        bands += f'<w:tblStyleColBandSize w:val="{col_band}"/>'
    sides = ("top", "left", "bottom", "right", "insideH", "insideV")
    borders = "<w:tblBorders>" + "".join(border(s) for s in sides) + "</w:tblBorders>"
    overrides = "".join(
        f'<w:tblStylePr w:type="{kind}"><w:rPr><w:color w:val="{color}"/></w:rPr>'
        f'<w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="{fill}"/></w:tcPr>'
        "</w:tblStylePr>"
        for kind, color, fill in conditions
    )
    marked = ' w:default="1"' if default else ""
    return (
        f'<w:style w:type="table"{marked} w:customStyle="1" w:styleId="{style_id}">'
        f'<w:name w:val="{style_id}"/><w:basedOn w:val="TableNormal"/>{whole}'
        f"<w:tblPr>{bands}{width}{borders}{layout}{cell_margins}</w:tblPr>{cell}{overrides}"
        "</w:style>"
    )


def styled_table(
    style_id: str | None,
    cells: list[list[str]],
    *,
    look_attributes: str | None = None,
    widths: list[int] | None = None,
    grid: list[int] | None = None,
    header_rows: int | tuple[int, ...] = 0,
) -> str:
    """A table in the table style, or in none when style_id is None; cells holds the
    paragraphs of each cell, row by row. look_attributes None leaves w:tblLook out. widths
    gives the grid and each cell's width; None leaves the table and cell widths out, which
    leaves them to the table style or to Word, over the given grid (an even one by
    default). The first header_rows rows, or the rows a tuple numbers from 1, repeat as
    header rows (w:tblHeader)."""
    cols = len(cells[0])
    grid = widths or grid or [TEXT_W // cols] * cols
    if len(grid) != cols:
        raise ValueError("the grid has a column for every cell of a row")
    if isinstance(header_rows, int):
        header_rows = tuple(range(1, header_rows + 1))
    tbl_pr = f'<w:tblStyle w:val="{style_id}"/>' if style_id is not None else ""
    if widths:
        tbl_pr += f'<w:tblW w:w="{sum(widths)}" w:type="dxa"/>'
    if look_attributes is not None:
        tbl_pr += f"<w:tblLook {look_attributes}/>"
    grid_xml = "".join(f'<w:gridCol w:w="{w}"/>' for w in grid)
    rows = []
    for number, row in enumerate(cells):
        if len(row) != cols:
            raise ValueError("every row has as many cells as the first")
        tcs = []
        for content, width in zip(row, grid):
            tc_pr = f'<w:tcPr><w:tcW w:w="{width}" w:type="dxa"/></w:tcPr>' if widths else ""
            tcs.append(f"<w:tc>{tc_pr}{content}</w:tc>")
        tr_pr = "<w:trPr><w:tblHeader/></w:trPr>" if number + 1 in header_rows else ""
        rows.append(f"<w:tr>{tr_pr}{''.join(tcs)}</w:tr>")
    return (
        f"<w:tbl><w:tblPr>{tbl_pr}</w:tblPr><w:tblGrid>{grid_xml}</w:tblGrid>"
        f"{''.join(rows)}</w:tbl>"
    )


def named_cells(rows: int, cols: int) -> list[list[str]]:
    """Cells holding r<row>c<column>, so that formatting.json says which cell each is."""
    return [
        [para(run(f"r{r}c{c}")) for c in range(1, cols + 1)] for r in range(1, rows + 1)
    ]


def table_case(n: int, label: str, table_xml: str) -> str:
    """A labelled table, and an empty paragraph after it, so that no two tables touch."""
    return para(run(f"Case {n:02d}: {label}")) + table_xml + para()


def table_style_look() -> Doc:
    even = [TEXT_W // 5] * 5
    cases = [
        ("every condition on", look(1, 1, 1, 1, 1, 1)),
        ("no w:tblLook", None),
        ("first row and last column on, the rest off", look(1, 0, 0, 1, 0, 0)),
        ("only the old bitmask, w:val 04A0", 'w:val="04A0"'),
        ("w:val 04A0 and w:firstRow 0", 'w:val="04A0" w:firstRow="0"'),
        # #172: are the options a partial w:tblLook leaves out off or on?
        ("only w:firstRow 1", 'w:firstRow="1"'),
    ]
    body = "".join(
        table_case(
            n, label,
            styled_table("Conditions", named_cells(5, 5), look_attributes=attrs, widths=even),
        )
        for n, (label, attrs) in enumerate(cases, 1)
    )
    return Doc(
        "E-table-style-look",
        "Which conditional format of a table style each cell of a 5x5 table takes, by its "
        "w:tblLook: every condition on, none written, first row and last column only, the "
        "old bitmask alone and with a newer attribute, and only w:firstRow; each format "
        "gives a text colour of its own, which formatting.json records, and a grey of its "
        "own",
        body,
        styles=table_style("Conditions", CONDITIONS, row_band=1, col_band=1),
    )


def table_style_bands() -> Doc:
    rows_only, cols_only = look(0, 0, 0, 0, 1, 0), look(0, 0, 0, 0, 0, 1)
    styles = "".join(
        [
            table_style("RowBandsUnset", ROW_BANDS),
            table_style("RowBands0", ROW_BANDS, row_band=0),
            table_style("RowBands1", ROW_BANDS, row_band=1),
            table_style("RowBands2", ROW_BANDS, row_band=2),
            table_style("ColBands2", COL_BANDS, col_band=2),
        ]
    )
    cases = [
        ("row bands, no band size written", "RowBandsUnset", 6, 3, rows_only),
        ("row band size 0", "RowBands0", 6, 3, rows_only),
        ("row band size 1", "RowBands1", 6, 3, rows_only),
        ("row band size 2", "RowBands2", 6, 3, rows_only),
        ("column band size 2", "ColBands2", 3, 6, cols_only),
    ]
    body = "".join(
        table_case(
            n, label,
            styled_table(style_id, named_cells(rows, cols), look_attributes=attrs,
                         widths=[TEXT_W // cols] * cols),
        )
        for n, (label, style_id, rows, cols, attrs) in enumerate(cases, 1)
    )
    return Doc(
        "E-table-style-bands",
        "How many rows or columns each band of a table style spans: row band sizes of 0, 1 "
        "and 2 and none written, and a column band size of 2; each band gives a text colour "
        "of its own and a grey of its own",
        body,
        styles=styles,
    )


def table_style_bands_5() -> Doc:
    # MS-OI29500 §17.7.6.7(c): Word restricts the band size to 0 to 3 (#172). ECMA-376's
    # schema allows any integer, but the Open XML SDK's does not allow more than 3. A
    # document of its own, so that a file Word refuses costs no other case.
    body = table_case(
        1, "row band size 5, which Word restricts to 0 to 3",
        styled_table("RowBands5", named_cells(8, 3), look_attributes=look(0, 0, 0, 0, 1, 0),
                     widths=[TEXT_W // 3] * 3),
    )
    return Doc(
        "E-table-style-bands-5",
        "How many rows each band of a table style spans with a row band size of 5, above the "
        "3 Word allows: by 5, by 3, by 1 or not at all; each band gives a text colour of its "
        "own and a grey of its own",
        body,
        styles=table_style("RowBands5", ROW_BANDS, row_band=5),
    )


def table_style_size(size: int, setting: int | None, mode: int = 15) -> Doc:
    # [MS-DOCX] 2.3.1, overrideTableStyleFontSizeAndJustification: when it is false, a
    # default paragraph style of 11 or 12 pt does not override a table style's font size,
    # and a justification of left does not override its justification. Word writes the
    # setting only in mode 15; the mode-14 twins lack it the way documents of Word 2010
    # do, in case Word adds it to the mode-15 documents that leave it out when it saves
    # them again.
    big = table_style(
        "Big", [],
        whole='<w:pPr><w:jc w:val="center"/></w:pPr>'
        '<w:rPr><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr>',
    )
    half = [TEXT_W // 2] * 2
    hp = half_points(size)
    said = {1: "on", None: "not written"}[setting]
    if mode != 15:
        said = f"not written, in compatibility mode {mode}"
    body = table_case(
        1, f"a table in the style Big (14 pt, centred); Normal {size} pt, left",
        styled_table("Big", named_cells(2, 2), widths=half),
    ) + para(run("Case 02: a paragraph after the table, in Normal"))
    suffix = "" if setting == 1 else "-none" if mode == 15 else f"-{mode}"
    return Doc(
        f"E-table-style-size-{size}pt{suffix}",
        f"Whether a Normal style of {size} pt, justified left, overrides the 14 pt and the "
        "centring of a table style, with the setting overrideTableStyleFontSizeAndJustification "
        f"{said}",
        body,
        styles=big,
        normal_props=f'<w:pPr><w:jc w:val="left"/></w:pPr>'
        f'<w:rPr><w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/></w:rPr>',
        mode=mode,
        compat_values=(
            (("overrideTableStyleFontSizeAndJustification", setting),) if mode == 15 else ()
        ),
    )


def table_style_not_allowed() -> Doc:
    # MS-OI29500 §17.7.6.4(a): Word does not allow w:tblW and w:tblLayout in a table
    # style's w:tblPr. A document of its own, so that a file Word refuses costs no other
    # case. The tables give no width of their own, over a grid of two 40 pt columns, 80 pt
    # in all, that each hold one word wider than 40 pt (77.4 pt in Carlito, 176 pt for the
    # table with the cell margins): an autofit column widens for it, a fixed one does not.
    # A width alone shows only if it is wider than the autofit table, so Width5000 gives
    # 250 pt; Width2000's 100 pt shows only together with its fixed layout.
    styles = "".join(
        [
            table_style("Width2000", [], width='<w:tblW w:w="2000" w:type="dxa"/>',
                        layout='<w:tblLayout w:type="fixed"/>'),
            table_style("NoWidth", []),
            table_style("Width5000", [], width='<w:tblW w:w="5000" w:type="dxa"/>'),
            table_style("FixedOnly", [], layout='<w:tblLayout w:type="fixed"/>'),
        ]
    )
    cells = [[para(run("Hamburgefontsiv")), para(run("Hamburgefontsiv"))]]
    narrow = [800, 800]
    body = table_case(
        1, "the style Width2000 gives w:tblW 100 pt and a fixed layout; a grid of two 40 pt "
        "columns, each holding one word wider than 40 pt",
        styled_table("Width2000", cells, grid=narrow),
    ) + table_case(
        2, "the style NoWidth, otherwise the same", styled_table("NoWidth", cells, grid=narrow)
    ) + table_case(
        3, "the style Width5000 gives only w:tblW 250 pt, otherwise the same",
        styled_table("Width5000", cells, grid=narrow),
    ) + table_case(
        4, "the style FixedOnly gives only a fixed layout, otherwise the same",
        styled_table("FixedOnly", cells, grid=narrow),
    )
    return Doc(
        "E-table-style-not-allowed",
        "Whether Word opens table styles whose w:tblPr holds w:tblW or w:tblLayout, which "
        "MS-OI29500 says it does not allow, and whether a table without its own width or "
        "layout takes them: both, the width alone (250 pt, wider than the table's autofit "
        "176 pt) and the fixed layout alone",
        body,
        styles=styles,
    )


def char_styled(value: str, style_id: str) -> str:
    """A paragraph whose one run is in the character style."""
    return para(f'<w:r><w:rPr><w:rStyle w:val="{style_id}"/></w:rPr>{text(value)}</w:r>')


# How Word combines bold, a toggle property, across styles (MS-OI29500 §17.7.3(a)): it starts
# from the document defaults, and each style whose value differs from them flips the result.
# E-style-toggles and E-style-toggles-defaults show it for paragraph and character styles.
# For a table style, the lowest style, two readings remain: it is one more such level (L),
# or its value is the base the styles above count from (B, a reading of MS-OI29500
# §17.7.6(b): "Word resets the value of the toggle property to the value specified by the
# table style if a value is present"). At the table style itself both give the same; they
# part in the styles above it. formatting.json records whether each cell's first character
# is bold.
def table_style_toggles() -> Doc:
    styles = "".join(
        [
            table_style("BoldTable", [], whole="<w:rPr><w:b/><w:bCs/></w:rPr>"),
            style("paragraph", "BoldPara", "<w:b/><w:bCs/>", based_on="Normal"),
            style("character", "CharBold", "<w:b/><w:bCs/>"),
        ]
    )
    # Defaults not bold: in BoldPara and in CharBold, L flips twice (not bold) and B keeps
    # the base (bold); in Normal both give bold. MS-OI29500 §17.7.8(a), a paragraph style
    # that resets the value, would also give bold in BoldPara, but not in CharBold.
    cells = [[
        para(run("in BoldPara"), p_style="BoldPara"),
        para(run("in Normal")),
        char_styled("in CharBold", "CharBold"),
    ]]
    body = table_case(
        1, "the table style BoldTable (bold); a paragraph in BoldPara (bold), one in Normal, "
        "and one in Normal whose run is in CharBold (bold)",
        styled_table("BoldTable", cells, widths=[TEXT_W // 3] * 3),
    )
    return Doc(
        "E-table-style-toggles",
        "How a bold table style combines with a bold paragraph style and a bold character "
        "style, with document defaults that are not bold: as one more toggle level, which "
        "gives not bold in both, or as the base they count from, which gives bold",
        body,
        styles=styles,
    )


def table_style_toggles_defaults() -> Doc:
    styles = "".join(
        [
            table_style("BoldOn", [], whole="<w:rPr><w:b/><w:bCs/></w:rPr>"),
            table_style("BoldOff", [], whole='<w:rPr><w:b w:val="0"/><w:bCs w:val="0"/></w:rPr>'),
            style("character", "CharBold", "<w:b/><w:bCs/>"),
            style("character", "CharBoldOff", '<w:b w:val="0"/><w:bCs w:val="0"/>'),
        ]
    )
    # Defaults bold. In Normal, BoldOff gives not bold under both readings, and bold if Word
    # ignored the table style or followed the standard's own rule, which MS-OI29500
    # §17.7.6(b) quotes: a table style's false leaves the value as it is, and its true
    # toggles the level before it. BoldOn gives bold under both readings and if ignored, and
    # not bold only under that rule, so the two cells together tell the four apart. Under
    # BoldOff, a run in CharBoldOff is bold under L (two flips) and not bold under B (same
    # as the base); a run in CharBold is not bold under L (one flip) and bold under B.
    off_cells = [[
        para(run("in Normal")),
        char_styled("in CharBoldOff", "CharBoldOff"),
        char_styled("in CharBold", "CharBold"),
    ]]
    body = table_case(
        1, "the table style BoldOn (w:b on); a paragraph in Normal",
        styled_table("BoldOn", [[para(run("in Normal"))]], widths=[TEXT_W // 3]),
    ) + table_case(
        2, "the table style BoldOff (w:b off); a paragraph in Normal, one whose run is in "
        "CharBoldOff (w:b off), and one whose run is in CharBold (w:b on)",
        styled_table("BoldOff", off_cells, widths=[TEXT_W // 3] * 3),
    )
    return Doc(
        "E-table-style-toggles-defaults",
        "With bold document defaults, whether a table style's w:b off applies and whether its "
        "w:b on toggles the defaults off, as the standard says; and how a character style's "
        "bold combines with a table style's w:b off: as one more toggle level, which gives "
        "bold for CharBoldOff and not bold for CharBold, or as the base they count from, "
        "which gives the reverse",
        body,
        styles=styles,
        run_defaults_extra="<w:b/><w:bCs/>",
    )


def table_style_header_rows(setting: int) -> Doc:
    # #172: whether Word gives the first-row format of a table style to every header row
    # (w:tblHeader), by the setting differentiateMultirowTableHeaders ([MS-DOCX] 2.3.4).
    # The style also bands rows by one. [MS-DOCX] 2.3.4 says the setting decides whether the
    # conditional formatting of table rows applies to multi-row headers; whether that
    # includes the bands is a reading, which the render shows.
    formats = [c for c in CONDITIONS if c[0] == "firstRow"] + ROW_BANDS
    header = table_style("HeaderRows", formats, row_band=1)
    body = table_case(
        1, "rows 1 and 2 are header rows (w:tblHeader); the style formats its first row and "
        "bands rows by one",
        styled_table("HeaderRows", named_cells(6, 3), look_attributes=look(1, 0, 0, 0, 1, 0),
                     widths=[TEXT_W // 3] * 3, header_rows=2),
    )
    said = {1: "on", 0: "off"}[setting]
    return Doc(
        f"E-table-style-header-rows-{said}",
        "Which header rows of a table take its style's first-row format, with the setting "
        f"differentiateMultirowTableHeaders {said}: two header rows, then four banded rows; "
        "each format gives a text colour and a grey",
        body,
        styles=header,
        compat_values=(("differentiateMultirowTableHeaders", setting),),
    )


# Table autofit, rows across pages and repeated header rows (#368, layout plan PR 13).
# Calibri 11 pt, A4, 72 pt margins. A cell is (paragraphs, w:tcPr content); a row is its
# cells, or (cells, w:trPr content).
def cell(content: str, tc_pr: str = "") -> tuple[str, str]:
    return content, tc_pr


def width(value: int | None, kind: str) -> str:
    """A w:tcW: kind dxa (twips), pct (fiftieths of a percent) or auto (value 0)."""
    return f'<w:tcW w:w="{value or 0}" w:type="{kind}"/>'


def rule_table(
    rows: list,
    grid: list[int],
    *,
    tbl_w: tuple[int, str] | None = (0, "auto"),
    fixed: bool = False,
    border_eighths: int = 4,
    borders: dict[str, int] | None = None,
) -> str:
    """A table with single borders border_eighths eighths of a point wide (4 for 0.5 pt), or
    as wide as borders gives for each side, the grid saved as given, w:tblW as (value, type)
    or none, and an autofit layout unless fixed. rows holds each row's cells, or (cells,
    w:trPr content)."""
    tbl_pr = ""
    if tbl_w is not None:
        tbl_pr += f'<w:tblW w:w="{tbl_w[0]}" w:type="{tbl_w[1]}"/>'
    sides = ("top", "left", "bottom", "right", "insideH", "insideV")
    eighths = borders or dict.fromkeys(sides, border_eighths)
    tbl_pr += "<w:tblBorders>" + "".join(
        f'<w:{s} w:val="single" w:sz="{eighths[s]}" w:space="0" w:color="000000"/>'
        for s in sides
    ) + "</w:tblBorders>"
    if fixed:
        tbl_pr += '<w:tblLayout w:type="fixed"/>'
    tbl_pr += (
        '<w:tblCellMar><w:left w:w="108" w:type="dxa"/><w:right w:w="108" w:type="dxa"/>'
        "</w:tblCellMar>"
    )
    grid_xml = "".join(f'<w:gridCol w:w="{w}"/>' for w in grid)
    trs = []
    for row in rows:
        cells, tr_pr = row if isinstance(row, tuple) else (row, "")
        tcs = "".join(
            f"<w:tc>{f'<w:tcPr>{tc_pr}</w:tcPr>' if tc_pr else ''}{content}</w:tc>"
            for content, tc_pr in cells
        )
        trs.append(f"<w:tr>{f'<w:trPr>{tr_pr}</w:trPr>' if tr_pr else ''}{tcs}</w:tr>")
    return (
        f"<w:tbl><w:tblPr>{tbl_pr}</w:tblPr><w:tblGrid>{grid_xml}</w:tblGrid>"
        f"{''.join(trs)}</w:tbl>"
    )


# Short texts of different lengths, so that autofit gives the columns different widths.
AUTOFIT_TEXTS = ("Ab", "A longer cell of several words", "Middle text")
# A word no line can break, for the unbreakable minimum of a column.
AUTOFIT_WORD = "Interdepartmentalisationsprocedurecharacteristically"
EVEN_GRID = [TEXT_W // 3] * 3


def autofit_cells(tc_prs: list[str]) -> list:
    """Two rows of AUTOFIT_TEXTS, each cell with the w:tcPr content of its column."""
    return [
        [cell(para(run(f"{t} r{r}")), tc) for t, tc in zip(AUTOFIT_TEXTS, tc_prs)]
        for r in (1, 2)
    ]


def autofit_widths_cases() -> list[tuple[str, str]]:
    """Cases 2 to 5 of #368, which compatibility mode 14 repeats."""
    return [
        ("w:tblW 100 % (pct 5000), cells auto",
         rule_table(autofit_cells([width(0, "auto")] * 3), EVEN_GRID, tbl_w=(5000, "pct"))),
        ("w:tblW auto, cells of 72 pt each (216 pt, less than the 451.3 pt text width)",
         rule_table(autofit_cells([width(1440, "dxa")] * 3), [1440] * 3)),
        ("w:tblW auto, cells of 200 pt each (600 pt, more than the text width)",
         rule_table(autofit_cells([width(4000, "dxa")] * 3), [4000] * 3)),
        ("w:tblW auto, cells of 40 % each (120 %)",
         rule_table(autofit_cells([width(2000, "pct")] * 3), EVEN_GRID)),
    ]


def autofit_setting_cases(first: int) -> list[tuple[int, str, str]]:
    """Cases the settings of mode 14 act on (ISO/IEC 29500 §17.15.3): w:growAutofit, minimum
    content widths that sum to more than the text width; w:doNotAutofitConstrainedTables, an
    autofit table beside a floating picture with square wrap."""
    auto = width(0, "auto")
    picture = vml_picture(
        "image1.png", "f1",
        "position:absolute;margin-left:0;margin-top:0;width:216pt;height:108pt;z-index:1;"
        "mso-position-horizontal-relative:margin;mso-position-vertical-relative:paragraph",
        "square", first=True)
    return [
        (first, f"three cells each holding {AUTOFIT_WORD}, together wider than the text width",
         rule_table([[cell(para(run(AUTOFIT_WORD)), auto)] * 3], EVEN_GRID)),
        (first + 1, "a floating picture of 216 x 108 pt at the left margin, square wrap, then "
                    "an autofit table (cells auto) beside it, whose content is wider than the "
                    "room beside the picture",
         para(picture + run("The picture is anchored here.")) + rule_table(
             autofit_cells([auto] * 3), EVEN_GRID)),
    ]


def autofit_doc(name: str, purpose: str, cases: list[tuple[str, str]], **options) -> Doc:
    """The labelled cases, then the end; the picture of autofit_setting_cases when a case
    holds it."""
    body = "".join(table_case(n, label, xml) for n, label, xml in cases)
    media = (("image1.png", grey_png(96, 48)),) if "<v:imagedata" in body else ()
    return Doc(name, purpose, body + para(run("End of the document.")), media=media,
               **options)


def table_autofit() -> Doc:
    auto = width(0, "auto")
    nested = rule_table(
        [[cell(para(run(f"in{r}{c}"))) for c in "ab"] for r in (1, 2)], [1500, 1500]
    )
    cases = [
        (1, "no widths: w:tblW auto, no w:tcW, an even grid saved by us that autofit replaces",
         rule_table(autofit_cells([""] * 3), EVEN_GRID)),
        *[(n, label, xml) for n, (label, xml) in zip(range(2, 6), autofit_widths_cases())],
        (6, f"one cell holds the single word {AUTOFIT_WORD}, in a column of 36 pt",
         rule_table([[cell(para(run(AUTOFIT_WORD)), width(720, "dxa")),
                      cell(para(run("Short")), auto), cell(para(run("Other")), auto)]],
                    [720, 4153, 4153])),
        (7, "a nested table of two auto columns in the first cell",
         rule_table([[cell(para(run("Outer a")) + nested + para(), auto),
                      cell(para(run("Outer b")), auto)]], [4513, 4513])),
        (8, "row 1: a cell spanning two columns (w:gridSpan 2) of 300 pt; row 2: cells of 50 "
            "and 100 pt",
         rule_table([[cell(para(run("Spanning cell")),
                           width(6000, "dxa") + '<w:gridSpan w:val="2"/>')],
                     [cell(para(run("Fifty")), width(1000, "dxa")),
                      cell(para(run("Hundred")), width(2000, "dxa"))]], [1000, 2000])),
        (10, f"for contrast, a fixed layout (w:tblLayout fixed) of 72 pt columns, the first "
             f"holding {AUTOFIT_WORD}",
         rule_table([[cell(para(run(AUTOFIT_WORD)), width(1440, "dxa")),
                      cell(para(run("Short")), width(1440, "dxa")),
                      cell(para(run("Other")), width(1440, "dxa"))]],
                    [1440] * 3, tbl_w=(4320, "dxa"), fixed=True)),
        *autofit_setting_cases(11),
    ]
    return autofit_doc(
        "E-table-autofit-15",
        "Column widths Word's autofit gives tables from their content and preferred widths "
        "(no widths, 100 %, sums of cell widths below and above the text width, percents "
        "over 100, an unbreakable word, a nested table, a spanning cell; a fixed layout for "
        "contrast; minimum widths wider than the text, a table beside a floating picture), in "
        "compatibility mode 15",
        cases,
    )


def table_autofit_14(flag: str | None) -> Doc:
    said = {None: "", "doNotAutofitConstrainedTables": "-constrained",
            "growAutofit": "-grow"}[flag]
    setting = f", with w:{flag}" if flag else ""
    cases = [(n, label, xml) for n, (label, xml) in zip(range(2, 6), autofit_widths_cases())]
    cases += autofit_setting_cases(11)
    return autofit_doc(
        f"E-table-autofit{said}-14",
        "Column widths Word's autofit gives the tables of cases 2 to 5, 11 and 12 of "
        f"E-table-autofit-15 in compatibility mode 14{setting}",
        cases,
        mode=14,
        compat_flags=(flag,) if flag else (),
    )


def numbered_lines(prefix: str, count: int) -> str:
    """count paragraphs reading <prefix> 01, <prefix> 02, ..., one line each."""
    return "".join(para(run(f"{prefix} {i:02d}")) for i in range(1, count + 1))


def page_case(name: str, purpose: str, label: str, content: str, filler: int = 0) -> Doc:
    """A document of one case of rows across pages: its label, filler lines so that it
    starts low on page 1, and the content, with 6 pt borders that show where Word draws."""
    body = (para(run(f"Case: {label}")) + numbered_lines("Filler", filler) + content
            + para(run("End of the document.")))
    return Doc(name, purpose, body)


def row_cells(texts: list[str]) -> list:
    return [cell(para(run(t))) for t in texts]


def split_row_table(tr_pr: str, lines_in_cell: int) -> str:
    """Three columns; rows 1 and 2 short, row 3 with a cell of lines_in_cell lines."""
    third = [cell(numbered_lines("Long", lines_in_cell)), cell(para(run("r3c2"))),
             cell(para(run("r3c3")))]
    return rule_table(
        [row_cells(["r1c1", "r1c2", "r1c3"]), row_cells(["r2c1", "r2c2", "r2c3"]),
         (third, tr_pr), row_cells(["r4c1", "r4c2", "r4c3"])],
        EVEN_GRID, tbl_w=(TEXT_W, "dxa"), fixed=True, border_eighths=48,
    )


def media_id(name: str) -> str:
    """The relationship id of a picture part: rIdimage1 for image1.png."""
    return "rId" + name.rsplit(".", 1)[0]


def grey_png(width_px: int, height_px: int, level: int = 160) -> bytes:
    """A PNG of one grey level, 8-bit greyscale, with no time or other chunk that would vary,
    its pixels in stored deflate blocks (stored_zlib)."""
    def chunk(kind: bytes, data: bytes) -> bytes:
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))

    rows = b"".join(b"\x00" + bytes([level]) * width_px for _ in range(height_px))
    header = struct.pack(">IIBBBBB", width_px, height_px, 8, 0, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"IDAT", stored_zlib(rows)) + chunk(b"IEND", b""))


def stored_zlib(data: bytes) -> bytes:
    """data as a zlib stream of stored (uncompressed) deflate blocks (RFC 1950 and 1951,
    §3.2.4): unlike zlib.compress, the same bytes whatever zlib the platform has."""
    blocks = [data[i:i + 65535] for i in range(0, len(data), 65535)] or [b""]
    out = b"\x78\x01"
    for n, block in enumerate(blocks):
        last = 1 if n == len(blocks) - 1 else 0
        out += bytes([last]) + struct.pack("<HH", len(block), len(block) ^ 0xFFFF) + block
    return out + struct.pack(">I", zlib.adler32(data) & 0xFFFFFFFF)


# The picture frame shape type (o:spt 75), as Word writes it into the first w:pict of a
# document that has VML pictures (the re-saved copies under tests/corpus/resaved/ show it).
PICTURE_FRAME = (
    '<v:shapetype id="_x0000_t75" coordsize="21600,21600" o:spt="75" o:preferrelative="t" '
    'path="m@4@5l@4@11@9@11@9@5xe" filled="f" stroked="f"><v:stroke joinstyle="miter"/>'
    '<v:formulas><v:f eqn="if lineDrawn pixelLineWidth 0"/><v:f eqn="sum @0 1 0"/>'
    '<v:f eqn="sum 0 0 @1"/><v:f eqn="prod @2 1 2"/><v:f eqn="prod @3 21600 pixelWidth"/>'
    '<v:f eqn="prod @3 21600 pixelHeight"/><v:f eqn="sum @0 0 1"/><v:f eqn="prod @6 1 2"/>'
    '<v:f eqn="prod @7 21600 pixelWidth"/><v:f eqn="sum @8 21600 0"/>'
    '<v:f eqn="prod @7 21600 pixelHeight"/><v:f eqn="sum @10 21600 0"/></v:formulas>'
    '<v:path o:extrusionok="f" gradientshapeok="t" o:connecttype="rect"/>'
    '<o:lock v:ext="edit" aspectratio="t"/></v:shapetype>'
)


def vml_picture(name: str, shape_id: str, style: str, wrap: str | None = None,
                first: bool = False) -> str:
    """A run holding a VML picture (w:pict, v:shape of the picture frame type with
    v:imagedata) of the given CSS style; wrap is the w10:wrap type of a floating one. The
    first picture of a document defines the shape type (first)."""
    wrap_xml = f'<w10:wrap type="{wrap}"/>' if wrap else ""
    shape_type = PICTURE_FRAME if first else ""
    return (f'<w:r><w:pict>{shape_type}<v:shape id="{shape_id}" type="#_x0000_t75" '
            f'style="{style}"><v:imagedata r:id="{media_id(name)}" o:title=""/>{wrap_xml}'
            "</v:shape></w:pict></w:r>")


def vml_pictures() -> Doc:
    # #368, the maintainer's addition: pictures in VML (w:pict), as documents saved by
    # Word 2003 and earlier, and many converted ones, still hold them; inline and floating.
    words = " ".join(["Text around the picture"] * 30)
    inline = "width:72pt;height:36pt"
    tall = "width:36pt;height:48pt"
    floating = ("position:absolute;margin-left:{x}pt;margin-top:{y}pt;width:72pt;"
                "height:72pt;z-index:{z};mso-position-horizontal-relative:{h};"
                "mso-position-vertical-relative:{v}")
    cases = [
        ("an inline picture of 72 x 36 pt between words",
         para(run("Before ") + vml_picture("image1.png", "p1", inline, first=True)
              + run(" after."))),
        ("an inline picture 48 pt tall in a line of 11 pt text: the line grows",
         para(run("Before ") + vml_picture("image2.png", "p2", tall) + run(" after."))),
        ("a floating picture of 72 x 72 pt, 144 pt from the left margin and 6 pt below the "
         "paragraph's top, text wrapped square around it",
         para(vml_picture("image3.png", "p3", floating.format(
             x=144, y=6, z=1, h="margin", v="paragraph"), "square") + run(words))),
        ("a floating picture behind the text (a negative z-index), no wrap",
         para(vml_picture("image3.png", "p4", floating.format(
             x=36, y=0, z=-251658240, h="margin", v="paragraph"), "none") + run(words))),
        ("on a page of its own, a floating picture placed from the page's top left corner, "
         "300 pt across and 150 pt down, text wrapped square",
         para(vml_picture("image3.png", "p5", floating.format(
             x=300, y=150, z=2, h="page", v="page"), "square") + run(words + " " + words))),
    ]
    body = "".join(para(run(f"Case {n:02d}: {label}"), space=spacing(before=12),
                        page_break_before=n == 5) + content
                   for n, (label, content) in enumerate(cases, 1))
    return Doc(
        "E-vml-pictures",
        "Where Word draws VML pictures (w:pict, the picture frame shape type): inline in a "
        "line, taller than the line, floating with square wrap, behind the text and placed "
        "from the page; each picture is a grey PNG of its own size",
        body + para(run("End of the document.")),
        media=(("image1.png", grey_png(96, 48)), ("image2.png", grey_png(48, 64)),
               ("image3.png", grey_png(96, 96))),
    )


def exact_height_overflow() -> Doc:
    # #368, the maintainer's addition: rows of exact height (w:hRule exact) whose content is
    # taller than the row: what Word shows of it, and where the next row starts.
    def exact_table(height_twips: int, content: str, tc_pr: str = "") -> str:
        return rule_table(
            [row_cells(["r1c1", "r1c2", "r1c3"]),
             ([cell(content, tc_pr), cell(para(run("r2c2"))), cell(para(run("r2c3")))],
              f'<w:trHeight w:val="{height_twips}" w:hRule="exact"/>'),
             row_cells(["r3c1", "r3c2", "r3c3"])],
            EVEN_GRID, tbl_w=(TEXT_W, "dxa"), fixed=True, border_eighths=12,
        )

    big = para(run("Large text", size=28))
    cases = [
        ("row 2 is exactly 30 pt high and holds 5 lines",
         exact_table(600, numbered_lines("Cut", 5))),
        ("row 2 is exactly 8 pt high and holds one line of 11 pt text",
         exact_table(160, para(run("Small row")))),
        ("row 2 is exactly 20 pt high and holds 28 pt text", exact_table(400, big)),
        ("the same, the cell centred vertically (w:vAlign center)",
         exact_table(400, big, '<w:vAlign w:val="center"/>')),
        ("the same, the cell at the bottom (w:vAlign bottom)",
         exact_table(400, big, '<w:vAlign w:val="bottom"/>')),
        ("row 2 is exactly 20 pt high and holds an inline picture 60 pt tall",
         exact_table(400, para(vml_picture("image1.png", "p1", "width:30pt;height:60pt",
                                           first=True)))),
    ]
    body = "".join(table_case(n, label, xml) for n, (label, xml) in enumerate(cases, 1))
    return Doc(
        "E-table-row-exact-height",
        "What Word shows of rows of exact height (w:trHeight, w:hRule exact) whose content is "
        "taller than the row, and where the next row starts: lines, a row lower than one line, "
        "large text at each vertical alignment, and a picture",
        body + para(run("End of the document.")),
        media=(("image1.png", grey_png(40, 80)),),
    )


def rows_across_pages() -> list[Doc]:
    header = "<w:tblHeader/>"
    header_rows = [(row_cells([f"Head {r}a", f"Head {r}b", f"Head {r}c"]), header)
                   for r in (1, 2)]
    body_rows = [row_cells([f"Body {r:02d}a", f"Body {r:02d}b", f"Body {r:02d}c"])
                 for r in range(1, 61)]
    tall_header = [([cell(numbered_lines(f"Head {r}", 35)), cell(para(run(f"Head {r}b"))),
                     cell(para(run(f"Head {r}c")))], header) for r in (1, 2)]
    inner = rule_table([row_cells([f"Inner {r:02d}a", f"Inner {r:02d}b"]) for r in range(1, 31)],
                       [2000, 2000], tbl_w=(4000, "dxa"), fixed=True, border_eighths=48)
    nested = rule_table(
        [[cell(para(run("Outer cell")) + inner + para()), cell(para(run("Outer b")))]],
        [5000, TEXT_W - 5000], tbl_w=(TEXT_W, "dxa"), fixed=True, border_eighths=48,
    )
    return [
        page_case("E-table-row-split",
                  "Where Word splits a table row across pages, and which 6 pt borders it draws "
                  "at the bottom of page 1 and the top of page 2: the third row holds a cell "
                  "of 40 lines and starts low on page 1",
                  "row 3 holds a cell of 40 lines and starts low on page 1",
                  split_row_table("", 40), filler=30),
        page_case("E-table-row-cant-split",
                  "Where Word puts a table row that may not split (w:cantSplit), starting low "
                  "on page 1 with a cell of 40 lines",
                  "row 3 has w:cantSplit, holds a cell of 40 lines and starts low on page 1",
                  split_row_table("<w:cantSplit/>", 40), filler=30),
        page_case("E-table-row-cant-split-tall",
                  "What Word does with a table row that may not split (w:cantSplit) and is "
                  "taller than a page: a cell of 80 lines",
                  "row 3 has w:cantSplit and holds a cell of 80 lines, more than a page",
                  split_row_table("<w:cantSplit/>", 80)),
        page_case("E-table-header-repeat",
                  "Two header rows (w:tblHeader) repeated on every page a table of 60 body "
                  "rows crosses",
                  "rows 1 and 2 are header rows; 60 body rows follow",
                  rule_table(header_rows + body_rows, EVEN_GRID, tbl_w=(TEXT_W, "dxa"),
                             fixed=True, border_eighths=48)),
        page_case("E-table-header-repeat-tall",
                  "Whether Word repeats header rows (w:tblHeader) that alone take more than a "
                  "page: two header rows of 35 lines each, then 60 body rows, which cross at "
                  "least one more page",
                  "rows 1 and 2 are header rows of 35 lines each; 60 body rows follow",
                  rule_table(tall_header + body_rows, EVEN_GRID, tbl_w=(TEXT_W, "dxa"),
                             fixed=True, border_eighths=48)),
        page_case("E-table-row-split-nested",
                  "Where Word splits a row whose cell holds a nested table of 30 rows, "
                  "starting low on page 1",
                  "the outer row's first cell holds a nested table of 30 rows; it starts low "
                  "on page 1",
                  nested, filler=30),
    ]


# Lines and drawing (#186, layout-engine.md E6 and E27 to E33). Calibri 11 pt unless a case
# says otherwise; each case labels itself, and the sample it is about follows in a run of
# its own, so the word boxes of geometry.json show the sample alone.
def styled_run(value: str, props: str) -> str:
    """A run of text with these run properties (the children of w:rPr, in schema order)."""
    return f"<w:r>{f'<w:rPr>{props}</w:rPr>' if props else ''}{text(value)}</w:r>"


def soft_hyphenated(word: str, props: str) -> str:
    """A run of the word with a soft hyphen (w:softHyphen) at every hyphen in it."""
    pieces = word.split("-")
    inner = "<w:softHyphen/>".join(text(piece) for piece in pieces)
    return f"<w:r>{f'<w:rPr>{props}</w:rPr>' if props else ''}{inner}</w:r>"


def tab_run(props: str = "") -> str:
    return f"<w:r>{f'<w:rPr>{props}</w:rPr>' if props else ''}<w:tab/></w:r>"


def labelled(n: int, label: str, *samples: str, **props) -> str:
    """A paragraph: the case's label in a run of its own, then the samples."""
    return para(run(f"Case {n:02d}: {label}: ") + "".join(samples), **props)


# A word long enough to break at one of its soft hyphens in a narrow paragraph.
LONG_WORD = "ex-tra-or-di-nar-i-ly"
# One word of 150 letters with a soft hyphen after every syllable: wider than NARROW's 150
# pt even condensed, so its lines can only break at its soft hyphens.
LONGER_WORD = "-".join([LONG_WORD] * 10)
# A right indent that leaves 150 pt for the text.
NARROW = {"right": TEXT_W - 3000}


def spacing_condensed() -> Doc:
    cases = []
    for points in (3, 6):
        props = f'<w:spacing w:val="{-twips(points)}"/>'
        said = f"condensed by {points} pt"
        # At 11 pt, most letters are narrower than 6 pt, and condensed by 6 pt the word
        # would take no room; at 24 pt it still fills three lines.
        long_props, size = props, ""
        if points == 6:
            long_props, size = props + '<w:sz w:val="48"/><w:szCs w:val="48"/>', ", 24 pt text"
        cases += [
            (f"{said}{size}, a narrow paragraph of one long word with soft hyphens",
             [soft_hyphenated(LONGER_WORD, long_props)], {"ind": NARROW}),
            (f"{said}, characters after spaces", [styled_run("a b c d e f", props)], {}),
            (f"{said}, zero-width spaces (U+200B) between letters",
             [styled_run("ab\u200bcd\u200bef", props)], {}),
            (f"{said}, after a right tab", [tab_run(), styled_run("at the end", props)],
             {"tabs": [("right", TEXT_W)]}),
            (f"{said}, narrow letters", [styled_run("iiiii lllll", props)], {}),
        ]
    body = "".join(
        labelled(n, label, *samples, **props) for n, (label, samples, props) in enumerate(cases, 1)
    )
    return Doc(
        "E-spacing-condensed",
        "Letter spacing (w:spacing) of -3 and -6 pt: on text that breaks at a soft hyphen, "
        "after a space, on zero-width spaces, after a right tab, and on letters narrower than "
        "the spacing, which would go below zero width",
        body,
    )


def tab_margin() -> Doc:
    # The text runs from 0 to TEXT_W; a stop at TEXT_W + 360 is in the right margin, one at
    # PAGE_W - MARGIN + 360 is past the edge of the page.
    in_margin, past_edge = TEXT_W + 360, PAGE_W - MARGIN + 360
    cases = [
        ("a left stop 18 pt into the right margin", [("left", in_margin)]),
        ("a left stop 18 pt past the right edge of the page", [("left", past_edge)]),
        ("a right stop 18 pt into the right margin", [("right", in_margin)]),
    ]
    body = "".join(
        labelled(n, label, styled_run("before", ""), tab_run(), styled_run("after", ""),
                 tabs=stops)
        for n, (label, stops) in enumerate(cases, 1)
    )
    # A tab after "(", where no line may break, near the end of the line: one of these
    # lengths puts it past the last default stop the line has room for.
    for n, count in enumerate((86, 88, 90, 92, 94), len(cases) + 1):
        body += para(
            run(f"Case {n:02d}: {count} x, then an opening bracket and two tabs:"),
        ) + para(styled_run("x" * count + "(", "") + tab_run() + tab_run() + styled_run("end", ""))
    return Doc(
        "E-tab-margin",
        "Tab stops in the right margin and past the edge of the page, and two tabs after an "
        "opening bracket near the end of a line, where the line may not break",
        body,
    )


HIGHLIGHT = '<w:highlight w:val="darkYellow"/>'
RUN_SHADING = '<w:shd w:val="clear" w:color="auto" w:fill="D9D9D9"/>'


def highlight() -> Doc:
    base = styled_run("base", "")
    superscript = HIGHLIGHT + '<w:vertAlign w:val="superscript"/>'
    subscript = HIGHLIGHT + '<w:vertAlign w:val="subscript"/>'
    raised = '<w:position w:val="12"/>' + HIGHLIGHT
    cases = [
        ("highlighted", [styled_run("highlighted text", HIGHLIGHT)], {}),
        ("highlighted superscript", [base, styled_run("raised", superscript)], {}),
        ("highlighted subscript", [base, styled_run("lowered", subscript)], {}),
        ("highlighted text raised 6 pt (w:position 12)", [base, styled_run("raised", raised)],
         {}),
        ("highlighted under exact 9 pt line spacing", [styled_run("highlighted text", HIGHLIGHT)],
         {"space": spacing(line=("exact", 9))}),
        ("highlighted words of a justified paragraph, three spaces between them",
         [styled_run("   ".join(["highlighted"] * 18), HIGHLIGHT)], {"jc": "both"}),
        ("highlight and run shading on one run", [styled_run("both", HIGHLIGHT + RUN_SHADING)], {}),
        ("a highlighted tab", [styled_run("before", HIGHLIGHT), tab_run(HIGHLIGHT),
                               styled_run("after", HIGHLIGHT)], {}),
    ]
    body = "".join(
        labelled(n, label, *samples, **props) for n, (label, samples, props) in enumerate(cases, 1)
    )
    return Doc(
        "E-highlight",
        "The box of highlighted text (dark yellow): plain, superscript, subscript and raised "
        "text, under exact line spacing smaller than the font, at the hanging spaces of a "
        "justified line, over run shading, and on a tab",
        body,
    )


# Every value of ST_Underline but none.
UNDERLINES = [
    "single", "words", "double", "thick", "dotted", "dottedHeavy", "dash", "dashedHeavy",
    "dashLong", "dashLongHeavy", "dotDash", "dashDotHeavy", "dotDotDash", "dashDotDotHeavy",
    "wave", "wavyHeavy", "wavyDouble",
]


def underline_props(kind: str) -> str:
    return f'<w:u w:val="{kind}"/>'


def trailing_space_cases(first: int) -> list[str]:
    single = underline_props("single")
    return [
        labelled(first, "underlined words ending a line in three spaces, left aligned",
                 styled_run("underlined   " * 14, single)),
        labelled(first + 1, "underlined words ending a line in three spaces, justified",
                 styled_run("underlined   " * 14, single), jc="both"),
    ]


def underline() -> Doc:
    body = "".join(
        labelled(
            n, f"underline {kind}", styled_run("underlined sample text", underline_props(kind))
        )
        for n, kind in enumerate(UNDERLINES, 1)
    )
    n = len(UNDERLINES)
    single = underline_props("single")
    body += labelled(n + 1, "single under small capitals",
                     styled_run("Small Capitals", "<w:smallCaps/>" + single))
    body += labelled(n + 2, "single under a superscript in a word",
                     styled_run("1", single),
                     styled_run("st", single + '<w:vertAlign w:val="superscript"/>'),
                     styled_run(" place", single))
    body += labelled(n + 3, "single under a tab", styled_run("before", single), tab_run(single),
                     styled_run("after", single))
    body += labelled(n + 4, "words only, in a narrow paragraph that breaks at soft hyphens",
                     soft_hyphenated(" ".join([LONG_WORD] * 6), underline_props("words")),
                     ind=NARROW)
    body += "".join(trailing_space_cases(n + 5))
    return Doc(
        "E-underline",
        "Every kind of underline (w:u), and single underline under small capitals, under a "
        "superscript in a word and under a tab; words-only underline at soft hyphen breaks; "
        "underlined spaces at the end of a line without w:ulTrailSpace",
        body,
    )


def underline_trail_space() -> Doc:
    return Doc(
        "E-underline-trailspace",
        "Underlined spaces at the end of a line with the w:compat setting ulTrailSpace, the "
        "same cases as the last two of E-underline",
        "".join(trailing_space_cases(1)),
        compat_flags=("ulTrailSpace",),
    )


def strike() -> Doc:
    cases = []
    for size in (11, 24):
        for kind in ("strike", "dstrike"):
            said = "single" if kind == "strike" else "double"
            hp = half_points(size)
            cases.append((f"{said} strikethrough, {size} pt",
                          f'<w:{kind}/><w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/>'))
    body = "".join(
        labelled(n, label, styled_run("struck through", props))
        for n, (label, props) in enumerate(cases, 1)
    )
    return Doc(
        "E-strike",
        "Single and double strikethrough at 11 and 24 pt: where each line is drawn and how "
        "thick",
        body,
    )


LEADERS = ["dot", "hyphen", "middleDot", "underscore", "heavy"]
LEADER_STOPS = [("left", 4320, "x"), ("right", 4320, "x"), ("center", 4320, "x"),
                ("decimal", 4320, "12.34")]


def tab_leader() -> Doc:
    # Each label in a paragraph of its own: the sample starts with its tab at the left
    # margin, so the leader runs from there to the stop at 216 pt.
    body, n = "", 0
    for leader in LEADERS:
        for kind, pos, after in LEADER_STOPS:
            n += 1
            body += para(run(f"Case {n:02d}: {leader} leader to a {kind} stop at 216 pt:"))
            body += para(tab_run() + styled_run(after, ""), tabs=[(kind, pos, leader)])
    extra = [
        ("dot leader at 1 pt", '<w:sz w:val="2"/><w:szCs w:val="2"/>'),
        ("dot leader at a character scale of 50 %", '<w:w w:val="50"/>'),
        ("dot leader under an underlined tab", underline_props("single")),
    ]
    for label, props in extra:
        n += 1
        body += para(run(f"Case {n:02d}: {label}:"))
        body += para(tab_run(props) + styled_run("x", ""), tabs=[("left", 4320, "dot")])
    return Doc(
        "E-tab-leader",
        "Tab leaders (dot, hyphen, middle dot, underscore, heavy) to left, right, centre and "
        "decimal stops, and a dot leader at 1 pt, at a character scale of 50 % and under an "
        "underlined tab: how many glyphs, where the last ends, where line leaders lie",
        body,
    )


def bar_tab() -> Doc:
    two_lines = "two lines of text in one paragraph with a bar tab at 144 pt, " * 2
    cases = [
        ("single spacing", {}),
        ("1.5 lines spacing", {"space": spacing(line=("auto", 360))}),
        ("exact 18 pt spacing", {"space": spacing(line=("exact", 18))}),
        ("12 pt before and after", {"space": spacing(before=12, after=12)}),
        ("dark paragraph shading", {"shading": "404040"}),
    ]
    body = "".join(
        labelled(n, label, styled_run(two_lines, ""), tabs=[("bar", 2880)], **props)
        for n, (label, props) in enumerate(cases, 1)
    )
    # Two paragraphs in a row with the same bar tab: one line, or two?
    n = len(cases) + 1
    body += labelled(n, "the first of two paragraphs with a bar tab", tabs=[("bar", 2880)])
    body += labelled(n + 1, "the second of two paragraphs with a bar tab", tabs=[("bar", 2880)])
    return Doc(
        "E-bar-tab",
        "Bar tabs (w:tab w:val=bar) through paragraphs with single, 1.5 and exact spacing, "
        "with space before and after, on dark paragraph shading, and in two paragraphs in a "
        "row: their width, extent and colour",
        body,
    )


# Greys of 10 % to 90 % black, and a few saturated colours.
AUTO_COLOR_FILLS = [(f"{k * 10} % grey", f"{round(255 * (1 - k / 10)):02X}" * 3)
                    for k in range(1, 10)] + [
    ("red", "FF0000"), ("green", "00B050"), ("blue", "0000FF"), ("yellow", "FFFF00"),
    ("dark blue", "002060"),
]


def auto_color() -> Doc:
    body, n = "", 0
    for name, fill in AUTO_COLOR_FILLS:
        n += 1
        shade = f'<w:shd w:val="clear" w:color="auto" w:fill="{fill}"/>'
        body += labelled(n, f"run shading {name} ({fill})", styled_run("automatic colour", shade))
        n += 1
        body += labelled(n, f"paragraph shading {name} ({fill})",
                         styled_run("automatic colour", ""), shading=fill)
    return Doc(
        "E-auto-color",
        "Text in the automatic colour on run shading and on paragraph shading of greys from "
        "10 % to 90 % and of a few saturated colours: where Word turns it white",
        body,
    )


def superscript() -> Doc:
    cases = []
    for size in (11, 24):
        hp = half_points(size)
        size_props = f'<w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/>'
        for kind in ("superscript", "subscript"):
            cases.append((f"{kind} at {size} pt", size_props, kind))
    body = "".join(
        labelled(n, label, styled_run("Base", props),
                 styled_run("123", props + f'<w:vertAlign w:val="{kind}"/>'))
        for n, (label, props, kind) in enumerate(cases, 1)
    )
    return Doc(
        "E-superscript",
        "The size and the baseline shift of superscript and subscript text at 11 and 24 pt",
        body,
    )


# Pages (#186, layout-engine.md E10, E11, E13, E21, E26, E34). Every paragraph has exact
# 12 pt line spacing, so that a page holds 58 lines (697.9 pt of text height) and a case
# can put a paragraph at a known line of the page.
EXACT_12 = spacing(line=("exact", 12))
LINES_PER_PAGE = 58


def page_para(content: str = "", **props) -> str:
    return para(content, space=props.pop("space", EXACT_12), **props)


def filler(count: int, label: str, **props) -> str:
    """One paragraph of count lines: the label, then lines numbered from 2."""
    values = [label] + [f"filler line {n}" for n in range(2, count + 1)]
    return page_para(lines(values), **props)


def one_liners(prefix: str, count: int, **props) -> str:
    """count paragraphs of one line each, numbered."""
    return "".join(page_para(run(f"{prefix} {n}"), **props) for n in range(1, count + 1))


def page_footer(name: str = "footer1.xml") -> tuple[str, str]:
    return header_part(name, page_para(run("page ") + page_field()))


def section_odd() -> Doc:
    refs = (("headerReference", "default", "header1.xml"),
            ("footerReference", "default", "footer1.xml"))
    sections = [
        # (label, start type, first page number, page count)
        ("Section 1: one page", None, None, 1),
        ("Section 2: odd page start after a section that ends on an odd page; two pages",
         "oddPage", None, 2),
        ("Section 3: odd page start after a section that ends on an even page",
         "oddPage", None, 1),
        ("Section 4: even page start after a section that ends on an odd page",
         "evenPage", None, 1),
        ("Section 5: even page start after a section that ends on an even page; two pages",
         "evenPage", None, 2),
        # Section 5 ends on an odd page, so Section 6 begins on an even physical page whose
        # restarted number is 1: Section 6 and 7 tell parity by number from parity by
        # physical page.
        ("Section 6: odd page start after a section that ends on an odd page, page numbers "
         "restarted at 1", "oddPage", 1, 1),
        ("Section 7: even page start, page numbers restarted at 1", "evenPage", 1, 1),
    ]
    # Each section's properties are in the w:sectPr of its last paragraph, the last
    # section's at the end of the body; the start type of a section is in its own.
    body, properties = "", [
        section(kind=kind, page_start=start, refs=refs if n == 0 else ())
        for n, (_, kind, start, _) in enumerate(sections)
    ]
    for n, (label, _, _, pages) in enumerate(sections):
        texts = [label] + [f"{label.split(':')[0]}, page {page}" for page in range(2, pages + 1)]
        for i, value in enumerate(texts):
            last = i == len(texts) - 1 and n < len(sections) - 1
            body += page_para(run(value), page_break_before=i > 0,
                              sect=properties[n] if last else None)
    return Doc(
        "E-section-odd",
        "Sections that start on an odd or an even page, after a section that ends on an odd "
        "and on an even page, with page numbers restarted and not: the blank pages Word adds, "
        "whether they count and whether they have the header (header) and footer (page "
        "number)",
        body,
        headers=(header_part("header1.xml", page_para(run("header"))), page_footer()),
        last_section=properties[-1],
    )


def section_odd_first() -> Doc:
    refs = (("headerReference", "default", "header1.xml"),
            ("footerReference", "default", "footer1.xml"))
    body = page_para(run("Section 1: the first section of the document, even page start"),
                     sect=section(kind="evenPage", refs=refs))
    body += page_para(run("Section 2: next page start"))
    return Doc(
        "E-section-odd-first",
        "A first section that starts on an even page: whether Word adds a blank page before "
        "the document",
        body,
        headers=(header_part("header1.xml", page_para(run("header"))), page_footer()),
        last_section=section(),
    )


def section_continuous_headers() -> Doc:
    first = (("headerReference", "default", "header1.xml"),)
    second = (("headerReference", "default", "header2.xml"),)
    body = filler(29, "Section 1: 29 lines, with the header Header A", sect=section(refs=first))
    body += filler(60, "Section 2: a continuous section of 60 lines with the header Header B")
    return Doc(
        "E-section-continuous-headers",
        "A continuous section with a header of its own that starts in the middle of page 1 "
        "and runs onto page 2: from which page its header applies",
        body,
        headers=(header_part("header1.xml", page_para(run("Header A"))),
                 header_part("header2.xml", page_para(run("Header B")))),
        last_section=section(kind="continuous", refs=second),
    )


def section_next_column() -> Doc:
    body = page_para(run("Section 1: one column"), sect=section())
    body += page_para(run("Section 2: a next column start in a section of one column"))
    return Doc(
        "E-section-next-column",
        "A section that starts in the next column (w:type nextColumn) after a section of one "
        "column: whether it starts a new page",
        body,
        last_section=section(kind="nextColumn"),
    )


def keeplines_long() -> Doc:
    body = filler(29, "29 lines before the paragraph")
    body += filler(80, "a keep-lines paragraph of 80 lines, longer than a page", keep_lines=True)
    return Doc(
        "E-keeplines-long",
        "A paragraph with keep lines together (w:keepLines) longer than a page, starting in "
        "the middle of a page: whether it moves to the next page before it is split",
        body,
    )


def keepnext_chain() -> Doc:
    body = filler(55, "Case 01: 55 lines, then a keep-with-next chain of 4 that fits a page")
    body += one_liners("Case 01 chain", 3, keep_next=True) + page_para(run("Case 01 chain 4"))
    body += filler(30, "Case 02: 30 lines, then a keep-with-next chain of 70 lines",
                   page_break_before=True)
    body += one_liners("Case 02 chain", 69, keep_next=True) + page_para(run("Case 02 chain 70"))
    body += filler(54, "Case 03: 54 lines, then a chain of 3 that ends at a next page section "
                   "break", page_break_before=True)
    # A section's start type (w:type) is in its own w:sectPr, at its last paragraph, and
    # says how it begins after the section before (ECMA-376 §17.6.22): the type of the break
    # after Case 03's chain is in the w:sectPr after Case 04's chain, and that of the break
    # after Case 04's chain in the last section's.
    body += one_liners("Case 03 chain", 2, keep_next=True)
    body += page_para(run("Case 03 chain 3"), keep_next=True, sect=section())
    body += page_para(run("Case 03: the first paragraph of the next section"))
    body += filler(55, "Case 04: 55 lines, then a chain of 2 that ends at a continuous section "
                   "break, then two lines", page_break_before=True)
    body += page_para(run("Case 04 chain 1"), keep_next=True)
    body += page_para(run("Case 04 chain 2"), keep_next=True, sect=section(kind="nextPage"))
    body += page_para(lines(["Case 04: two lines after the", "continuous section break"]))
    body += filler(54, "Case 05: 54 lines, then a chain of 3 before a paragraph with page break "
                   "before", page_break_before=True)
    body += one_liners("Case 05 chain", 3, keep_next=True)
    body += page_para(run("Case 05: page break before"), page_break_before=True)
    return Doc(
        "E-keepnext-chain",
        "Keep-with-next chains near the end of a page: one that fits a page, one longer than "
        "a page, one that ends at a next page and one at a continuous section break, one "
        "before a paragraph with page break before",
        body,
        last_section=section(kind="continuous"),
    )


def page_break_before_top() -> Doc:
    body = page_para(run("Case 01: page break before, on the first paragraph of the document"),
                     page_break_before=True)
    body += filler(LINES_PER_PAGE - 1, "57 lines, which fill the rest of the page")
    body += page_para(run("Case 02: page break before, on a paragraph that starts a page by "
                          "flowing"), page_break_before=True)
    return Doc(
        "E-page-break-before-top",
        "Page break before (w:pageBreakBefore) on the first paragraph of the document, and on "
        "a paragraph that starts a page anyway: whether Word adds an empty page",
        body,
    )


SHADE = "D9D9D9"


def para_shading() -> Doc:
    text = "shaded text that runs over several lines of one paragraph, " * 9
    cases = [
        ("a hanging first line (left 72 pt, hanging 36 pt)",
         {"ind": {"left": 1440, "hanging": 720}}),
        ("negative indents (left and right -18 pt)", {"ind": {"left": -360, "right": -360}}),
        ("12 pt before and after", {"space": spacing(before=12, after=12, line=("exact", 12))}),
        ("the first of two shaded paragraphs in a row", {}),
        ("the second of two shaded paragraphs in a row", {}),
    ]
    # An unshaded paragraph after each of the first three cases keeps them apart; Cases 04
    # and 05 touch.
    body = "".join(
        page_para(run(f"Case {n:02d}: {label}: ") + run(text), shading=SHADE, **props)
        + (page_para(run("not shaded")) if n <= 3 else "")
        for n, (label, props) in enumerate(cases, 1)
    )
    # Ten lines that start on line 54 of a page, after 53: the paragraph splits 5 and 5.
    body += filler(LINES_PER_PAGE - 5, "lines before the next case", page_break_before=True)
    body += filler(10, "Case 06: a shaded paragraph of 10 lines that starts on line 54",
                   shading=SHADE)
    return Doc(
        "E-para-shading",
        "The extent of paragraph shading: with a hanging first line, with negative indents, "
        "with space before and after, in two shaded paragraphs in a row, and in a paragraph "
        "split over two pages",
        body,
    )


def tall(kind: str, count: int) -> str:
    return page_para(lines([f"{kind} line {n}" for n in range(1, count + 1)]))


HEADER_REFS = (("headerReference", "default", "header1.xml"),
               ("footerReference", "default", "footer1.xml"))


def header_tall() -> Doc:
    return Doc(
        "E-header-tall",
        "A header of 8 lines, taller than the top margin, and a footer of 8 lines, taller "
        "than the bottom margin: where the body starts and ends",
        filler(70, "the body: 70 lines"),
        headers=(header_part("header1.xml", tall("header", 8)),
                 header_part("footer1.xml", tall("footer", 8))),
        last_section=section(refs=HEADER_REFS),
    )


def header_space_after() -> Doc:
    # 35.4 pt from the edge, two lines of 12 pt: 59.4 pt, inside the 72 pt margin; 83.4 pt
    # with the 24 pt after, past it.
    header = page_para(run("header line 1")) + page_para(
        run("header line 2, with 24 pt after"), space=spacing(after=24, line=("exact", 12))
    )
    return Doc(
        "E-header-space-after",
        "A header of two lines whose last paragraph has 24 pt after, taller with it than the "
        "top margin: whether the space after counts toward the header's height",
        filler(70, "the body: 70 lines"),
        headers=(header_part("header1.xml", header),),
        last_section=section(refs=(HEADER_REFS[0],)),
    )


def header_negative_margin() -> Doc:
    return Doc(
        "E-header-negative-margin",
        "Negative top and bottom margins (-72 pt: the body edge stays at 72 pt whatever the "
        "header), with a header and a footer of 8 lines each",
        filler(70, "the body: 70 lines"),
        headers=(header_part("header1.xml", tall("header", 8)),
                 header_part("footer1.xml", tall("footer", 8))),
        last_section=section(refs=HEADER_REFS, margins=(-MARGIN, -MARGIN)),
    )


# Pages, second batch (#298): the questions the references of the first left open, and the
# cases of #186's comments. Exact 12 pt lines, as in the first batch.


def field(instr: str, result: str = "1", props: str = "") -> str:
    """A simple field (w:fldSimple) whose placeholder result Word replaces when it lays the
    page out; props are the run properties of that result."""
    r_pr = f"<w:rPr>{props}</w:rPr>" if props else ""
    return f'<w:fldSimple w:instr=" {attr(instr)} "><w:r>{r_pr}{text(result)}</w:r></w:fldSimple>'


def complex_field(instr: str, result: str, props: str) -> str:
    """A field of w:fldChar and w:instrText runs, every one of them with these run
    properties, so that the field's code has them too (a language, say)."""
    r_pr = f"<w:rPr>{props}</w:rPr>"
    return (
        f'<w:r>{r_pr}<w:fldChar w:fldCharType="begin"/></w:r>'
        f'<w:r>{r_pr}<w:instrText xml:space="preserve"> {escape(instr)} </w:instrText></w:r>'
        f'<w:r>{r_pr}<w:fldChar w:fldCharType="separate"/></w:r>'
        f"<w:r>{r_pr}{text(result)}</w:r>"
        f'<w:r>{r_pr}<w:fldChar w:fldCharType="end"/></w:r>'
    )


def counting_footer(name: str = "footer1.xml", label: str = "page") -> tuple[str, str]:
    """A footer that says the page's number, the pages of the document and of the section."""
    return header_part(name, page_para(
        run(f"{label} ") + field("PAGE") + run(" of ") + field("NUMPAGES") + run(", ")
        + field("SECTIONPAGES") + run(" in this section")
    ))


def sectioned(sections: list[tuple[list[str], str]]) -> str:
    """Sections of one-line paragraphs, given as (the paragraphs' contents, w:sectPr):
    each section's properties are in its last paragraph, but the last section's, which
    are the document's (Doc.last_section)."""
    body = ""
    for n, (contents, properties) in enumerate(sections):
        last = n == len(sections) - 1
        body += "".join(page_para(c) for c in contents[:-1])
        body += page_para(contents[-1], sect=None if last else properties)
    return body


def section_restart_parity() -> Doc:
    # E-section-odd showed that a section that restarts its numbering with an odd or even
    # start gets no blank page, and that its number went up by one where it had the wrong
    # parity (#301). An even start restarted at 5 after a page numbered 1 tells "restart + 1"
    # (6) from "the restart is ignored" (2); Section 3 continues the numbering after an even
    # number either way, so it gets a blank page; a restart at 0 shows what Word does with
    # 0. The footer counts the document's and the section's pages, so the counts show
    # whether a blank page or a skipped number counts.
    sections = [
        # (label, start type, first page number, page count)
        ("Section 1: one page", None, None, 1),
        ("Section 2: even page start after a section that ends on page 1, page numbers "
         "restarted at 5", "evenPage", 5, 1),
        ("Section 3: even page start after a section that ends on an even page number; two "
         "pages", "evenPage", None, 2),
        ("Section 4: odd page start, page numbers restarted at 0", "oddPage", 0, 1),
        ("Section 5: next page start, page numbers continued", None, None, 1),
    ]
    properties = [
        section(kind=kind, page_start=start, refs=HEADER_REFS if n == 0 else ())
        for n, (_, kind, start, _) in enumerate(sections)
    ]
    body = ""
    for n, (label, _, _, pages) in enumerate(sections):
        texts = [label] + [f"{label.split(':')[0]}, page {page}" for page in range(2, pages + 1)]
        for i, value in enumerate(texts):
            last = i == len(texts) - 1 and n < len(sections) - 1
            body += page_para(run(value), page_break_before=i > 0,
                              sect=properties[n] if last else None)
    return Doc(
        "E-section-restart-parity",
        "Odd and even section starts with page numbers restarted at 5 (after page 1, even "
        "start) and at 0 (odd start), and a blank page before a section that continues the "
        "numbering: the page numbers Word gives, and whether NUMPAGES and SECTIONPAGES in "
        "the footer count blank pages and skipped numbers",
        body,
        headers=(header_part("header1.xml", page_para(run("header"))), counting_footer()),
        last_section=properties[-1],
    )


def section_continuous_title() -> Doc:
    # E-section-continuous-headers showed that a continuous section's headers apply from
    # its first page of its own (#186). With a title page (w:titlePg), is that page its
    # first page, with the first-page header? And SECTIONPAGES: does the page where the
    # continuous section starts count toward the section before?
    first = (("headerReference", "default", "header1.xml"),
             ("footerReference", "default", "footer1.xml"))
    second = (("headerReference", "default", "header2.xml"),
              ("headerReference", "first", "header3.xml"),
              ("footerReference", "default", "footer2.xml"),
              ("footerReference", "first", "footer3.xml"))
    body = filler(29, "Section 1: 29 lines, with Header A and Footer A", sect=section(refs=first))
    body += filler(100, "Section 2: a continuous section of 100 lines with a title page, with "
                   "Header B and Footer B, and First header B and First footer B")
    return Doc(
        "E-section-continuous-title",
        "A continuous section with a title page (w:titlePg) that starts in the middle of page "
        "1 and runs onto pages 2 and 3: which header and footer each page takes, and whether "
        "SECTIONPAGES counts page 1 toward the section before",
        body,
        headers=(
            header_part("header1.xml", page_para(run("Header A"))),
            counting_footer("footer1.xml", "Footer A, page"),
            header_part("header2.xml", page_para(run("Header B"))),
            header_part("header3.xml", page_para(run("First header B"))),
            counting_footer("footer2.xml", "Footer B, page"),
            counting_footer("footer3.xml", "First footer B, page"),
        ),
        last_section=section(kind="continuous", refs=second, title_page=True),
    )


def keepnext_break() -> Doc:
    # #186: the layout takes the lines after a page break to start a page whatever the keep
    # rules say, so a paragraph kept with the next that holds a page break is not moved, and
    # is split when the paragraph after it does not fit. Each label fits one line, so that
    # the line counts are exact; widow control is off, so that only the keep rules decide.
    off = {"widow_control": False}
    cases = [
        # (lines before, lines after the break, a heading whose first run is the break)
        (56, 6, False),
        (20, 56, False),
        (56, None, True),
    ]
    body = ""
    for n, (before, after, heading) in enumerate(cases, 1):
        what = "a heading after a page break" if heading else "a paragraph with a page break"
        body += filler(before, f"Case {n:02d}: {before} lines, then {what}, kept with the next",
                       page_break_before=n > 1, **off)
        if heading:
            kept = page_break_run() + run(f"Case {n:02d} heading")
        else:
            kept = (run(f"Case {n:02d} kept: A") + page_break_run()
                    + lines([f"Case {n:02d} kept, line {k}" for k in range(1, after + 1)]))
        body += page_para(kept, keep_next=True, **off)
        body += page_para(lines([f"Case {n:02d} together, line {k}" for k in range(1, 4)]),
                          keep_lines=True, **off)
    return Doc(
        "E-keepnext-break",
        "Keep with next on a paragraph that holds a page break, followed by a paragraph of 3 "
        "lines kept together: 56 lines, then the kept paragraph's first line, its break and 6 "
        "lines, which with the 3 fit the page; 20 lines, then a break and 56 lines, after "
        "which the 3 do not fit; and a heading whose first run is a page break; without widow "
        "control",
        body,
    )


def header_no_room() -> Doc:
    # 35.4 pt from each edge, 33 lines of 12 pt each: the header ends at 431.4 pt and the
    # footer starts at 410.5 pt, so they overlap and leave the body no room (#186).
    return Doc(
        "E-header-no-room",
        "A header and a footer of 33 lines each, which together leave the body no room on "
        "the page: where Word puts the body and how many pages it makes",
        filler(10, "the body: 10 lines"),
        headers=(header_part("header1.xml", tall("header", 33)),
                 header_part("footer1.xml", tall("footer", 33))),
        last_section=section(refs=HEADER_REFS),
    )


def header_taller_than_page() -> Doc:
    # 80 lines of 12 pt, 960 pt, taller than the 841.9 pt page (#186).
    return Doc(
        "E-header-taller-than-page",
        "A header of 80 lines, taller than the whole page: how much of it Word draws, where "
        "the body goes and how many pages it makes",
        filler(10, "the body: 10 lines"),
        headers=(header_part("header1.xml", tall("header", 80)),),
        last_section=section(refs=(HEADER_REFS[0],)),
    )


def header_space_before() -> Doc:
    # E-header-space-after showed that a header's last space after counts toward its height
    # (#186). 35.4 pt from the edge, 24 pt before and two lines: 83.4 pt with the space
    # before, past the 72 pt margin, and 59.4 pt without it, inside.
    header = page_para(
        run("header line 1, with 24 pt before"), space=spacing(before=24, line=("exact", 12))
    ) + page_para(run("header line 2"))
    return Doc(
        "E-header-space-before",
        "A header of two lines whose first paragraph has 24 pt before, taller with it than "
        "the top margin: whether the space before counts toward the header's height",
        filler(70, "the body: 70 lines"),
        headers=(header_part("header1.xml", header),),
        last_section=section(refs=(HEADER_REFS[0],)),
    )


# The heights of the first line of each case of E-footer-tall-bottom: the 53rd line of the
# page ends at 707 to 720 pt, over the whole range E-header-tall leaves open.
FOOTER_FIRST_LINES = list(range(11, 25))


def footer_tall_bottom() -> Doc:
    # E-header-tall bounds the body's bottom under a footer of 8 lines between 707.4 and
    # 719.4 pt. Here each case starts a page with one line of 11 to 24 pt, then 52 lines of
    # 12 pt: whether the 53rd line still fits places the bottom to 1 pt. Widow control
    # is off, so a line that does not fit goes to the next page alone.
    body = ""
    for n, first in enumerate(FOOTER_FIRST_LINES, 1):
        label = f"Case {n:02d}: a first line of {first} pt, then 52 lines of 12 pt"
        body += para(run(label), space=spacing(line=("exact", first)), widow_control=False,
                     page_break_before=n > 1)
        body += page_para(lines([f"Case {n:02d}, line {k}" for k in range(2, 54)]),
                          widow_control=False)
    return Doc(
        "E-footer-tall-bottom",
        "A footer of 8 lines, taller than the bottom margin, and pages whose first line is "
        "11 to 24 pt high, then 52 lines of 12 pt: where the body ends, to 1 pt",
        body,
        headers=(header_part("footer1.xml", tall("footer", 8)),),
        last_section=section(refs=(HEADER_REFS[1],)),
    )


def paragraph_borders(space: int = 4, sides: tuple[str, ...] = ("top", "left", "bottom", "right")
                      ) -> str:
    """A w:pBdr of single 0.5 pt lines on these sides, space points from the text."""
    lines_xml = "".join(
        f'<w:{side} w:val="single" w:sz="4" w:space="{space}" w:color="000000"/>'
        for side in sides
    )
    return f"<w:pBdr>{lines_xml}</w:pBdr>"


def para_shading_borders() -> Doc:
    # E-para-shading showed the fill about 1.5 pt wider than the indents on each side, and
    # shaded paragraphs without space between them as one band (#298). With borders, does
    # the fill reach the border lines? And is the space between two shaded paragraphs
    # shaded?
    text = "shaded text that runs over several lines of one paragraph, " * 5
    apart = page_para(run("not shaded"))
    cases = [
        ("a box border 4 pt from the text", {"borders": paragraph_borders(4)}, True),
        ("a box border 0 pt from the text", {"borders": paragraph_borders(0)}, True),
        ("a left border only, 4 pt from the text",
         {"borders": paragraph_borders(4, ("left",))}, True),
        ("the first of two shaded paragraphs, with 12 pt after",
         {"space": spacing(after=12, line=("exact", 12))}, False),
        ("the second of two shaded paragraphs, with 12 pt before",
         {"space": spacing(before=12, line=("exact", 12))}, True),
        ("the first of two shaded paragraphs in a row with the same box border",
         {"borders": paragraph_borders(4)}, False),
        ("the second of two shaded paragraphs in a row with the same box border",
         {"borders": paragraph_borders(4)}, True),
    ]
    body = "".join(
        page_para(run(f"Case {n:02d}: {label}: ") + run(text), shading=SHADE, **props)
        + (apart if separate else "")
        for n, (label, props, separate) in enumerate(cases, 1)
    )
    return Doc(
        "E-para-shading-borders",
        "Paragraph shading with a box border 4 and 0 pt from the text and with a left border "
        "only, two shaded paragraphs with 24 pt between them, and two in a row with the same "
        "box border: how far the fill reaches",
        body,
    )


# The general formatting switches (\*) for numbers (ECMA-376 §17.16.4.3).
NUMBER_SWITCHES = [
    "Arabic", "ArabicDash", "ALPHABETIC", "alphabetic", "ROMAN", "roman", "Ordinal",
    "CardText", "OrdText", "Hex", "DollarText",
]
# The first page numbers of E-number-formats: 0, letters of two and three (27, 53) and of 65
# (1665) characters, and Roman numerals of 4000 and above.
NUMBER_STARTS = [0, 27, 53, 1665, 4000, 4999]
GERMAN = '<w:lang w:val="de-DE"/>'


def number_formats() -> Doc:
    # #186: the layout writes 0 as decimal, Roman thousands as repeated M, letters capped at
    # 64, ordinals with English suffixes, and nothing for a Chicago number past 116 (MS-OI29500
    # 2.1.548 k). Each case is a section of one page whose numbers start where its label
    # says; PAGE shows its number in every format.
    sections = []
    for n, start in enumerate(NUMBER_STARTS, 1):
        contents = [run(f"Case {n:02d}: page numbers restarted at {start}")]
        contents += [run(f"PAGE \\* {switch}: ") + field(f"PAGE \\* {switch}")
                     for switch in NUMBER_SWITCHES]
        if start == 27:
            contents += [run(f"PAGE \\* {switch} in German: ")
                         + complex_field(f"PAGE \\* {switch}", "1", GERMAN)
                         for switch in ("Ordinal", "OrdText", "CardText")]
        sections.append((contents, section(page_start=start)))
    n = len(NUMBER_STARTS)
    for k, start in enumerate((116, 117), n + 1):
        contents = [run(f"Case {k:02d}: Chicago page numbers restarted at {start}"),
                    run("PAGE: ") + field("PAGE"),
                    run("PAGE \\* Arabic: ") + field("PAGE \\* Arabic")]
        sections.append((contents, section(page_start=start, page_format="chicago")))
    contents = [run(f"Case {n + 3:02d}: the number of pages")]
    contents += [run(f"NUMPAGES \\* {switch}: ") + field(f"NUMPAGES \\* {switch}")
                 for switch in ("Arabic", "ROMAN", "alphabetic", "CardText")]
    sections.append((contents, section()))
    return Doc(
        "E-number-formats",
        "PAGE fields in the eleven number formats of the \\* switch (Arabic to DollarText) at "
        "page numbers 0, 27, 53, 1665, 4000 and 4999, ordinals and numbers in words in German, "
        "Chicago page numbers at 116 and 117, and NUMPAGES in four formats",
        sectioned(sections),
        last_section=sections[-1][1],
    )


def bold_missing() -> Doc:
    # #186: bold in a family without a Bold face, which the layout draws by stroking the
    # outlines 1/30 of the size wide. Lucida Console and Impact come with Windows with a
    # Regular face only. The label is in Calibri, the sample in a run of its own.
    bold, italic = "<w:b/><w:bCs/>", "<w:i/><w:iCs/>"
    samples = [(24, "regular", "")] + [(size, "bold", bold) for size in (8, 11, 24, 48)] + [
        (24, "italic", italic), (24, "bold italic", bold + italic)]
    body, n = "", 0
    for font in ("Lucida Console", "Impact"):
        for size, label, toggles in samples:
            n += 1
            body += para(run(f"Case {n:02d}: {font} {size} pt {label}: ")
                         + run("Hamburgefonts", font, size, toggles))
    return Doc(
        "E-bold-missing",
        "Bold, italic and bold italic in families without those faces, Lucida Console and "
        "Impact, at 8 to 48 pt: how Word draws the bold it makes up (stroke width) and the "
        "italic (slant)",
        body,
    )


def soft_hyphen_font() -> Doc:
    # #186: the layout keeps a soft hyphen in the run of the character before it, so a line
    # that ends there shows a hyphen in that character's face and size. Here the soft hyphen
    # has a font and size of its own, Noto Sans 20 pt; and a w:sym of Noto Sans, not a symbol
    # font, whose code F0AD stands for U+00AD. In a paragraph 150 pt wide, the word of two
    # halves (155 pt in Calibri) can break only at it; in a full-width paragraph it does not.
    half = "Hamburgefontsiv"
    big = rpr("Noto Sans", 20)
    soft = f"<w:r>{big}<w:softHyphen/></w:r>"
    symbol = '<w:r><w:sym w:font="Noto Sans" w:char="F0AD"/></w:r>'
    cases = [
        ("a soft hyphen in Noto Sans 20 pt between two halves in Calibri, at the end of a line",
         soft, NARROW),
        ("the same in the middle of a line", soft, None),
        ("a w:sym of Noto Sans F0AD between two halves in Calibri, at the end of a line",
         symbol, NARROW),
        ("the same in the middle of a line", symbol, None),
    ]
    body = ""
    for n, (label, middle, ind) in enumerate(cases, 1):
        body += para(run(f"Case {n:02d}: {label}:"))
        body += para(run(half) + middle + run(half), ind=ind)
    return Doc(
        "E-soft-hyphen-font",
        "A soft hyphen in another font and size than the letters before it, and a w:sym "
        "standing for one, at the end of a line and in the middle of one: whether Word breaks "
        "there and in which font and size it draws the hyphen",
        body,
    )


# Lines and drawing, second batch (#298): what the references of E-underline, E-superscript,
# E-auto-color, E-highlight, E-underline-trailspace and E-tab-margin left open.
SIZE_24 = '<w:sz w:val="48"/><w:szCs w:val="48"/>'
SUPERSCRIPT = '<w:vertAlign w:val="superscript"/>'


def underline_24() -> Doc:
    # E-underline measured every kind at 11 pt only: do the dash lengths, the thick and
    # double geometry and the wave scale with the size? It also showed one line under a
    # superscript in a word; here runs of two sizes, and a superscript with no underlined
    # text next to it.
    single = underline_props("single")
    body = "".join(
        labelled(n, f"underline {kind} at 24 pt",
                 styled_run("underlined", SIZE_24 + underline_props(kind)))
        for n, kind in enumerate(UNDERLINES, 1)
    )
    n = len(UNDERLINES)
    body += labelled(n + 1, "single across runs of 11, 24 and 11 pt",
                     styled_run("small ", single), styled_run("LARGE", SIZE_24 + single),
                     styled_run(" small", single))
    body += labelled(n + 2, "single under a superscript whose neighbours are not underlined",
                     styled_run("base ", ""), styled_run("123", single + SUPERSCRIPT),
                     styled_run(" base", ""))
    body += labelled(n + 3, "double across runs of 11 and 24 pt",
                     styled_run("small ", underline_props("double")),
                     styled_run("LARGE", SIZE_24 + underline_props("double")))
    return Doc(
        "E-underline-24",
        "Every kind of underline at 24 pt, a single and a double underline across runs of "
        "11 and 24 pt, and an underlined superscript whose neighbours are not underlined",
        body,
    )


SCRIPT_SIZES = [8, 9, 10, 12, 16, 20, 36, 48]


def superscript_sizes() -> Doc:
    # E-superscript gave the size (65 % rounded to a half point) and the shifts at 11 and
    # 24 pt only; the subscript's drop fits no simple rule yet.
    cases = []
    for size in SCRIPT_SIZES:
        hp = half_points(size)
        size_props = f'<w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/>'
        for kind in ("superscript", "subscript"):
            cases.append((f"{kind} at {size} pt", size_props, kind))
    body = "".join(
        labelled(n, label, styled_run("Base", props),
                 styled_run("123", props + f'<w:vertAlign w:val="{kind}"/>'))
        for n, (label, props, kind) in enumerate(cases, 1)
    )
    return Doc(
        "E-superscript-sizes",
        "The size and the baseline shift of superscript and subscript text at 8, 9, 10, 12, "
        "16, 20, 36 and 48 pt",
        body,
    )


# Run shading between the colours where E-auto-color's text stayed black (404040, 4D4D4D)
# and turned white (333333): greys from 343434 to 3F3F3F, and a dark red that the two luma
# formulas the reference left (Rec. 601 and Rec. 709) put on either side.
AUTO_COLOR_THRESHOLD_FILLS = [f"{v:02X}" * 3 for v in range(0x34, 0x40)] + ["E00000"]


def auto_color_threshold() -> Doc:
    body, n = "", 0
    for fill in AUTO_COLOR_THRESHOLD_FILLS:
        n += 1
        shade = f'<w:shd w:val="clear" w:color="auto" w:fill="{fill}"/>'
        body += labelled(n, f"run shading {fill}", styled_run("automatic colour", shade))
    # E-bar-tab's bar was black on 404040, where text stays black too.
    two_lines = "two lines of text in one paragraph with a bar tab at 144 pt, " * 2
    for fill in ("333333", "000000"):
        n += 1
        body += labelled(n, f"a bar tab on paragraph shading {fill}", styled_run(two_lines, ""),
                         tabs=[("bar", 2880)], shading=fill)
    return Doc(
        "E-auto-color-threshold",
        "Text in the automatic colour on run shading of greys from 343434 to 3F3F3F and of "
        "E00000, and bar tabs on paragraph shading of 333333 and 000000: where Word turns "
        "text white, and whether the bar follows",
        body,
    )


def highlight_tall() -> Doc:
    # E-highlight showed the highlight filling the line box of single spacing, of a raised
    # run and of an exact line; here lines taller than the font.
    cases = [
        ("highlighted, 1.5 lines spacing", {"space": spacing(line=("auto", 360))}),
        ("highlighted, double spacing", {"space": spacing(line=("auto", 480))}),
        ("highlighted, at least 24 pt spacing", {"space": spacing(line=("atLeast", 24))}),
        ("highlighted, exact 24 pt spacing", {"space": spacing(line=("exact", 24))}),
    ]
    body = "".join(
        labelled(n, label, styled_run("highlighted text", HIGHLIGHT), **props)
        for n, (label, props) in enumerate(cases, 1)
    )
    body += labelled(len(cases) + 1, "highlighted 11 pt text on a line with a 24 pt run",
                     styled_run("highlighted text", HIGHLIGHT), styled_run(" LARGE", SIZE_24))
    return Doc(
        "E-highlight-tall",
        "Highlighted text in lines taller than its font: 1.5 lines, double, at least 24 pt "
        "and exact 24 pt spacing, and next to a 24 pt run",
        body,
    )


# The number of spaces after five underlined words, and where they end in Calibri 11 pt
# (the test measures it with Carlito, its metric twin).
TRAILING_SPACES = [
    (75, "11 pt before the right margin"),
    (84, "11 pt past the right margin"),
    (96, "41 pt past the right margin"),
    (120, "101 pt past the right margin, 29 pt past the edge of the page"),
]


def underline_trailspace_margin() -> Doc:
    # E-underline-trailspace showed underlined trailing spaces up to the paragraph mark on
    # left-aligned lines, and not past the margin on justified ones. Here left-aligned
    # lines whose trailing spaces end before the margin, past it and past the edge of the
    # page: five words of 10 letters, the spaces, and a word too wide to follow them on
    # the line. The label has a paragraph of its own, so the sample starts its line.
    single = underline_props("single")
    body = ""
    for n, (count, where) in enumerate(TRAILING_SPACES, 1):
        body += para(run(f"Case {n:02d}: 5 underlined words, {count} spaces and an underlined "
                         f"word, left aligned; in Calibri 11 pt the spaces end {where}:"))
        body += para(styled_run(" ".join(["underlined"] * 5) + " " * count + "underlined",
                                single))
    return Doc(
        "E-underline-trailspace-margin",
        "Underlined trailing spaces that end before the right margin, past it and past the "
        "edge of the page on left-aligned lines, with the w:compat setting ulTrailSpace: "
        "whether the underline stops at the margin or the edge of the page",
        body,
        compat_flags=("ulTrailSpace",),
    )


def tab_margin_alone(mode: int) -> Doc:
    # E-tab-margin showed that Word puts a tab whose left stop lies past the margin on a
    # line of its own. Where does that tab end? An underline and a leader show its extent.
    in_margin, past_edge = TEXT_W + 360, PAGE_W - MARGIN + 360
    single = underline_props("single")
    cases = [
        ("a left stop 18 pt into the right margin, the tab underlined", in_margin, single, None),
        ("a left stop 18 pt past the right edge of the page, the tab underlined", past_edge,
         single, None),
        ("a left stop 18 pt into the right margin, with a dot leader", in_margin, "", "dot"),
    ]
    body = ""
    for n, (label, pos, props, leader) in enumerate(cases, 1):
        stop = ("left", pos, leader) if leader else ("left", pos)
        body += para(run(f"Case {n:02d}: {label}:"))
        body += para(styled_run("before", "") + tab_run(props) + styled_run("after", ""),
                     tabs=[stop])
    suffix = "" if mode == 15 else f"-{mode}"
    return Doc(
        f"E-tab-margin-alone{suffix}",
        "A tab whose left stop lies in the right margin or past the edge of the page, which "
        f"Word puts on a line of its own, in compatibility mode {mode}: where that tab ends, "
        "shown by its underline and by a dot leader",
        body,
        mode=mode,
    )


def list_label_height() -> Doc:
    # E-list-label-format showed a 20 pt label on 11 pt text raising the line by its ascent
    # only, and a mark's character style giving the label its size but not its underline.
    underline = '<w:u w:val="single"/>'
    cases = [
        ("level 20 pt, mark 11 pt", {}, label_level(SIZE_20), ""),
        ("level 20 pt, mark 20 pt", {"mark": SIZE_20}, label_level(SIZE_20), ""),
        ("level without a size, mark 20 pt", {"mark": SIZE_20}, label_level(), ""),
        ("level without an underline, mark underlined", {"mark": underline}, label_level(), ""),
    ]
    numbering, body = label_lists(cases)
    n = len(cases)
    body += para(run(f"Case {n + 1:02d}: no list, mark 20 pt"), mark=SIZE_20)
    body += para(run(f"Case {n + 2:02d}: no list, mark 11 pt"))
    return Doc(
        "E-list-label-height",
        "The line height of list items whose label is larger than their text: a 20 pt label "
        "with an 11 pt and with a 20 pt mark, a label sized by the mark alone, and a 20 pt "
        "mark without a list; and whether a mark's direct underline reaches the label",
        body,
        numbering=numbering,
    )


# Table styles, second batch (#298 and #172's comments).
def table_style_size_defaults(mode: int, normal_jc: str = "left") -> Doc:
    # E-table-style-size-11pt* could not test [MS-DOCX] 2.3.1: Word treats Normal's 11 pt,
    # equal to the document defaults, and its w:jc left, equal to the default, as not set.
    # Here the defaults are 12 pt and right-aligned, and Normal sets the two values the
    # setting is about, 11 pt and left: off, they leave the table style's 14 pt and its
    # centring alone; on, they override them. With Normal right-aligned instead, over
    # defaults that leave the alignment out, the control: off, right is not left, so it
    # overrides the centring.
    big = table_style(
        "Big", [],
        whole='<w:pPr><w:jc w:val="center"/></w:pPr>'
        '<w:rPr><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr>',
    )
    said = "on" if mode == 15 else f"not written, in compatibility mode {mode}"
    defaults_jc = "right" if normal_jc == "left" else None
    defaults_said = " and right-aligned" if defaults_jc else ""
    body = table_case(
        1, f"a table in the style Big (14 pt, centred); document defaults 12 pt{defaults_said}, "
        f"Normal 11 pt and {normal_jc}-aligned",
        styled_table("Big", named_cells(2, 2), widths=[TEXT_W // 2] * 2),
    ) + para(run("Case 02: a paragraph after the table, in Normal"))
    suffix = ("" if normal_jc == "left" else f"-{normal_jc}") + ("" if mode == 15 else f"-{mode}")
    return Doc(
        f"E-table-style-defaults-size{suffix}",
        f"Whether a Normal style of 11 pt, aligned {normal_jc}, overrides the 14 pt and the "
        "centring of a table style when the document defaults are 12 pt"
        + (" and aligned right" if defaults_jc else "")
        + f", with the setting overrideTableStyleFontSizeAndJustification {said}",
        body,
        styles=big,
        normal_props=f'<w:pPr><w:jc w:val="{normal_jc}"/></w:pPr>'
        '<w:rPr><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr>',
        run_default_size=12,
        paragraph_defaults_extra=f'<w:jc w:val="{defaults_jc}"/>' if defaults_jc else "",
        mode=mode,
    )


def table_style_rows_cols() -> Doc:
    # E-table-style-look could not show whether first and last rows or first and last
    # columns win in the corners: its style has corner formats. Nor whether a first row or
    # column whose flag is on but whose format the style lacks still starts the bands after
    # it.
    edges = [c for c in CONDITIONS if c[0] in ("firstRow", "lastRow", "firstCol", "lastCol")]
    styles = "".join([
        table_style("NoCorners", edges),
        table_style("RowBandsOnly", ROW_BANDS, row_band=1),
        table_style("ColBandsOnly", COL_BANDS, col_band=1),
    ])
    even = [TEXT_W // 5] * 5
    body = table_case(
        1, "a style with first and last row and column formats but no corner formats, every "
        "flag on, no bands",
        styled_table("NoCorners", named_cells(5, 5), look_attributes=look(1, 1, 1, 1, 0, 0),
                     widths=even),
    ) + table_case(
        2, "a style with row band formats only; first row flag on, row bands on",
        styled_table("RowBandsOnly", named_cells(6, 3), look_attributes=look(1, 0, 0, 0, 1, 0),
                     widths=[TEXT_W // 3] * 3),
    ) + table_case(
        3, "a style with column band formats only; first column flag on, column bands on",
        styled_table("ColBandsOnly", named_cells(3, 6), look_attributes=look(0, 0, 1, 0, 0, 1),
                     widths=[TEXT_W // 6] * 6),
    ) + table_case(
        # #298: the last row (column) is band 2 if the bands count it, and plain if not.
        4, "a style with row band formats only; last row flag on, row bands on",
        styled_table("RowBandsOnly", named_cells(4, 3), look_attributes=look(0, 1, 0, 0, 1, 0),
                     widths=[TEXT_W // 3] * 3),
    ) + table_case(
        5, "a style with column band formats only; last column flag on, column bands on",
        styled_table("ColBandsOnly", named_cells(3, 4), look_attributes=look(0, 0, 0, 1, 0, 1),
                     widths=[TEXT_W // 4] * 4),
    )
    return Doc(
        "E-table-style-rows-cols",
        "Which format a corner cell takes when the table style has first and last row and "
        "column formats but no corner formats, and whether bands start after a first row or "
        "column, or count a last row or column, whose flag is on but whose format the style "
        "lacks; each format gives a text colour and a grey",
        body,
        styles=styles,
    )


def table_style_header_rows_3(setting: int | None, mode: int = 15) -> Doc:
    # E-table-style-header-rows-on and -off rendered alike: two header rows, both with the
    # first-row format. Here three header rows, a first-row format with a thick bottom
    # border, and a table long enough to repeat its header rows on a second page. Word
    # dropped the 0 from the copy of E-table-style-header-rows-off it exported (#344), so
    # -off may again show the setting on; mode 14, where Word writes no such setting,
    # shows the layout of Word 2010 instead.
    first = [c for c in CONDITIONS if c[0] == "firstRow"][0]
    kind, color, fill = first
    first_row = (
        f'<w:tblStylePr w:type="{kind}"><w:rPr><w:color w:val="{color}"/></w:rPr>'
        '<w:tcPr><w:tcBorders><w:bottom w:val="single" w:sz="24" w:space="0" '
        'w:color="000000"/></w:tcBorders>'
        f'<w:shd w:val="clear" w:color="auto" w:fill="{fill}"/></w:tcPr></w:tblStylePr>'
    )
    # The first-row format goes before the bands, in the order of ST_TblStyleOverrideType.
    head, tail = table_style("ThreeHeaders", ROW_BANDS, row_band=1).split("<w:tblStylePr", 1)
    style_xml = head + first_row + "<w:tblStylePr" + tail
    body = table_case(
        1, "rows 1 to 3 are header rows (w:tblHeader); the style formats its first row with a "
        "3 pt bottom border and bands rows by one; 70 rows, over two pages",
        styled_table("ThreeHeaders", named_cells(70, 3), look_attributes=look(1, 0, 0, 0, 1, 0),
                     widths=[TEXT_W // 3] * 3, header_rows=3),
    )
    if mode == 15:
        said = {1: "on", 0: "off"}[setting]
        name = f"E-table-style-header-rows-3-{said}"
        setting_said = f"with the setting differentiateMultirowTableHeaders {said}"
        values = (("differentiateMultirowTableHeaders", setting),)
    else:
        name = f"E-table-style-header-rows-3-{mode}"
        setting_said = (f"in compatibility mode {mode}, which has no setting "
                        "differentiateMultirowTableHeaders")
        values = ()
    return Doc(
        name,
        "Which of three header rows take the first-row format of their table style, with its "
        "bottom border, on the first page and where they repeat on the second, "
        f"{setting_said}; each format gives a text colour and a grey",
        body,
        styles=style_xml,
        compat_values=values,
        mode=mode,
    )


def format_with_borders(kind: str, borders: str) -> str:
    """A conditional format of CONDITIONS' colours for kind, with these w:tcBorders."""
    color, fill = next((c, f) for k, c, f in CONDITIONS if k == kind)
    return (
        f'<w:tblStylePr w:type="{kind}"><w:rPr><w:color w:val="{color}"/></w:rPr>'
        f"<w:tcPr><w:tcBorders>{borders}</w:tcBorders>"
        f'<w:shd w:val="clear" w:color="auto" w:fill="{fill}"/></w:tcPr></w:tblStylePr>'
    )


def table_style_header_rows_format() -> Doc:
    # #298's comments on #317's choices for header rows that no render shows: whether the
    # header rows make one group for the first-row format's borders, whether each takes the
    # corner formats, whether a header row after a body row takes the first-row format and
    # leaves the bands, and whether header rows are banded when the first-row flag is off.
    borders = (
        '<w:top w:val="double" w:sz="4" w:space="0" w:color="000000"/>'
        '<w:bottom w:val="single" w:sz="24" w:space="0" w:color="000000"/>'
        '<w:insideH w:val="dotted" w:sz="8" w:space="0" w:color="000000"/>'
    )
    # The styles' own borders are single, ½ pt, so double and dotted lines and the 3 pt line
    # can only come from the first-row format.
    head, tail = table_style("HeaderBorders", []).split("</w:style>", 1)
    header_borders = head + format_with_borders("firstRow", borders) + "</w:style>" + tail
    corners = [c for c in CONDITIONS if c[0] in ("firstRow", "firstCol", "lastCol", "neCell",
                                                 "nwCell")]
    first_row = [c for c in CONDITIONS if c[0] == "firstRow"]
    styles = "".join([
        header_borders,
        table_style("HeaderCorners", corners),
        table_style("HeaderBands", first_row + ROW_BANDS, row_band=1),
    ])
    body = table_case(
        1, "rows 1 and 2 are header rows; the style's first-row format has a double top "
        "border, a 3 pt bottom border and a dotted inside border; first row flag on, no bands",
        styled_table("HeaderBorders", named_cells(4, 3), look_attributes=look(1, 0, 0, 0, 0, 0),
                     widths=[TEXT_W // 3] * 3, header_rows=2),
    ) + table_case(
        2, "rows 1 and 2 are header rows; the style has first row, first column, last column "
        "and top corner formats; those three flags on, no bands",
        styled_table("HeaderCorners", named_cells(4, 4), look_attributes=look(1, 0, 1, 1, 0, 0),
                     widths=[TEXT_W // 4] * 4, header_rows=2),
    ) + table_case(
        3, "rows 1 and 3 are header rows, row 2 is not; the style has a first-row format and "
        "bands rows by one; first row flag on, row bands on",
        styled_table("HeaderBands", named_cells(6, 3), look_attributes=look(1, 0, 0, 0, 1, 0),
                     widths=[TEXT_W // 3] * 3, header_rows=(1, 3)),
    ) + table_case(
        4, "rows 1 and 2 are header rows; the style has a first-row format and bands rows by "
        "one; first row flag off, row bands on",
        styled_table("HeaderBands", named_cells(6, 3), look_attributes=look(0, 0, 0, 0, 1, 0),
                     widths=[TEXT_W // 3] * 3, header_rows=2),
    )
    return Doc(
        "E-table-style-header-rows-format",
        "How header rows take the first-row format of their table style: its borders as one "
        "group or row by row, the top corner formats, a header row after a body row, and "
        "header rows with the first row flag off; each format gives a text colour and a grey",
        body,
        styles=styles,
    )


def table_style_two_defaults() -> Doc:
    # #298, from the review of #319: officewright-doc takes the last table style marked
    # w:default, as ECMA-376 Part 1 §17.7.4.17 says and as Word does for paragraph styles
    # (E-default-paragraph-style, -after-normal). Normal Table is marked too, first, as in
    # every document here. Each of the two gives the table a colour and a grey of its own.
    defaults = [("FirstDefault", "C00000", "D9D9D9"), ("SecondDefault", "0070C0", "A6A6A6")]
    styles = "".join(
        table_style(
            style_id, [], whole=f'<w:rPr><w:color w:val="{color}"/></w:rPr>',
            cell=f'<w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="{fill}"/></w:tcPr>',
            default=True,
        )
        for style_id, color, fill in defaults
    )
    body = table_case(
        1, "a table without w:tblStyle; Normal Table, FirstDefault (red text, light grey) and "
        "SecondDefault (blue text, dark grey) are all marked w:default",
        styled_table(None, named_cells(2, 3), widths=[TEXT_W // 3] * 3),
    )
    return Doc(
        "E-table-style-two-defaults",
        "Which table style a table without w:tblStyle takes when three table styles are "
        "marked w:default: Normal Table, then two styles that each give the table a text "
        "colour and a grey",
        body,
        styles=styles,
    )


def cell_margin(side: str, width: int, kind: str) -> str:
    return f'<w:{side} w:w="{width}" w:type="{kind}"/>'


WRAPPING = "text long enough to wrap inside its cell, to show where its lines start and end"


def margined_table(
    tbl_cell_mar: str, cells: list[tuple[str, str]], grid: list[int], style: str = ""
) -> str:
    """A one-row table with fixed layout and single borders, in the table style style (or in
    none), the table's cell margins tbl_cell_mar (a w:tblCellMar, or ""), and per cell its
    text and w:tcMar (or "")."""
    sides = ("top", "left", "bottom", "right", "insideH", "insideV")
    tbl_pr = (
        (f'<w:tblStyle w:val="{style}"/>' if style else "")
        + f'<w:tblW w:w="{sum(grid)}" w:type="dxa"/><w:tblInd w:w="0" w:type="dxa"/>'
        "<w:tblBorders>" + "".join(border(s) for s in sides) + "</w:tblBorders>"
        '<w:tblLayout w:type="fixed"/>' + tbl_cell_mar
    )
    tcs = "".join(
        f'<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/>{tc_mar}</w:tcPr>'
        f"{para(run(value))}</w:tc>"
        for (value, tc_mar), width in zip(cells, grid)
    )
    grid_xml = "".join(f'<w:gridCol w:w="{w}"/>' for w in grid)
    return (f"<w:tbl><w:tblPr>{tbl_pr}</w:tblPr><w:tblGrid>{grid_xml}</w:tblGrid>"
            f"<w:tr>{tcs}</w:tr></w:tbl>")


SIDES = ("top", "left", "bottom", "right")


def table_cell_margins() -> Doc:
    # #172, case 9: a cell's own margins of type pct and auto, which no note settles, and
    # of type nil, which Word ignores (MS-OI29500 §17.4.77(a)), against the table's
    # margins of 5 pt on every side.
    table_mar = "<w:tblCellMar>" + "".join(cell_margin(s, 100, "dxa") for s in SIDES) + (
        "</w:tblCellMar>")
    kinds = [("pct", 500), ("auto", 200), ("nil", 0)]
    cells = [
        (f"cell margins of type {kind}: " + WRAPPING,
         "<w:tcMar>" + "".join(cell_margin(s, width, kind) for s in SIDES) + "</w:tcMar>")
        for kind, width in kinds
    ] + [("no cell margins of its own: " + WRAPPING, "")]
    body = table_case(
        1, "the table's cell margins 5 pt on every side; a cell's own margins of type pct "
        "(w:w 500, 10 %), auto (w:w 200) and nil, and one without",
        margined_table(table_mar, cells, [TEXT_W // 4] * 4),
    )
    return Doc(
        "E-table-cell-margins",
        "A cell's own margins (w:tcMar) of type pct, auto and nil, against the table's margins "
        "of 5 pt: where the text of each cell starts and ends",
        body,
    )


def table_cell_margins_nil() -> Doc:
    # #172, case 10: a table's own margins of type nil, one side per table; the notes on
    # the top and bottom point to Word ignoring nil, those on the start and end do not
    # say. The tables are in a style whose margins are 10 pt on every side, unlike Normal
    # Table's (0 at the top and bottom, 5.4 pt at the start and end), so a side shows 0
    # if Word takes nil as written, and 10 pt if it ignores it.
    margins = "<w:tblCellMar>" + "".join(cell_margin(s, 200, "dxa") for s in SIDES) + (
        "</w:tblCellMar>")
    body = ""
    for n, side in enumerate(SIDES, 1):
        table_mar = f"<w:tblCellMar>{cell_margin(side, 0, 'nil')}</w:tblCellMar>"
        body += table_case(
            n, f"the table's {side} cell margin of type nil; the style Margins10 has 10 pt "
            "on every side",
            margined_table(table_mar, [(WRAPPING, ""), (WRAPPING, "")], [TEXT_W // 2] * 2,
                           style="Margins10"),
        )
    return Doc(
        "E-table-cell-margins-nil",
        "A table's own cell margins (w:tblCellMar) of type nil, one side per table, in a table "
        "style with margins of 10 pt: whether Word ignores them, so that the table style's "
        "margins apply",
        body,
        styles=table_style("Margins10", [], cell_margins=margins),
    )


def floating_table(spec: str) -> str:
    """A one-row table of 200 pt floated by w:tblpPr with only the alignment spec."""
    sides = ("top", "left", "bottom", "right", "insideH", "insideV")
    tbl_pr = (
        f"<w:tblpPr {spec}/>"
        '<w:tblW w:w="4000" w:type="dxa"/>'
        "<w:tblBorders>" + "".join(border(s) for s in sides) + "</w:tblBorders>"
        '<w:tblLayout w:type="fixed"/>'
    )
    return (f"<w:tbl><w:tblPr>{tbl_pr}</w:tblPr><w:tblGrid><w:gridCol w:w=\"4000\"/>"
            "</w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w=\"4000\" w:type=\"dxa\"/></w:tcPr>"
            f"{para(run('the floating table'))}</w:tc></w:tr></w:tbl>")


def table_float_align() -> Doc:
    # #172, case 11: a floating table placed only by an alignment, both distances left
    # out. MS-OI29500 §17.4.57(b) says Word ignores a w:tblpPr whose distances are 0, whose
    # horizontal anchor is the text and whose vertical anchor is not.
    text = "Text around the floating table. " * 30
    body = ""
    for n, (label, spec) in enumerate([
        ("a table with only w:tblpYSpec bottom", 'w:tblpYSpec="bottom"'),
        ("a table with only w:tblpXSpec center", 'w:tblpXSpec="center"'),
    ], 1):
        body += para(run(f"Case {n:02d}: {label}, then text around it"),
                     page_break_before=n > 1)
        body += floating_table(spec) + para(run(text))
    return Doc(
        "E-table-float-align",
        "Floating tables (w:tblpPr) placed only by an alignment, bottom or centre, with both "
        "distances left out: whether Word floats them and where, or keeps them in the text",
        body,
    )


def bold_italic_missing() -> Doc:
    # #245: E-italic-missing showed italic text in a family without an Italic face drawn
    # with Regular, slanted. Bold italic in a family without a Bold Italic face could take
    # Bold, slanted, or Italic, emboldened. Garamond, which comes with Office, has Regular,
    # Italic and Bold faces only; Liberation Sans, installed complete, shows each face as it
    # is. The label is in Calibri, the sample in a run of its own.
    bold, italic = "<w:b/><w:bCs/>", "<w:i/><w:iCs/>"
    body, n = "", 0
    for font in ("Garamond", "Liberation Sans"):
        for size in (11, 20):
            for label, toggles in (("regular", ""), ("italic", italic), ("bold", bold),
                                   ("bold italic", bold + italic)):
                n += 1
                body += para(run(f"Case {n:02d}: {font} {size} pt {label}: ")
                             + run("Hamburgefonts gjpqy", font, size, toggles))
    return Doc(
        "E-bold-italic-missing",
        "Bold italic text in a family without a Bold Italic face (#245): Garamond, installed "
        "with Regular, Italic and Bold only, and Liberation Sans, installed complete, at 11 "
        "and 20 pt in regular, italic, bold and bold italic",
        body,
    )


def linked_levels(
    fmt: str, templates: list[str], style_id: str, named_by: int = 2
) -> list[str]:
    """Levels of a list with these templates, each indented further than the one before, so
    the label and the indent show the level; level named_by names the paragraph style."""
    return [
        level(ilvl, fmt, template, left=720 * (ilvl + 1), hanging=360,
              p_style=style_id if ilvl == named_by else None)
        for ilvl, template in enumerate(templates)
    ]


def list_level_linked_no_level() -> Doc:
    # #293: a paragraph style that writes a list but no level, which level 2 of that list
    # names with its w:pStyle. ECMA-376 §17.9.23 gives the paragraph the level that names
    # its style; E-list-level-inherit's case 04 showed that a level the style writes comes
    # first, and its case 05 a style without a level that no level names. Its comments add
    # a style that writes level 9, past the last level (MS-OI29500 §17.9.3(a) lets Word
    # read up to 255), and a style that inherits level 0 from the style it is based on.
    # Each list has a style of its own; the labels of the levels differ, so the label
    # shows the level.
    numbering = (
        abstract_num(0, "5C3A0031", linked_levels("lowerLetter", ["%1)", "(%2)", "[%3]"],
                                                  "LinkedNoLevel"))
        + abstract_num(1, "5C3A0032", linked_levels(
            "lowerRoman", [f"L{ilvl}-%{ilvl + 1}" for ilvl in range(9)], "LinkedLevel9"))
        + abstract_num(2, "5C3A0033", linked_levels("upperLetter", ["%1)", "(%2)", "[%3]"],
                                                  "LinkedBasedOn"))
        + num(1, 0) + num(2, 1) + num(3, 2)
    )
    styles = "".join([
        para_style("LinkedNoLevel", '<w:numPr><w:numId w:val="1"/></w:numPr>'),
        para_style("LinkedLevel9", '<w:numPr><w:ilvl w:val="9"/><w:numId w:val="2"/></w:numPr>'),
        para_style("LinkedBase", '<w:numPr><w:ilvl w:val="0"/><w:numId w:val="3"/></w:numPr>'),
        # Based on LinkedBase, and nothing of its own.
        '<w:style w:type="paragraph" w:customStyle="1" w:styleId="LinkedBasedOn">'
        '<w:name w:val="LinkedBasedOn"/><w:basedOn w:val="LinkedBase"/></w:style>',
    ])
    body = "".join(
        para(run(f"Case {n:02d}: {label}"), p_style=style)
        for n, (style, label) in enumerate([
            ("LinkedNoLevel",
             "the style LinkedNoLevel gives instance 1 and no level; level 2 names the style"),
            ("LinkedNoLevel", "a second paragraph in the same style"),
            ("LinkedLevel9", "the style LinkedLevel9 gives instance 2 and level 9, past the "
             "last of its nine levels; level 2 names the style"),
            ("LinkedBasedOn", "the style LinkedBasedOn writes nothing and is based on "
             "LinkedBase, which gives instance 3 and level 0; level 2 names LinkedBasedOn"),
        ], 1)
    )
    return Doc(
        "E-list-level-linked-no-level",
        "Which list level a paragraph takes whose style gives a list but no level, when "
        "level 2 of that list names the style (#293): level 2, as ECMA-376 says, or level 0; "
        "and when the style gives level 9, past the last level, or inherits level 0 from the "
        "style it is based on",
        body,
        numbering=numbering,
        styles=styles,
    )


def list_level_linked_defaults() -> Doc:
    # #293's comments: the document defaults give a level to every paragraph, and a style
    # gives a list but no level, which level 2 of the list names. Word may take the level of
    # the defaults, as a level the style hierarchy writes, the level that names the style,
    # or level 0. The defaults give level 1, not the comment's 0, so the label tells the
    # three apart. A document of its own, since the defaults reach every paragraph.
    numbering = abstract_num(0, "5C3A0034", linked_levels(
        "lowerLetter", ["%1)", "(%2)", "[%3]"], "LinkedDefaults")) + num(1, 0)
    body = para(run("Case 01: the style LinkedDefaults gives instance 1 and no level, the "
                    "document defaults give level 1; level 2 names the style"),
                p_style="LinkedDefaults")
    body += para(run("Case 02: a paragraph in Normal, which has level 1 from the document "
                     "defaults but no list"))
    return Doc(
        "E-list-level-linked-defaults",
        "Which list level a paragraph takes whose style gives a list but no level, when the "
        "document defaults give level 1 and level 2 of the list names the style (#293): the "
        "level of the defaults, the level that names the style, or level 0",
        body,
        numbering=numbering,
        styles=para_style("LinkedDefaults", '<w:numPr><w:numId w:val="1"/></w:numPr>'),
        paragraph_defaults_list='<w:numPr><w:ilvl w:val="1"/></w:numPr>',
    )


# Batch 8 (#409): what the references of #318, #321 and #368 left open.

# Borders of different widths on each side, so that a page shows which one Word draws at a
# split: top 6 pt, bottom 3 pt, between rows 1.5 pt, the vertical ones 0.5 pt; and those of
# a nested table, 4.5, 2.25 and 0.75 pt.
SPLIT_BORDERS = {"top": 48, "left": 4, "bottom": 24, "right": 4, "insideH": 12, "insideV": 4}
INNER_BORDERS = {"top": 36, "left": 4, "bottom": 18, "right": 4, "insideH": 6, "insideV": 4}


def split_case(n: int, label: str, content: str, filler_lines: int) -> str:
    """A case of E-table-split-borders on a page of its own: its label, filler lines so that
    the table starts low on the page, the table and a paragraph after it."""
    return (para(run(f"Case {n:02d}: {label}"), page_break_before=n > 1)
            + numbered_lines(f"Case {n:02d} filler", filler_lines) + content
            + para(run(f"Case {n:02d}: after the table")))


def table_split_borders() -> Doc:
    def split_table(rows: list) -> str:
        return rule_table(rows, EVEN_GRID, tbl_w=(TEXT_W, "dxa"), fixed=True,
                          borders=SPLIT_BORDERS)

    long_row = [cell(numbered_lines("Long", 40)), cell(para(run("r3c2"))),
                cell(para(run("r3c3")))]
    first = [row_cells(["r1c1", "r1c2", "r1c3"]), row_cells(["r2c1", "r2c2", "r2c3"])]
    header = [(row_cells([f"Head {r}a", f"Head {r}b", f"Head {r}c"]), "<w:tblHeader/>")
              for r in (1, 2)]
    body_rows = [row_cells([f"Body {r:02d}a", f"Body {r:02d}b", f"Body {r:02d}c"])
                 for r in range(1, 41)]
    inner = rule_table([row_cells([f"Inner {r:02d}a", f"Inner {r:02d}b"]) for r in range(1, 31)],
                       [2000, 2000], tbl_w=(4000, "dxa"), fixed=True, borders=INNER_BORDERS)
    nested = rule_table(
        [[cell(para(run("Outer cell")) + inner + para()), cell(para(run("Outer b")))]],
        [5000, TEXT_W - 5000], tbl_w=(TEXT_W, "dxa"), fixed=True, borders=SPLIT_BORDERS,
    )
    body = "".join([
        split_case(1, "row 3 of 4 holds a cell of 40 lines and crosses the page",
                   split_table(first + [long_row, row_cells(["r4c1", "r4c2", "r4c3"])]), 30),
        split_case(2, "row 3 is the last row, holds a cell of 40 lines and crosses the page",
                   split_table(first + [long_row]), 30),
        split_case(3, "rows 1 and 2 are header rows; 40 body rows cross the page",
                   split_table(header + body_rows), 20),
        split_case(4, "the outer row's first cell holds a nested table of 30 rows (top 4.5, "
                      "bottom 2.25, between rows 0.75 pt) and crosses the page", nested, 30),
    ])
    return Doc(
        "E-table-split-borders",
        "Which border Word draws where a table crosses a page, with borders of different "
        "widths (top 6 pt, bottom 3 pt, between rows 1.5 pt): a split middle row, a split "
        "last row, repeated header rows, and a split row holding a nested table with "
        "borders of its own; each case starts a page",
        body,
    )


# Unbreakable words of three widths, which together are wider than the text.
SHARE_WORDS = (AUTOFIT_WORD, "Supercalifragilisticexpialidocious", "Internationalisation")


def table_autofit_shares() -> Doc:
    auto = width(0, "auto")
    cases = [
        (1, "row 1: a cell spanning three columns (w:gridSpan 3) of 400 pt; row 2: cells of "
            "50, 100 and 150 pt",
         rule_table([[cell(para(run("Spanning three")),
                           width(8000, "dxa") + '<w:gridSpan w:val="3"/>')],
                     [cell(para(run("Fifty")), width(1000, "dxa")),
                      cell(para(run("Hundred")), width(2000, "dxa")),
                      cell(para(run("Hundred fifty")), width(3000, "dxa"))]],
                    [1000, 2000, 3000])),
        (2, "row 1: a cell spanning two columns of 300 pt; row 2: cells of 150 and 50 pt, the "
            "first spanned column's cell the wider",
         rule_table([[cell(para(run("Spanning cell")),
                           width(6000, "dxa") + '<w:gridSpan w:val="2"/>')],
                     [cell(para(run("Hundred fifty")), width(3000, "dxa")),
                      cell(para(run("Fifty")), width(1000, "dxa"))]], [3000, 1000])),
        (3, "three cells, auto, each holding one unbreakable word of another width ("
            + ", ".join(SHARE_WORDS) + "), together wider than the text width",
         rule_table([[cell(para(run(word)), auto) for word in SHARE_WORDS]], EVEN_GRID)),
        (4, "w:tblW 100 % (pct 5000), cells auto, borders of 6 pt",
         rule_table(autofit_cells([auto] * 3), EVEN_GRID, tbl_w=(5000, "pct"),
                    border_eighths=48)),
        (5, "w:tblW 100 % (pct 5000), cells auto, borders of 3 pt",
         rule_table(autofit_cells([auto] * 3), EVEN_GRID, tbl_w=(5000, "pct"),
                    border_eighths=24)),
        # Cases 01 and 02 span up to the last column; here the span ends before it, where
        # the explanations of E-table-autofit-15's case 8 give different grids.
        (6, "row 1: a cell spanning the first two of three columns, of 300 pt, and a cell of "
            "50 pt; row 2: cells of 50, 100 and 50 pt",
         rule_table([[cell(para(run("Spanning two")),
                           width(6000, "dxa") + '<w:gridSpan w:val="2"/>'),
                      cell(para(run("Third")), width(1000, "dxa"))],
                     [cell(para(run("Fifty")), width(1000, "dxa")),
                      cell(para(run("Hundred")), width(2000, "dxa")),
                      cell(para(run("Last")), width(1000, "dxa"))]], [1000, 2000, 1000])),
    ]
    return autofit_doc(
        "E-table-autofit-shares",
        "How Word's autofit shares width in compatibility mode 15: a cell spanning three "
        "columns, a spanning cell over a wider first column, minimum widths of three "
        "different words that together exceed the text width, 100 % tables with borders of 6 "
        "and 3 pt, and a span that ends before the last column",
        cases,
    )


def table_autofit_beside(mode: int, flag: str | None = None) -> Doc:
    said = {None: "", "doNotAutofitConstrainedTables": "-constrained"}[flag]
    setting = f", with w:{flag}" if flag else ""
    picture = vml_picture(
        "image1.png", "f1",
        "position:absolute;margin-left:0;margin-top:0;width:216pt;height:108pt;z-index:1;"
        "mso-position-horizontal-relative:margin;mso-position-vertical-relative:paragraph",
        "square", first=True)
    small = [[cell(para(run(f"Cell {r}{c}")), width(0, "auto")) for c in "abc"] for r in (1, 2)]
    fixed = [[cell(para(run(f"Cell {r}{c}")), width(1000, "dxa")) for c in "abc"]
             for r in (1, 2)]
    cases = [
        (1, "a floating picture of 216 x 108 pt at the left margin, square wrap, then an "
            "autofit table (cells auto) of about 120 pt, which fits beside it",
         para(picture + run("The picture is anchored here.")) + rule_table(small, EVEN_GRID)),
        (2, "the same picture, then a fixed table of three 50 pt columns (150 pt), which fits "
            "beside it",
         para(vml_picture(
             "image1.png", "f2",
             "position:absolute;margin-left:0;margin-top:0;width:216pt;height:108pt;"
             "z-index:2;mso-position-horizontal-relative:margin;"
             "mso-position-vertical-relative:paragraph", "square")
              + run("The picture is anchored here."))
         + rule_table(fixed, [1000] * 3, tbl_w=(3000, "dxa"), fixed=True)),
    ]
    # Each case starts a page, so that case 02's picture cannot meet case 01's.
    body = "".join(para(run(f"Case {n:02d}: {label}"), page_break_before=n > 1) + xml + para()
                   for n, label, xml in cases)
    return Doc(
        f"E-table-autofit-beside{said}-{mode}",
        "Whether a table that fits beside a floating picture of 216 x 108 pt with square wrap "
        "stays beside it: an autofit table of about 120 pt and a fixed one of 150 pt, each on "
        "a page of its own, in "
        f"compatibility mode {mode}{setting}",
        body + para(run("End of the document.")),
        media=(("image1.png", grey_png(96, 48)),),
        mode=mode,
        compat_flags=(flag,) if flag else (),
    )


def table_autofit_grow_15() -> Doc:
    return autofit_doc(
        "E-table-autofit-grow-15",
        "The cases w:growAutofit changed in compatibility mode 14 (E-table-autofit-grow-14: "
        "minimum widths wider than the text, a table beside a floating picture), in "
        "compatibility mode 15 with w:growAutofit",
        autofit_setting_cases(1),
        compat_flags=("growAutofit",),
    )


def header_repeat_limits() -> list[Doc]:
    header = "<w:tblHeader/>"
    body_rows = [row_cells([f"Body {r:02d}a", f"Body {r:02d}b", f"Body {r:02d}c"])
                 for r in range(1, 11)]

    def header_row(r: int, count: int) -> tuple:
        return ([cell(numbered_lines(f"Head {r}", count)), cell(para(run(f"Head {r}b"))),
                 cell(para(run(f"Head {r}c")))], header)

    def table_of(rows: list) -> str:
        return rule_table(rows, EVEN_GRID, tbl_w=(TEXT_W, "dxa"), fixed=True,
                          border_eighths=48)

    return [
        page_case("E-table-header-repeat-limit",
                  "Whether Word repeats header rows (w:tblHeader) that together take just "
                  "under a page: two header rows of 22 lines each (about 600 pt), 10 body "
                  "rows, then a paragraph between the table and the end of the document",
                  "rows 1 and 2 are header rows of 22 lines each; 10 body rows follow",
                  table_of([header_row(1, 22), header_row(2, 22)] + body_rows)
                  + para(run("After the table."))),
        page_case("E-table-header-repeat-single-tall",
                  "Whether Word repeats a single header row (w:tblHeader) taller than a page, "
                  "a cell of 60 lines, followed by 10 body rows and the end of the document",
                  "row 1 is a header row of 60 lines; 10 body rows follow",
                  table_of([header_row(1, 60)] + body_rows)),
    ]


def exact_height_valign() -> Doc:
    def table_of(rule: str, tc_pr: str) -> str:
        return rule_table(
            [row_cells(["r1c1", "r1c2", "r1c3"]),
             ([cell(para(run("Short")), tc_pr), cell(para(run("r2c2"))),
               cell(para(run("r2c3")))], f'<w:trHeight w:val="800" w:hRule="{rule}"/>'),
             row_cells(["r3c1", "r3c2", "r3c3"])],
            EVEN_GRID, tbl_w=(TEXT_W, "dxa"), fixed=True, border_eighths=12,
        )

    cases = [
        ("row 2 is exactly 40 pt high and holds one line, at the top (w:vAlign top)",
         table_of("exact", '<w:vAlign w:val="top"/>')),
        ("the same, centred (w:vAlign center)", table_of("exact", '<w:vAlign w:val="center"/>')),
        ("the same, at the bottom (w:vAlign bottom)",
         table_of("exact", '<w:vAlign w:val="bottom"/>')),
        ("row 2 is at least 40 pt high (w:hRule atLeast) and holds one line, centred",
         table_of("atLeast", '<w:vAlign w:val="center"/>')),
    ]
    body = "".join(table_case(n, label, xml) for n, (label, xml) in enumerate(cases, 1))
    return Doc(
        "E-table-row-exact-valign",
        "Where Word puts a line in a row of exact height (w:trHeight, w:hRule exact) taller "
        "than it, at each vertical alignment (top, centre, bottom), and in a row of at least "
        "that height for contrast",
        body + para(run("End of the document.")),
    )


def nested_empty_paragraph_14() -> Doc:
    inner = rule_table([[cell(para(run(f"in{r}{c}"))) for c in "ab"] for r in (1, 2)],
                       [1500, 1500], tbl_w=(3000, "dxa"), fixed=True)

    def outer(after: str) -> str:
        return rule_table([[cell(para(run("Outer a")) + inner + after),
                            cell(para(run("Outer b")))]],
                          [4513, 4513], tbl_w=(TEXT_W, "dxa"), fixed=True)

    cases = [
        ("the first cell holds a line, a nested table and an empty paragraph", outer(para())),
        ("the same, with a paragraph of text after the nested table",
         outer(para(run("After the nested table")))),
    ]
    body = "".join(table_case(n, label, xml) for n, (label, xml) in enumerate(cases, 1))
    return Doc(
        "E-table-nested-empty-para-14",
        "Whether the empty paragraph that ends a cell after a nested table takes height in "
        "compatibility mode 14 (in mode 15 it takes none): a nested table followed by an "
        "empty paragraph, and by a paragraph of text for contrast",
        body + para(run("End of the document.")),
        mode=14,
    )


def shaded_lines(count: int, height: float, band: int) -> str:
    """count empty paragraphs of an exact height, shaded in bands of band paragraphs that
    alternate between grey and dark grey, so that a raster shows how many Word drew."""
    return "".join(
        para(space=spacing(line=("exact", height)),
             shading=GREY if (n // band) % 2 == 0 else "808080")
        for n in range(count)
    )


# Letter and A5, in twips.
LETTER = (12240, 15840)
A5 = (8391, 11906)


def header_cut() -> Doc:
    # E-header-no-room and E-header-taller-than-page: Word draws 28 lines of 12 pt of a header
    # taller than the room for it on A4, with the header 35.4 pt from the edge. The cut may
    # fall after 28 lines, or at a height (about 336 pt, or 0.4 of the page), or at a
    # distance from the edge; each section changes one of those.
    header_lines = page_para(lines([f"header line {n}" for n in range(1, 41)]))
    cases = [
        ("a header of 400 empty lines of exact 1 pt, shaded in bands of 20, on A4",
         "headerReference", shaded_lines(400, 1, 20), {}),
        ("a header of 200 empty lines of exact 2 pt, shaded in bands of 10, on A4",
         "headerReference", shaded_lines(200, 2, 10), {}),
        ("a header of 40 lines of 12 pt, 72 pt from the edge, on A4",
         "headerReference", header_lines, {"header_distance": 1440}),
        ("a header of 40 lines of 12 pt on a Letter page", "headerReference", header_lines,
         {"size": LETTER}),
        ("a header of 40 lines of 12 pt on an A5 page", "headerReference", header_lines,
         {"size": A5}),
        ("a footer of 200 empty lines of exact 2 pt, shaded in bands of 10, on A4",
         "footerReference", shaded_lines(200, 2, 10), {}),
    ]
    # A section without a header of its own shows the previous section's, so the footer's
    # case gets an empty header (E-section-restart-parity shows Word doing so).
    parts, sections = [], []
    for n, (label, element, content, options) in enumerate(cases, 1):
        kind = "header" if element == "headerReference" else "footer"
        name = f"{kind}{n}.xml"
        parts.append(header_part(name, content))
        refs = ((element, "default", name),)
        if kind == "footer":
            parts.append(header_part(f"header{n}.xml", page_para()))
            refs = (("headerReference", "default", f"header{n}.xml"),) + refs
        sections.append(([run(f"Case {n:02d}: {label}")], section(refs=refs, **options)))
    return Doc(
        "E-header-cut",
        "Where Word cuts a header or footer taller than the room for it: lines of exact 1 and "
        "2 pt, a header 72 pt from the edge, a Letter and an A5 page, and a footer under an "
        "empty header; each case is a section of its own",
        sectioned(sections),
        headers=tuple(parts),
        last_section=sections[-1][1],
    )


def para_shading_borders_sides() -> Doc:
    # E-para-shading-borders: with a box border the fill reaches the lines; with a left
    # border only it keeps its border-less extent; the space between two shaded paragraphs
    # is shaded.
    text = "shaded text that runs over several lines of one paragraph, " * 5
    apart = page_para(run("not shaded"))
    after_12 = spacing(after=12, line=("exact", 12))
    before_12 = spacing(before=12, line=("exact", 12))
    body = "".join([
        page_para(run("Case 01: left and right borders, 4 pt from the text: ") + run(text),
                  shading=SHADE, borders=paragraph_borders(4, ("left", "right"))) + apart,
        page_para(run("Case 02: top and bottom borders, 4 pt from the text: ") + run(text),
                  shading=SHADE, borders=paragraph_borders(4, ("top", "bottom"))) + apart,
        page_para(run("Case 03: a left border only, 0 pt from the text: ") + run(text),
                  shading=SHADE, borders=paragraph_borders(0, ("left",))) + apart,
        page_para(run("Case 04: a shaded paragraph with 12 pt after: ") + run(text),
                  shading=SHADE, space=after_12),
        page_para(run("Case 04: then an unshaded one with 12 pt before"), space=before_12)
        + apart,
        page_para(run("Case 05: an unshaded paragraph with 12 pt after"), space=after_12),
        page_para(run("Case 05: then a shaded one with 12 pt before: ") + run(text),
                  shading=SHADE, space=before_12),
    ])
    return Doc(
        "E-para-shading-borders-sides",
        "Paragraph shading with left and right borders only, top and bottom borders only, "
        "and a left border 0 pt from the text, and the space between a shaded and an "
        "unshaded paragraph, each way round: how far the fill reaches",
        body,
    )


# Page numbers whose letters repeat 20, 30, 31, 40, 50 and 64 times (520, 780, 781, 1040,
# 1300, 1664), between 53, which Word shows as letters, and 1665, which it does not; and
# Roman numerals past 4999.
LETTER_STARTS = [520, 780, 781, 1040, 1300, 1664, 5000, 10000]


def number_formats_letters() -> Doc:
    sections = []
    for n, start in enumerate(LETTER_STARTS, 1):
        contents = [run(f"Case {n:02d}: page numbers restarted at {start}")]
        contents += [run(f"PAGE \\* {switch}: ") + field(f"PAGE \\* {switch}")
                     for switch in ("Arabic", "ALPHABETIC", "alphabetic", "ROMAN", "roman")]
        sections.append((contents, section(page_start=start)))
    return Doc(
        "E-number-formats-letters",
        "PAGE fields as letters and Roman numerals at page numbers 520, 780, 781, 1040, 1300 "
        "and 1664, whose letters repeat 20 to 64 times, and at 5000 and 10000, past 4999",
        sectioned(sections),
        last_section=sections[-1][1],
    )


# Every half point from 8 to 14 pt.
SCRIPT_STEPS = [8 + k / 2 for k in range(13)]


def superscript_steps() -> Doc:
    # E-superscript-sizes: the rise and the drop fit no simple formula at 8 to 48 pt.
    body, n = "", 0
    for font in ("Calibri", "Liberation Sans"):
        fonts = (f'<w:rFonts w:ascii="{font}" w:hAnsi="{font}" w:eastAsia="{font}" '
                 f'w:cs="{font}"/>')
        for size in SCRIPT_STEPS:
            n += 1
            hp = half_points(size)
            props = fonts + f'<w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/>'
            body += labelled(
                n, f"{font} {size:g} pt, superscript then subscript",
                styled_run("Base", props),
                styled_run("123", props + '<w:vertAlign w:val="superscript"/>'),
                styled_run(" Base", props),
                styled_run("456", props + '<w:vertAlign w:val="subscript"/>'))
    return Doc(
        "E-superscript-steps",
        "The size and the baseline shift of superscript and subscript text at every half "
        "point from 8 to 14 pt, in Calibri and in Liberation Sans",
        body,
    )


def table_float_center_margins() -> Doc:
    # E-table-float-align: a table centred by w:tblpXSpec, horizontal anchor left out, sat at
    # the centre of the page, which equal margins could not tell from the centre between
    # them. Here the left margin is 144 pt and the right one 36 pt; each table is placed
    # vertically from the page, so that none covers another.
    body = ""
    for n, (label, anchor, y) in enumerate([
        ("w:tblpXSpec center, no horizontal anchor", "", 2000),
        ("w:tblpXSpec center, w:horzAnchor margin", ' w:horzAnchor="margin"', 4400),
        ("w:tblpXSpec center, w:horzAnchor page", ' w:horzAnchor="page"', 6800),
        ("w:tblpXSpec center, w:horzAnchor text", ' w:horzAnchor="text"', 9200),
    ], 1):
        spec = f'w:vertAnchor="page"{anchor} w:tblpXSpec="center" w:tblpY="{y}"'
        body += para(run(f"Case {n:02d}: a table with {label}, {y // 20} pt from the top of "
                         "the page"))
        body += floating_table(spec) + para()
    last = section(sides=(2880, 720))
    return Doc(
        "E-table-float-center-margins",
        "Floating tables centred by w:tblpXSpec on a page whose left margin is 144 pt and "
        "right margin 36 pt, with no horizontal anchor and anchored to the margin, the page "
        "and the text: whether Word centres them between the margins or on the page",
        body,
        last_section=last,
    )


def table_style_size_pair(defaults: int, normal: int) -> Doc:
    # E-table-style-defaults-size-14 and E-table-style-size-*-14 left two rules for when
    # Normal's size overrides a table style's in mode 14: its 11 or 12 pt is ignored unless
    # smaller than the defaults, or its size is ignored when it equals the defaults or is
    # 12 pt. Defaults 13 with Normal 12 (the first keeps 12, the second 14) and defaults 10
    # with Normal 11 (the first keeps 14, the second 11) tell them apart.
    big = table_style(
        "Big", [],
        whole='<w:pPr><w:jc w:val="center"/></w:pPr>'
        '<w:rPr><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr>',
    )
    hp = half_points(normal)
    body = table_case(
        1, f"a table in the style Big (14 pt, centred); document defaults {defaults} pt, "
        f"Normal {normal} pt and left-aligned",
        styled_table("Big", named_cells(2, 2), widths=[TEXT_W // 2] * 2),
    ) + para(run("Case 02: a paragraph after the table, in Normal"))
    return Doc(
        f"E-table-style-defaults-size-{defaults}-{normal}-14",
        f"Whether a Normal style of {normal} pt overrides the 14 pt of a table style when the "
        f"document defaults are {defaults} pt, in compatibility mode 14",
        body,
        styles=big,
        normal_props='<w:pPr><w:jc w:val="left"/></w:pPr>'
        f'<w:rPr><w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/></w:rPr>',
        run_default_size=defaults,
        mode=14,
    )


# Spaces after five underlined words in Calibri 11 pt, and where they end, for a paragraph
# indented 72 pt from the right margin (2.49 pt a space; 75 end 11 pt before the margin).
TRAILING_SPACES_INDENT = [
    (46, "11 pt before the right indent"),
    (65, "36 pt past the right indent, 36 pt before the margin"),
    (84, "11 pt past the right margin"),
    (120, "101 pt past the right margin, 29 pt past the edge of the page"),
]


def underline_trailspace_indent() -> Doc:
    # E-underline-trailspace-margin: with ulTrailSpace, the underline of trailing spaces on a
    # left-aligned line stops at the right margin. Here the paragraph has a right indent of
    # 72 pt: does it stop at the indent or at the margin?
    single = underline_props("single")
    body = ""
    for n, (count, where) in enumerate(TRAILING_SPACES_INDENT, 1):
        body += para(run(f"Case {n:02d}: right indent 72 pt; 5 underlined words, {count} "
                         f"spaces and an underlined word; the spaces end {where}:"))
        body += para(styled_run(" ".join(["underlined"] * 5) + " " * count + "underlined",
                                single), ind={"right": 1440})
    return Doc(
        "E-underline-trailspace-indent",
        "Underlined trailing spaces in paragraphs with a right indent of 72 pt, ending before "
        "the indent, between it and the margin, past the margin and past the edge of the "
        "page, with the w:compat setting ulTrailSpace: whether the underline stops at the "
        "indent or at the margin",
        body,
        compat_flags=("ulTrailSpace",),
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
        table_style_look(),
        table_style_bands(),
        table_style_bands_5(),
        *(
            table_style_size(size, setting, mode)
            for size in (11, 12, 13)
            for setting, mode in ((1, 15), (None, 15), (None, 14))
        ),
        table_style_not_allowed(),
        table_style_toggles(),
        table_style_toggles_defaults(),
        table_style_header_rows(1),
        table_style_header_rows(0),
        spacing_condensed(),
        tab_margin(),
        highlight(),
        underline(),
        underline_trail_space(),
        strike(),
        tab_leader(),
        bar_tab(),
        auto_color(),
        superscript(),
        section_odd(),
        section_odd_first(),
        section_continuous_headers(),
        section_next_column(),
        keeplines_long(),
        keepnext_chain(),
        page_break_before_top(),
        para_shading(),
        header_tall(),
        header_space_after(),
        header_negative_margin(),
        section_restart_parity(),
        section_continuous_title(),
        keepnext_break(),
        header_no_room(),
        header_taller_than_page(),
        header_space_before(),
        footer_tall_bottom(),
        para_shading_borders(),
        number_formats(),
        bold_missing(),
        soft_hyphen_font(),
        underline_24(),
        superscript_sizes(),
        auto_color_threshold(),
        highlight_tall(),
        underline_trailspace_margin(),
        tab_margin_alone(15),
        tab_margin_alone(14),
        tab_margin_alone(12),
        list_label_height(),
        table_style_size_defaults(15),
        table_style_size_defaults(14),
        table_style_size_defaults(14, "right"),
        table_style_rows_cols(),
        table_style_header_rows_3(1),
        table_style_header_rows_3(0),
        table_style_header_rows_3(None, 14),
        table_style_header_rows_format(),
        table_style_two_defaults(),
        table_cell_margins(),
        table_cell_margins_nil(),
        table_float_align(),
        bold_italic_missing(),
        list_level_linked_no_level(),
        list_level_linked_defaults(),
        table_autofit(),
        table_autofit_14(None),
        table_autofit_14("doNotAutofitConstrainedTables"),
        table_autofit_14("growAutofit"),
        *rows_across_pages(),
        exact_height_overflow(),
        vml_pictures(),
        table_split_borders(),
        table_autofit_shares(),
        table_autofit_beside(15),
        table_autofit_beside(14),
        table_autofit_beside(14, "doNotAutofitConstrainedTables"),
        table_autofit_grow_15(),
        *header_repeat_limits(),
        exact_height_valign(),
        nested_empty_paragraph_14(),
        header_cut(),
        para_shading_borders_sides(),
        number_formats_letters(),
        superscript_steps(),
        table_float_center_margins(),
        table_style_size_pair(13, 12),
        table_style_size_pair(10, 11),
        underline_trailspace_indent(),
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
            f'<w:sz w:val="{half_points(doc.run_default_size)}"/>'
            f'<w:szCs w:val="{half_points(doc.run_default_size)}"/>'
            '<w:lang w:val="en-US" w:eastAsia="en-US" w:bidi="ar-SA"/>'
            "</w:rPr></w:rPrDefault>"
        )
    normal_default = ' w:default="1"' if doc.normal_default else ""
    return (
        f'<w:styles xmlns:w="{W_NS}">'
        "<w:docDefaults>"
        f"{run_defaults}"
        f"<w:pPrDefault><w:pPr><w:widowControl/>{doc.paragraph_defaults_list}"
        '<w:spacing w:after="0" w:line="240" w:lineRule="auto"/>'
        f"{doc.paragraph_defaults_extra}</w:pPr></w:pPrDefault>"
        "</w:docDefaults>"
        f"{doc.styles_before_normal}"
        f'<w:style w:type="paragraph"{normal_default} w:styleId="Normal">'
        f'<w:name w:val="Normal"/><w:qFormat/>{doc.normal_props}</w:style>'
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


def relationships(rels: list[tuple[str, ...]]) -> str:
    """(type, target) numbered rId1, rId2 ..., or (type, target, id)."""
    entries = "".join(
        f'<Relationship Id="{rel[2] if len(rel) > 2 else f"rId{i}"}" Type="{REL}/{rel[0]}"'
        f' Target="{rel[1]}"/>'
        for i, rel in enumerate(rels, 1)
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
    if "<v:" in doc.body:
        namespaces += f' xmlns:v="{V_NS}" xmlns:o="{O_NS}" xmlns:w10="{W10_NS}"'
    document = (
        f"<w:document {namespaces}>"
        f"<w:body>{doc.body}{doc.last_section or sect_pr()}</w:body></w:document>"
    )
    numbering = None
    if doc.numbering:
        numbering = f'<w:numbering xmlns:w="{W_NS}">{doc.numbering}</w:numbering>'

    used = document + styles_xml(doc) + (numbering or "") + "".join(x for _, x in doc.headers)
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
    for name, _ in doc.headers:
        kind = "header" if name.startswith("header") else "footer"
        word_rels.append((kind, name, part_id(name)))
        overrides.append((f"/word/{name}", f"{CT_W}.{kind}+xml"))
    for name, _ in doc.media:
        word_rels.append(("image", f"media/{name}", media_id(name)))
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
        + ('<Default Extension="png" ContentType="image/png"/>' if doc.media else "")
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
    for name, root in doc.headers:
        result.append((f"word/{name}", xml_part(root)))
    result.append(("word/_rels/document.xml.rels", xml_part(relationships(word_rels))))
    xml_parts = list(result)
    result += [(f"word/media/{name}", data) for name, data in doc.media]
    for name, data in xml_parts:
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
