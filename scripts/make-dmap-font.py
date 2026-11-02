#!/usr/bin/env python3
"""Writes the DMAP font of the test corpus,
tests/fonts-word/test-dmap/OfficewrightTestDMAP-Regular.ttf, from the TrueType Noto Sans
Regular of tests/fonts-word/noto-truetype/, for #458: does Word read a font's DMAP table?

ISO/IEC 14496-22:2026 adds a DMAP table with the layout of cmap; a reader that knows it
looks a character up in DMAP first and falls back to cmap where DMAP maps it to no glyph.
This font is Noto Sans Regular with a DMAP table whose one subtable, Windows Unicode (3, 1)
of format 4, maps:

- U+0041 A to the glyph cmap gives B, and U+0042 B to the glyph cmap gives A, so through
  DMAP the text "AB" is drawn as B then A, and through cmap alone as A then B;
- U+E000, which cmap does not map, to the glyph cmap gives Z, so through DMAP the font
  draws it as Z, and through cmap alone it has no glyph for it.

It has names of its own, and every other table is Noto Sans's, byte for byte, but for the
checksum of the whole font in the head table. The SIL Open Font License 1.1 of Noto Sans
covers it, and its license file is that of Noto Sans.

    make-dmap-font.py                     write the font
    make-dmap-font.py --check             fail if the committed font is not what this writes
    make-dmap-font.py --from SRC --to OUT write it from another copy of NotoSans-Regular.ttf
"""

from __future__ import annotations

import argparse
import importlib.util
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "tests" / "fonts-word" / "noto-truetype" / "NotoSans-Regular.ttf"
OUT = ROOT / "tests" / "fonts-word" / "test-dmap" / "OfficewrightTestDMAP-Regular.ttf"

# The table directory, the checksums and the names of the symbol font's script.
_spec = importlib.util.spec_from_file_location(
    "make_symbol_font", ROOT / "scripts" / "make-symbol-font.py"
)
symbol = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(symbol)

FAMILY = "Officewright Test DMAP"
POSTSCRIPT = "OfficewrightTestDMAP-Regular"
DESCRIPTION = (
    "Noto Sans Regular with a DMAP table, for the tests of Officewright: through DMAP, "
    "A and B swap glyphs, and U+E000 takes the glyph of Z."
)
WINDOWS, UNICODE_BMP, ENGLISH_US = 3, 1, 0x409
# What DMAP maps each character to: the cmap glyph of another character.
DMAP = {0x41: 0x42, 0x42: 0x41, 0xE000: 0x5A}


class FontError(Exception):
    pass


def cmap_glyph(cmap: bytes, code: int) -> int:
    """The glyph the Windows Unicode (3, 1) subtable, of format 4, maps `code` to, or 0."""
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
    for s in range(segments):
        if starts[s] <= code <= ends[s]:
            if range_offsets[s] == 0:
                return (code + deltas[s]) & 0xFFFF
            at = range_offsets_at + 2 * s + range_offsets[s] + 2 * (code - starts[s])
            (glyph,) = struct.unpack_from(">H", cmap, at)
            return (glyph + deltas[s]) & 0xFFFF if glyph else 0
    return 0


def dmap_table(cmap: bytes) -> bytes:
    """A DMAP table with one subtable, (3, 1) of format 4: a segment for each character of
    DMAP, through idDelta, then the closing segment at U+FFFF."""
    mapped = {}
    for code, like in sorted(DMAP.items()):
        glyph = cmap_glyph(cmap, like)
        if glyph == 0:
            raise FontError(f"cmap maps no glyph to U+{like:04X}")
        mapped[code] = glyph
    if cmap_glyph(cmap, 0xE000) != 0:
        raise FontError("cmap maps U+E000 already")
    ends = list(mapped) + [0xFFFF]
    starts = list(mapped) + [0xFFFF]
    deltas = [(glyph - code) & 0xFFFF for code, glyph in mapped.items()] + [1]
    count = len(ends)
    body = (
        struct.pack(f">{count}H", *ends)
        + b"\x00\x00"
        + struct.pack(f">{count}H", *starts)
        + struct.pack(f">{count}H", *deltas)
        + struct.pack(f">{count}H", *([0] * count))
    )
    subtable = struct.pack(">HHHH", 4, 14 + len(body), 0, 2 * count)
    subtable += struct.pack(">HHH", *symbol.search_fields(count, 2)) + body
    return struct.pack(">HHHHI", 0, 1, WINDOWS, UNICODE_BMP, 12) + subtable


def dmap_names(name: bytes) -> bytes:
    """A name table of format 0 with this font's names and those the symbol font keeps from
    the source, all Windows Unicode English (United States), in name ID order."""
    source = symbol.source_names(name)
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
        if name_id in symbol.KEPT_NAMES or name_id >= symbol.FIRST_KEPT_FEATURE_NAME:
            names[name_id] = value
    records, strings = b"", b""
    for name_id in sorted(names):
        raw = names[name_id].encode("utf-16-be")
        records += struct.pack(
            ">HHHHHH", WINDOWS, UNICODE_BMP, ENGLISH_US, name_id, len(raw), len(strings)
        )
        strings += raw
    return struct.pack(">HHH", 0, len(names), 6 + len(records)) + records + strings


def dmap_font(source: bytes) -> bytes:
    """The DMAP font made from the bytes of NotoSans-Regular.ttf."""
    found = symbol.tables(source)
    if b"DMAP" in found:
        raise FontError("the source has a DMAP table already")
    found[b"DMAP"] = dmap_table(found[b"cmap"])
    found[b"name"] = dmap_names(found[b"name"])
    found.pop(b"DSIG", None)
    return symbol.sfnt(found)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true", help="compare with the committed font")
    parser.add_argument("--from", dest="source", type=Path, default=SOURCE, metavar="SRC")
    parser.add_argument("--to", dest="out", type=Path, default=OUT, metavar="OUT")
    args = parser.parse_args()
    try:
        font = dmap_font(args.source.read_bytes())
    except (OSError, FontError, symbol.FontError, struct.error, UnicodeDecodeError) as error:
        print(f"make-dmap-font.py: {args.source}: {error}", file=sys.stderr)
        return 1
    if args.check:
        if not args.out.is_file() or args.out.read_bytes() != font:
            print(f"make-dmap-font.py: {args.out} is not what this script writes", file=sys.stderr)
            return 1
        return 0
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_bytes(font)
    return 0


if __name__ == "__main__":
    sys.exit(main())
