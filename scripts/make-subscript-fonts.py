#!/usr/bin/env python3
"""Writes the two subscript fonts of the test corpus from the TrueType Noto Sans Regular of
tests/fonts-word/noto-truetype/, for #586: how far does Word drop a subscript?

- tests/fonts-word/test-descent/OfficewrightTestDescent-Regular.ttf, Officewright Test
  Descent: Noto Sans with its descent 300 units (0.3 em) deeper in each of the three fields
  that give one: the hhea table's descender (-293 to -593), and the OS/2 table's
  sTypoDescender (-293 to -593) and usWinDescent (395 to 695). The layout reads
  sTypoDescender for this font, a TrueType one that sets USE_TYPO_METRICS, and would read
  usWinDescent for one that does not; GDI, through which Word measures text, gives
  usWinDescent as a font's descent; and the hhea descender is the one other descent a font
  gives. Moving all three by as much keeps them as far apart as Noto Sans has them, so
  whichever Word reads, it reads 0.3 em more, and the external leading GDI adds (the hhea
  line less the Windows one, none in Noto Sans) stays as it was.
- tests/fonts-word/test-subscript/OfficewrightTestSubscript-Regular.ttf, Officewright Test
  Subscript: Noto Sans with an OS/2 ySubscriptYOffset of 300 units (0.3 em), four times its
  75.

Each has names of its own, and its OS/2 table names no vendor (NONE), as the symbol and DMAP
fonts' do; every other table is Noto Sans's, byte for byte, but for the checksum of the
whole font in the head table. The SIL Open Font License 1.1 of Noto Sans covers them, and
their license file is that of Noto Sans.

What the candidate rules of #586 predict for the drop of a subscript of text at a size of s
points, drawn at 65 % of s, in points (Word rounds that size to a half point and its drops
to whole half points; these are not rounded): "typo" and "win", the layout's rule, the
descent at s less the descent at 0.65 s, which is 0.35 s times the descent in em, with
sTypoDescender or usWinDescent; "OS/2", ySubscriptYOffset at the size the subscript is drawn
at, 0.65 s times it in em, and "OS/2 at s", the same at s; and "fixed", a fixed fraction,
0.088 s. N is Noto Sans, D Officewright Test Descent, S Officewright Test Subscript. Every
prediction is proportional to s: the rows of size 1 give the drop per point of size, for the
sizes of E-subscript-drop-derived (12, every point from 20 to 48, and 72) the table leaves
out.

    font  size     typo      win     OS/2  OS/2 at s    fixed
    N        1  0.10255  0.13825  0.04875   0.07500   0.08800
    N       12    1.231    1.659    0.585     0.900     1.056
    N       24    2.461    3.318    1.170     1.800     2.112
    N       36    3.692    4.977    1.755     2.700     3.168
    N       48    4.922    6.636    2.340     3.600     4.224
    N       72    7.384    9.954    3.510     5.400     6.336
    D        1  0.20755  0.24325  0.04875   0.07500   0.08800
    D       12    2.491    2.919    0.585     0.900     1.056
    D       24    4.981    5.838    1.170     1.800     2.112
    D       36    7.472    8.757    1.755     2.700     3.168
    D       48    9.962   11.676    2.340     3.600     4.224
    D       72   14.944   17.514    3.510     5.400     6.336
    S        1  0.10255  0.13825  0.19500   0.30000   0.08800
    S       12    1.231    1.659    2.340     3.600     1.056
    S       24    2.461    3.318    4.680     7.200     2.112
    S       36    3.692    4.977    7.020    10.800     3.168
    S       48    4.922    6.636    9.360    14.400     4.224
    S       72    7.384    9.954   14.040    21.600     6.336

In Noto Sans alone the rules lie close together (typo and fixed 0.7 pt apart at 48 pt, OS/2
at s and fixed 0.62 pt). Between the fonts they part: a rule that reads the descent drops
D's subscripts 0.105 s further than N's (1.26 pt at 12 pt) and S's as far as N's; one that
reads ySubscriptYOffset drops S's 0.14625 s further (0.225 s at s; 1.755 pt and 2.7 pt at
12 pt) and D's as far as N's; a fixed fraction drops all three as far.

    make-subscript-fonts.py                     write both fonts
    make-subscript-fonts.py --check             fail if a committed font is not what this
                                                writes
    make-subscript-fonts.py --from SRC --to DIR write them into DIR/test-descent/ and
                                                DIR/test-subscript/ from another copy of
                                                NotoSans-Regular.ttf

scripts/check-test-fonts.sh writes them from upstream's copy and compares.
"""

from __future__ import annotations

import argparse
import importlib.util
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "tests" / "fonts-word" / "noto-truetype" / "NotoSans-Regular.ttf"
OUT = ROOT / "tests" / "fonts-word"

# The table directory, the checksums and the names of the symbol font's script.
_spec = importlib.util.spec_from_file_location(
    "make_symbol_font", ROOT / "scripts" / "make-symbol-font.py"
)
symbol = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(symbol)

WINDOWS, UNICODE_BMP, ENGLISH_US = 3, 1, 0x409
# How much deeper the Descent font's descent is, and the Subscript font's ySubscriptYOffset,
# in font units (Noto Sans has 1000 to the em).
DESCENT_DEEPER = 300
SUBSCRIPT_Y_OFFSET = 300
# Byte offsets of the fields (OpenType, hhea and OS/2); an OS/2 table of any version has
# them, version 0's ending with usWinDescent.
HHEA_DESCENDER = 6
OS2_Y_SUBSCRIPT_Y_OFFSET = 16
OS2_VENDOR = 58
OS2_TYPO_DESCENDER = 70
OS2_WIN_DESCENT = 76
OS2_VERSION_0_LENGTH = 78


class FontError(Exception):
    pass


class Face:
    """One of the fonts: its folder under tests/fonts-word, its family and PostScript names,
    and its description (name ID 10). A family name has at most 31 characters, the most
    GDI's LOGFONT holds, so that Word finds the font by its whole name."""

    def __init__(self, folder: str, family: str, postscript: str, description: str):
        self.folder, self.family = folder, family
        self.postscript, self.description = postscript, description

    @property
    def file(self) -> str:
        return f"{self.postscript}.ttf"


DESCENT = Face(
    "test-descent",
    "Officewright Test Descent",
    "OfficewrightTestDescent-Regular",
    "Noto Sans Regular with a descent 0.3 em deeper in hhea, sTypoDescender and "
    "usWinDescent, for the tests of Officewright: how far Word drops a subscript.",
)
SUBSCRIPT = Face(
    "test-subscript",
    "Officewright Test Subscript",
    "OfficewrightTestSubscript-Regular",
    "Noto Sans Regular with an OS/2 ySubscriptYOffset of 0.3 em, for the tests of "
    "Officewright: how far Word drops a subscript.",
)
FACES = (DESCENT, SUBSCRIPT)


def names(name: bytes, face: Face) -> bytes:
    """A name table of format 0 with the face's names and those the symbol font keeps from
    the source, all Windows Unicode English (United States), in name ID order."""
    source = symbol.source_names(name)
    version = source.get(5, "")
    if not version.startswith("Version "):
        raise FontError(f"the version name {version!r} does not start with 'Version '")
    found = {
        1: face.family,
        2: "Regular",
        3: f"{version.removeprefix('Version ')};NONE;{face.postscript}",
        4: f"{face.family} Regular",
        6: face.postscript,
        10: face.description,
    }
    for name_id, value in source.items():
        if name_id in symbol.KEPT_NAMES or name_id >= symbol.FIRST_KEPT_FEATURE_NAME:
            found[name_id] = value
    records, strings = b"", b""
    for name_id in sorted(found):
        raw = found[name_id].encode("utf-16-be")
        records += struct.pack(
            ">HHHHHH", WINDOWS, UNICODE_BMP, ENGLISH_US, name_id, len(raw), len(strings)
        )
        strings += raw
    return struct.pack(">HHH", 0, len(found), 6 + len(records)) + records + strings


def deeper_descent(found: dict[bytes, bytes]) -> None:
    """The descent DESCENT_DEEPER units deeper in the hhea descender, sTypoDescender and
    usWinDescent."""
    hhea = bytearray(found[b"hhea"])
    (descender,) = struct.unpack_from(">h", hhea, HHEA_DESCENDER)
    struct.pack_into(">h", hhea, HHEA_DESCENDER, descender - DESCENT_DEEPER)
    found[b"hhea"] = bytes(hhea)
    os2 = bytearray(found[b"OS/2"])
    (typo,) = struct.unpack_from(">h", os2, OS2_TYPO_DESCENDER)
    struct.pack_into(">h", os2, OS2_TYPO_DESCENDER, typo - DESCENT_DEEPER)
    (win,) = struct.unpack_from(">H", os2, OS2_WIN_DESCENT)
    struct.pack_into(">H", os2, OS2_WIN_DESCENT, win + DESCENT_DEEPER)
    found[b"OS/2"] = bytes(os2)


def subscript_offset(found: dict[bytes, bytes]) -> None:
    """ySubscriptYOffset SUBSCRIPT_Y_OFFSET units."""
    os2 = bytearray(found[b"OS/2"])
    (offset,) = struct.unpack_from(">h", os2, OS2_Y_SUBSCRIPT_Y_OFFSET)
    if offset == SUBSCRIPT_Y_OFFSET:
        raise FontError(f"the source's ySubscriptYOffset is {offset} already")
    struct.pack_into(">h", os2, OS2_Y_SUBSCRIPT_Y_OFFSET, SUBSCRIPT_Y_OFFSET)
    found[b"OS/2"] = bytes(os2)


CHANGES = {DESCENT.family: deeper_descent, SUBSCRIPT.family: subscript_offset}


def font(source: bytes, face: Face) -> bytes:
    """The face made from the bytes of NotoSans-Regular.ttf."""
    found = symbol.tables(source)
    if len(found[b"OS/2"]) < OS2_VERSION_0_LENGTH:
        raise FontError("the OS/2 table is too short for a descent")
    if len(found.get(b"hhea", b"")) < HHEA_DESCENDER + 2:
        raise FontError("no hhea table with a descender")
    CHANGES[face.family](found)
    found[b"name"] = names(found[b"name"], face)
    # The vendor (achVendID) of a font Noto Sans's vendor did not make.
    os2 = found[b"OS/2"]
    found[b"OS/2"] = os2[:OS2_VENDOR] + b"NONE" + os2[OS2_VENDOR + 4 :]
    found.pop(b"DSIG", None)
    return symbol.sfnt(found)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true", help="compare with the committed fonts")
    parser.add_argument("--from", dest="source", type=Path, default=SOURCE, metavar="SRC")
    parser.add_argument("--to", dest="out", type=Path, default=OUT, metavar="DIR")
    args = parser.parse_args()
    try:
        source = args.source.read_bytes()
        fonts = [(args.out / face.folder / face.file, font(source, face)) for face in FACES]
    except (OSError, FontError, symbol.FontError, struct.error, UnicodeDecodeError) as error:
        print(f"make-subscript-fonts.py: {args.source}: {error}", file=sys.stderr)
        return 1
    if args.check:
        stale = [path for path, data in fonts if not path.is_file() or path.read_bytes() != data]
        for path in stale:
            print(f"make-subscript-fonts.py: {path} is not what this script writes",
                  file=sys.stderr)
        return 1 if stale else 0
    for path, data in fonts:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    return 0


if __name__ == "__main__":
    sys.exit(main())
