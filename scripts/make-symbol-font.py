#!/usr/bin/env python3
"""Writes the symbol font of the test corpus,
tests/fonts-word/test-symbol/OfficewrightTestSymbol-Regular.ttf, from the TrueType Noto Sans
Regular of tests/fonts-word/noto-truetype/.

Word draws text in a symbol font through the font's Windows symbol character map (`cmap`
subtable of platform 3, encoding 0), which holds the codes U+F020 to U+F0FF, and treats a
character U+0020 to U+00FF in such a font as the code U+F000 above it. No font of tests/fonts
has such a map, and the symbol fonts of Windows cannot be committed. This font is Noto Sans
Regular with one: U+F020 to U+F0FF map to the glyphs Noto Sans maps U+0020 to U+00FF to, and
no other character is mapped. It has names of its own; its OS/2 table gives the symbol
character set as its only code page, the Private Use Area as its only Unicode range, U+F020
and U+F0FF as its first and last characters, and NONE as its vendor; and every other table is
Noto Sans's, byte for byte, but for the checksum of the whole font in the head table, so its
outlines and metrics are those of Noto Sans. The SIL Open Font License 1.1 of Noto Sans covers
it, and its license file is that of Noto Sans.

    make-symbol-font.py                     write the font
    make-symbol-font.py --check             fail if the committed font is not what this writes
    make-symbol-font.py --from SRC --to OUT write it from another copy of NotoSans-Regular.ttf

scripts/check-test-fonts.sh writes it from upstream's copy and compares the two.
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "tests" / "fonts-word" / "noto-truetype" / "NotoSans-Regular.ttf"
OUT = ROOT / "tests" / "fonts-word" / "test-symbol" / "OfficewrightTestSymbol-Regular.ttf"

FAMILY = "Officewright Test Symbol"
POSTSCRIPT = "OfficewrightTestSymbol-Regular"
DESCRIPTION = (
    "Noto Sans Regular with a Windows symbol character map, for the tests of Officewright: "
    "the codes U+F020 to U+F0FF map to the glyphs of U+0020 to U+00FF."
)
# Name records kept from Noto Sans: the copyright notice, version, trademark notice, designer,
# license and license URL, and the names of its stylistic sets (256 and up), which its GSUB
# table refers to. Its family, style, unique, full and PostScript names give way to this
# font's, its description to DESCRIPTION, and its manufacturer and URLs are left out.
KEPT_NAMES = (0, 5, 7, 9, 13, 14)
FIRST_KEPT_FEATURE_NAME = 256

# The names of a symbol font are of platform 3 (Windows) and encoding 0 (symbol), like its
# character map (OpenType, `name` table); 0x409 is English (United States).
WINDOWS, SYMBOL, UNICODE_BMP, ENGLISH_US = 3, 0, 1, 0x409
FIRST, LAST = 0x20, 0xFF
SYMBOL_BASE = 0xF000
# Bit 31 of ulCodePageRange1, the symbol character set; bit 60 of the Unicode ranges, the
# Private Use Area (bit 28 of ulUnicodeRange2).
SYMBOL_CODE_PAGE = 1 << 31
PRIVATE_USE_AREA = 1 << 28


class FontError(Exception):
    pass


def tables(data: bytes) -> dict[bytes, bytes]:
    """The tables of a TrueType font, by tag."""
    if len(data) < 12 or data[:4] != b"\x00\x01\x00\x00":
        raise FontError("not a TrueType font")
    (count,) = struct.unpack_from(">H", data, 4)
    found = {}
    for i in range(count):
        if 12 + 16 * (i + 1) > len(data):
            raise FontError("the table directory runs past the end of the font")
        tag, _, offset, length = struct.unpack_from(">4sIII", data, 12 + 16 * i)
        if offset + length > len(data):
            raise FontError(f"table {tag!r} runs past the end of the font")
        found[tag] = data[offset : offset + length]
    for tag in (b"cmap", b"name", b"OS/2", b"head"):
        if tag not in found:
            raise FontError(f"no {tag.decode()} table")
    return found


def unicode_glyphs(cmap: bytes) -> dict[int, int]:
    """The glyph of each character U+0020 to U+00FF that the Windows Unicode (3, 1) subtable,
    of format 4, maps."""
    (count,) = struct.unpack_from(">H", cmap, 2)
    for i in range(count):
        platform, encoding, offset = struct.unpack_from(">HHI", cmap, 4 + 8 * i)
        if (platform, encoding) == (WINDOWS, UNICODE_BMP):
            break
    else:
        raise FontError("no Windows Unicode (3, 1) cmap subtable")
    (format_,) = struct.unpack_from(">H", cmap, offset)
    if format_ != 4:
        raise FontError(f"the (3, 1) cmap subtable has format {format_}, not 4")
    (seg_x2,) = struct.unpack_from(">H", cmap, offset + 6)
    segments = seg_x2 // 2
    ends = struct.unpack_from(f">{segments}H", cmap, offset + 14)
    starts = struct.unpack_from(f">{segments}H", cmap, offset + 16 + seg_x2)
    deltas = struct.unpack_from(f">{segments}H", cmap, offset + 16 + 2 * seg_x2)
    range_offsets_at = offset + 16 + 3 * seg_x2
    range_offsets = struct.unpack_from(f">{segments}H", cmap, range_offsets_at)
    glyphs = {}
    for code in range(FIRST, LAST + 1):
        for s in range(segments):
            if starts[s] <= code <= ends[s]:
                if range_offsets[s] == 0:
                    glyph = (code + deltas[s]) & 0xFFFF
                else:
                    at = range_offsets_at + 2 * s + range_offsets[s] + 2 * (code - starts[s])
                    (glyph,) = struct.unpack_from(">H", cmap, at)
                    if glyph:
                        glyph = (glyph + deltas[s]) & 0xFFFF
                if glyph:
                    glyphs[code] = glyph
                break
    if FIRST not in glyphs:
        raise FontError("the font maps no glyph to U+0020")
    return glyphs


def search_fields(count: int, unit: int) -> tuple[int, int, int]:
    """searchRange, entrySelector and rangeShift of a binary search over count entries of
    unit bytes."""
    power = 1
    while power * 2 <= count:
        power *= 2
    return power * unit, power.bit_length() - 1, (count - power) * unit


def symbol_cmap(glyphs: dict[int, int]) -> bytes:
    """A cmap table with one subtable, (3, 0) of format 4, mapping SYMBOL_BASE + code to the
    glyph of code: one segment per run of codes whose glyphs follow one another, through
    idDelta, then the closing segment at U+FFFF."""
    segments: list[list[int]] = []  # start, end, first glyph
    for code in sorted(glyphs):
        char, glyph = SYMBOL_BASE + code, glyphs[code]
        last = segments[-1] if segments else None
        if last and char == last[1] + 1 and glyph == last[2] + (char - last[0]):
            last[1] = char
        else:
            segments.append([char, char, glyph])
    ends = [end for _, end, _ in segments] + [0xFFFF]
    starts = [start for start, _, _ in segments] + [0xFFFF]
    deltas = [(glyph - start) & 0xFFFF for start, _, glyph in segments] + [1]
    count = len(ends)
    body = (
        struct.pack(f">{count}H", *ends)
        + b"\x00\x00"
        + struct.pack(f">{count}H", *starts)
        + struct.pack(f">{count}H", *deltas)
        + struct.pack(f">{count}H", *([0] * count))
    )
    subtable = struct.pack(">HHHH", 4, 14 + len(body), 0, 2 * count)
    subtable += struct.pack(">HHH", *search_fields(count, 2)) + body
    return struct.pack(">HHHHI", 0, 1, WINDOWS, SYMBOL, 12) + subtable


def source_names(name: bytes) -> dict[int, str]:
    """The Windows English (United States) names of the source font, by name ID."""
    format_, count, strings = struct.unpack_from(">HHH", name, 0)
    if format_ != 0:
        raise FontError(f"name table of format {format_}, not 0")
    found = {}
    for i in range(count):
        platform, encoding, language, name_id, length, offset = struct.unpack_from(
            ">HHHHHH", name, 6 + 12 * i
        )
        if (platform, encoding, language) == (WINDOWS, UNICODE_BMP, ENGLISH_US):
            raw = name[strings + offset : strings + offset + length]
            found[name_id] = raw.decode("utf-16-be")
    return found


def symbol_names(name: bytes) -> bytes:
    """A name table of format 0 with this font's names and those kept from the source, all
    Windows symbol English (United States), in name ID order."""
    source = source_names(name)
    version = source.get(5, "")
    if not version.startswith("Version "):
        raise FontError(f"the version name {version!r} does not start with 'Version '")
    names = {
        1: FAMILY,
        2: "Regular",
        3: f"{version.removeprefix('Version ')};NONE;{POSTSCRIPT}",
        4: f"{FAMILY} Regular",
        6: POSTSCRIPT,
        10: DESCRIPTION,
    }
    for name_id, value in source.items():
        if name_id in KEPT_NAMES or name_id >= FIRST_KEPT_FEATURE_NAME:
            names[name_id] = value
    records, strings = b"", b""
    for name_id in sorted(names):
        raw = names[name_id].encode("utf-16-be")
        records += struct.pack(
            ">HHHHHH", WINDOWS, SYMBOL, ENGLISH_US, name_id, len(raw), len(strings)
        )
        strings += raw
    return struct.pack(">HHH", 0, len(names), 6 + len(records)) + records + strings


def symbol_os2(os2: bytes, glyphs: dict[int, int]) -> bytes:
    """The OS/2 table with the symbol character set as the only code page, the Private Use
    Area as the only Unicode range, the first and last characters of the symbol map, and
    vendor NONE."""
    (version,) = struct.unpack_from(">H", os2, 0)
    if version < 1 or len(os2) < 86:
        raise FontError(f"OS/2 table of version {version}, without code page ranges")
    table = bytearray(os2)
    struct.pack_into(">IIII", table, 42, 0, PRIVATE_USE_AREA, 0, 0)
    table[58:62] = b"NONE"
    struct.pack_into(">HH", table, 64, SYMBOL_BASE + min(glyphs), SYMBOL_BASE + max(glyphs))
    struct.pack_into(">II", table, 78, SYMBOL_CODE_PAGE, 0)
    return bytes(table)


def checksum(table: bytes) -> int:
    padded = table + b"\x00" * (-len(table) % 4)
    return sum(struct.unpack(f">{len(padded) // 4}I", padded)) & 0xFFFFFFFF


def sfnt(found: dict[bytes, bytes]) -> bytes:
    """A TrueType font of these tables, in tag order, each padded to four bytes, with
    head.checkSumAdjustment set."""
    tags = sorted(found)
    head = bytearray(found[b"head"])
    head[8:12] = b"\x00\x00\x00\x00"
    found = {**found, b"head": bytes(head)}
    directory = struct.pack(">IHHHH", 0x00010000, len(tags), *search_fields(len(tags), 16))
    offset = 12 + 16 * len(tags)
    body = b""
    head_at = 0
    for tag in tags:
        table = found[tag]
        if tag == b"head":
            head_at = offset + len(body)
        directory += struct.pack(">4sIII", tag, checksum(table), offset + len(body), len(table))
        body += table + b"\x00" * (-len(table) % 4)
    font = bytearray(directory + body)
    struct.pack_into(">I", font, head_at + 8, (0xB1B0AFBA - checksum(bytes(font))) & 0xFFFFFFFF)
    return bytes(font)


def symbol_font(source: bytes) -> bytes:
    """The symbol font made from the bytes of NotoSans-Regular.ttf."""
    found = tables(source)
    glyphs = unicode_glyphs(found[b"cmap"])
    found[b"cmap"] = symbol_cmap(glyphs)
    found[b"name"] = symbol_names(found[b"name"])
    found[b"OS/2"] = symbol_os2(found[b"OS/2"], glyphs)
    found.pop(b"DSIG", None)
    return sfnt(found)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true", help="compare with the committed font")
    parser.add_argument("--from", dest="source", type=Path, default=SOURCE, metavar="SRC")
    parser.add_argument("--to", dest="out", type=Path, default=OUT, metavar="OUT")
    args = parser.parse_args()
    try:
        font = symbol_font(args.source.read_bytes())
    except (OSError, FontError, struct.error, UnicodeDecodeError) as error:
        print(f"make-symbol-font.py: {args.source}: {error}", file=sys.stderr)
        return 1
    if args.check:
        if not args.out.is_file() or args.out.read_bytes() != font:
            print(f"make-symbol-font.py: {args.out} is not what this script writes", file=sys.stderr)
            return 1
        return 0
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_bytes(font)
    return 0


if __name__ == "__main__":
    sys.exit(main())
