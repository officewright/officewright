#!/usr/bin/env python3
"""Writes the generated business documents of tests/corpus/generated/.

    make-business-docs.py [--check | --list]

CVs and invoices in English, and a contract, an invoice and a letter or memo each in
Ukrainian, Russian, German and Chinese. Every person, company, address, phone number and
bank account in them is invented. Each document except the Chinese ones also comes as an
open-font twin (name suffix "-open"), the same document with Carlito for Calibri and
Liberation Sans, Serif and Mono for Arial, Times New Roman and Courier New: the
metric-compatible pairs of CLAUDE.md, which CI has in tests/fonts, so the twin's Word
reference can be compared on CI with the very fonts Word used.

The packages are stored ZIPs with fixed dates, so the bytes are the same on every platform;
--check fails when a committed file differs from what this script writes or has no
SOURCES.md row, and --list prints the rows. Python standard library only.
"""

from __future__ import annotations

import argparse
import json
import re
import struct
import sys
import zipfile
import zlib
from dataclasses import dataclass, field
from io import BytesIO
from pathlib import Path
from xml.dom import minidom
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "tests" / "corpus" / "generated"
SOURCES = ROOT / "tests" / "corpus" / "SOURCES.md"
GERMAN_TEXTS = ROOT / "tests" / "corpus" / "generated-texts" / "de.json"

W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R_NS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
WP_NS = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
A_NS = "http://schemas.openxmlformats.org/drawingml/2006/main"
PIC_NS = "http://schemas.openxmlformats.org/drawingml/2006/picture"
WORD_URI = "http://schemas.microsoft.com/office/word"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
CT_W = "application/vnd.openxmlformats-officedocument.wordprocessingml"
NAMESPACES = (
    f'xmlns:w="{W_NS}" xmlns:r="{R_NS}" xmlns:wp="{WP_NS}" xmlns:a="{A_NS}" xmlns:pic="{PIC_NS}"'
)

# The metric-compatible pairs of CLAUDE.md: what an open-font twin uses instead.
OPEN_FONTS = {
    "Calibri": "Carlito",
    "Arial": "Liberation Sans",
    "Times New Roman": "Liberation Serif",
    "Courier New": "Liberation Mono",
}

LETTER = (12240, 15840)
A4 = (11906, 16838)
EMU_PER_TWIP = 635


def attr(value: object) -> str:
    return escape(str(value), {'"': "&quot;"})


def twips(points: float) -> int:
    return round(points * 20)


def cm(value: float) -> int:
    return round(value / 2.54 * 1440)


# --- runs and paragraphs ------------------------------------------------------------------


def rpr(
    bold: bool = False,
    italic: bool = False,
    size: float | None = None,
    color: str | None = None,
    font: str | None = None,
    caps: bool = False,
    small_caps: bool = False,
    spacing: float | None = None,
    underline: bool = False,
    style: str | None = None,
    superscript: bool = False,
    east: str | None = None,
) -> str:
    out = ""
    if style:
        out += f'<w:rStyle w:val="{style}"/>'
    if font or east:
        fonts = (
            f' w:ascii="{attr(font)}" w:hAnsi="{attr(font)}" w:cs="{attr(font)}"' if font else ""
        )
        if east:
            fonts += f' w:eastAsia="{attr(east)}"'
        out += f"<w:rFonts{fonts}/>"
    if bold:
        out += "<w:b/><w:bCs/>"
    if italic:
        out += "<w:i/><w:iCs/>"
    if caps:
        out += "<w:caps/>"
    if small_caps:
        out += "<w:smallCaps/>"
    if color:
        out += f'<w:color w:val="{color}"/>'
    if spacing is not None:
        out += f'<w:spacing w:val="{twips(spacing)}"/>'
    if size is not None:
        out += f'<w:sz w:val="{round(size * 2)}"/><w:szCs w:val="{round(size * 2)}"/>'
    if underline:
        out += '<w:u w:val="single"/>'
    if superscript:
        out += '<w:vertAlign w:val="superscript"/>'
    return f"<w:rPr>{out}</w:rPr>" if out else ""


def r(text: str, **props) -> str:
    """A run; tabs ("\\t") and line breaks ("\\n") in the text become w:tab and w:br."""
    pieces = []
    for index, chunk in enumerate(re.split(r"(\t|\n)", text)):
        if chunk == "\t":
            pieces.append("<w:tab/>")
        elif chunk == "\n":
            pieces.append("<w:br/>")
        elif chunk:
            pieces.append(f'<w:t xml:space="preserve">{escape(chunk)}</w:t>')
    return f"<w:r>{rpr(**props)}{''.join(pieces)}</w:r>"


def fld(instruction: str, result: str, **props) -> str:
    """A complex field with its cached result, as Word writes PAGE and NUMPAGES."""
    style = rpr(**props)
    return (
        f'<w:r>{style}<w:fldChar w:fldCharType="begin"/></w:r>'
        f'<w:r>{style}<w:instrText xml:space="preserve"> {instruction} </w:instrText></w:r>'
        f'<w:r>{style}<w:fldChar w:fldCharType="separate"/></w:r>'
        f"<w:r>{style}<w:t>{escape(result)}</w:t></w:r>"
        f'<w:r>{style}<w:fldChar w:fldCharType="end"/></w:r>'
    )


def page_of(template: str, **props) -> str:
    """'Page {page} of {pages}' with PAGE and NUMPAGES fields."""
    out = ""
    for piece in re.split(r"(\{page\}|\{pages\})", template):
        if piece == "{page}":
            out += fld("PAGE", "1", **props)
        elif piece == "{pages}":
            out += fld("NUMPAGES", "1", **props)
        elif piece:
            out += r(piece, **props)
    return out


@dataclass(frozen=True)
class Tab:
    position: int  # twips
    kind: str = "left"  # left, right, center, decimal
    leader: str | None = None  # dot, hyphen, underscore


def p(
    *content: str,
    style: str | None = None,
    align: str | None = None,
    before: float | None = None,
    after: float | None = None,
    line: float | None = None,
    rule: str = "auto",
    left: int | None = None,
    right: int | None = None,
    first: int | None = None,
    hanging: int | None = None,
    first_chars: int | None = None,
    tabs: tuple[Tab, ...] = (),
    numbering: tuple[int, int] | None = None,
    keep_next: bool = False,
    keep_lines: bool = False,
    page_break_before: bool = False,
    border_bottom: str | None = None,
    shading: str | None = None,
    run_props: str = "",
) -> str:
    """A paragraph. line is in points for exact/atLeast and in lines for auto."""
    props = ""
    if style:
        props += f'<w:pStyle w:val="{style}"/>'
    if keep_next:
        props += "<w:keepNext/>"
    if keep_lines:
        props += "<w:keepLines/>"
    if page_break_before:
        props += "<w:pageBreakBefore/>"
    if numbering:
        props += (
            f'<w:numPr><w:ilvl w:val="{numbering[1]}"/><w:numId w:val="{numbering[0]}"/></w:numPr>'
        )
    if border_bottom:
        props += (
            '<w:pBdr><w:bottom w:val="single" w:sz="6" w:space="1"'
            f' w:color="{border_bottom}"/></w:pBdr>'
        )
    if shading:
        props += f'<w:shd w:val="clear" w:color="auto" w:fill="{shading}"/>'
    if tabs:
        props += (
            "<w:tabs>"
            + "".join(
                f'<w:tab w:val="{t.kind}"'
                + (f' w:leader="{t.leader}"' if t.leader else "")
                + f' w:pos="{t.position}"/>'
                for t in tabs
            )
            + "</w:tabs>"
        )
    spacing = ""
    if before is not None:
        spacing += f' w:before="{twips(before)}"'
    if after is not None:
        spacing += f' w:after="{twips(after)}"'
    if line is not None:
        value = round(line * 240) if rule == "auto" else twips(line)
        spacing += f' w:line="{value}" w:lineRule="{rule}"'
    if spacing:
        props += f"<w:spacing{spacing}/>"
    indent = ""
    for name, value in (
        ("left", left),
        ("right", right),
        ("firstLine", first),
        ("hanging", hanging),
    ):
        if value is not None:
            indent += f' w:{name}="{value}"'
    if first_chars is not None:
        indent += f' w:firstLineChars="{first_chars}"'
    if indent:
        props += f"<w:ind{indent}/>"
    if align:
        props += f'<w:jc w:val="{align}"/>'
    if run_props:
        props += run_props
    return f"<w:p>{f'<w:pPr>{props}</w:pPr>' if props else ''}{''.join(content)}</w:p>"


def footnote_ref(number: int) -> str:
    return (
        '<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteReference'
        f' w:id="{number}"/></w:r>'
    )


def hyperlink(rid: str, text: str) -> str:
    return f'<w:hyperlink r:id="{rid}" w:history="1">{r(text, style="Hyperlink")}</w:hyperlink>'


def picture(rid: str, number: int, name: str, width_cm: float, height_cm: float) -> str:
    """An inline picture of the given size."""
    cx, cy = round(width_cm * 360000), round(height_cm * 360000)
    return (
        "<w:r><w:drawing>"
        f'<wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/>'
        '<wp:effectExtent l="0" t="0" r="0" b="0"/>'
        f'<wp:docPr id="{number}" name="{attr(name)}"/>'
        '<wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr>'
        f'<a:graphic><a:graphicData uri="{PIC_NS}"><pic:pic>'
        f'<pic:nvPicPr><pic:cNvPr id="{number}" name="{attr(name)}"/><pic:cNvPicPr/></pic:nvPicPr>'
        "<pic:blipFill><a:blip"
        f' r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
        f'<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr>'
        "</pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
    )


# --- tables -------------------------------------------------------------------------------


@dataclass(frozen=True)
class Cell:
    content: str  # paragraphs
    span: int = 1
    merge: str | None = None  # "restart" or "continue" (vertical merge)
    fill: str | None = None
    valign: str | None = None
    borders: str | None = None  # override: "none" or "bottom"


def cell(*paragraphs: str, **kwargs) -> Cell:
    return Cell("".join(paragraphs) or p(), **kwargs)


def table(
    rows: list[list[Cell]],
    widths: list[int],
    borders: str = "single",
    layout: str = "fixed",
    header_rows: int = 0,
    cant_split: bool = False,
    margins: tuple[int, int] = (108, 108),
    align: str | None = None,
    indent: int | None = None,
    row_heights: dict[int, tuple[int, str]] | None = None,
    border_color: str = "000000",
) -> str:
    props = f'<w:tblW w:w="{sum(widths)}" w:type="dxa"/>'
    if align:
        props += f'<w:jc w:val="{align}"/>'
    if indent is not None:
        props += f'<w:tblInd w:w="{indent}" w:type="dxa"/>'
    if borders == "single":
        sides = "".join(
            f'<w:{side} w:val="single" w:sz="4" w:space="0" w:color="{border_color}"/>'
            for side in ("top", "left", "bottom", "right", "insideH", "insideV")
        )
        props += f"<w:tblBorders>{sides}</w:tblBorders>"
    elif borders == "horizontal":
        sides = "".join(
            f'<w:{side} w:val="single" w:sz="4" w:space="0" w:color="{border_color}"/>'
            for side in ("top", "bottom", "insideH")
        )
        props += f"<w:tblBorders>{sides}</w:tblBorders>"
    props += f'<w:tblLayout w:type="{layout}"/>'
    props += (
        f'<w:tblCellMar><w:left w:w="{margins[0]}" w:type="dxa"/>'
        f'<w:right w:w="{margins[1]}" w:type="dxa"/></w:tblCellMar>'
    )
    props += (
        '<w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0"'
        ' w:noHBand="0" w:noVBand="1"/>'
    )
    grid = "".join(f'<w:gridCol w:w="{w}"/>' for w in widths)
    body = ""
    for index, row in enumerate(rows):
        row_props = ""
        if cant_split:
            row_props += "<w:cantSplit/>"
        if row_heights and index in row_heights:
            height, rule = row_heights[index]
            row_props += f'<w:trHeight w:val="{height}" w:hRule="{rule}"/>'
        if index < header_rows:
            row_props += "<w:tblHeader/>"
        cells = ""
        column = 0
        for c in row:
            width = sum(widths[column : column + c.span])
            column += c.span
            cell_props = f'<w:tcW w:w="{width}" w:type="dxa"/>'
            if c.span > 1:
                cell_props += f'<w:gridSpan w:val="{c.span}"/>'
            if c.merge:
                cell_props += (
                    f'<w:vMerge w:val="{c.merge}"/>' if c.merge == "restart" else "<w:vMerge/>"
                )
            if c.borders == "none":
                cell_props += (
                    "<w:tcBorders>"
                    + "".join(f'<w:{s} w:val="nil"/>' for s in ("top", "left", "bottom", "right"))
                    + "</w:tcBorders>"
                )
            elif c.borders == "bottom":
                cell_props += (
                    '<w:tcBorders><w:bottom w:val="single" w:sz="8" w:space="0" '
                    f'w:color="{border_color}"/></w:tcBorders>'
                )
            if c.fill:
                cell_props += f'<w:shd w:val="clear" w:color="auto" w:fill="{c.fill}"/>'
            if c.valign:
                cell_props += f'<w:vAlign w:val="{c.valign}"/>'
            cells += f"<w:tc><w:tcPr>{cell_props}</w:tcPr>{c.content}</w:tc>"
        body += f"<w:tr>{f'<w:trPr>{row_props}</w:trPr>' if row_props else ''}{cells}</w:tr>"
    return f"<w:tbl><w:tblPr>{props}</w:tblPr><w:tblGrid>{grid}</w:tblGrid>{body}</w:tbl>"


# --- numbering ----------------------------------------------------------------------------


def level(
    index: int,
    fmt: str,
    text: str,
    left: int,
    hanging: int,
    bold: bool = False,
    font: str | None = None,
    suffix: str = "tab",
    legal: bool = False,
) -> str:
    run = ""
    if font or bold:
        run = (
            "<w:rPr>"
            + (
                f'<w:rFonts w:ascii="{attr(font)}" w:hAnsi="{attr(font)}" w:hint="default"/>'
                if font
                else ""
            )
            + ("<w:b/><w:bCs/>" if bold else "")
            + "</w:rPr>"
        )
    return (
        f'<w:lvl w:ilvl="{index}"><w:start w:val="1"/><w:numFmt w:val="{fmt}"/>'
        + ("<w:isLgl/>" if legal else "")
        + (f'<w:suff w:val="{suffix}"/>' if suffix != "tab" else "")
        + f'<w:lvlText w:val="{attr(text)}"/><w:lvlJc w:val="left"/>'
        f'<w:pPr><w:ind w:left="{left}" w:hanging="{hanging}"/></w:pPr>{run}</w:lvl>'
    )


def numbering_xml(definitions: list[tuple[int, str, list[str]]]) -> str:
    """abstractNum and num elements: (id, nsid, levels) each, with num id = abstract id."""
    abstract = "".join(
        f'<w:abstractNum w:abstractNumId="{number}"><w:nsid w:val="{nsid}"/>'
        f'<w:multiLevelType w:val="hybridMultilevel"/>{"".join(levels)}</w:abstractNum>'
        for number, nsid, levels in definitions
    )
    nums = "".join(
        f'<w:num w:numId="{number}"><w:abstractNumId w:val="{number}"/></w:num>'
        for number, _, _ in definitions
    )
    return abstract + nums


BULLET = 1
DECIMAL = 2
SECTIONS = 3  # 1. / 1.1. / 1.1.1.
BRACKETED = 4  # [1]

STANDARD_NUMBERING = [
    (BULLET, "4A3B2C11", [level(0, "bullet", "•", 360, 360), level(1, "bullet", "–", 720, 360)]),
    (DECIMAL, "4A3B2C12", [level(0, "decimal", "%1.", 360, 360)]),
    (
        SECTIONS,
        "4A3B2C13",
        [
            level(0, "decimal", "%1.", 360, 360, bold=True),
            level(1, "decimal", "%1.%2.", 567, 567),
            level(2, "decimal", "%1.%2.%3.", 1134, 567),
        ],
    ),
    (BRACKETED, "4A3B2C14", [level(0, "decimal", "[%1]", 567, 567)]),
]


# --- pictures -----------------------------------------------------------------------------


def png_rgb(width: int, height: int, pixel) -> bytes:
    """An RGB PNG drawn by pixel(x, y) -> (r, g, b)."""
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            rows += bytes(pixel(x, y))

    def chunk(kind: bytes, body: bytes) -> bytes:
        return (
            struct.pack(">I", len(body))
            + kind
            + body
            + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(rows), 9))
        + chunk(b"IEND", b"")
    )


def logo(colour: tuple[int, int, int]) -> bytes:
    """A made-up company mark: a coloured disc beside three bars, on white."""

    def pixel(x: int, y: int) -> tuple[int, int, int]:
        if (x - 40) ** 2 + (y - 40) ** 2 <= 30**2:
            return colour
        for bar in range(3):
            top = 18 + bar * 18
            if 84 <= x < 230 - bar * 40 and top <= y < top + 10:
                return (90, 90, 90)
        return (255, 255, 255)

    return png_rgb(240, 80, pixel)


def portrait() -> bytes:
    """A grey head-and-shoulders silhouette, standing in for a CV photo."""

    def pixel(x: int, y: int) -> tuple[int, int, int]:
        if (x - 60) ** 2 + (y - 55) ** 2 <= 28**2:
            return (150, 150, 150)
        if y >= 95 and (x - 60) ** 2 / 55**2 + (y - 150) ** 2 / 60**2 <= 1:
            return (150, 150, 150)
        return (225, 225, 225)

    return png_rgb(120, 150, pixel)


# --- documents ----------------------------------------------------------------------------


@dataclass
class Doc:
    name: str
    purpose: str
    lang: str
    body: str
    font: str
    size: float
    page: tuple[int, int] = LETTER
    margins: tuple[int, int, int, int] = (1440, 1440, 1440, 1440)  # top, right, bottom, left
    header: str = ""
    footer: str = ""
    first_header: str | None = None
    first_footer: str | None = None
    numbering: list = field(default_factory=lambda: STANDARD_NUMBERING)
    images: dict[str, bytes] = field(default_factory=dict)  # rId -> PNG, rIds "rIdImg1"...
    header_images: dict[str, bytes] = field(default_factory=dict)
    links: dict[str, str] = field(default_factory=dict)  # rId -> URL
    footnotes: list[str] = field(default_factory=list)  # paragraphs of footnotes 1, 2...
    east_asia: str | None = None
    heading_font: str | None = None
    decimal: str = "."
    list_separator: str = ","
    twin: bool = True


def heading_styles(doc: Doc) -> str:
    font = doc.heading_font or doc.font

    def style(
        style_id: str, name: str, size: float, before: float, after: float, outline: int
    ) -> str:
        return (
            f'<w:style w:type="paragraph" w:styleId="{style_id}"><w:name w:val="{name}"/><w:basedOn'
            ' w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority'
            ' w:val="9"/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:spacing'
            f' w:before="{twips(before)}" w:after="{twips(after)}"/><w:outlineLvl'
            f' w:val="{outline}"/></w:pPr><w:rPr><w:rFonts w:ascii="{attr(font)}"'
            f' w:hAnsi="{attr(font)}" w:cs="{attr(font)}"/><w:b/><w:bCs/><w:sz'
            f' w:val="{round(size * 2)}"/><w:szCs w:val="{round(size * 2)}"/></w:rPr></w:style>'
        )

    return (
        style("Heading1", "heading 1", doc.size + 3, 12, 4, 0)
        + style("Heading2", "heading 2", doc.size + 1, 8, 2, 1)
        + style("Title", "Title", doc.size + 12, 0, 6, 0).replace('<w:outlineLvl w:val="0"/>', "")
    )


def styles_xml(doc: Doc) -> str:
    east = doc.east_asia or doc.font
    half = round(doc.size * 2)
    lang = "en-US" if doc.east_asia else doc.lang
    east_lang = doc.lang if doc.east_asia else "en-US"
    hidden = '<w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/>'
    return (
        f'<w:styles xmlns:w="{W_NS}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts'
        f' w:ascii="{attr(doc.font)}" w:hAnsi="{attr(doc.font)}" w:eastAsia="{attr(east)}"'
        f' w:cs="{attr(doc.font)}"/><w:sz w:val="{half}"/><w:szCs w:val="{half}"/><w:lang'
        f' w:val="{lang}" w:eastAsia="{east_lang}"'
        ' w:bidi="ar-SA"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="0"'
        ' w:line="240" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style'
        ' w:type="paragraph" w:default="1" w:styleId="Normal"><w:name'
        ' w:val="Normal"/><w:qFormat/></w:style>'
        + heading_styles(doc)
        + '<w:style w:type="paragraph" w:styleId="Header"><w:name w:val="header"/><w:basedOn'
        f' w:val="Normal"/>{hidden}<w:pPr><w:tabs><w:tab w:val="center" w:pos="4680"/><w:tab'
        ' w:val="right" w:pos="9360"/></w:tabs></w:pPr></w:style><w:style w:type="paragraph"'
        ' w:styleId="Footer"><w:name w:val="footer"/><w:basedOn'
        f' w:val="Normal"/>{hidden}<w:pPr><w:tabs><w:tab w:val="center" w:pos="4680"/><w:tab'
        ' w:val="right" w:pos="9360"/></w:tabs></w:pPr></w:style><w:style w:type="paragraph"'
        ' w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:basedOn'
        f' w:val="Normal"/>{hidden}<w:rPr><w:sz w:val="{max(half - 4, 14)}"/><w:szCs'
        f' w:val="{max(half - 4, 14)}"/></w:rPr></w:style><w:style w:type="character"'
        ' w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph'
        ' Font"/><w:uiPriority w:val="1"/><w:semiHidden/><w:unhideWhenUsed/></w:style><w:style'
        ' w:type="character" w:styleId="FootnoteReference"><w:name w:val="footnote'
        f' reference"/>{hidden}<w:rPr><w:vertAlign'
        ' w:val="superscript"/></w:rPr></w:style><w:style w:type="character"'
        f' w:styleId="Hyperlink"><w:name w:val="Hyperlink"/>{hidden}<w:rPr><w:color'
        ' w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style><w:style w:type="table"'
        ' w:default="1" w:styleId="TableNormal"><w:name w:val="Normal'
        f' Table"/>{hidden}<w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0"'
        ' w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right'
        ' w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style><w:style w:type="numbering"'
        ' w:default="1" w:styleId="NoList"><w:name w:val="No'
        f' List"/>{hidden}</w:style></w:styles>'
    )


# --- package parts ------------------------------------------------------------------------


def xml_part(root: str) -> bytes:
    return ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n' + root).encode()


def compat_setting(name: str, value: int) -> str:
    return f'<w:compatSetting w:name="{name}" w:uri="{WORD_URI}" w:val="{value}"/>'


def settings_xml(doc: Doc) -> str:
    compat = "".join(
        compat_setting(name, value)
        for name, value in (
            ("compatibilityMode", 15),
            ("overrideTableStyleFontSizeAndJustification", 1),
            ("enableOpenTypeFeatures", 1),
            ("doNotFlipMirrorIndents", 1),
            ("differentiateMultirowTableHeaders", 1),
        )
    )
    notes = (
        '<w:footnotePr><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr>'
        if doc.footnotes
        else ""
    )
    east = doc.lang if doc.east_asia else "en-US"
    spacing = "compressPunctuation" if doc.east_asia else "doNotCompress"
    return (
        f'<w:settings xmlns:w="{W_NS}">'
        '<w:zoom w:percent="100"/>'
        '<w:defaultTabStop w:val="720"/>'
        f'<w:characterSpacingControl w:val="{spacing}"/>'
        f"{notes}<w:compat>{compat}</w:compat>"
        f'<w:themeFontLang w:val="{"en-US" if doc.east_asia else doc.lang}" w:eastAsia="{east}"'
        ' w:bidi="ar-SA"/>'
        f'<w:decimalSymbol w:val="{doc.decimal}"/>'
        f'<w:listSeparator w:val="{doc.list_separator}"/>'
        "</w:settings>"
    )


def sect_pr(doc: Doc, refs: list[tuple[str, str, str]]) -> str:
    width, height = doc.page
    top, right, bottom, left = doc.margins
    references = "".join(
        f'<w:{kind}Reference w:type="{which}" r:id="{rid}"/>' for kind, which, rid in refs
    )
    title = "<w:titlePg/>" if doc.first_header is not None or doc.first_footer is not None else ""
    return (
        f"<w:sectPr>{references}"
        f'<w:pgSz w:w="{width}" w:h="{height}"/>'
        f'<w:pgMar w:top="{top}" w:right="{right}" w:bottom="{bottom}" w:left="{left}" '
        'w:header="708" w:footer="708" w:gutter="0"/>'
        f'<w:cols w:space="708"/>{title}<w:docGrid w:linePitch="360"/></w:sectPr>'
    )


def closed(content: str) -> str:
    """The content, with the empty paragraph Word adds after a story's last table (it adds
    one on opening when a story ends with a table, which the re-saved copy would show)."""
    return content + p() if not content or content.endswith("</w:tbl>") else content


def story(tag: str, content: str) -> str:
    return f"<w:{tag} {NAMESPACES}>{closed(content)}</w:{tag}>"


def footnotes_xml(notes: list[str]) -> str:
    separator = (
        '<w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240"'
        ' w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote'
        ' w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240"'
        ' w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>'
    )
    body = "".join(
        f'<w:footnote w:id="{number}"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>'
        '<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r>'
        f'{r(" " + text)}</w:p></w:footnote>'
        for number, text in enumerate(notes, 1)
    )
    return f"<w:footnotes {NAMESPACES}>{separator}{body}</w:footnotes>"


def core_xml(doc: Doc, twin: bool = False) -> str:
    return (
        '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/'
        'metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" '
        'xmlns:dcterms="http://purl.org/dc/terms/" '
        'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
        f"<dc:title>{escape(doc.name + ('-open' if twin else ''))}</dc:title>"
        f"<dc:description>{escape(doc.purpose)}</dc:description>"
        "<dc:creator>Officewright</dc:creator>"
        "<cp:lastModifiedBy>Officewright</cp:lastModifiedBy>"
        '<dcterms:created xsi:type="dcterms:W3CDTF">2026-10-13T00:00:00Z</dcterms:created>'
        '<dcterms:modified xsi:type="dcterms:W3CDTF">2026-10-13T00:00:00Z</dcterms:modified>'
        "</cp:coreProperties>"
    )


def relationships(rels: list[tuple[str, str, str, bool]]) -> str:
    """(id, type, target, external) each."""
    entries = "".join(
        f'<Relationship Id="{rid}" Type="{REL}/{kind}" Target="{attr(target)}"'
        + (' TargetMode="External"' if external else "")
        + "/>"
        for rid, kind, target, external in rels
    )
    ns = "http://schemas.openxmlformats.org/package/2006/relationships"
    return f'<Relationships xmlns="{ns}">{entries}</Relationships>'


def fonts_named(xml: str) -> set[str]:
    """The font names of the w:rFonts elements in the markup."""
    names = set()
    for element in re.findall(r"<w:rFonts\b[^>]*>", xml):
        names.update(re.findall(r'w:(?:ascii|hAnsi|eastAsia|cs)="([^"]+)"', element))
    return names


def font_table_xml(xml: str) -> str:
    names = fonts_named(xml)
    entries = "".join(f'<w:font w:name="{attr(name)}"/>' for name in sorted(names))
    return f'<w:fonts xmlns:w="{W_NS}">{entries}</w:fonts>'


def open_fonts(xml: str) -> str:
    """The same markup with the open metric-compatible fonts named instead."""
    pattern = "|".join(re.escape(name) for name in OPEN_FONTS)

    def swap(element: re.Match) -> str:
        return re.sub(
            rf'(w:(?:ascii|hAnsi|eastAsia|cs|name)=")({pattern})(")',
            lambda m: m.group(1) + OPEN_FONTS[m.group(2)] + m.group(3),
            element.group(0),
        )

    return re.sub(r"<w:(?:rFonts|font)\b[^>]*>", swap, xml)


def parts(doc: Doc, twin: bool = False) -> list[tuple[str, bytes]]:
    """The package parts of a document (or of its open-font twin), in stored order."""
    word_rels = [
        ("rId1", "styles", "styles.xml", False),
        ("rId2", "settings", "settings.xml", False),
        ("rId3", "fontTable", "fontTable.xml", False),
        ("rId4", "numbering", "numbering.xml", False),
    ]
    stories: list[tuple[str, str]] = []
    refs = []
    for kind, which, content in (
        ("header", "default", doc.header),
        ("header", "first", doc.first_header),
        ("footer", "default", doc.footer),
        ("footer", "first", doc.first_footer),
    ):
        if content is None or (which == "default" and not content):
            continue
        name = f"{kind}{sum(1 for n, _ in stories if n.startswith(kind)) + 1}.xml"
        rid = f"rId{len(word_rels) + 1}"
        word_rels.append((rid, kind, name, False))
        refs.append((kind, which, rid))
        stories.append((name, story("hdr" if kind == "header" else "ftr", content)))
    if doc.footnotes:
        word_rels.append((f"rId{len(word_rels) + 1}", "footnotes", "footnotes.xml", False))
    for rid in doc.images:
        word_rels.append((rid, "image", f"media/{rid.lower()}.png", False))
    for rid, url in doc.links.items():
        word_rels.append((rid, "hyperlink", url, True))
    document = (
        f"<w:document {NAMESPACES}><w:body>{closed(doc.body)}{sect_pr(doc, refs)}</w:body>"
        "</w:document>"
    )
    overrides = [
        ("/word/document.xml", f"{CT_W}.document.main+xml"),
        ("/word/styles.xml", f"{CT_W}.styles+xml"),
        ("/word/settings.xml", f"{CT_W}.settings+xml"),
        ("/word/fontTable.xml", f"{CT_W}.fontTable+xml"),
        ("/word/numbering.xml", f"{CT_W}.numbering+xml"),
        ("/docProps/core.xml", "application/vnd.openxmlformats-package.core-properties+xml"),
    ]
    for name, _ in stories:
        kind = "header" if name.startswith("header") else "footer"
        overrides.append((f"/word/{name}", f"{CT_W}.{kind}+xml"))
    if doc.footnotes:
        overrides.append(("/word/footnotes.xml", f"{CT_W}.footnotes+xml"))
    content_types = (
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default'
        ' Extension="png" ContentType="image/png"/><Default Extension="rels"'
        ' ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default'
        ' Extension="xml" ContentType="application/xml"/>'
        + "".join(f'<Override PartName="{n}" ContentType="{t}"/>' for n, t in overrides)
        + "</Types>"
    )
    root_rels = relationships(
        [
            ("rId1", "officeDocument", "word/document.xml", False),
        ]
    ).replace(
        "</Relationships>",
        '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/'
        'relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>',
    )
    texts = {
        "word/document.xml": document,
        "word/styles.xml": styles_xml(doc),
        "word/numbering.xml": (
            f'<w:numbering xmlns:w="{W_NS}">{numbering_xml(doc.numbering)}</w:numbering>'
        ),
        **{f"word/{name}": xml for name, xml in stories},
    }
    if doc.footnotes:
        texts["word/footnotes.xml"] = footnotes_xml(doc.footnotes)
    if twin:
        texts = {name: open_fonts(xml) for name, xml in texts.items()}
    texts["word/fontTable.xml"] = font_table_xml("".join(texts.values()))
    result = [
        ("[Content_Types].xml", xml_part(content_types)),
        ("_rels/.rels", xml_part(root_rels)),
        ("docProps/core.xml", xml_part(core_xml(doc, twin))),
        ("word/document.xml", xml_part(texts["word/document.xml"])),
        ("word/styles.xml", xml_part(texts["word/styles.xml"])),
        ("word/settings.xml", xml_part(settings_xml(doc))),
        ("word/fontTable.xml", xml_part(texts["word/fontTable.xml"])),
        ("word/numbering.xml", xml_part(texts["word/numbering.xml"])),
    ]
    for name, _ in stories:
        result.append((f"word/{name}", xml_part(texts[f"word/{name}"])))
        if doc.header_images and name.startswith("header"):
            rels = [(rid, "image", f"media/{rid.lower()}.png", False) for rid in doc.header_images]
            result.append((f"word/_rels/{name}.rels", xml_part(relationships(rels))))
    if doc.footnotes:
        result.append(("word/footnotes.xml", xml_part(texts["word/footnotes.xml"])))
    result.append(("word/_rels/document.xml.rels", xml_part(relationships(word_rels))))
    for rid, data in {**doc.images, **doc.header_images}.items():
        result.append((f"word/media/{rid.lower()}.png", data))
    for name, data in result:
        if name.endswith((".xml", ".rels")):
            try:
                minidom.parseString(data)
            except Exception as error:  # any parse failure is a bug in this script
                raise ValueError(f"{doc.name}: {name} is not well-formed XML: {error}") from error
    return result


def package(doc: Doc, twin: bool = False) -> bytes:
    out = BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as archive:
        for name, data in parts(doc, twin):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 0
            info.external_attr = 0
            archive.writestr(info, data, zipfile.ZIP_STORED)
    return out.getvalue()


# --- English CVs --------------------------------------------------------------------------

TEXT_LETTER = LETTER[0] - 2 * 1440  # 9360 twips


def cv_classic() -> Doc:
    """One column, headings with a rule under them, dates on a right tab."""
    right = Tab(TEXT_LETTER, "right")

    def job(title: str, place: str, dates: str, points: list[str]) -> str:
        return p(
            r(title, bold=True),
            r(", " + place),
            r("\t" + dates, italic=True),
            tabs=(right,),
            before=6,
            keep_next=True,
        ) + "".join(p(r(point), numbering=(BULLET, 0), after=2) for point in points)

    def section(title: str) -> str:
        return p(
            r(title.upper(), bold=True, color="1F3864", spacing=0.5),
            border_bottom="1F3864",
            before=14,
            after=4,
            keep_next=True,
        )

    body = (
        p(r("Jordan Ellis", size=26, bold=True), align="center", after=2)
        + p(r("Senior Data Analyst", size=13, color="595959"), align="center", after=4)
        + p(
            r("Portland, OR  ·  (555) 010-2234  ·  "),
            hyperlink("rIdLink1", "jordan.ellis@example.com"),
            align="center",
            after=6,
        )
        + section("Profile")
        + p(
            r(
                "Data analyst with nine years of experience in logistics and retail, turning"
                " operational data into forecasts, dashboards and decisions. Comfortable with SQL,"
                " Python and the people who have to act on the numbers."
            ),
            align="both",
            line=1.15,
        )
        + section("Experience")
        + job(
            "Senior Data Analyst",
            "Bluepeak Logistics",
            "2021 – present",
            [
                "Built the weekly demand forecast for 42 warehouses; error fell from 14 % to 6 %.",
                (
                    "Replaced 30 spreadsheet reports with one self-service dashboard used by 300"
                    " staff."
                ),
                "Mentored four junior analysts and ran the team's SQL review sessions.",
            ],
        )
        + job(
            "Data Analyst",
            "Harbor & Pine Retail",
            "2017 – 2021",
            [
                "Modelled store staffing against footfall, saving 3,100 hours a quarter.",
                "Designed the A/B testing process for regional promotions.",
            ],
        )
        + job(
            "Junior Analyst",
            "Cobalt Street Insurance",
            "2015 – 2017",
            [
                "Cleaned and documented the claims history used by the pricing team.",
            ],
        )
        + section("Education")
        + p(
            r("B.Sc. Statistics", bold=True),
            r(", Westlake State University"),
            r("\t2011 – 2015", italic=True),
            tabs=(right,),
            before=4,
        )
        + section("Skills")
        + table(
            [
                [
                    cell(p(r("SQL, PostgreSQL"), numbering=(BULLET, 0))),
                    cell(p(r("Python, pandas"), numbering=(BULLET, 0))),
                    cell(p(r("Forecasting"), numbering=(BULLET, 0))),
                ],
                [
                    cell(p(r("Power BI"), numbering=(BULLET, 0))),
                    cell(p(r("dbt, Airflow"), numbering=(BULLET, 0))),
                    cell(p(r("Experiment design"), numbering=(BULLET, 0))),
                ],
            ],
            [3120, 3120, 3120],
            borders="none",
            margins=(0, 0),
        )
        + section("Languages")
        + p(r("English (native), Spanish (professional working proficiency)"))
    )
    return Doc(
        "cv-en-classic",
        "CV, one column: centred name block, headings with a bottom rule, dates on a right tab,"
        " bullet lists, a borderless skills table; Calibri 11, Letter",
        "en-US",
        body,
        "Calibri",
        11,
        links={"rIdLink1": "mailto:jordan.ellis@example.com"},
    )


def cv_sidebar() -> Doc:
    """Two columns made with a table: a shaded sidebar with a photo, the main column."""
    side = "DEEAF6"

    def side_head(text: str) -> str:
        return p(r(text.upper(), bold=True, size=9, color="1F4E79", spacing=1), before=10, after=2)

    def main_head(text: str) -> str:
        return p(
            r(text, bold=True, size=13, color="1F4E79"),
            border_bottom="9DC3E6",
            before=10,
            after=4,
            keep_next=True,
        )

    def job(title: str, place: str, dates: str, text: str) -> str:
        return (
            p(r(title, bold=True), keep_next=True, before=4)
            + p(r(place + "  |  " + dates, italic=True, color="595959"), keep_next=True)
            + p(r(text), align="both", after=4)
        )

    sidebar = (
        p(picture("rIdImg1", 1, "Photo", 3.0, 3.75), align="center", before=6)
        + side_head("Contact")
        + p(r("Riverside Lane 12\nAshby Hollow, MA 00000", size=9))
        + p(r("(555) 014-7781", size=9), before=3)
        + p(r("m.okafor@example.com", size=9), before=3)
        + side_head("Skills")
        + "".join(
            p(r(skill, size=9), numbering=(BULLET, 0))
            for skill in [
                "Project planning",
                "Budget control",
                "Contract negotiation",
                "Risk registers",
                "MS Project, Jira",
            ]
        )
        + side_head("Certificates")
        + p(r("Project management professional, 2019", size=9))
        + p(r("Agile practitioner, 2021", size=9), before=3)
    )
    main = (
        p(r("Morgan Okafor", bold=True, size=24, color="1F4E79"))
        + p(r("Construction Project Manager", size=12, color="595959"), after=6)
        + main_head("Profile")
        + p(
            r(
                "Project manager for public and commercial buildings up to 40 million dollars."
                " Known for schedules that hold and for keeping owners, architects and trades in"
                " one conversation."
            ),
            align="both",
        )
        + main_head("Experience")
        + job(
            "Senior Project Manager",
            "Granite Ridge Builders",
            "2020 – present",
            "Led the delivery of a 120-bed care home and two school extensions, all within 3 % of"
            " budget; introduced weekly look-ahead planning across six sites.",
        )
        + job(
            "Project Manager",
            "Northgate Civil Works",
            "2015 – 2020",
            "Managed bridge refurbishments and a municipal depot; coordinated 25 subcontractors and"
            " the permitting with three agencies.",
        )
        + job(
            "Site Engineer",
            "Lakeshore Homes",
            "2012 – 2015",
            "Set out foundations and services for 300 homes; kept the quality log and as-built"
            " drawings.",
        )
        + main_head("Education")
        + p(
            r("M.Eng. Civil Engineering", bold=True), r(", Eastbrook Institute of Technology, 2012")
        )
    )
    body = table(
        [[cell(sidebar, fill=side), cell(main)]], [2880, 6480], borders="none", margins=(180, 180)
    )
    return Doc(
        "cv-en-sidebar",
        "CV in two columns made with a borderless table: a shaded sidebar with an inline photo"
        " placeholder, small caps-like spaced headings, bullets; Arial 10, Letter",
        "en-US",
        body,
        "Arial",
        10,
        images={"rIdImg1": portrait()},
        margins=(1080, 1440, 1080, 1440),
    )


def cv_leaders() -> Doc:
    """Two pages, dotted tab leaders to the dates, small capitals, exact line spacing."""
    text = A4[0] - 2 * cm(2.5)
    lead = Tab(text, "right", "dot")

    def head(title: str) -> str:
        return p(
            r(title, small_caps=True, bold=True, size=13, spacing=1),
            before=12,
            after=3,
            keep_next=True,
            line=16,
            rule="exact",
        )

    def entry(left: str, dates: str, detail: str) -> str:
        return p(
            r(left, bold=True),
            r("\t" + dates),
            tabs=(lead,),
            line=14,
            rule="exact",
            keep_next=True,
            before=4,
        ) + p(r(detail, italic=True), line=14, rule="exact", left=cm(0.6), align="both")

    jobs = [
        (
            "Head of Translation Services, Meridian Language Partners",
            "2019–2026",
            (
                "Ran a team of eleven translators and two terminologists; set up the review"
                " workflow and the style guides for legal and medical clients."
            ),
        ),
        (
            "Senior Translator, Atlas Technical Documentation",
            "2014–2019",
            (
                "Translated maintenance manuals and safety data sheets from German and French;"
                " built the shared termbase."
            ),
        ),
        (
            "Translator, Bellwether Publishing",
            "2010–2014",
            "Translated non-fiction titles and catalogues; proofread for the in-house magazine.",
        ),
        (
            "In-house Interpreter, Crescent Trade Fairs",
            "2008–2010",
            "Interpreted at exhibitor meetings and press conferences in three languages.",
        ),
    ]
    body = (
        p(r("Dr. Elena Varga", size=22, small_caps=True, bold=True), after=2)
        + p(
            r("Translator and Terminologist  •  Lindenweg 4, Riverton  •  (555) 017-3390"),
            after=8,
            border_bottom="808080",
        )
        + head("Professional Experience")
        + "".join(entry(*job) for job in jobs)
        + head("Education")
        + entry(
            "Ph.D. Translation Studies, Hillcrest University",
            "2008",
            "Thesis: consistency of terminology in multilingual technical documentation.",
        )
        + entry(
            "M.A. Applied Linguistics, Hillcrest University",
            "2004",
            "Specialisation in legal and technical translation.",
        )
        + head("Selected Projects")
        + "".join(
            entry(f"Project {n}: {name}", year, detail)
            for n, (name, year, detail) in enumerate(
                [
                    (
                        "Rail signalling manuals",
                        "2025",
                        (
                            "1.2 million words, four languages, delivered in nine months with a"
                            " team of seven."
                        ),
                    ),
                    (
                        "Clinical trial documents",
                        "2023",
                        (
                            "Informed consent forms and protocols for 14 sites; back-translation"
                            " and reconciliation."
                        ),
                    ),
                    (
                        "Machine safety data sheets",
                        "2021",
                        (
                            "Converted 900 sheets to the new regulation format, with a validated"
                            " termbase."
                        ),
                    ),
                    (
                        "Annual reports of a utility",
                        "2018",
                        "Three consecutive reports, typeset in parallel with the source.",
                    ),
                    (
                        "Museum catalogue",
                        "2016",
                        "Catalogue texts and wall labels for a travelling exhibition.",
                    ),
                ],
                1,
            )
        )
        + head("Languages")
        + entry("Hungarian, English", "native", "Both languages spoken at home and at school.")
        + entry("German, French", "C2", "Certified translator for legal documents.")
        + entry("Italian", "B2", "Reading and correspondence.")
        + head("Memberships")
        + entry(
            "Association of Technical Translators",
            "since 2011",
            "Member of the terminology working group.",
        )
        + entry("Riverton Interpreters' Guild", "since 2009", "Mentor for new members.")
    )
    footer = p(page_of("Page {page} of {pages}", size=9), align="right")
    return Doc(
        "cv-en-leaders",
        "Two-page CV: dotted tab leaders to right-aligned dates, small capitals, exact 14 pt line"
        " spacing, keep-with-next entries, Page X of Y footer; Times New Roman 11, A4",
        "en-US",
        body,
        "Times New Roman",
        11,
        page=A4,
        margins=(cm(2.5),) * 4,
        footer=footer,
    )


def cv_compact() -> Doc:
    """Dense one page: narrow margins, line spacing below single, three-column skills."""
    width = LETTER[0] - 2 * 864
    right = Tab(width, "right")

    def head(title: str) -> str:
        return p(
            r(title, bold=True, size=11, color="C00000"),
            before=6,
            after=1,
            keep_next=True,
            line=0.9,
        )

    def item(text: str) -> str:
        return p(r(text, size=9.5), numbering=(BULLET, 0), line=0.9)

    body = (
        p(
            r("SAM RIVERA", bold=True, size=20, spacing=2),
            r("\tsam.rivera@example.com  |  (555) 011-0042", size=9),
            tabs=(right,),
            line=0.9,
        )
        + p(
            r("Full-stack developer  |  "),
            hyperlink("rIdLink1", "portfolio.example.com"),
            r("  |  Austin, TX"),
            line=0.9,
            border_bottom="C00000",
            after=4,
        )
        + head("Experience")
        + p(
            r("Lead Developer – Quillmark Software", bold=True, size=10),
            r("\tMar 2022 – now", size=9),
            tabs=(right,),
            line=0.9,
        )
        + "".join(
            item(t)
            for t in [
                "Rebuilt the billing service in Go; p99 latency down from 900 ms to 120 ms.",
                (
                    "Introduced contract tests between 14 services; release rollbacks fell by two"
                    " thirds."
                ),
                "Ran hiring loops for 9 engineers; wrote the onboarding guide.",
            ]
        )
        + p(
            r("Developer – Ferncliff Health Apps", bold=True, size=10),
            r("\tJun 2018 – Feb 2022", size=9),
            tabs=(right,),
            line=0.9,
            before=3,
        )
        + "".join(
            item(t)
            for t in [
                "Built the appointment booking front end in TypeScript and React for 200 clinics.",
                (
                    "Moved the deployment from hand-run scripts to a pipeline with preview"
                    " environments."
                ),
            ]
        )
        + p(
            r("Junior Developer – Sparrow Media", bold=True, size=10),
            r("\tSep 2016 – May 2018", size=9),
            tabs=(right,),
            line=0.9,
            before=3,
        )
        + item("Maintained the content management system and its image pipeline.")
        + head("Skills")
        + table(
            [
                [
                    cell(p(r("Languages", bold=True, size=9))),
                    cell(p(r("Frameworks", bold=True, size=9))),
                    cell(p(r("Tools", bold=True, size=9))),
                ],
                [
                    cell(p(r("Go, TypeScript, Python, SQL", size=9))),
                    cell(p(r("React, Next.js, gRPC", size=9))),
                    cell(p(r("PostgreSQL, Kafka, Terraform", size=9))),
                ],
            ],
            [width // 3] * 3,
            borders="horizontal",
            border_color="C00000",
            margins=(60, 60),
        )
        + head("Education")
        + p(
            r("B.Sc. Computer Science, Pine Valley College", size=9.5),
            r("\t2016", size=9),
            tabs=(right,),
            line=0.9,
        )
        + head("Open source")
        + item("Maintainer of a small date-parsing library (1.2k stars).")
        + item("Contributor to a static site generator: image resizing and incremental builds.")
    )
    return Doc(
        "cv-en-compact",
        "Dense one-page CV: narrow margins, line spacing 0.9, letter-spaced name, right tabs, a"
        " three-column skills table with horizontal rules only, hyperlinks; Calibri 10, Letter",
        "en-US",
        body,
        "Calibri",
        10,
        margins=(864,) * 4,
        links={"rIdLink1": "https://portfolio.example.com/"},
    )


def cv_academic() -> Doc:
    """Academic CV: justified text, numbered publications in brackets, a footnote."""
    publications = [
        (
            "Ashworth, T. and Lindqvist, M. (2025). Tidal energy siting under uncertainty. Journal"
            " of Coastal Systems, 41(3), 211–236."
        ),
        (
            "Ashworth, T. (2024). A practical guide to seabed surveys for small research groups."
            " Field Methods in Marine Science, 12, 55–79."
        ),
        (
            "Ashworth, T., Romero, P. and Chen, L. (2023). Sediment transport near offshore"
            " foundations: a ten-year record. Ocean Engineering Letters, 8(2), 101–129."
        ),
        (
            "Lindqvist, M. and Ashworth, T. (2021). Measuring turbulence with low-cost acoustic"
            " sensors. Instruments for the Sea, 19, 300–318."
        ),
        (
            "Ashworth, T. (2019). Coastal models and the people who use them. In: Harlan, J. (ed.)"
            " Science at the Shoreline. Eastport University Press, 45–68."
        ),
        (
            "Ashworth, T. and Okonkwo, B. (2017). Storm surge and harbour design: lessons from"
            " three winters. Port Engineering Quarterly, 33(4), 12–30."
        ),
    ]
    body = (
        p(r("Curriculum Vitae", size=12, italic=True), align="center")
        + p(r("Dr. Thomas Ashworth", size=18, bold=True), align="center", after=2)
        + p(
            r(
                "Senior Lecturer in Coastal Engineering, Department of Civil Engineering, Eastport"
                " University"
            ),
            align="center",
            after=12,
        )
        + p(r("Research interests", bold=True), style="Heading1")
        + p(
            r(
                "Marine renewable energy, sediment transport and the design of coastal structures."
                " My work combines field measurements with numerical models and is used by harbour"
                " authorities and energy developers"
            ),
            footnote_ref(1),
            r(". I lead the coastal laboratory and supervise six doctoral students."),
            align="both",
            line=1.15,
        )
        + p(r("Appointments", bold=True), style="Heading1")
        + p(
            r("2019–present\tSenior Lecturer, Eastport University"),
            tabs=(Tab(1800),),
            left=1800,
            hanging=1800,
        )
        + p(
            r("2014–2019\tLecturer, Eastport University"),
            tabs=(Tab(1800),),
            left=1800,
            hanging=1800,
        )
        + p(
            r("2011–2014\tResearch Fellow, Northern Marine Institute"),
            tabs=(Tab(1800),),
            left=1800,
            hanging=1800,
        )
        + p(r("Selected publications", bold=True), style="Heading1")
        + "".join(
            p(r(text), numbering=(BRACKETED, 0), align="both", after=4) for text in publications
        )
        + p(r("Teaching", bold=True), style="Heading1")
        + p(
            r(
                "Fluid mechanics (second year, 180 students), coastal engineering design (fourth"
                " year), and a graduate course on field methods. Course leader for the coastal"
                " engineering master's programme since 2021."
            ),
            align="both",
            line=1.15,
        )
        + p(r("Grants", bold=True), style="Heading1")
        + table(
            [
                [
                    cell(p(r("Years", bold=True))),
                    cell(p(r("Funder and title", bold=True))),
                    cell(p(r("Amount", bold=True), align="right")),
                ]
            ]
            + [
                [cell(p(r(y))), cell(p(r(t))), cell(p(r(a), align="right"))]
                for y, t, a in [
                    (
                        "2024–2027",
                        "National Science Fund: Tidal sites under changing seabeds",
                        "$ 840,000",
                    ),
                    ("2021–2023", "Coastal Authority: Harbour wave climate study", "$ 210,000"),
                    (
                        "2018–2020",
                        "Energy Research Council: Sensor networks for tidal turbines",
                        "$ 365,000",
                    ),
                ]
            ],
            [1500, 6160, 1700],
            header_rows=1,
        )
    )
    footer = p(
        r("T. Ashworth – CV\t"),
        page_of("{page}"),
        style="Footer",
        tabs=(Tab(TEXT_LETTER, "right"),),
    )
    return Doc(
        "cv-en-academic",
        "Academic CV: justified text with line spacing 1.15, Heading 1 style, hanging-indent"
        " appointments, bracketed numbered publications, a footnote, a grants table with a header"
        " row; Times New Roman 12, Letter",
        "en-US",
        body,
        "Times New Roman",
        12,
        footer=footer,
        footnotes=[
            "Including the harbour authorities of three ports and two offshore wind developers."
        ],
    )


# --- English invoices ---------------------------------------------------------------------


def money(value: float, symbol: str = "$", decimal: str = ".", group: str = ",") -> str:
    whole, cents = f"{value:,.2f}".split(".")
    whole = whole.replace(",", group)
    return (
        f"{symbol}{whole}{decimal}{cents}"
        if symbol in "$£"
        else f"{whole}{decimal}{cents} {symbol}".strip()
    )


ITEMS = [
    ("Copy paper, A4, 80 g/m², box of 5 reams", 12, 24.90),
    ("Ballpoint pens, blue, pack of 50", 6, 11.25),
    ("Lever arch files, assorted colours", 40, 2.85),
    ("Desk organiser, mesh, black", 8, 14.60),
    ("Whiteboard markers, set of 4", 15, 5.40),
    ("Sticky notes, 76 × 76 mm, 12 pads", 20, 6.95),
    ("Toner cartridge, compatible, black", 4, 68.00),
    ("Laminating pouches, A4, 100 pieces", 3, 17.30),
]


def invoice_header(company: str, address: str, colour: str, rid: str) -> str:
    return table(
        [
            [
                cell(p(picture(rid, 101, "Logo", 4.2, 1.4))),
                cell(
                    p(r(company, bold=True, size=11), align="right"),
                    p(r(address, size=8.5, color="595959"), align="right"),
                ),
            ]
        ],
        [4680, 4680],
        borders="none",
        margins=(0, 0),
    ) + p(border_bottom=colour, after=0)


def invoice_standard() -> Doc:
    colour = "2E75B6"
    rows = [
        [
            cell(p(r(h, bold=True, color="FFFFFF"), align=a), fill=colour)
            for h, a in (
                ("Description", "left"),
                ("Qty", "right"),
                ("Unit price", "right"),
                ("Amount", "right"),
            )
        ]
    ]
    total = 0.0
    for name, qty, price in ITEMS:
        amount = qty * price
        total += amount
        rows.append(
            [
                cell(p(r(name))),
                cell(p(r(str(qty)), align="right")),
                cell(p(r(money(price)), align="right")),
                cell(p(r(money(amount)), align="right")),
            ]
        )
    tax = round(total * 0.0825, 2)
    right = Tab(TEXT_LETTER, "right")
    label = Tab(TEXT_LETTER - 1800, "right")
    body = (
        p(r("INVOICE", size=26, bold=True, color=colour, spacing=2), before=6, after=6)
        + table(
            [
                [
                    cell(
                        p(r("Bill to", bold=True, color=colour)),
                        p(
                            r(
                                "Ashgrove Dental Practice\nAttn: Accounts Payable\n77 Orchard"
                                " Road\nMillbrook, OH 43050"
                            )
                        ),
                    ),
                    cell(
                        p(r("Ship to", bold=True, color=colour)),
                        p(
                            r(
                                "Ashgrove Dental Practice\nReception, 2nd floor\n77 Orchard"
                                " Road\nMillbrook, OH 43050"
                            )
                        ),
                    ),
                    cell(
                        p(r("Invoice no.\t", bold=True), r("INV-2026-0417"), tabs=(Tab(1300),)),
                        p(r("Date\t", bold=True), r("March 12, 2026"), tabs=(Tab(1300),)),
                        p(r("Due\t", bold=True), r("April 11, 2026"), tabs=(Tab(1300),)),
                        p(r("Customer\t", bold=True), r("C-00342"), tabs=(Tab(1300),)),
                    ),
                ]
            ],
            [3000, 3000, 3360],
            borders="none",
            margins=(0, 120),
        )
        + p(after=8)
        + table(
            rows,
            [5160, 900, 1600, 1700],
            borders="horizontal",
            border_color="9DC3E6",
            margins=(80, 80),
        )
        + p(r("Subtotal\t"), r(money(total)), tabs=(label, right), before=8)
        + p(r("Sales tax 8.25 %\t"), r(money(tax)), tabs=(label, right))
        + p(
            r("Total due\t", bold=True),
            r(money(total + tax), bold=True),
            tabs=(label, right),
            border_bottom=colour,
        )
        + p(
            r("Payment terms: 30 days. Please quote the invoice number with your payment."),
            before=18,
        )
        + p(
            r("Bank transfer to Lakeview Community Bank, account 000123456, routing 000000000."),
            before=4,
        )
    )
    footer = p(
        r("Larkspur Office Supply Co.  ·  Thank you for your business\t", size=8, color="808080"),
        page_of("Page {page} of {pages}", size=8, color="808080"),
        style="Footer",
        tabs=(right,),
    )
    return Doc(
        "invoice-en-standard",
        "Invoice: logo and address in a header table, coloured title, three-column address block"
        " with tabs, an item table with a filled header row and horizontal rules, right-aligned"
        " amounts, totals on right tabs, Page X of Y footer; Calibri 10, Letter",
        "en-US",
        body,
        "Calibri",
        10,
        header=invoice_header(
            "Larkspur Office Supply Co.",
            "410 Commerce Way, Suite 12\nSpringdale, OH 43001\n(555) 012-9000  · "
            " billing@larkspur.example.com",
            colour,
            "rIdImg1",
        ),
        header_images={"rIdImg1": logo((46, 117, 182))},
        footer=footer,
    )


def invoice_decimal() -> Doc:
    """Amounts aligned on decimal tabs inside the cells, credits in brackets, a footnote."""
    lines = [
        ("Consulting, February (38.5 h × $145.00)", "5,582.50"),
        ("Travel expenses, receipts attached", "412.80"),
        ("Software licence, annual", "1,200.00"),
        ("Discount, long-term client (5 %)", "(359.77)"),
        ("Credit note CN-118, overbilled hours", "(290.00)"),
        ("Translation of user guide, 9 pages", "855.00"),
    ]
    dec = Tab(1300, "decimal")
    rows = [
        [
            cell(p(r("Item", bold=True)), fill="E7E6E6"),
            cell(p(r("Amount (USD)", bold=True), align="right"), fill="E7E6E6"),
        ]
    ]
    rows += [[cell(p(r(text))), cell(p(r("\t" + amount), tabs=(dec,)))] for text, amount in lines]
    rows.append(
        [
            cell(p(r("Total", bold=True)), fill="E7E6E6"),
            cell(p(r("\t7,400.53", bold=True), tabs=(dec,)), fill="E7E6E6"),
        ]
    )
    body = (
        p(r("Fenwick & Hale Advisory", bold=True, size=14))
        + p(r("Invoice FH-2026-031  ·  March 2, 2026", color="595959"), after=10)
        + p(r("Client: Tamarack Water District, 12 Reservoir Road, Elm Falls, CO 80001"), after=8)
        + table(rows, [7060, 2300])
        + p(
            r("Bracketed amounts are credits"),
            footnote_ref(1),
            r(". All amounts in US dollars."),
            before=8,
        )
        + p(r("Questions about this invoice: accounts@fenwick-hale.example.com"), before=4)
    )
    return Doc(
        "invoice-en-decimal",
        "Invoice whose amounts align on decimal tabs inside table cells, with bracketed credits,"
        " filled first and last rows, and a footnote; Arial 9, Letter",
        "en-US",
        body,
        "Arial",
        9,
        footnotes=["Credits reduce the total; see credit note CN-118 sent on February 20, 2026."],
    )


def invoice_long() -> Doc:
    """About 70 lines over several pages: the header row repeats, rows do not split."""
    rows = [
        [
            cell(p(r(h, bold=True), align=a), fill="D9E2F3")
            for h, a in (
                ("#", "right"),
                ("Part no.", "left"),
                ("Description", "left"),
                ("Qty", "right"),
                ("Price", "right"),
                ("Total", "right"),
            )
        ]
    ]
    total = 0.0
    for n in range(1, 71):
        name, qty, price = ITEMS[(n * 3) % len(ITEMS)]
        qty = (qty + n) % 17 + 1
        price = round(price * (1 + (n % 5) / 10), 2)
        amount = qty * price
        total += amount
        description = (
            name if n % 9 else name + " (back-ordered item, ships separately when stock arrives)"
        )
        rows.append(
            [
                cell(p(r(str(n)), align="right")),
                cell(p(r(f"P-{4000 + n * 7}"))),
                cell(p(r(description))),
                cell(p(r(str(qty)), align="right")),
                cell(p(r(money(price)), align="right")),
                cell(p(r(money(amount)), align="right")),
            ]
        )
    right = Tab(TEXT_LETTER, "right")
    body = (
        p(r("Packing list and invoice", bold=True, size=16), after=4)
        + p(
            r(
                "Order 55-2093  ·  Delivered to: Briar Hill Elementary School, 9 Schoolhouse Lane,"
                " Ridgeway, PA 15001"
            ),
            after=8,
        )
        + table(
            rows,
            [500, 1100, 4560, 700, 1200, 1300],
            header_rows=1,
            cant_split=True,
            margins=(70, 70),
        )
        + p(
            r("Total\t", bold=True),
            r(money(total), bold=True),
            tabs=(right,),
            before=8,
            keep_next=True,
            border_bottom="000000",
        )
        + p(r("Received in good order (signature and date):"), before=24)
        + p(r("\t"), tabs=(Tab(5000, "left", "underscore"),), before=18)
    )
    footer = p(r("Order 55-2093\t\t"), page_of("Page {page} of {pages}"), style="Footer")
    return Doc(
        "invoice-en-long",
        "Invoice of 70 lines over several pages: a header row repeated on every page, rows that"
        " may not split, a long description that wraps, totals kept with their rule, an"
        " underscore leader signature line; Calibri 10, Letter",
        "en-US",
        body,
        "Calibri",
        10,
        footer=footer,
    )


def invoice_merged() -> Doc:
    """Grouped items with vertically merged group cells and a VAT summary with spans."""
    groups = [
        (
            "Hardware",
            [
                ("Laptop, 14-inch", 3, 899.00),
                ("Docking station", 3, 189.00),
                ("Monitor, 27-inch", 3, 249.00),
            ],
        ),
        ("Services", [("Device setup", 9, 35.00), ("Data migration", 3, 120.00)]),
        ("Software", [("Office suite, annual", 3, 99.00)]),
    ]
    rows = [[cell(p(r(h, bold=True)), fill="FCE4D6") for h in ("Group", "Item", "Qty", "Net")]]
    net = 0.0
    for group, items in groups:
        for index, (name, qty, price) in enumerate(items):
            amount = qty * price
            net += amount
            first = (
                cell(p(r(group, bold=True)), merge="restart", valign="center")
                if index == 0
                else cell(p(), merge="continue")
            )
            rows.append(
                [
                    first,
                    cell(p(r(name))),
                    cell(p(r(str(qty)), align="right")),
                    cell(p(r(money(amount, "£")), align="right")),
                ]
            )
    vat = round(net * 0.2, 2)
    summary = [
        [
            cell(p(r("VAT summary", bold=True)), span=2, fill="FCE4D6"),
            cell(p(r("Amount", bold=True), align="right"), fill="FCE4D6"),
        ],
        [
            cell(p(r("Net"))),
            cell(p(r("standard rate"))),
            cell(p(r(money(net, "£")), align="right")),
        ],
        [
            cell(p(r("VAT 20 %"))),
            cell(p(r("on all lines"))),
            cell(p(r(money(vat, "£")), align="right")),
        ],
        [
            cell(p(r("Total", bold=True)), span=2),
            cell(p(r(money(net + vat, "£"), bold=True), align="right")),
        ],
    ]
    body = (
        p(r("TAX INVOICE", bold=True, size=18, color="C55A11"), align="right")
        + p(r("Kestrel IT Services Ltd  ·  VAT no. GB 000 0000 00"), align="right", after=10)
        + p(
            r(
                "Invoice 7781  ·  12 March 2026  ·  Customer: Oakfield Veterinary Clinic, 3 Mill"
                " Street, Westbury"
            ),
            after=8,
        )
        + table(rows, [1800, 4560, 1000, 2000])
        + p(after=8)
        + table(summary, [2400, 2800, 2000], align="right")
    )
    return Doc(
        "invoice-en-merged",
        "Tax invoice with grouped items: vertically merged group cells centred vertically, a"
        " right-aligned VAT summary table with column spans, filled header rows; Arial 10, Letter",
        "en-GB",
        body,
        "Arial",
        10,
    )


def invoice_receipt() -> Doc:
    """A till receipt on a narrow page, columns made with tabs in a monospaced font."""
    width = cm(8) - 2 * cm(0.5)
    right = Tab(width, "right")
    lines = [
        ("Espresso", 2, 2.60),
        ("Cappuccino, large", 1, 3.90),
        ("Croissant", 3, 2.10),
        ("Blueberry muffin", 1, 2.75),
        ("Sparkling water", 2, 1.80),
        ("Sandwich, egg & cress", 1, 4.95),
    ]
    total = sum(qty * price for _, qty, price in lines)
    body = (
        p(r("THE COPPER KETTLE", bold=True), align="center")
        + p(r("18 Market Square, Hollowmere"), align="center")
        + p(r("Tel (555) 019-2200"), align="center", after=6)
        + p(r("Receipt 0045-2291   03/12/2026 08:41"), border_bottom="000000", after=4)
        + "".join(
            p(r(f"{qty} x {name}\t{qty * price:.2f}"), tabs=(right,)) for name, qty, price in lines
        )
        + p(r(f"SUBTOTAL\t{total:.2f}"), tabs=(right,), before=4, border_bottom="000000")
        + p(r(f"TOTAL\t{total:.2f}", bold=True), tabs=(right,), before=2)
        + p(r(f"CARD\t{total:.2f}"), tabs=(right,))
        + p(r("Thank you, see you soon!"), align="center", before=10)
    )
    return Doc(
        "invoice-en-receipt",
        "Till receipt on an 8 cm wide page: columns made with right tabs in a monospaced font,"
        " rules as paragraph borders; Courier New 9",
        "en-US",
        body,
        "Courier New",
        9,
        page=(cm(8), cm(16)),
        margins=(cm(0.5),) * 4,
    )


# --- Ukrainian and Russian ----------------------------------------------------------------

DSTU_MARGINS = (cm(2), cm(1.5), cm(2), cm(3))  # top, right, bottom, left
TEXT_DSTU = A4[0] - cm(1.5) - cm(3)


def clause_doc(
    name: str,
    purpose: str,
    lang: str,
    title: str,
    place_date: tuple[str, str],
    preamble: str,
    sections: list[tuple[str, list[str]]],
    parties: tuple[tuple[str, list[str]], tuple[str, list[str]]],
    page_label: str,
) -> Doc:
    """A contract set the way Ukrainian and Russian contracts usually are: Times New Roman 12,
    justified with a 1.25 cm first-line indent, numbered sections and clauses, a signature
    table, page numbers at the bottom."""
    right = Tab(TEXT_DSTU, "right")
    body = (
        p(r(title, bold=True, size=13), align="center", after=6)
        + p(r(place_date[0] + "\t" + place_date[1]), tabs=(right,), after=10)
        + p(r(preamble), align="both", first=cm(1.25), after=6)
    )
    for heading, clauses in sections:
        body += p(r(heading, bold=True), numbering=(SECTIONS, 0), before=8, after=4, keep_next=True)
        body += "".join(
            p(r(text), numbering=(SECTIONS, 1), align="both", after=2) for text in clauses
        )
    (left_title, left_lines), (right_title, right_lines) = parties

    def block(heading: str, lines: list[str]) -> str:
        return (
            p(r(heading, bold=True), after=4)
            + "".join(p(r(line)) for line in lines)
            + p(r("\t"), tabs=(Tab(2400, "left", "underscore"),), before=24)
            + p(r("(підпис)" if lang == "uk-UA" else "(подпись)", size=9))
        )

    body += p(before=12) + table(
        [[cell(block(left_title, left_lines)), cell(block(right_title, right_lines))]],
        [TEXT_DSTU // 2, TEXT_DSTU - TEXT_DSTU // 2],
        borders="none",
        margins=(0, 113),
    )
    footer = p(page_of(page_label, size=10), align="center")
    return Doc(
        name,
        purpose,
        lang,
        body,
        "Times New Roman",
        12,
        page=A4,
        margins=DSTU_MARGINS,
        footer=footer,
        decimal=",",
        list_separator=";",
    )


def contract_uk() -> Doc:
    return clause_doc(
        "contract-uk",
        "Ukrainian supply contract: title, place and date on a right tab, justified text with a"
        " 1.25 cm first-line indent, multi-level numbering 1. / 1.1., an underscore-leader"
        " signature table, Сторінка X з Y footer; Times New Roman 12, A4, DSTU margins",
        "uk-UA",
        "ДОГОВІР ПОСТАЧАННЯ № 17/03-2026",
        ("м. Київ", "«12» березня 2026 р."),
        "Товариство з обмеженою відповідальністю «Зелений Луг» (далі – Постачальник) в особі"
        " директора, що діє на підставі Статуту, з однієї сторони, та Товариство з обмеженою"
        " відповідальністю «Світлиця Торг» (далі – Покупець) в особі генерального директора, що діє"
        " на підставі Статуту, з іншої сторони, уклали цей договір про таке:",
        [
            (
                "ПРЕДМЕТ ДОГОВОРУ",
                [
                    (
                        "Постачальник зобов’язується поставити Покупцю саджанці декоративних рослин"
                        " (далі – Товар) за асортиментом, у кількості та за цінами, визначеними у"
                        " специфікаціях, а Покупець зобов’язується прийняти та оплатити Товар."
                    ),
                    "Специфікації є невід’ємною частиною цього договору.",
                ],
            ),
            (
                "ЦІНА ТА ПОРЯДОК РОЗРАХУНКІВ",
                [
                    (
                        "Загальна сума договору становить 248 600,00 грн (двісті сорок вісім тисяч"
                        " шістсот гривень 00 копійок), у тому числі ПДВ 20 % – 41 433,33 грн."
                    ),
                    (
                        "Покупець сплачує вартість кожної партії Товару протягом 10 (десяти)"
                        " банківських днів з дати її поставки."
                    ),
                    "Оплата здійснюється в безготівковій формі на поточний рахунок Постачальника.",
                ],
            ),
            (
                "ПОСТАВКА ТА ПРИЙМАННЯ ТОВАРУ",
                [
                    (
                        "Поставка здійснюється транспортом Постачальника до складу Покупця у м."
                        " Біла Церква."
                    ),
                    (
                        "Приймання Товару за кількістю та якістю оформлюється видатковою накладною,"
                        " що підписується обома сторонами."
                    ),
                ],
            ),
            (
                "ВІДПОВІДАЛЬНІСТЬ СТОРІН",
                [
                    (
                        "За прострочення оплати Покупець сплачує пеню у розмірі 0,1 % від суми"
                        " боргу за кожен день прострочення."
                    ),
                    (
                        "У разі поставки Товару неналежної якості Постачальник замінює його"
                        " протягом 5 (п’яти) робочих днів."
                    ),
                ],
            ),
            (
                "СТРОК ДІЇ ДОГОВОРУ",
                [
                    (
                        "Договір набирає чинності з моменту його підписання і діє до 31 грудня 2026"
                        " року."
                    ),
                    "Договір складено у двох примірниках, що мають однакову юридичну силу.",
                ],
            ),
        ],
        (
            (
                "ПОСТАЧАЛЬНИК:",
                [
                    "ТОВ «Зелений Луг»",
                    "вул. Садова, 5, м. Фастів",
                    "Код ЄДРПОУ 00000001",
                    "IBAN UA00 0000 0000 0000 0000 0000 0000 1",
                ],
            ),
            (
                "ПОКУПЕЦЬ:",
                [
                    "ТОВ «Світлиця Торг»",
                    "просп. Миру, 18, м. Біла Церква",
                    "Код ЄДРПОУ 00000002",
                    "IBAN UA00 0000 0000 0000 0000 0000 0000 2",
                ],
            ),
        ),
        "Сторінка {page} з {pages}",
    )


def invoice_table(
    header: list[str],
    lines: list[tuple[str, str, str, str]],
    widths: list[int],
    fill: str,
) -> str:
    """Numbered item lines: № | name | unit | qty | price | sum."""
    rows = [[cell(p(r(h, bold=True), align="center"), fill=fill, valign="center") for h in header]]
    for number, (name, unit, qty, price) in enumerate(lines, 1):
        amount = float(qty.replace(",", ".")) * float(price.replace(" ", "").replace(",", "."))
        rows.append(
            [
                cell(p(r(str(number)), align="center")),
                cell(p(r(name))),
                cell(p(r(unit), align="center")),
                cell(p(r(qty), align="right")),
                cell(p(r(price), align="right")),
                cell(p(r(money(amount, "", ",", " ").strip()), align="right")),
            ]
        )
    return table(rows, widths, margins=(57, 57))


def invoice_uk() -> Doc:
    right = Tab(TEXT_DSTU, "right")
    lines = [
        ("Саджанці туї західної «Смарагд», 80–100 см", "шт.", "40", "450,00"),
        ("Саджанці гортензії волотистої, 3 л", "шт.", "60", "320,00"),
        ("Ґрунтосуміш для хвойних, 50 л", "мішок", "25", "185,00"),
        ("Доставка по області", "послуга", "1", "1 500,00"),
    ]
    total = sum(float(q) * float(pr.replace(" ", "").replace(",", ".")) for _, _, q, pr in lines)
    vat = total / 6
    body = (
        p(
            r("Постачальник: ", bold=True),
            r(
                "ТОВ «Зелений Луг», код ЄДРПОУ 00000001, вул. Садова, 5, м. Фастів, тел. (000)"
                " 000-00-01"
            ),
        )
        + p(
            r("Банк: ", bold=True),
            r("АТ «Приклад Банк», IBAN UA00 0000 0000 0000 0000 0000 0000 1"),
            after=4,
        )
        + p(r("Покупець: ", bold=True), r("ТОВ «Світлиця Торг», код ЄДРПОУ 00000002"), after=10)
        + p(
            r("Рахунок на оплату № 245 від 12 березня 2026 р.", bold=True, size=14),
            align="center",
            after=8,
            border_bottom="000000",
        )
        + invoice_table(
            ["№", "Найменування", "Од.", "Кількість", "Ціна, грн", "Сума, грн"],
            lines,
            [500, 3900, 900, 1100, 1200, 1300],
            "F2F2F2",
        )
        + p(
            r("Разом без ПДВ:\t" + money(total - vat, "", ",", " ").strip()),
            tabs=(right,),
            before=6,
        )
        + p(r("ПДВ 20 %:\t" + money(vat, "", ",", " ").strip()), tabs=(right,))
        + p(r("Всього до сплати:\t" + money(total, "", ",", " ").strip(), bold=True), tabs=(right,))
        + p(
            r(
                f"Всього найменувань {len(lines)}, на суму"
                f" {money(total, '', ',', ' ').strip()} грн."
            ),
            before=8,
        )
        + p(
            r(
                "Сорок три тисячі триста двадцять п’ять гривень 00 копійок, у тому числі ПДВ.",
                bold=True,
            )
        )
        + p(r("Рахунок дійсний до сплати до 22 березня 2026 р."), before=8)
        + p(
            r("Виписав(ла):\t", bold=True),
            r("\t"),
            tabs=(Tab(1800), Tab(5200, "left", "underscore")),
            before=20,
        )
    )
    return Doc(
        "invoice-uk",
        "Ukrainian invoice for payment: supplier and buyer lines with bold labels, a bordered item"
        " table with a filled header row, totals with VAT on a right tab, the sum in words; Arial"
        " 10, A4",
        "uk-UA",
        body,
        "Arial",
        10,
        page=A4,
        margins=DSTU_MARGINS,
        decimal=",",
        list_separator=";",
    )


def letter_uk() -> Doc:
    width = A4[0] - 2 * cm(2)
    body = (
        table(
            [
                [
                    cell(
                        p(r("ТОВ «Світлиця Торг»", bold=True, size=13, color="2F5496")),
                        p(
                            r(
                                "просп. Миру, 18, м. Біла Церква, 09100\nтел. (000) 000-00-02,"
                                " office@svitlytsia.example.com",
                                size=9,
                                color="595959",
                            )
                        ),
                    ),
                    cell(p(r("Вих. № 34\nвід 12.03.2026", size=10), align="right")),
                ]
            ],
            [width - 2600, 2600],
            borders="none",
            margins=(0, 0),
        )
        + p(border_bottom="2F5496", after=12)
        + p(r("Директору\nТОВ «Зелений Луг»\nпану Олегу ПЕТРЕНКУ"), left=cm(9), after=12)
        + p(r("Щодо графіка поставок у квітні", bold=True), after=8)
        + p(r("Шановний пане Олеже!"), after=6)
        + p(
            r(
                "Дякуємо за своєчасну поставку першої партії саджанців за договором № 17/03-2026. У"
                " зв’язку з розширенням торговельної площі просимо розглянути можливість збільшити"
                " квітневу поставку на 30 % та розділити її на дві частини: 7 та 21 квітня."
            ),
            align="both",
            first=cm(1.25),
            line=1.15,
            after=6,
        )
        + p(
            r(
                "Просимо також надіслати оновлений прайс-лист на гортензії та туї з урахуванням "
                "сезонних знижок. Остаточну специфікацію ми готові погодити до 25 березня."
            ),
            align="both",
            first=cm(1.25),
            line=1.15,
            after=6,
        )
        + p(
            r("Будемо вдячні за швидку відповідь."),
            align="both",
            first=cm(1.25),
            line=1.15,
            after=18,
        )
        + p(r("З повагою,"))
        + p(
            r("Генеральний директор\t"),
            r("Марія КОВАЛЬЧУК"),
            tabs=(Tab(width, "right"),),
            before=18,
        )
    )
    return Doc(
        "letter-uk",
        "Ukrainian business letter: letterhead and outgoing number in a borderless table, a rule,"
        " the addressee block indented 9 cm, bold subject, justified paragraphs with first-line"
        " indent and line spacing 1.15, signature on a right tab; Calibri 11, A4",
        "uk-UA",
        body,
        "Calibri",
        11,
        page=A4,
        margins=(cm(2),) * 4,
        decimal=",",
        list_separator=";",
    )


def contract_ru() -> Doc:
    return clause_doc(
        "contract-ru",
        "Russian services contract: title, place and date on a right tab, justified text with a"
        " 1.25 cm first-line indent, multi-level numbering 1. / 1.1., an underscore-leader"
        " signature table, Страница X из Y footer; Times New Roman 12, A4",
        "ru-RU",
        "ДОГОВОР ОКАЗАНИЯ УСЛУГ № 08-2026",
        ("г. Новоград", "«12» марта 2026 г."),
        "Общество с ограниченной ответственностью «Северный ветер» (далее – Заказчик) в лице"
        " генерального директора, действующего на основании Устава, с одной стороны, и Общество с"
        " ограниченной ответственностью «Альфа Сервис» (далее – Исполнитель) в лице директора,"
        " действующего на основании Устава, с другой стороны, заключили настоящий договор о"
        " нижеследующем:",
        [
            (
                "ПРЕДМЕТ ДОГОВОРА",
                [
                    (
                        "Исполнитель обязуется оказать Заказчику услуги по техническому"
                        " обслуживанию инженерных систем здания по адресу: г. Новоград, ул. Лесная,"
                        " д. 7, а Заказчик обязуется принять и оплатить эти услуги."
                    ),
                    (
                        "Перечень и периодичность работ приведены в приложении № 1, являющемся"
                        " неотъемлемой частью договора."
                    ),
                ],
            ),
            (
                "СТОИМОСТЬ УСЛУГ И ПОРЯДОК РАСЧЁТОВ",
                [
                    (
                        "Стоимость услуг составляет 96 000,00 руб. (девяносто шесть тысяч рублей 00"
                        " копеек) в месяц, НДС не облагается."
                    ),
                    (
                        "Оплата производится ежемесячно до 10-го числа месяца, следующего за"
                        " отчётным, на основании акта оказанных услуг."
                    ),
                ],
            ),
            (
                "ПРАВА И ОБЯЗАННОСТИ СТОРОН",
                [
                    (
                        "Исполнитель обязан оказывать услуги качественно и в сроки, установленные"
                        " графиком."
                    ),
                    (
                        "Заказчик обязан обеспечить доступ сотрудников Исполнителя к оборудованию в"
                        " рабочее время."
                    ),
                    (
                        "Исполнитель вправе привлекать третьих лиц, оставаясь ответственным за их"
                        " действия перед Заказчиком."
                    ),
                ],
            ),
            (
                "ОТВЕТСТВЕННОСТЬ СТОРОН",
                [
                    (
                        "За нарушение сроков оплаты Заказчик уплачивает неустойку в размере 0,1 %"
                        " от суммы долга за каждый день просрочки."
                    ),
                    (
                        "Стороны освобождаются от ответственности при наступлении обстоятельств"
                        " непреодолимой силы."
                    ),
                ],
            ),
            (
                "СРОК ДЕЙСТВИЯ ДОГОВОРА",
                [
                    (
                        "Договор вступает в силу с момента подписания и действует до 31 декабря"
                        " 2026 г."
                    ),
                    (
                        "Договор составлен в двух экземплярах, имеющих одинаковую юридическую силу,"
                        " по одному для каждой из сторон."
                    ),
                ],
            ),
        ],
        (
            (
                "ЗАКАЗЧИК:",
                [
                    "ООО «Северный ветер»",
                    "ул. Лесная, д. 7, г. Новоград",
                    "ИНН 0000000001",
                    "р/с 00000000000000000001",
                ],
            ),
            (
                "ИСПОЛНИТЕЛЬ:",
                [
                    "ООО «Альфа Сервис»",
                    "пр. Строителей, д. 21, г. Новоград",
                    "ИНН 0000000002",
                    "р/с 00000000000000000002",
                ],
            ),
        ),
        "Страница {page} из {pages}",
    )


def invoice_ru() -> Doc:
    """The usual Russian invoice: bank details in a bordered grid with merged cells first."""
    widths = [4600, 900, 3080]
    bank = table(
        [
            [
                cell(p(r("ПАО «Пример Банк», г. Новоград", size=9)), merge="restart"),
                cell(p(r("БИК", size=9))),
                cell(p(r("000000001", size=9))),
            ],
            [
                cell(p(), merge="continue"),
                cell(p(r("Сч. №", size=9)), merge="restart"),
                cell(p(r("30101810000000000001", size=9)), merge="restart"),
            ],
            [
                cell(p(r("Банк получателя", size=8))),
                cell(p(), merge="continue"),
                cell(p(), merge="continue"),
            ],
            [
                cell(p(r("ИНН 0000000002   КПП 000000002", size=9))),
                cell(p(r("Сч. №", size=9)), merge="restart"),
                cell(p(r("40702810000000000002", size=9)), merge="restart"),
            ],
            [
                cell(p(r("ООО «Альфа Сервис»", size=9))),
                cell(p(), merge="continue"),
                cell(p(), merge="continue"),
            ],
            [
                cell(p(r("Получатель", size=8))),
                cell(p(), merge="continue"),
                cell(p(), merge="continue"),
            ],
        ],
        widths,
        margins=(57, 57),
    )
    lines = [
        ("Техническое обслуживание инженерных систем, март 2026", "мес.", "1", "96 000,00"),
        ("Замена фильтров приточной вентиляции", "шт.", "12", "1 450,00"),
        ("Диагностика насосного оборудования", "усл.", "2", "4 800,00"),
        ("Выезд аварийной бригады в нерабочее время", "усл.", "1", "6 500,00"),
    ]
    total = sum(float(q) * float(pr.replace(" ", "").replace(",", ".")) for _, _, q, pr in lines)
    right = Tab(sum(widths), "right")
    body = (
        bank
        + p(
            r("Счёт на оплату № 112 от 12 марта 2026 г.", bold=True, size=14),
            before=12,
            after=4,
            border_bottom="000000",
        )
        + p(
            r(
                "Поставщик:\t",
            ),
            r("ООО «Альфа Сервис», ИНН 0000000002, пр. Строителей, д. 21, г. Новоград", bold=True),
            tabs=(Tab(1600),),
            left=1600,
            hanging=1600,
            before=6,
        )
        + p(
            r("Покупатель:\t"),
            r("ООО «Северный ветер», ИНН 0000000001, ул. Лесная, д. 7, г. Новоград", bold=True),
            tabs=(Tab(1600),),
            left=1600,
            hanging=1600,
            before=4,
            after=8,
        )
        + invoice_table(
            ["№", "Товары (работы, услуги)", "Ед.", "Кол-во", "Цена", "Сумма"],
            lines,
            [500, 4300, 700, 900, 1100, 1080],
            "FFFFFF",
        )
        + p(r("Итого:\t" + money(total, "", ",", " ").strip(), bold=True), tabs=(right,), before=4)
        + p(r("Без налога (НДС)\t–", bold=True), tabs=(right,))
        + p(r("Всего к оплате:\t" + money(total, "", ",", " ").strip(), bold=True), tabs=(right,))
        + p(
            r(
                f"Всего наименований {len(lines)}, на сумму"
                f" {money(total, '', ',', ' ').strip()} руб."
            ),
            before=6,
        )
        + p(
            r("Сто двадцать девять тысяч пятьсот рублей 00 копеек", bold=True),
            border_bottom="000000",
            after=12,
        )
        + p(
            r("Руководитель\t\t"),
            r("Бухгалтер\t"),
            tabs=(
                Tab(1500),
                Tab(4300, "left", "underscore"),
                Tab(6000),
                Tab(sum(widths), "left", "underscore"),
            ),
            before=12,
        )
    )
    return Doc(
        "invoice-ru",
        "Russian invoice for payment: bank details in a bordered grid with vertical and horizontal"
        " merges, labels on hanging indents, an item table, totals on right tabs, the sum in words,"
        " underscore-leader signature lines; Arial 9, A4",
        "ru-RU",
        body,
        "Arial",
        9,
        page=A4,
        margins=(cm(1.5), cm(1.5), cm(1.5), cm(2)),
        decimal=",",
        list_separator=";",
    )


def memo_ru() -> Doc:
    width = A4[0] - 2 * cm(2)
    body = (
        table(
            [
                [
                    cell(p(r("Кому:", bold=True))),
                    cell(p(r("Генеральному директору ООО «Северный ветер» А. С. Миронову"))),
                ],
                [
                    cell(p(r("От кого:", bold=True))),
                    cell(p(r("Начальника хозяйственного отдела Е. В. Тихоновой"))),
                ],
                [cell(p(r("Дата:", bold=True))), cell(p(r("12.03.2026")))],
                [
                    cell(p(r("Тема:", bold=True))),
                    cell(p(r("О замене оборудования в переговорной комнате"))),
                ],
            ],
            [1800, width - 1800],
            borders="horizontal",
            border_color="A6A6A6",
            margins=(0, 113),
        )
        + p(
            r("СЛУЖЕБНАЯ ЗАПИСКА", bold=True, size=14, spacing=2),
            align="center",
            before=14,
            after=10,
        )
        + p(
            r(
                "В переговорной комнате № 3 вышли из строя проектор и система видеоконференцсвязи."
                " Ремонт оборудования экономически нецелесообразен: срок службы истёк в 2024 году."
            ),
            align="both",
            first=cm(1.25),
            after=6,
        )
        + p(r("Прошу согласовать закупку:"), align="both", first=cm(1.25), after=2)
        + p(
            r("лазерного проектора яркостью не менее 4 000 лм – 1 шт.;"),
            numbering=(BULLET, 0),
            align="both",
        )
        + p(
            r("комплекта для видеоконференций с камерой и спикерфоном – 1 шт.;"),
            numbering=(BULLET, 0),
            align="both",
        )
        + p(
            r("настенного экрана 200 × 150 см – 1 шт."),
            numbering=(BULLET, 0),
            align="both",
            after=6,
        )
        + p(
            r(
                "Ориентировочная стоимость – 185 000 руб. Коммерческие предложения трёх поставщиков"
                " прилагаются."
            ),
            align="both",
            first=cm(1.25),
            after=18,
        )
        + p(r("Приложение: на 3 л. в 1 экз."), after=18)
        + p(
            r("Начальник хозяйственного отдела\t"), r("Е. В. Тихонова"), tabs=(Tab(width, "right"),)
        )
    )
    return Doc(
        "memo-ru",
        "Russian internal memo: a To/From/Date/Subject table with horizontal rules only, a"
        " letter-spaced centred title, justified paragraphs with first-line indent, a bulleted"
        " list, signature on a right tab; Calibri 11, A4",
        "ru-RU",
        body,
        "Calibri",
        11,
        page=A4,
        margins=(cm(2),) * 4,
        decimal=",",
        list_separator=";",
    )


# --- German ---------------------------------------------------------------------------------

PARAGRAPHS = 5  # § 1 / (1)
GERMAN_NUMBERING = STANDARD_NUMBERING + [
    (
        PARAGRAPHS,
        "4A3B2C15",
        [
            level(0, "decimal", "§ %1", 0, 0, bold=True, suffix="space"),
            level(1, "decimal", "(%2)", 567, 567),
        ],
    ),
]


def german() -> dict:
    """The German texts, kept in tests/corpus/generated-texts/de.json: they are the content
    of test documents, which the spelling check (typos, English) leaves alone there."""
    return json.loads(GERMAN_TEXTS.read_text(encoding="utf-8"))


def contract_de() -> Doc:
    t = german()["contract"]
    width = A4[0] - cm(2.5) - cm(2)
    body = (
        p(r(t["title"], bold=True, size=18), align="center", after=12)
        + p(r(t["between"]), align="center", after=4)
        + p(r(t["client"]), align="center", after=2)
        + p(r(t["client_role"], italic=True), align="center", after=6)
        + p(r(t["and"]), align="center", after=4)
        + p(r(t["contractor"]), align="center", after=2)
        + p(r(t["contractor_role"], italic=True), align="center", after=12)
    )
    for number, (heading, paragraphs) in enumerate(t["sections"], 1):
        body += p(
            r(heading, bold=True), numbering=(PARAGRAPHS, 0), before=10, after=4, keep_next=True
        )
        for index, text in enumerate(paragraphs):
            first = number == t["footnote_section"] and index == 0
            note = footnote_ref(1) if first else ""
            body += p(r(text), note, numbering=(PARAGRAPHS, 1), align="both", after=3, line=1.15)
    line = Tab(3600, "left", "underscore")
    body += p(r(t["place_date"]), before=24) + table(
        [
            [
                cell(p(r("\t"), tabs=(line,), before=30), p(r(role, size=9)))
                for role in t["signatures"]
            ]
        ],
        [width // 2, width - width // 2],
        borders="none",
        margins=(0, 0),
    )
    return Doc(
        "contract-de",
        "German services contract: centred parties, headings numbered § 1 with a space suffix,"
        " paragraphs numbered (1), justified with line spacing 1.15, a footnote, signature lines,"
        " Seite X von Y footer; Arial 11, A4",
        "de-DE",
        body,
        "Arial",
        11,
        page=A4,
        margins=(cm(2.5), cm(2), cm(2), cm(2.5)),
        footer=p(page_of(t["page"], size=9), align="center"),
        numbering=GERMAN_NUMBERING,
        decimal=",",
        list_separator=";",
        footnotes=[t["footnote"]],
    )


def invoice_de() -> Doc:
    """A DIN 5008 style invoice: return line over the address field, an info block, totals."""
    t = german()["invoice"]
    width = A4[0] - cm(2.5) - cm(2)
    right = Tab(width, "right")

    def de_money(value: float) -> str:
        return money(value, "€", ",", ".")

    aligns = ("left", "left", "right", "right", "right")
    rows = [
        [cell(p(r(h, bold=True), align=a), borders="bottom") for h, a in zip(t["columns"], aligns)]
    ]
    net = 0.0
    for number, (name, qty, price) in enumerate(t["lines"], 1):
        amount = float(qty.replace(",", ".")) * float(price.replace(".", "").replace(",", "."))
        net += amount
        rows.append(
            [
                cell(p(r(str(number)))),
                cell(p(r(name))),
                cell(p(r(qty), align="right")),
                cell(p(r(price + " €"), align="right")),
                cell(p(r(de_money(amount)), align="right")),
            ]
        )
    vat = round(net * 0.19, 2)
    address = p(r(t["return_line"], size=7, underline=True), after=4) + p(r(t["address"]))
    info = "".join(
        p(r(f"{label}\t{value}", size=9), tabs=(Tab(1700),)) for label, value in t["info"]
    )
    body = (
        table(
            [[cell(address), cell(info)]],
            [cm(8.5), width - cm(8.5)],
            borders="none",
            margins=(0, 0),
            row_heights={0: (cm(4.5), "exact")},
        )
        + p(r(t["title"], bold=True, size=14), before=18, after=8)
        + p(r(t["salutation"]), after=6)
        + p(r(t["intro"]), after=8)
        + table(rows, [700, 4400, 1000, 1300, 1550], borders="none", margins=(60, 60))
        + p(r(f"{t['net']}\t{de_money(net)}"), tabs=(right,), before=8, border_bottom="BFBFBF")
        + p(r(f"{t['vat']}\t{de_money(vat)}"), tabs=(right,))
        + p(
            r(f"{t['total']}\t{de_money(net + vat)}", bold=True),
            tabs=(right,),
            border_bottom="000000",
        )
        + p(r(t["terms"]), before=12)
        + p(r(t["closing"]), before=12)
        + p(r(t["signature"]), before=18)
    )
    third = width // 3
    footer = table(
        [[cell(p(r(text, size=7, color="595959"))) for text in t["footer"]]],
        [third, third, width - 2 * third],
        borders="none",
        margins=(0, 0),
    )
    return Doc(
        "invoice-de",
        "German invoice in the DIN 5008 manner: a return line in 7 pt underlined over the address"
        " field, an info block on tabs, both in a table row of exact height, an item table with"
        " bottom borders on the header cells only, totals on right tabs, a three-column footer"
        " table; Calibri 10, A4",
        "de-DE",
        body,
        "Calibri",
        10,
        page=A4,
        margins=(cm(2.7), cm(2), cm(2), cm(2.5)),
        footer=footer,
        decimal=",",
        list_separator=";",
    )


def letter_de() -> Doc:
    t = german()["letter"]
    body = (
        p(r(t["sender"], bold=True, size=16, color="7030A0"), align="right")
        + p(r(t["sender_line"], size=8, color="595959"), align="right", after=30)
        + p(r(t["address"]), after=24)
        + p(r(t["date"]), align="right", after=18)
        + p(r(t["subject"], bold=True), after=12)
        + p(r(t["salutation"]), after=8)
        + "".join(
            p(r(text), align="both", line=1.15, after=18 if text == t["paragraphs"][-1] else 8)
            for text in t["paragraphs"]
        )
        + p(r(t["closing"]), after=36)
        + p(r(t["name"]))
        + p(r(t["role"]), after=18)
        + p(r(t["enclosure_title"], bold=True))
        + p(r(t["enclosure"]))
    )
    return Doc(
        "letter-de",
        "German business letter: right-aligned letterhead, the address block, date on the right,"
        " bold subject, justified paragraphs with line spacing 1.15, closing and enclosure line;"
        " Times New Roman 11, A4",
        "de-DE",
        body,
        "Times New Roman",
        11,
        page=A4,
        margins=(cm(2), cm(2), cm(2), cm(2.5)),
        decimal=",",
        list_separator=";",
    )


# --- Chinese --------------------------------------------------------------------------------

ARTICLES = 6  # 第一条
CHINESE_NUMBERING = STANDARD_NUMBERING + [
    (
        ARTICLES,
        "4A3B2C16",
        [level(0, "chineseCounting", "第%1条", 0, 0, bold=True, suffix="space")],
    ),
]


def contract_zh() -> Doc:
    width = A4[0] - 2 * cm(2.5)
    hei = {"east": "SimHei", "bold": True}
    articles = [
        (
            "服务内容",
            (
                "乙方为甲方提供企业管理系统的运行维护服务，包括系统巡检、故障处理、数据备份与恢复、"
                "版本升级以及用户培训，具体内容见附件一《服务清单》。"
            ),
        ),
        (
            "服务期限",
            "本合同服务期限自2026年4月1日起至2027年3月31日止。期满前三十日内，双方可协商续签。",
        ),
        (
            "费用及支付方式",
            (
                "服务费用总额为人民币壹拾贰万元整（¥120,000.00），按季度平均支付。"
                "甲方应在每季度首月十日前，将当季费用汇入乙方指定账户。"
            ),
        ),
        (
            "双方权利义务",
            (
                "甲方应为乙方提供必要的工作条件和系统访问权限；乙方应按照约定的服务标准及时响应，"
                "一般故障四小时内处理，重大故障三十分钟内响应。"
            ),
        ),
        (
            "保密",
            (
                "双方对在履行本合同过程中知悉的对方商业秘密和技术资料负有保密义务，"
                "该义务在本合同终止后两年内继续有效。"
            ),
        ),
        (
            "违约责任",
            (
                "任何一方违反本合同约定，应赔偿由此给对方造成的直接经济损失。乙方未按约定响应的，"
                "每次按当季服务费的百分之一扣减。"
            ),
        ),
        (
            "争议解决",
            "因本合同引起的争议，双方应友好协商解决；协商不成的，任何一方均可向甲方所在地人民法院提起诉讼。",
        ),
        ("其他", "本合同一式两份，甲乙双方各执一份，自双方签字盖章之日起生效。"),
    ]
    body = (
        p(r("技术服务合同", size=18, **hei), align="center", after=12)
        + p(r("合同编号：XH-2026-018"), align="right", after=8)
        + p(r("甲方（委托方）：星河软件有限公司"))
        + p(r("乙方（服务方）：青松信息技术有限公司"), after=8)
        + p(
            r(
                "根据《中华人民共和国民法典》及有关规定，甲乙双方经友好协商，"
                "就乙方向甲方提供技术服务事宜达成如下协议："
            ),
            align="both",
            first_chars=200,
            line=1.5,
            after=6,
        )
    )
    for heading, text in articles:
        body += p(r(heading, **hei), numbering=(ARTICLES, 0), before=6, keep_next=True, line=1.5)
        body += p(r(text), align="both", first_chars=200, line=1.5)
    signatures = table(
        [
            [
                cell(
                    p(r("甲方（盖章）：星河软件有限公司"), line=1.5),
                    p(r("代表人签字："), line=1.5),
                    p(r("日期：2026年3月12日"), line=1.5),
                ),
                cell(
                    p(r("乙方（盖章）：青松信息技术有限公司"), line=1.5),
                    p(r("代表人签字："), line=1.5),
                    p(r("日期：2026年3月12日"), line=1.5),
                ),
            ]
        ],
        [width // 2, width - width // 2],
        borders="none",
        margins=(0, 0),
    )
    body += p(before=12) + signatures
    footer = p(page_of("第 {page} 页 共 {pages} 页", size=9), align="center")
    return Doc(
        "contract-zh",
        "Chinese technical services contract: SimHei title and article headings numbered 第一条"
        " (chineseCounting) with a space suffix, SimSun body justified with a two-character"
        " first-line indent (firstLineChars) and line spacing 1.5, signature table, 第 X 页 共 Y 页"
        " footer; A4",
        "zh-CN",
        body,
        "Times New Roman",
        12,
        page=A4,
        margins=(cm(2.5),) * 4,
        footer=footer,
        numbering=CHINESE_NUMBERING,
        east_asia="SimSun",
        twin=False,
    )


def quote_zh() -> Doc:
    width = A4[0] - 2 * cm(2)
    ya = "Microsoft YaHei"
    lines = [
        ("企业管理系统运维服务", "年度", "1", "96,000.00"),
        ("数据备份存储扩容", "TB", "4", "2,500.00"),
        ("用户培训（现场）", "天", "3", "1,800.00"),
        ("系统安全评估报告", "份", "1", "6,600.00"),
    ]
    rows = [
        [
            cell(p(r(h, bold=True, east=ya), align="center"), fill="DDEBF7", valign="center")
            for h in ("序号", "项目", "单位", "数量", "单价（元）", "金额（元）")
        ]
    ]
    total = 0.0
    for number, (name, unit, qty, price) in enumerate(lines, 1):
        amount = int(qty) * float(price.replace(",", ""))
        total += amount
        rows.append(
            [
                cell(p(r(str(number)), align="center")),
                cell(p(r(name))),
                cell(p(r(unit), align="center")),
                cell(p(r(qty), align="right")),
                cell(p(r(price), align="right")),
                cell(p(r(f"{amount:,.2f}"), align="right")),
            ]
        )
    rows.append(
        [
            cell(p(r("合计", bold=True)), span=5),
            cell(p(r(f"{total:,.2f}", bold=True), align="right")),
        ]
    )
    body = (
        p(r("青松信息技术有限公司", bold=True, size=14, east=ya, color="1F4E79"), align="center")
        + p(r("报 价 单", bold=True, size=20, east=ya), align="center", before=6, after=10)
        + table(
            [
                [
                    cell(p(r("客户：星河软件有限公司"))),
                    cell(p(r("报价编号：QS-Q-2026-0312"), align="right")),
                ],
                [
                    cell(p(r("联系人：王经理"))),
                    cell(p(r("报价日期：2026年3月12日"), align="right")),
                ],
            ],
            [width // 2, width - width // 2],
            borders="none",
            margins=(0, 0),
        )
        + p(after=6)
        + table(rows, [700, 3600, 900, 900, 1600, width - 7700], margins=(80, 80))
        + p(r(f"合计金额（大写）：人民币壹拾壹万捌仟元整（¥{total:,.2f}）", bold=True), before=8)
        + p(r("说明：", bold=True), before=8)
        + p(r("本报价含增值税，有效期三十天；"), numbering=(DECIMAL, 0))
        + p(r("付款方式：合同签订后支付百分之五十，验收后支付余款；"), numbering=(DECIMAL, 0))
        + p(r("如有疑问，请联系销售部，电话：000-0000-0000。"), numbering=(DECIMAL, 0))
    )
    return Doc(
        "quote-zh",
        "Chinese quotation: centred company name and spaced title in Microsoft YaHei, a borderless"
        " info table, an item table with a filled header row, a total row spanning five columns,"
        " the amount in Chinese financial numerals, a numbered list; A4",
        "zh-CN",
        body,
        "Arial",
        10.5,
        page=A4,
        margins=(cm(2),) * 4,
        east_asia=ya,
        twin=False,
    )


def notice_zh() -> Doc:
    width = A4[0] - cm(2.8) - cm(2.6)
    fang = "FangSong"

    def para(text: str) -> str:
        return p(r(text), align="both", first_chars=200, line=28, rule="exact")

    body = (
        p(
            r("关于调整夏季办公时间的通知", size=22, east="SimHei"),
            align="center",
            after=18,
            line=36,
            rule="exact",
        )
        + p(r("公司各部门："), line=28, rule="exact")
        + para(
            "根据夏季气候特点，为保障员工身体健康、提高工作效率，经公司研究决定，"
            "自2026年6月1日起至9月30日止，调整办公时间。现将有关事项通知如下："
        )
        + para("一、上午办公时间为8:30至12:00，下午办公时间为14:00至17:30。")
        + para("二、各部门应合理安排值班人员，确保对外服务和应急工作不受影响。")
        + para("三、请各部门负责人做好本部门员工的通知工作，并认真遵守考勤制度。")
        + para("特此通知。")
        + p(
            r("星河软件有限公司行政部"),
            align="right",
            before=28,
            line=28,
            rule="exact",
            right=cm(1),
        )
        + p(r("2026年3月12日"), align="right", line=28, rule="exact", right=cm(1.5))
    )
    return Doc(
        "notice-zh",
        "Chinese official notice: SimHei title, FangSong 16 pt body justified with a two-character"
        " first-line indent and exact 28 pt line spacing, sender and date right-aligned with right"
        " indents; A4",
        "zh-CN",
        body,
        "Times New Roman",
        16,
        page=A4,
        margins=(cm(3.7), cm(2.6), cm(3.5), cm(2.8)),
        east_asia=fang,
        twin=False,
    )


# --- the list --------------------------------------------------------------------------------


def documents() -> list[Doc]:
    return [
        cv_classic(),
        cv_sidebar(),
        cv_leaders(),
        cv_compact(),
        cv_academic(),
        invoice_standard(),
        invoice_decimal(),
        invoice_long(),
        invoice_merged(),
        invoice_receipt(),
        contract_uk(),
        invoice_uk(),
        letter_uk(),
        contract_ru(),
        invoice_ru(),
        memo_ru(),
        contract_de(),
        invoice_de(),
        letter_de(),
        contract_zh(),
        quote_zh(),
        notice_zh(),
    ]


def variants(docs: list[Doc]) -> list[tuple[str, Doc, bool]]:
    """(file name, document, twin) for every file this script writes."""
    out = []
    for doc in docs:
        out.append((f"{doc.name}.docx", doc, False))
        if doc.twin:
            out.append((f"{doc.name}-open.docx", doc, True))
    return out


def sources_row(name: str, doc: Doc, twin: bool) -> str:
    purpose = doc.purpose
    if twin:
        markup = "".join(x.decode("utf-8") for n, x in parts(doc, True) if n.endswith(".xml"))
        used = sorted(fonts_named(markup) & set(OPEN_FONTS.values()))
        purpose += f"; open-font twin ({', '.join(used)})"
    return (
        f"| generated/{name} | Written by scripts/make-business-docs.py; fictional people,"
        f" companies and addresses | BUSL-1.1 (this repository) | {purpose} |"
    )


def stored_parts(path: Path) -> list[tuple[str, bytes]]:
    with zipfile.ZipFile(path) as archive:
        return [(info.filename, archive.read(info)) for info in archive.infolist()]


def check(docs: list[Doc]) -> list[str]:
    """What differs between the committed documents and the ones this script writes."""
    problems = []
    expected = {name: (doc, twin) for name, doc, twin in variants(docs)}
    present = {p.name for p in OUT_DIR.glob("*")} if OUT_DIR.is_dir() else set()
    for name in sorted(present - set(expected)):
        problems.append(f"tests/corpus/generated/{name} is not written by this script")
    sources = SOURCES.read_text(encoding="utf-8")
    for name, (doc, twin) in expected.items():
        path = OUT_DIR / name
        if name not in present:
            problems.append(f"tests/corpus/generated/{name} is missing")
        elif path.read_bytes() != package(doc, twin):
            committed = f"tests/corpus/generated/{name}"
            if stored_parts(path) != parts(doc, twin):
                problems.append(f"{committed} differs from what this script writes")
            else:
                problems.append(f"{committed} has the parts this script writes in other ZIP bytes")
        if sources_row(name, doc, twin) not in sources:
            problems.append(f"tests/corpus/SOURCES.md has no up-to-date row for generated/{name}")
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="compare instead of writing")
    mode.add_argument("--list", action="store_true", help="print the SOURCES.md rows")
    args = parser.parse_args()
    docs = documents()
    if args.list:
        for name, doc, twin in variants(docs):
            print(sources_row(name, doc, twin))
        return 0
    if args.check:
        problems = check(docs)
        for problem in problems:
            print(f"make-business-docs.py: {problem}", file=sys.stderr)
        if problems:
            print(
                "make-business-docs.py: run scripts/make-business-docs.py and commit the result",
                file=sys.stderr,
            )
            return 1
        print(f"make-business-docs.py: {len(variants(docs))} documents are up to date")
        return 0
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for name, doc, twin in variants(docs):
        (OUT_DIR / name).write_bytes(package(doc, twin))
    print(
        f"make-business-docs.py: wrote {len(variants(docs))} documents to"
        f" {OUT_DIR.relative_to(ROOT)}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
