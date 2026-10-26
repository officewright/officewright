"""The symbol font of tests/fonts-word/test-symbol/ is what scripts/make-symbol-font.py writes
from the TrueType Noto Sans Regular, and it is the symbol font THIRD-PARTY-NOTICES.md describes:
read here without the script's code, its character map is a Windows symbol one with the glyphs
of Noto Sans for U+0020 to U+00FF at U+F020 to U+F0FF and nothing else, its names and OS/2 are
those of a symbol font named Officewright Test Symbol, and every other table is Noto Sans's."""

import importlib.util
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "make-symbol-font.py"
SOURCE = ROOT / "tests" / "fonts-word" / "noto-truetype" / "NotoSans-Regular.ttf"
FONT = ROOT / "tests" / "fonts-word" / "test-symbol" / "OfficewrightTestSymbol-Regular.ttf"
CFF_SOURCE = ROOT / "tests" / "fonts" / "noto" / "NotoSans-Regular.otf"


def load():
    spec = importlib.util.spec_from_file_location("make_symbol_font", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def directory(data):
    """(tag, checksum, offset, length) of each table."""
    (count,) = struct.unpack_from(">H", data, 4)
    return [struct.unpack_from(">4sIII", data, 12 + 16 * i) for i in range(count)]


def tables(data):
    return {tag: data[offset : offset + length] for tag, _, offset, length in directory(data)}


def subtables(cmap):
    """(platform, encoding, format, offset) of each subtable."""
    (count,) = struct.unpack_from(">H", cmap, 2)
    found = []
    for i in range(count):
        platform, encoding, offset = struct.unpack_from(">HHI", cmap, 4 + 8 * i)
        (format_,) = struct.unpack_from(">H", cmap, offset)
        found.append((platform, encoding, format_, offset))
    return found


def format4(cmap, offset):
    """Every character the format 4 subtable maps to a glyph other than 0."""
    (seg_x2,) = struct.unpack_from(">H", cmap, offset + 6)
    n = seg_x2 // 2
    ends = struct.unpack_from(f">{n}H", cmap, offset + 14)
    starts = struct.unpack_from(f">{n}H", cmap, offset + 16 + seg_x2)
    deltas = struct.unpack_from(f">{n}H", cmap, offset + 16 + 2 * seg_x2)
    ro_at = offset + 16 + 3 * seg_x2
    range_offsets = struct.unpack_from(f">{n}H", cmap, ro_at)
    mapped = {}
    for s in range(n):
        for char in range(starts[s], ends[s] + 1):
            if range_offsets[s]:
                at = ro_at + 2 * s + range_offsets[s] + 2 * (char - starts[s])
                (glyph,) = struct.unpack_from(">H", cmap, at)
                glyph = (glyph + deltas[s]) & 0xFFFF if glyph else 0
            else:
                glyph = (char + deltas[s]) & 0xFFFF
            if glyph:
                mapped[char] = glyph
    return mapped


def format12(cmap, offset):
    (groups,) = struct.unpack_from(">I", cmap, offset + 12)
    mapped = {}
    for i in range(groups):
        start, end, glyph = struct.unpack_from(">III", cmap, offset + 16 + 12 * i)
        for char in range(start, end + 1):
            mapped[char] = glyph + char - start
    return mapped


def names(name):
    """((platform, encoding, language), name ID, text) of each record."""
    _, count, strings = struct.unpack_from(">HHH", name, 0)
    found = []
    for i in range(count):
        platform, encoding, language, name_id, length, offset = struct.unpack_from(
            ">HHHHHH", name, 6 + 12 * i
        )
        raw = name[strings + offset : strings + offset + length]
        found.append(((platform, encoding, language), name_id, raw.decode("utf-16-be")))
    return found


def checksum(data):
    data += b"\x00" * (-len(data) % 4)
    return sum(struct.unpack(f">{len(data) // 4}I", data)) & 0xFFFFFFFF


class TestSymbolFont(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.font = FONT.read_bytes()
        cls.source = SOURCE.read_bytes()
        cls.tables = tables(cls.font)
        cls.source_tables = tables(cls.source)

    def test_the_committed_font_is_what_the_script_writes(self):
        self.assertEqual(load().symbol_font(self.source), self.font)

    def test_the_only_character_map_is_a_symbol_one_with_noto_sans_glyphs(self):
        cmap = self.tables[b"cmap"]
        self.assertEqual([s[:3] for s in subtables(cmap)], [(3, 0, 4)])
        mapped = format4(cmap, subtables(cmap)[0][3])
        # Noto Sans's own map, read from its format 12 subtable.
        source_cmap = self.source_tables[b"cmap"]
        (offset,) = [s[3] for s in subtables(source_cmap) if s[:3] == (3, 10, 12)]
        unicode = format12(source_cmap, offset)
        expected = {0xF000 + c: unicode[c] for c in range(0x20, 0x100) if c in unicode}
        self.assertEqual(mapped, expected)
        # U+0020 to U+007E and U+00A0 to U+00FF: Noto Sans has no glyph for the controls.
        self.assertEqual(len(expected), 0x7F - 0x20 + 0x100 - 0xA0)

    def test_the_names_are_its_own_and_keep_noto_sans_notices(self):
        records = names(self.tables[b"name"])
        self.assertEqual({key for key, _, _ in records}, {(3, 0, 0x409)})
        by_id = {name_id: value for _, name_id, value in records}
        self.assertEqual(by_id[1], "Officewright Test Symbol")
        self.assertEqual(by_id[2], "Regular")
        self.assertEqual(by_id[4], "Officewright Test Symbol Regular")
        self.assertEqual(by_id[6], "OfficewrightTestSymbol-Regular")
        self.assertIn("OfficewrightTestSymbol-Regular", by_id[3])
        for name_id in (1, 3, 4, 6, 16, 17):
            self.assertNotIn("Noto", by_id.get(name_id, ""), name_id)
        source = {name_id: value for key, name_id, value in names(self.source_tables[b"name"])
                  if key == (3, 1, 0x409)}
        for name_id in (0, 5, 7, 9, 13, 14, 256, 257, 258, 259):
            self.assertEqual(by_id[name_id], source[name_id], name_id)
        for name_id in (8, 11, 12):
            self.assertNotIn(name_id, by_id)
        self.assertEqual(sorted(by_id), [0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 13, 14, 256, 257, 258, 259])

    def test_os2_gives_the_symbol_code_page_only(self):
        os2, source = self.tables[b"OS/2"], self.source_tables[b"OS/2"]
        self.assertEqual(struct.unpack_from(">IIII", os2, 42), (0, 1 << 28, 0, 0))
        self.assertEqual(os2[58:62], b"NONE")
        self.assertEqual(struct.unpack_from(">HH", os2, 64), (0xF020, 0xF0FF))
        self.assertEqual(struct.unpack_from(">II", os2, 78), (1 << 31, 0))
        # The Unicode ranges, the vendor, the first and last characters and the code pages.
        changed = set(range(42, 62)) | set(range(64, 68)) | set(range(78, 86))
        self.assertEqual(len(os2), len(source))
        for at in range(len(os2)):
            if at not in changed:
                self.assertEqual(os2[at], source[at], f"OS/2 byte {at}")

    def test_every_other_table_is_noto_sans(self):
        self.assertEqual(set(self.tables), set(self.source_tables) - {b"DSIG"})
        for tag, table in self.tables.items():
            if tag == b"head":
                # Only checkSumAdjustment differs.
                self.assertEqual(table[:8] + table[12:], self.source_tables[tag][:8]
                                 + self.source_tables[tag][12:])
            elif tag not in (b"cmap", b"name", b"OS/2"):
                self.assertEqual(table, self.source_tables[tag], tag)

    def test_the_binary_search_fields_and_the_table_order_are_right(self):
        # OpenType: searchRange is the largest power of two not above the count times the
        # unit, entrySelector its log2, rangeShift the rest; tables in tag order, each after
        # the last.
        def fields(count, unit):
            power = 1 << (count.bit_length() - 1)
            return power * unit, power.bit_length() - 1, (count - power) * unit

        (count,) = struct.unpack_from(">H", self.font, 4)
        self.assertEqual(struct.unpack_from(">HHH", self.font, 6), fields(count, 16))
        entries = directory(self.font)
        tags = [tag for tag, _, _, _ in entries]
        self.assertEqual(tags, sorted(tags))
        offsets = [offset for _, _, offset, _ in entries]
        self.assertEqual(offsets, sorted(offsets))
        self.assertEqual(offsets[0], 12 + 16 * count)
        cmap = self.tables[b"cmap"]
        (offset,) = [s[3] for s in subtables(cmap)]
        (seg_x2,) = struct.unpack_from(">H", cmap, offset + 6)
        self.assertEqual(struct.unpack_from(">HHH", cmap, offset + 8), fields(seg_x2 // 2, 2))
        # The subtable's length is its own, to the end of the table.
        (length,) = struct.unpack_from(">H", cmap, offset + 2)
        self.assertEqual(offset + length, len(cmap))

    def test_checksums_are_right(self):
        for tag, stored, offset, length in directory(self.font):
            table = self.font[offset : offset + length]
            if tag == b"head":
                table = table[:8] + b"\x00\x00\x00\x00" + table[12:]
            self.assertEqual(stored, checksum(table), tag)
            self.assertEqual(offset % 4, 0, tag)
        self.assertEqual(checksum(self.font), 0xB1B0AFBA)

    def run_script(self, *args):
        return subprocess.run(
            [sys.executable, "-I", "-B", str(SCRIPT), *args],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_check_refuses_another_font(self):
        self.assertEqual(self.run_script("--check").returncode, 0)
        with tempfile.TemporaryDirectory() as tmp:
            other = Path(tmp) / "font.ttf"
            changed = bytearray(self.font)
            changed[-1] ^= 1
            other.write_bytes(bytes(changed))
            result = self.run_script("--check", "--to", str(other))
            self.assertEqual(result.returncode, 1)
            self.assertIn("is not what this script writes", result.stderr)
            missing = Path(tmp) / "missing.ttf"
            self.assertEqual(self.run_script("--check", "--to", str(missing)).returncode, 1)

    def test_refuses_a_source_that_is_not_truetype(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "font.ttf"
            result = self.run_script("--from", str(CFF_SOURCE), "--to", str(out))
            self.assertEqual(result.returncode, 1)
            self.assertIn("not a TrueType font", result.stderr)
            self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
