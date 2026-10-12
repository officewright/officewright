#!/usr/bin/env python3
"""Imports reference renders made by scripts/windows/make-reference.ps1 into tests/corpus/.

    corpus-import.py RESULTS_DIR --group word-rules [--font-dir DIR]... [--dry-run]

RESULTS_DIR is the -OutputDir of make-reference.ps1 (for example /mnt/c/officewright-ref/out
under WSL). For every <name>.json there the script checks that

  - Word rendered the document and found no personal data in its output;
  - the input is the committed tests/corpus/<group>/<name>.docx, byte for byte;
  - the PDF and the re-saved DOCX are the files the JSON describes;
  - neither file names a person, a company or a user profile path in its metadata
    (a second check, independent of the one on Windows);
  - the re-saved copy names no language and no number format the input did not have
    (they would say where Word ran), unless --allow-locale accepts them;
  - Word embedded no font in the re-saved copy;
  - Word kept the compatibility mode of the input, and pdfinfo and pdftotext count the pages
    Word counted.

The PDF itself is not committed: it holds subsets of the fonts Word used, Microsoft's among
them. It stays with the maintainer, and its SHA-256 goes into SOURCES.md. Into
tests/corpus/reference/<group>/<name>/ go instead

  - page-001.png, ...: every page rasterized by pdftoppm at 110 dpi in 8-bit grey, the
    resolution and colour model the SSIM comparison uses;
  - geometry.json: page sizes and the position of every line and word (pdftotext
    -bbox-layout, in points from the top left of the page), the page count, Word's own page
    breaks (the w:lastRenderedPageBreak marks of the re-saved copy), and the fonts the PDF
    references without embedding them.

Word does not embed every font (Times New Roman and Arial, for example), and poppler would
draw those with substitutes of its own. So before rasterizing, the script embeds the font
Word used into a temporary copy of the PDF: the face in a --font-dir folder whose PostScript
name is the PDF's font name (under WSL, /mnt/c/Windows/Fonts and the user font folder
/mnt/c/Users/<name>/AppData/Local/Microsoft/Windows/Fonts). Only the glyph shapes come from
that file; the positions stay those of the PDF. SOURCES.md and geometry.json name each such
font with its version and SHA-256, never with its path.

The re-saved DOCX goes to tests/corpus/resaved/<group>/<name>.docx, and the document's row to
the "Reference renders" table of tests/corpus/SOURCES.md. Nothing is written unless every
document passes. Needs pdfinfo, pdffonts, pdftoppm and pdftotext (poppler-utils) and, for a
PDF that does not embed every font, qpdf 11 or later.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile
import zlib
from dataclasses import dataclass
from pathlib import Path
from typing import Callable
from xml.etree import ElementTree

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "tests" / "corpus"

# Raster resolution, as CLAUDE.md prescribes for the SSIM comparison.
RASTER_DPI = 110

# The geometry.json format this script writes.
GEOMETRY_FORMAT = 1

XHTML = "{http://www.w3.org/1999/xhtml}"

# The make-reference.ps1 output format this script reads.
SCRIPT_VERSION = 2

# The printer references are made with. Another printer is noted without its name, which
# can say where the machine is (a network printer).
REFERENCE_PRINTER = "Microsoft Print to PDF"

# Metadata values our own documents carry; anything else in a name field is refused.
ALLOWED_NAMES = {"", "Officewright"}

# A path into a user profile: C:\Users\<name>, /Users/<name>, file:///C:/Users/...
PROFILE_PATH = re.compile(rb"(?i)(\\|/)(users|documents and settings)(\\|/)")

REFERENCE_HEADER = "| File | Rendered from | Application and version | Date | Notes |"

W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
WORD_URI = "http://schemas.microsoft.com/office/word"

# Values written into the Markdown table: nothing that could break a row.
VERSION = re.compile(r"^[0-9][0-9.]*$")
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
WORD = re.compile(r"^[0-9A-Za-z]*$")
FONT = re.compile(r"^[^|\x00-\x1f]{1,64}$")
FONT_VERSION = re.compile(r"^[^|\x00-\x1f]{0,64}$")
PRINTER = re.compile(r"^[^|\x00-\x1f]{0,256}$")

REQUIRED = {
    "script_version": int,
    "file": str,
    "sha256": str,
    "resaved_file": str,
    "resaved_sha256": str,
    "pdf_file": str,
    "pdf_sha256": str,
    "status": str,
    "pages": int,
    "compatibility_mode": int,
    "fonts": list,
    "fonts_missing": list,
    "privacy_findings": list,
    "word_version": str,
    "word_build": str,
    "printer": str,
    "default_printer": str,
    "windows_version": str,
    "rendered": str,
}


class Refused(Exception):
    """A result that must not enter the corpus; the message says why."""


@dataclass(frozen=True)
class Tools:
    """The poppler tools, replaceable in tests."""

    info: Callable[[Path], str]
    xmp: Callable[[Path], str]
    fonts: Callable[[Path], str]
    # Writes one PGM file per page into the directory and returns them in page order.
    raster: Callable[[Path, Path], list[Path]]
    # pdftotext -bbox-layout output.
    bbox: Callable[[Path], str]
    # The poppler version, for the provenance notes.
    version: Callable[[], str]
    # qpdf --json=2: the PDF's objects.
    objects: Callable[[Path], str]
    # qpdf --update-from-json: writes the PDF (first path) with the update (second) applied
    # to a new file (third).
    update: Callable[[Path, Path, Path], None]


def run_tool(*args: str) -> str:
    try:
        done = subprocess.run(args, check=True, capture_output=True)
    except FileNotFoundError as error:
        package = "qpdf" if args[0] == "qpdf" else "poppler-utils"
        raise Refused(f"{args[0]} is not installed (apt install {package})") from error
    except subprocess.CalledProcessError as error:
        message = error.stderr.decode(errors="replace").strip()
        raise Refused(f"{' '.join(args)} failed: {message}") from error
    return done.stdout.decode("utf-8", errors="replace")


def in_page_order(pages: list[Path]) -> list[Path]:
    """pdftoppm's page-N.pgm files by page number (it pads N to the page count's width)."""
    return sorted(pages, key=lambda path: int(path.stem.rsplit("-", 1)[1]))


def poppler_raster(pdf: Path, directory: Path) -> list[Path]:
    run_tool("pdftoppm", "-r", str(RASTER_DPI), "-gray", str(pdf), str(directory / "page"))
    return in_page_order(list(directory.glob("page-*.pgm")))


def qpdf_update(pdf: Path, update: Path, copy: Path) -> None:
    run_tool("qpdf", str(pdf), str(copy), f"--update-from-json={update}")


def poppler_version() -> str:
    try:
        done = subprocess.run(["pdftoppm", "-v"], capture_output=True, check=False)
    except FileNotFoundError as error:
        raise Refused("pdftoppm is not installed (apt install poppler-utils)") from error
    match = re.search(r"version ([0-9.]+)", done.stderr.decode(errors="replace"))
    return match.group(1) if match else "unknown"


TOOLS = Tools(
    info=lambda path: run_tool("pdfinfo", str(path)),
    xmp=lambda path: run_tool("pdfinfo", "-meta", str(path)),
    fonts=lambda path: run_tool("pdffonts", str(path)),
    raster=poppler_raster,
    bbox=lambda path: run_tool("pdftotext", "-bbox-layout", str(path), "-"),
    version=poppler_version,
    objects=lambda path: run_tool("qpdf", "--json=2", str(path)),
    update=qpdf_update,
)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_record(path: Path) -> dict:
    """The JSON of one document, with its fields checked."""
    try:
        # PowerShell may write a byte order mark.
        record = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, ValueError) as error:
        raise Refused(f"{path.name}: cannot read it: {error}") from error
    if not isinstance(record, dict):
        raise Refused(f"{path.name}: not a JSON object")
    for key, kind in REQUIRED.items():
        value = record.get(key)
        # ConvertTo-Json in Windows PowerShell 5.1 writes a one-element array as its element.
        if kind is list and isinstance(value, str):
            value = record[key] = [value]
        if kind is list and value is None:
            value = record[key] = []
        if not isinstance(value, kind) or isinstance(value, bool):
            raise Refused(f"{path.name}: field {key} is missing or not a {kind.__name__}")
    if record["script_version"] != SCRIPT_VERSION:
        raise Refused(
            f"{path.name}: written by make-reference.ps1 format {record['script_version']}, "
            f"this script reads format {SCRIPT_VERSION}"
        )
    if record["status"] != "rendered":
        detail = record.get("error") or ", ".join(record["privacy_findings"])
        raise Refused(f"{record['file']}: status {record['status']}: {detail}")
    if record["privacy_findings"]:
        findings = ", ".join(record["privacy_findings"])
        raise Refused(f"{record['file']}: personal data: {findings}")
    file = record["file"]
    if re.search(r"[\\/:]", file) or not file.endswith(".docx") or file == ".docx":
        raise Refused(f"{path.name}: file must be the name of a .docx file, not {file!r}")
    stem = file[: -len(".docx")]
    for key, expected in (("pdf_file", f"{stem}.pdf"), ("resaved_file", f"{stem}.resaved.docx")):
        if record[key] != expected:
            raise Refused(f"{path.name}: {key} must be {expected!r}, not {record[key]!r}")
    checks = [
        ("word_version", VERSION),
        ("word_build", VERSION),
        ("windows_version", VERSION),
        ("rendered", DATE),
    ]
    for key, pattern in checks:
        if not pattern.match(record[key]):
            raise Refused(f"{path.name}: {key} {record[key]!r} is not in the expected form")
    for key in ("printer", "default_printer"):
        if not PRINTER.match(record[key]):
            raise Refused(f"{path.name}: {key} is not a printer name")
    if not WORD.match(str(record.get("windows_display_version") or "")):
        raise Refused(f"{path.name}: windows_display_version is not in the expected form")
    for font in record["fonts_missing"]:
        if not isinstance(font, str) or not FONT.match(font):
            raise Refused(f"{path.name}: fonts_missing holds {font!r}, which is not a font name")
    return record


def local_name(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def docx_privacy_problems(path: Path) -> list[str]:
    """Personal data in a DOCX package: names in the document properties, profile paths."""
    problems = []
    with zipfile.ZipFile(path) as archive:
        for info in archive.infolist():
            data = archive.read(info)
            if PROFILE_PATH.search(data):
                problems.append(f"{info.filename} contains a user profile path")
            if info.filename in ("docProps/core.xml", "docProps/app.xml"):
                try:
                    root = ElementTree.fromstring(data)
                except ElementTree.ParseError as error:
                    problems.append(f"{info.filename} is not well-formed: {error}")
                    continue
                for element in root:
                    name = local_name(element.tag)
                    value = (element.text or "").strip()
                    if name in ("creator", "lastModifiedBy") and value not in ALLOWED_NAMES:
                        problems.append(f"{info.filename}: {name} is {value!r}")
                    if name in ("Company", "Manager") and value:
                        problems.append(f"{info.filename}: {name} is {value!r}")
    return problems


def read_parts(path: Path) -> dict[str, bytes]:
    with zipfile.ZipFile(path) as archive:
        return {info.filename: archive.read(info) for info in archive.infolist()}


def settings_root(parts: dict[str, bytes]) -> ElementTree.Element | None:
    data = parts.get("word/settings.xml")
    return ElementTree.fromstring(data) if data else None


def compatibility_mode(parts: dict[str, bytes]) -> int:
    """The compatibility mode a package asks for; 12 when it names none ([MS-DOCX] 2.3.5)."""
    root = settings_root(parts)
    if root is not None:
        for setting in root.iter(f"{W}compatSetting"):
            if (
                setting.get(f"{W}name") == "compatibilityMode"
                and setting.get(f"{W}uri") == WORD_URI
            ):
                return int(setting.get(f"{W}val", "12"))
    return 12


def compat_flags(parts: dict[str, bytes]) -> set[str]:
    """What w:compat sets: legacy flags by name, compatSetting elements as name=value."""
    root = settings_root(parts)
    compat = root.find(f"{W}compat") if root is not None else None
    if compat is None:
        return set()
    flags = set()
    for element in compat:
        name = local_name(element.tag)
        value = element.get(f"{W}val", "true")
        if name == "compatSetting":
            flags.add(f"{element.get(f'{W}name', '')}={value}")
        elif value not in ("0", "false", "off"):
            flags.add(name)
    return flags


def locale_marks(parts: dict[str, bytes]) -> tuple[set[str], dict[str, str]]:
    """The language tags a package names, and its decimal symbol and list separator."""
    languages = set()
    for name, data in parts.items():
        if not (name.startswith("word/") and name.endswith(".xml")):
            continue
        root = ElementTree.fromstring(data)
        for element in root.iter():
            if local_name(element.tag) in ("lang", "themeFontLang"):
                languages.update(v for v in element.attrib.values() if v and v != "x-none")
    numbers = {}
    root = settings_root(parts)
    if root is not None:
        for key in ("decimalSymbol", "listSeparator"):
            element = root.find(f"{W}{key}")
            if element is not None:
                numbers[key] = element.get(f"{W}val", "")
    return languages, numbers


def locale_problems(original: dict[str, bytes], resaved: dict[str, bytes]) -> list[str]:
    """Languages and number formats Word added: they tell where Word ran."""
    before_languages, before_numbers = locale_marks(original)
    after_languages, after_numbers = locale_marks(resaved)
    problems = [
        f"the re-saved copy names the language {tag}"
        for tag in sorted(after_languages - before_languages)
    ]
    for key, value in sorted(after_numbers.items()):
        if before_numbers.get(key) != value:
            problems.append(f"the re-saved copy has {key} {value!r}")
    return problems


def embedded_font_problems(parts: dict[str, bytes]) -> list[str]:
    """Fonts Word embedded in a package: they must not reach the repository."""
    problems = [f"{name} is an embedded font" for name in parts if name.startswith("word/fonts/")]
    for name in ("word/fontTable.xml", "word/settings.xml"):
        if name not in parts:
            continue
        for element in ElementTree.fromstring(parts[name]).iter():
            if local_name(element.tag) in (
                "embedRegular", "embedBold", "embedItalic", "embedBoldItalic",
                "embedTrueTypeFonts",
            ):
                problems.append(f"{name} has {local_name(element.tag)}")
    return problems


def png_from_pgm(data: bytes) -> bytes:
    """An 8-bit grey PNG from a binary PGM (P5) with a maximum value of 255."""
    fields = []
    position = 0
    while len(fields) < 4:
        while data[position : position + 1].isspace():
            position += 1
        if data[position : position + 1] == b"#":
            position = data.index(b"\n", position) + 1
            continue
        end = position
        while not data[end : end + 1].isspace():
            end += 1
        fields.append(data[position:end])
        position = end
    magic, width, height, maximum = fields[0], int(fields[1]), int(fields[2]), int(fields[3])
    if magic != b"P5" or maximum != 255:
        raise Refused(f"pdftoppm wrote an unexpected image ({magic!r}, maximum {maximum})")
    pixels = data[position + 1 :]
    if len(pixels) != width * height:
        raise Refused("pdftoppm wrote an image of the wrong size")
    rows = b"".join(b"\x00" + pixels[y * width : (y + 1) * width] for y in range(height))

    def chunk(kind: bytes, body: bytes) -> bytes:
        crc = zlib.crc32(kind + body) & 0xFFFFFFFF
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", crc)

    header = struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)
    # The resolution, in pixels per metre, so the file says its scale.
    per_metre = round(RASTER_DPI / 0.0254)
    physical = struct.pack(">IIB", per_metre, per_metre, 1)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"pHYs", physical)
        + chunk(b"IDAT", zlib.compress(rows, 9))
        + chunk(b"IEND", b"")
    )


def box(element: ElementTree.Element) -> list[float]:
    return [round(float(element.get(key, "0")), 3) for key in ("xMin", "yMin", "xMax", "yMax")]


def page_geometry(bbox: str) -> list[dict]:
    """Pages, lines and words from pdftotext -bbox-layout output."""
    root = ElementTree.fromstring(bbox)
    pages = []
    for number, page in enumerate(root.iter(f"{XHTML}page"), 1):
        lines = []
        for line in page.iter(f"{XHTML}line"):
            words = [
                {"text": word.text or "", "box": box(word)} for word in line.iter(f"{XHTML}word")
            ]
            text = " ".join(word["text"] for word in words)
            lines.append({"text": text, "box": box(line), "words": words})
        pages.append(
            {
                "number": number,
                "width": round(float(page.get("width", "0")), 3),
                "height": round(float(page.get("height", "0")), 3),
                "lines": lines,
            }
        )
    return pages


def rendered_page_breaks(parts: dict[str, bytes]) -> list[dict]:
    """Word's page breaks: the w:lastRenderedPageBreak marks of a saved document.

    Paragraphs are counted in document order from 0, table cells included; the offset is
    the number of characters of the paragraph's text before the mark.
    """
    root = ElementTree.fromstring(parts["word/document.xml"])
    breaks = []
    for index, paragraph in enumerate(root.iter(f"{W}p")):
        text = ""
        marks = []
        for element in paragraph.iter():
            name = local_name(element.tag)
            if name == "t":
                text += element.text or ""
            elif name == "tab":
                text += "\t"
            elif name == "lastRenderedPageBreak":
                marks.append(len(text))
        for offset in marks:
            breaks.append(
                {
                    "paragraph": index,
                    "offset": offset,
                    "text_before": text[max(0, offset - 30) : offset],
                    "text_after": text[offset : offset + 30],
                }
            )
    return breaks


def pdf_privacy_problems(info: str, xmp: str, data: bytes) -> list[str]:
    """Personal data in a PDF: the Author entry, XMP creator fields, profile paths."""
    problems = []
    for line in info.splitlines():
        key, _, value = line.partition(":")
        if key.strip() == "Author" and value.strip() not in ALLOWED_NAMES:
            problems.append(f"PDF Author is {value.strip()!r}")
    if xmp.strip():
        for tag in ("dc:creator", "pdf:Author", "xmp:Author"):
            for match in re.finditer(rf"<{tag}>(.*?)</{tag}>", xmp, re.S):
                names = re.findall(r"<rdf:li[^>]*>(.*?)</rdf:li>", match.group(1), re.S)
                for name in names or [match.group(1)]:
                    if name.strip() not in ALLOWED_NAMES:
                        problems.append(f"PDF XMP {tag} is {name.strip()!r}")
    if PROFILE_PATH.search(data):
        problems.append("PDF contains a user profile path")
    return problems


def pdf_pages(info: str) -> int:
    match = re.search(r"^Pages:\s+(\d+)\s*$", info, re.M)
    if not match:
        raise Refused("pdfinfo reports no page count")
    return int(match.group(1))


def not_embedded(pdffonts: str) -> list[str]:
    """Fonts pdffonts lists with emb = no.

    pdffonts prints a header, a dashed rule and a row per font: name, type, encoding, emb,
    sub, uni, object, generation.
    """
    lines = pdffonts.splitlines()
    rule = next((i for i, line in enumerate(lines) if line.startswith("---")), None)
    if rule is None:
        return []
    names = []
    for line in lines[rule + 1 :]:
        # Read from the right: a long name pushes the columns aside, and the type may hold
        # spaces ("CID TrueType"), but the last five fields are always emb, sub, uni, object
        # and generation, and a PDF font name has no spaces.
        fields = line.split()
        if len(fields) < 7:
            continue
        if fields[-5] == "no":
            names.append(re.sub(r"^[A-Z]{6}\+", "", fields[0]))
    return sorted(set(names))


FONT_SUFFIXES = {".ttf", ".otf", ".ttc", ".otc"}
SFNT_VERSIONS = {b"\x00\x01\x00\x00", b"OTTO", b"true"}


class FontFormatError(ValueError):
    """A file in a --font-dir folder that is not a font this script reads; it is skipped."""


@dataclass(frozen=True)
class FontFace:
    """One face of a font file in a --font-dir folder."""

    path: Path
    # The face's index in a collection (.ttc), 0 in a single font.
    index: int
    collection: bool
    postscript: str
    full_name: str
    version: str
    # CFF outlines, which a PDF embeds as /FontFile3 /Subtype /OpenType.
    cff: bool


def name_strings(table: bytes) -> dict[int, str]:
    """The strings of a name table by name ID, in US English where the font has it."""
    if len(table) < 6:
        raise FontFormatError("name table too short")
    count, storage = struct.unpack(">HH", table[2:6])
    best: dict[int, tuple[int, str]] = {}
    for record in range(count):
        start = 6 + 12 * record
        if start + 12 > len(table):
            raise FontFormatError("name records past the end of the table")
        platform, encoding, language, name_id, length, offset = struct.unpack(
            ">HHHHHH", table[start : start + 12]
        )
        data = table[storage + offset : storage + offset + length]
        if len(data) != length:
            continue
        if platform == 3 and encoding in (0, 1, 10):
            rank = 3 if language == 0x409 else 2
            text = data.decode("utf-16-be", errors="replace")
        elif platform == 1 and encoding == 0:
            rank, text = 1, data.decode("mac_roman")
        else:
            continue
        if rank > best.get(name_id, (0, ""))[0]:
            best[name_id] = (rank, text.strip())
    return {name_id: text for name_id, (_, text) in best.items()}


def table_directory(read: Callable[[int, int], bytes], offset: int) -> dict[bytes, tuple[int, int]]:
    """The tables of the face whose offset table is at offset: tag -> (offset, length)."""
    if read(offset, 4) not in SFNT_VERSIONS:
        raise FontFormatError("not a TrueType or OpenType font")
    (count,) = struct.unpack(">H", read(offset + 4, 2))
    records = read(offset + 12, 16 * count)
    tables = {}
    for start in range(0, len(records), 16):
        tag, _, table_offset, length = struct.unpack(">4sIII", records[start : start + 16])
        tables[tag] = (table_offset, length)
    return tables


def font_faces(path: Path) -> list[FontFace]:
    """The faces of a font file, read from its table directories and name tables only."""
    with path.open("rb") as file:

        def read(offset: int, size: int) -> bytes:
            file.seek(offset)
            data = file.read(size)
            if len(data) != size:
                raise FontFormatError("truncated")
            return data

        collection = read(0, 4) == b"ttcf"
        if collection:
            (count,) = struct.unpack(">I", read(8, 4))
            if count > 1024:
                raise FontFormatError("implausible collection")
            offsets = struct.unpack(f">{count}I", read(12, 4 * count))
        else:
            offsets = (0,)
        faces = []
        for index, offset in enumerate(offsets):
            tables = table_directory(read, offset)
            if b"name" not in tables:
                continue
            names = name_strings(read(*tables[b"name"]))
            postscript = names.get(6, "")
            if not postscript:
                continue
            faces.append(
                FontFace(
                    path=path,
                    index=index,
                    collection=collection,
                    postscript=postscript,
                    full_name=names.get(4, postscript),
                    version=names.get(5, ""),
                    cff=b"CFF " in tables or b"CFF2" in tables,
                )
            )
        return faces


def font_index(directories: list[Path]) -> dict[str, list[FontFace]]:
    """The faces in the folders by PostScript name; files that are no fonts are skipped."""
    index: dict[str, list[FontFace]] = {}
    for directory in directories:
        if not directory.is_dir():
            raise Refused(f"--font-dir {directory} is not a folder")
        for path in sorted(directory.iterdir()):
            if path.suffix.lower() not in FONT_SUFFIXES or not path.is_file():
                continue
            try:
                faces = font_faces(path)
            except (OSError, FontFormatError, struct.error):
                continue
            for face in faces:
                index.setdefault(face.postscript, []).append(face)
    return index


def face_bytes(face: FontFace) -> bytes:
    """The face as a font file of its own: the file itself, or the face cut out of a
    collection, with its tables copied after a new table directory."""
    data = face.path.read_bytes()
    if not face.collection:
        return data
    (offset,) = struct.unpack(">I", data[12 + 4 * face.index : 16 + 4 * face.index])

    def read(start: int, size: int) -> bytes:
        chunk = data[start : start + size]
        if len(chunk) != size:
            raise FontFormatError("truncated")
        return chunk

    tables = sorted(table_directory(read, offset).items())
    count = len(tables)
    power = 1 << (count.bit_length() - 1)
    header = read(offset, 4) + struct.pack(
        ">HHHH", count, power * 16, power.bit_length() - 1, count * 16 - power * 16
    )
    position = 12 + 16 * count
    directory, body = b"", b""
    for tag, (start, length) in tables:
        padded = read(start, length).ljust((length + 3) // 4 * 4, b"\0")
        checksum = sum(struct.unpack(f">{len(padded) // 4}I", padded)) & 0xFFFFFFFF
        directory += struct.pack(">4sIII", tag, checksum, position + len(body), length)
        body += padded
    return header + directory + body


@dataclass(frozen=True)
class DrawnFont:
    """A font the PDF does not embed, and the face the rasters draw it with."""

    name: str
    face: FontFace
    data: bytes

    def describe(self) -> dict:
        entry = {
            "font": self.name,
            "full_name": self.face.full_name,
            "version": self.face.version,
            "sha256": hashlib.sha256(self.face.path.read_bytes()).hexdigest(),
        }
        if self.face.collection:
            entry["face_in_collection"] = self.face.index
        return entry


def drawn_font(name: str, index: dict[str, list[FontFace]]) -> DrawnFont:
    """The face for a font the PDF names without embedding it; refused unless exactly one."""
    if "," in name:
        raise Refused(
            f"{name}: Word names a style it synthesizes after the comma, which an embedded "
            "face would lose"
        )
    faces = index.get(name, [])
    if not faces:
        raise Refused(
            f"{name} is not embedded in the PDF, and no --font-dir folder has a font with "
            "that PostScript name (under WSL: /mnt/c/Windows/Fonts, and "
            "/mnt/c/Users/<name>/AppData/Local/Microsoft/Windows/Fonts for fonts installed "
            "for one user)"
        )
    by_content = {}
    for face in faces:
        by_content.setdefault(face_bytes(face), face)
    if len(by_content) > 1:
        files = ", ".join(sorted({face.path.name for face in faces}))
        raise Refused(f"{name}: --font-dir folders hold different fonts of that name: {files}")
    ((data, face),) = by_content.items()
    if not FONT.match(face.full_name) or not FONT_VERSION.match(face.version):
        raise Refused(
            f"{name}: unexpected name or version in {face.path.name}: "
            f"{face.full_name!r}, {face.version!r}"
        )
    return DrawnFont(name, face, data)


def embedding_update(objects: dict, fonts: dict[str, Path], cff: dict[str, bool]) -> dict:
    """A qpdf JSON update that embeds each font file into the font descriptors that name
    the font and embed nothing; fonts maps the PDF font name to the file to embed."""
    header, body = objects["qpdf"]
    next_id = header["maxobjectid"]
    update = {}
    for key, value in sorted(body.items()):
        descriptor = value.get("value") if isinstance(value, dict) else None
        if not isinstance(descriptor, dict) or descriptor.get("/Type") != "/FontDescriptor":
            continue
        name = str(descriptor.get("/FontName", ""))[1:]
        if name not in fonts or any(
            k in descriptor for k in ("/FontFile", "/FontFile2", "/FontFile3")
        ):
            continue
        next_id += 1
        path = fonts[name]
        if cff[name]:
            stream, entry = {"/Subtype": "/OpenType"}, "/FontFile3"
        else:
            stream, entry = {"/Length1": path.stat().st_size}, "/FontFile2"
        update[f"obj:{next_id} 0 R"] = {"stream": {"dict": stream, "datafile": str(path)}}
        update[key] = {"value": {**descriptor, entry: f"{next_id} 0 R"}}
    return {
        "qpdf": [
            {
                "jsonversion": 2,
                "pushedinheritedpageresources": False,
                "calledgetallpages": False,
                "maxobjectid": next_id,
            },
            update,
        ]
    }


def with_fonts_embedded(pdf: Path, drawn: list[DrawnFont], directory: Path, tools: Tools) -> Path:
    """A copy of the PDF in the directory with the drawn fonts embedded."""
    files, cff = {}, {}
    for number, font in enumerate(drawn):
        path = directory / f"font-{number}.bin"
        path.write_bytes(font.data)
        files[font.name] = path
        cff[font.name] = font.face.cff
    try:
        objects = json.loads(tools.objects(pdf))
        update = embedding_update(objects, files, cff)
    except (json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
        raise Refused(f"{pdf.name}: cannot read qpdf's JSON: {error}") from error
    update_path = directory / "embed.json"
    update_path.write_text(json.dumps(update), encoding="utf-8")
    copy = directory / "embedded.pdf"
    tools.update(pdf, update_path, copy)
    left = not_embedded(tools.fonts(copy))
    if left:
        raise Refused(f"{pdf.name}: still not embedded after embedding: {', '.join(left)}")
    return copy


def reference_row(
    group: str, record: dict, drawn: list[DrawnFont], poppler: str, dropped: list[str]
) -> str:
    base = Path(record["file"]).stem
    windows = record["windows_version"]
    display = record.get("windows_display_version") or ""
    if display:
        windows = f"{windows} ({display})"
    printer = f"printer {REFERENCE_PRINTER}"
    if record["printer"] != REFERENCE_PRINTER:
        printer = f"printer other than {REFERENCE_PRINTER}, name not recorded"
    notes = [
        f"compatibility mode {record['compatibility_mode']}",
        f"{record['pages']} page{'s' if record['pages'] != 1 else ''}",
        printer,
        "fonts Word substituted: " + (", ".join(record["fonts_missing"]) or "none"),
        "w:compat settings Word dropped from the re-saved copy: " + (", ".join(dropped) or "none"),
        "fields as saved",
        f"PDF not committed, SHA-256 {record['pdf_sha256']}",
        f"rasters and geometry by poppler {poppler} at {RASTER_DPI} dpi, grey",
        "fonts the PDF does not embed, rasterized with the maintainer's installed copies: "
        + (
            ", ".join(
                f"{font.name} ({font.face.full_name}, {font.face.version or 'no version'}, "
                f"SHA-256 {font.describe()['sha256']})"
                for font in drawn
            )
            or "none"
        ),
    ]
    return (
        f"| reference/{group}/{base}/ "
        f"| {group}/{record['file']}, re-saved by Word as resaved/{group}/{base}.docx "
        f"| Microsoft Word {record['word_version']}, build {record['word_build']}, "
        f"Windows {windows} "
        f"| {record['rendered']} "
        f"| {'; '.join(notes)} |"
    )


def upsert_reference_row(sources: str, row: str) -> str:
    """SOURCES.md with the row in the reference table, replacing a row for the same file."""
    lines = sources.split("\n")
    try:
        header = lines.index(REFERENCE_HEADER)
    except ValueError as error:
        raise Refused(f"SOURCES.md has no table header {REFERENCE_HEADER!r}") from error
    key = row.split("|")[1].strip()
    end = header + 2  # past the header and the separator
    while end < len(lines) and lines[end].startswith("|"):
        if lines[end].split("|")[1].strip() == key:
            lines[end] = row
            return "\n".join(lines)
        end += 1
    rows = sorted(lines[header + 2 : end] + [row])
    return "\n".join(lines[: header + 2] + rows + lines[end:])


@dataclass(frozen=True)
class Checked:
    record: dict
    pdf: Path
    resaved: Path
    row: str
    geometry: dict
    drawn: list[DrawnFont]


def check_result(
    json_path: Path,
    group: str,
    corpus: Path,
    tools: Tools,
    allow_locale: bool = False,
    fonts: dict[str, list[FontFace]] | None = None,
) -> Checked:
    record = load_record(json_path)
    results = json_path.parent
    original = corpus / group / record["file"]
    if not original.is_file():
        raise Refused(f"{record['file']}: tests/corpus/{group}/ has no such document")
    if sha256(original) != record["sha256"]:
        raise Refused(f"{record['file']}: Word rendered other bytes than the committed file")
    pdf = results / record["pdf_file"]
    resaved = results / record["resaved_file"]
    for path, key in ((pdf, "pdf_sha256"), (resaved, "resaved_sha256")):
        if not path.is_file():
            raise Refused(f"{record['file']}: {path.name} is missing")
        if sha256(path) != record[key]:
            raise Refused(f"{record['file']}: {path.name} changed after Word wrote it")
    info = tools.info(pdf)
    try:
        original_parts = read_parts(original)
        resaved_parts = read_parts(resaved)
        problems = docx_privacy_problems(resaved)
        problems += embedded_font_problems(resaved_parts)
        if not allow_locale:
            problems += locale_problems(original_parts, resaved_parts)
        mode = compatibility_mode(original_parts)
        resaved_mode = compatibility_mode(resaved_parts)
        # Word drops a legacy flag it does not apply in the document's mode, and adds
        # settings of its own (useWord2013TrackBottomHyphenation). The render is still
        # Word's for this input, so the import records the difference rather than refusing.
        dropped = sorted(compat_flags(original_parts) - compat_flags(resaved_parts))
        added = sorted(compat_flags(resaved_parts) - compat_flags(original_parts))
        page_breaks = rendered_page_breaks(resaved_parts)
    except (zipfile.BadZipFile, ElementTree.ParseError, ValueError, KeyError) as error:
        raise Refused(f"{record['file']}: cannot read a package: {error}") from error
    problems += pdf_privacy_problems(info, tools.xmp(pdf), pdf.read_bytes())
    if problems:
        raise Refused(f"{record['file']}: {'; '.join(problems)}")
    for source, value in (
        ("Word reported", record["compatibility_mode"]),
        ("the re-saved copy has", resaved_mode),
    ):
        if value != mode:
            raise Refused(
                f"{record['file']}: {source} compatibility mode {value}, the input asks for {mode}"
            )
    pages = pdf_pages(info)
    if pages != record["pages"]:
        raise Refused(
            f"{record['file']}: Word counted {record['pages']} pages, the PDF has {pages}"
        )
    try:
        geometry_pages = page_geometry(tools.bbox(pdf))
    except (ElementTree.ParseError, ValueError) as error:
        raise Refused(f"{record['file']}: cannot read pdftotext's output: {error}") from error
    if len(geometry_pages) != pages:
        raise Refused(f"{record['file']}: pdftotext found {len(geometry_pages)} pages, not {pages}")
    missing_embeds = not_embedded(tools.fonts(pdf))
    try:
        drawn = [drawn_font(name, fonts or {}) for name in missing_embeds]
    except Refused as error:
        raise Refused(f"{record['file']}: {error}") from error
    poppler = tools.version()
    geometry = {
        "format": GEOMETRY_FORMAT,
        "document": f"{group}/{record['file']}",
        "pdf_sha256": record["pdf_sha256"],
        "made_with": f"pdftotext {poppler} -bbox-layout",
        "units": "points, origin at the top left of the page, y downwards",
        "page_count": pages,
        "fonts_not_embedded": missing_embeds,
        "fonts_rasterized_with": [font.describe() for font in drawn],
        "compat_flags_dropped": dropped,
        "compat_flags_added": added,
        "page_breaks": page_breaks,
        "pages": geometry_pages,
    }
    row = reference_row(group, record, drawn, poppler, dropped)
    return Checked(record, pdf, resaved, row, geometry, drawn)


def import_results(
    results: Path,
    group: str,
    corpus: Path,
    tools: Tools,
    dry_run: bool,
    allow_locale: bool = False,
    font_dirs: list[Path] | None = None,
) -> list[Checked]:
    if not (corpus / group).is_dir():
        raise Refused(f"tests/corpus/{group} does not exist")
    fonts = font_index(font_dirs or [])
    records = sorted(p for p in results.glob("*.json") if p.name != "run.json")
    if not records:
        raise Refused(f"no results in {results}")
    checked = []
    errors = []
    for path in records:
        try:
            checked.append(check_result(path, group, corpus, tools, allow_locale, fonts))
        except Refused as error:
            errors.append(str(error))
    if errors:
        raise Refused("nothing imported:\n  " + "\n  ".join(errors))
    if dry_run:
        return checked
    # Rasterize everything before writing anything, so a failure leaves the corpus as it was.
    rasters = {}
    with tempfile.TemporaryDirectory() as scratch:
        for item in checked:
            directory = Path(scratch) / Path(item.record["file"]).stem
            directory.mkdir()
            pdf = item.pdf
            if item.drawn:
                pdf = with_fonts_embedded(item.pdf, item.drawn, directory, tools)
            pages = [png_from_pgm(page.read_bytes()) for page in tools.raster(pdf, directory)]
            if len(pages) != item.geometry["page_count"]:
                raise Refused(
                    f"{item.record['file']}: pdftoppm wrote {len(pages)} pages, "
                    f"not {item.geometry['page_count']}"
                )
            rasters[item.record["file"]] = pages
    sources_path = corpus / "SOURCES.md"
    sources = sources_path.read_text(encoding="utf-8")
    for item in checked:
        base = Path(item.record["file"]).stem
        reference = corpus / "reference" / group / base
        if reference.exists():
            shutil.rmtree(reference)
        reference.mkdir(parents=True)
        for number, png in enumerate(rasters[item.record["file"]], 1):
            (reference / f"page-{number:03}.png").write_bytes(png)
        geometry = json.dumps(item.geometry, indent=1, ensure_ascii=False) + "\n"
        (reference / "geometry.json").write_text(geometry, encoding="utf-8")
        resaved = corpus / "resaved" / group / f"{base}.docx"
        resaved.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(item.resaved, resaved)
        sources = upsert_reference_row(sources, item.row)
    sources_path.write_text(sources, encoding="utf-8")
    return checked


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("results", type=Path, help="the -OutputDir of make-reference.ps1")
    parser.add_argument(
        "--group", required=True, help="the folder under tests/corpus/, e.g. word-rules"
    )
    parser.add_argument(
        "--font-dir",
        type=Path,
        action="append",
        default=[],
        help="a folder with the fonts Word used, for fonts the PDFs do not embed (repeatable)",
    )
    parser.add_argument("--dry-run", action="store_true", help="check everything, copy nothing")
    parser.add_argument(
        "--allow-locale",
        action="store_true",
        help="accept language tags and number formats Word added (the maintainer decides)",
    )
    args = parser.parse_args()
    try:
        checked = import_results(
            args.results,
            args.group,
            CORPUS,
            TOOLS,
            args.dry_run,
            args.allow_locale,
            args.font_dir,
        )
    except Refused as error:
        print(f"corpus-import.py: {error}", file=sys.stderr)
        return 1
    verb = "would import" if args.dry_run else "imported"
    for item in checked:
        pages = item.record["pages"]
        print(f"{verb} {item.record['file']}: {pages} page{'s' if pages != 1 else ''}")
        for flag in item.geometry["compat_flags_dropped"]:
            print(f"  note: Word dropped the w:compat setting {flag} from the re-saved copy")
        for font in item.drawn:
            print(
                f"  note: {font.name} is not embedded; rasterized with "
                f"{font.face.full_name} {font.face.version}"
            )
    print(f"corpus-import.py: {verb} {len(checked)} reference renders")
    return 0


if __name__ == "__main__":
    sys.exit(main())
