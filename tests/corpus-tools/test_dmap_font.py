"""The DMAP font of tests/fonts-word/test-dmap/ is what scripts/make-dmap-font.py writes from
the TrueType Noto Sans Regular, and it is the DMAP font THIRD-PARTY-NOTICES.md describes: read
here without the script's code, its DMAP table maps A to the glyph of B, B to that of A and
U+E000 to that of Z and nothing else, its cmap is Noto Sans's, its names are those of a font
named Officewright Test DMAP, its OS/2 names no vendor, and every other table is Noto Sans's."""

import importlib.util
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from test_symbol_font import checksum, directory, format4, format12, names, subtables, tables

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "make-dmap-font.py"
SOURCE = ROOT / "tests" / "fonts-word" / "noto-truetype" / "NotoSans-Regular.ttf"
FONT = ROOT / "tests" / "fonts-word" / "test-dmap" / "OfficewrightTestDMAP-Regular.ttf"
CFF_SOURCE = ROOT / "tests" / "fonts" / "noto" / "NotoSans-Regular.otf"


def load():
    spec = importlib.util.spec_from_file_location("make_dmap_font", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class TestDmapFont(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.font = FONT.read_bytes()
        cls.source = SOURCE.read_bytes()
        cls.tables = tables(cls.font)
        cls.source_tables = tables(cls.source)
        # Noto Sans's own map, read from its format 12 subtable.
        cmap = cls.source_tables[b"cmap"]
        (offset,) = [s[3] for s in subtables(cmap) if s[:3] == (3, 10, 12)]
        cls.unicode = format12(cmap, offset)

    def test_the_committed_font_is_what_the_script_writes(self):
        self.assertEqual(load().dmap_font(self.source), self.font)

    def test_dmap_swaps_a_and_b_and_gives_u_e000_the_glyph_of_z(self):
        dmap = self.tables[b"DMAP"]
        self.assertEqual([s[:3] for s in subtables(dmap)], [(3, 1, 4)])
        mapped = format4(dmap, subtables(dmap)[0][3])
        self.assertEqual(mapped, {0x41: self.unicode[0x42], 0x42: self.unicode[0x41],
                                  0xE000: self.unicode[0x5A]})
        # The three glyphs differ, and Noto Sans maps no glyph to U+E000.
        self.assertEqual(len({self.unicode[c] for c in (0x41, 0x42, 0x5A)}), 3)
        self.assertNotIn(0xE000, self.unicode)

    def test_the_character_map_is_noto_sans(self):
        self.assertEqual(self.tables[b"cmap"], self.source_tables[b"cmap"])

    def test_the_names_are_its_own_and_keep_noto_sans_notices(self):
        records = names(self.tables[b"name"])
        self.assertEqual({key for key, _, _ in records}, {(3, 1, 0x409)})
        by_id = {name_id: value for _, name_id, value in records}
        self.assertEqual(by_id[1], "Officewright Test DMAP")
        self.assertEqual(by_id[2], "Regular")
        self.assertEqual(by_id[4], "Officewright Test DMAP Regular")
        self.assertEqual(by_id[6], "OfficewrightTestDMAP-Regular")
        self.assertIn("OfficewrightTestDMAP-Regular", by_id[3])
        self.assertIn("DMAP", by_id[10])
        for name_id in (1, 3, 4, 6, 16, 17):
            self.assertNotIn("Noto", by_id.get(name_id, ""), name_id)
        source = {name_id: value for key, name_id, value in names(self.source_tables[b"name"])
                  if key == (3, 1, 0x409)}
        for name_id in (0, 5, 7, 9, 13, 14, 256, 257, 258, 259):
            self.assertEqual(by_id[name_id], source[name_id], name_id)
        self.assertEqual(sorted(by_id), [0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 13, 14, 256, 257, 258, 259])

    def test_every_other_table_is_noto_sans(self):
        self.assertEqual(set(self.tables), set(self.source_tables) - {b"DSIG"} | {b"DMAP"})
        for tag, table in self.tables.items():
            if tag == b"head":
                # Only checkSumAdjustment differs.
                self.assertEqual(table[:8] + table[12:], self.source_tables[tag][:8]
                                 + self.source_tables[tag][12:])
            elif tag == b"OS/2":
                # Only the vendor differs.
                self.assertEqual(table[58:62], b"NONE")
                self.assertEqual(table[:58] + table[62:], self.source_tables[tag][:58]
                                 + self.source_tables[tag][62:])
            elif tag not in (b"DMAP", b"name"):
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
        dmap = self.tables[b"DMAP"]
        (offset,) = [s[3] for s in subtables(dmap)]
        (seg_x2,) = struct.unpack_from(">H", dmap, offset + 6)
        self.assertEqual(struct.unpack_from(">HHH", dmap, offset + 8), fields(seg_x2 // 2, 2))
        # The subtable's length is its own, to the end of the table, and it ends with the
        # segment of U+FFFF.
        (length,) = struct.unpack_from(">H", dmap, offset + 2)
        self.assertEqual(offset + length, len(dmap))
        self.assertEqual(struct.unpack_from(">H", dmap, offset + 14 + seg_x2 - 2), (0xFFFF,))

    def test_checksums_are_right(self):
        for tag, stored, offset, length in directory(self.font):
            table = self.font[offset : offset + length]
            if tag == b"head":
                table = table[:8] + b"\x00\x00\x00\x00" + table[12:]
            self.assertEqual(stored, checksum(table), tag)
            self.assertEqual(offset % 4, 0, tag)
        self.assertEqual(checksum(self.font), 0xB1B0AFBA)

    def test_a_digital_signature_of_the_source_is_left_out(self):
        # The signature would no longer match the font; Noto Sans has none to leave out.
        script = load()
        signed = script.symbol.tables(self.source)
        signed[b"DSIG"] = struct.pack(">IHH", 1, 0, 0)
        self.assertNotIn(b"DSIG", tables(script.dmap_font(script.symbol.sfnt(signed))))

    def test_refuses_an_os2_table_too_short_for_a_vendor(self):
        script = load()
        short = script.symbol.tables(self.source)
        short[b"OS/2"] = short[b"OS/2"][:60]
        with self.assertRaisesRegex(script.FontError, "OS/2 table is too short for a vendor"):
            script.dmap_font(script.symbol.sfnt(short))

    def test_refuses_a_cmap_that_maps_u_e000(self):
        # A format 4 subtable (3, 1) with segments for A to B, Z, U+E000 and U+FFFF.
        script = load()
        codes = [(0x41, 0x42), (0x5A, 0x5A), (0xE000, 0xE000), (0xFFFF, 0xFFFF)]
        count = len(codes)
        body = (struct.pack(f">{count}H", *[end for _, end in codes]) + b"\x00\x00"
                + struct.pack(f">{count}H", *[start for start, _ in codes])
                + struct.pack(f">{count}H", 1, 1, 1, 1) + struct.pack(f">{count}H", 0, 0, 0, 0))
        subtable = struct.pack(">HHHHHHH", 4, 14 + len(body), 0, 2 * count, 8, 2, 0) + body
        cmap = struct.pack(">HHHHI", 0, 1, 3, 1, 12) + subtable
        with self.assertRaisesRegex(script.FontError, "cmap maps U\\+E000 already"):
            script.dmap_table(cmap)
        # Without U+E000 the same map is taken.
        self.assertTrue(script.dmap_table(cmap.replace(struct.pack(">H", 0xE000), b"\xe0\x01")))

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

    def test_refuses_a_source_that_is_not_truetype_or_has_dmap(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "font.ttf"
            result = self.run_script("--from", str(CFF_SOURCE), "--to", str(out))
            self.assertEqual(result.returncode, 1)
            self.assertIn("not a TrueType font", result.stderr)
            result = self.run_script("--from", str(FONT), "--to", str(out))
            self.assertEqual(result.returncode, 1)
            self.assertIn("has a DMAP table already", result.stderr)
            self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
