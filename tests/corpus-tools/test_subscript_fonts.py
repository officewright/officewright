"""The subscript fonts of tests/fonts-word/test-descent/ and tests/fonts-word/test-subscript/
are what scripts/make-subscript-fonts.py writes from the TrueType Noto Sans Regular, and they
are the fonts THIRD-PARTY-NOTICES.md describes: read here without the script's code,
Officewright Test Descent differs from Noto Sans only in a descent 300 units deeper in the
hhea descender, sTypoDescender and usWinDescent, Officewright Test Subscript only in an OS/2
ySubscriptYOffset of 300 units, both in their names and in an OS/2 that names no vendor, and
every other table is Noto Sans's. The predictions the script's comment gives for each rule of
#586 follow from those metrics."""

import importlib.util
import re
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from test_symbol_font import checksum, directory, names, tables

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "make-subscript-fonts.py"
SOURCE = ROOT / "tests" / "fonts-word" / "noto-truetype" / "NotoSans-Regular.ttf"
DESCENT = ROOT / "tests" / "fonts-word" / "test-descent" / "OfficewrightTestDescent-Regular.ttf"
SUBSCRIPT = (ROOT / "tests" / "fonts-word" / "test-subscript"
             / "OfficewrightTestSubscript-Regular.ttf")
CFF_SOURCE = ROOT / "tests" / "fonts" / "noto" / "NotoSans-Regular.otf"


def load():
    spec = importlib.util.spec_from_file_location("make_subscript_fonts", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def metrics(font_tables):
    """unitsPerEm, the hhea descender, sTypoDescender, usWinDescent and ySubscriptYOffset,
    read at their offsets in the OpenType head, hhea and OS/2 tables."""
    (units,) = struct.unpack_from(">H", font_tables[b"head"], 18)
    (hhea,) = struct.unpack_from(">h", font_tables[b"hhea"], 6)
    os2 = font_tables[b"OS/2"]
    (offset,) = struct.unpack_from(">h", os2, 16)
    (typo,) = struct.unpack_from(">h", os2, 70)
    (win,) = struct.unpack_from(">H", os2, 76)
    return {"units": units, "hhea": hhea, "typo": typo, "win": win, "subscript": offset}


def differing_bytes(a, b):
    """The offsets at which two tables of one length differ."""
    return [k for k, (x, y) in enumerate(zip(a, b)) if x != y]


class TestSubscriptFonts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = SOURCE.read_bytes()
        cls.source_tables = tables(cls.source)
        cls.fonts = {"descent": DESCENT.read_bytes(), "subscript": SUBSCRIPT.read_bytes()}
        cls.tables = {key: tables(data) for key, data in cls.fonts.items()}

    def test_the_committed_fonts_are_what_the_script_writes(self):
        script = load()
        self.assertEqual(script.font(self.source, script.DESCENT), self.fonts["descent"])
        self.assertEqual(script.font(self.source, script.SUBSCRIPT), self.fonts["subscript"])

    def test_noto_sans_is_the_source_the_notices_describe(self):
        # The values the fonts change, in Noto Sans: 1000 units to the em, a descent of 293
        # in hhea and sTypoDescender and of 395 in usWinDescent, ySubscriptYOffset 75.
        self.assertEqual(metrics(self.source_tables), {
            "units": 1000, "hhea": -293, "typo": -293, "win": 395, "subscript": 75})

    def test_the_descent_font_has_only_its_descent_deeper(self):
        self.assertEqual(metrics(self.tables["descent"]), {
            "units": 1000, "hhea": -593, "typo": -593, "win": 695, "subscript": 75})
        # hhea: only the descender (bytes 6 and 7); OS/2: only sTypoDescender (70, 71),
        # usWinDescent (76, 77) and the vendor (58 to 61).
        hhea, source_hhea = self.tables["descent"][b"hhea"], self.source_tables[b"hhea"]
        self.assertEqual(len(hhea), len(source_hhea))
        self.assertTrue(set(differing_bytes(hhea, source_hhea)) <= {6, 7})
        os2, source_os2 = self.tables["descent"][b"OS/2"], self.source_tables[b"OS/2"]
        self.assertEqual(len(os2), len(source_os2))
        self.assertTrue(set(differing_bytes(os2, source_os2)) <= {58, 59, 60, 61, 70, 71, 76, 77})
        self.assertEqual(os2[58:62], b"NONE")

    def test_the_subscript_font_has_only_its_subscript_offset_changed(self):
        self.assertEqual(metrics(self.tables["subscript"]), {
            "units": 1000, "hhea": -293, "typo": -293, "win": 395, "subscript": 300})
        self.assertEqual(self.tables["subscript"][b"hhea"], self.source_tables[b"hhea"])
        os2, source_os2 = self.tables["subscript"][b"OS/2"], self.source_tables[b"OS/2"]
        self.assertEqual(len(os2), len(source_os2))
        self.assertTrue(set(differing_bytes(os2, source_os2)) <= {16, 17, 58, 59, 60, 61})
        self.assertEqual(os2[58:62], b"NONE")

    def test_every_other_table_is_noto_sans(self):
        changed = {"descent": {b"head", b"hhea", b"OS/2", b"name"},
                   "subscript": {b"head", b"OS/2", b"name"}}
        for key, font_tables in self.tables.items():
            with self.subTest(key):
                self.assertEqual(set(font_tables), set(self.source_tables) - {b"DSIG"})
                for tag, table in font_tables.items():
                    if tag == b"head":
                        # Only checkSumAdjustment differs.
                        source = self.source_tables[tag]
                        self.assertEqual(table[:8] + table[12:], source[:8] + source[12:])
                    elif tag not in changed[key]:
                        self.assertEqual(table, self.source_tables[tag], tag)
                    else:
                        self.assertNotEqual(table, self.source_tables[tag], tag)

    def test_the_names_are_their_own_and_keep_noto_sans_notices(self):
        source = {name_id: value for key, name_id, value in names(self.source_tables[b"name"])
                  if key == (3, 1, 0x409)}
        expected = {"descent": ("Officewright Test Descent", "OfficewrightTestDescent-Regular"),
                    "subscript": ("Officewright Test Subscript",
                                  "OfficewrightTestSubscript-Regular")}
        for key, (family, postscript) in expected.items():
            with self.subTest(key):
                records = names(self.tables[key][b"name"])
                self.assertEqual({k for k, _, _ in records}, {(3, 1, 0x409)})
                by_id = {name_id: value for _, name_id, value in records}
                self.assertEqual(by_id[1], family)
                # GDI's LOGFONT holds a family name of at most 31 characters.
                self.assertLessEqual(len(by_id[1]), 31)
                self.assertEqual(by_id[2], "Regular")
                self.assertEqual(by_id[4], f"{family} Regular")
                self.assertEqual(by_id[6], postscript)
                self.assertEqual(by_id[3], f"2.015;NONE;{postscript}")
                self.assertIn("subscript", by_id[10])
                for name_id in (1, 3, 4, 6, 16, 17):
                    self.assertNotIn("Noto", by_id.get(name_id, ""), name_id)
                for name_id in (0, 5, 7, 9, 13, 14, 256, 257, 258, 259):
                    self.assertEqual(by_id[name_id], source[name_id], name_id)
                self.assertEqual(sorted(by_id),
                                 [0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 13, 14, 256, 257, 258, 259])
        # The file of each is named after its PostScript name.
        self.assertEqual(DESCENT.stem, expected["descent"][1])
        self.assertEqual(SUBSCRIPT.stem, expected["subscript"][1])

    def test_checksums_binary_search_fields_and_table_order_are_right(self):
        def fields(count, unit):
            power = 1 << (count.bit_length() - 1)
            return power * unit, power.bit_length() - 1, (count - power) * unit

        for key, data in self.fonts.items():
            with self.subTest(key):
                (count,) = struct.unpack_from(">H", data, 4)
                self.assertEqual(struct.unpack_from(">HHH", data, 6), fields(count, 16))
                entries = directory(data)
                tags = [tag for tag, _, _, _ in entries]
                self.assertEqual(tags, sorted(tags))
                offsets = [offset for _, _, offset, _ in entries]
                self.assertEqual(offsets, sorted(offsets))
                self.assertEqual(offsets[0], 12 + 16 * count)
                for tag, stored, offset, length in entries:
                    table = data[offset : offset + length]
                    if tag == b"head":
                        table = table[:8] + b"\x00\x00\x00\x00" + table[12:]
                    self.assertEqual(stored, checksum(table), tag)
                    self.assertEqual(offset % 4, 0, tag)
                self.assertEqual(checksum(data), 0xB1B0AFBA)

    def test_the_predictions_follow_from_the_metrics(self):
        # The table in the script's comment: per font and size, the drop each rule predicts.
        doc = SCRIPT.read_text(encoding="utf-8")
        rows = re.findall(r"^    ([NDS]) +(\d+)((?: +\d+\.\d+){5})$", doc, re.M)
        fonts = {"N": self.source_tables, "D": self.tables["descent"],
                 "S": self.tables["subscript"]}
        self.assertEqual([(f, int(s)) for f, s, _ in rows],
                         [(f, s) for f in "NDS" for s in (1, 12, 24, 36, 48, 72)])
        for font, size, values in rows:
            m = metrics(fonts[font])
            em = m["units"]
            s = int(size)
            predicted = [0.35 * s * -m["typo"] / em, 0.35 * s * m["win"] / em,
                         0.65 * s * m["subscript"] / em, s * m["subscript"] / em, 0.088 * s]
            written = [float(v) for v in values.split()]
            places = 5 if s == 1 else 3
            for rule, got, want in zip(("typo", "win", "OS/2", "OS/2 at s", "fixed"),
                                       written, predicted):
                with self.subTest(font=font, size=s, rule=rule):
                    self.assertAlmostEqual(got, want, delta=0.5 * 10 ** -places)
        # What the comment says the fonts tell apart: a descent rule drops D's subscripts
        # 0.105 s further than N's, an OS/2 rule S's 0.14625 s (0.225 s at s), at 12 pt
        # 1.26, 1.755 and 2.7 pt; in N at 48 pt, typo and fixed predict 4.92 and 4.22 pt, 0.7
        # pt apart, OS/2 at s 3.60 pt, 0.62 pt from fixed, OS/2 2.34 pt and win 6.64 pt.
        n, d, sub = (metrics(fonts[k]) for k in "NDS")
        self.assertAlmostEqual(0.35 * (d["typo"] - n["typo"]) / -1000, 0.105)
        self.assertAlmostEqual(0.35 * (d["win"] - n["win"]) / 1000, 0.105)
        self.assertAlmostEqual(0.65 * (sub["subscript"] - n["subscript"]) / 1000, 0.14625)
        self.assertAlmostEqual((sub["subscript"] - n["subscript"]) / 1000, 0.225)
        prose = " ".join(doc.split())
        for claim in ("drops D's subscripts 0.105 s further than N's (1.26 pt at 12 pt)",
                      "drops S's 0.14625 s further (0.225 s at s; 1.755 pt and 2.7 pt at 12 pt)",
                      "at 48 pt, typo 4.92 pt and fixed 4.22 pt (0.7 pt apart), OS/2 at s 3.60 pt "
                      "(0.62 pt from fixed), but OS/2 at 0.65 s 2.34 pt and win 6.64 pt."):
            self.assertIn(claim, prose)
        self.assertAlmostEqual(0.105 * 12, 1.26)
        self.assertAlmostEqual(0.14625 * 12, 1.755)
        self.assertAlmostEqual(0.225 * 12, 2.7)
        self.assertAlmostEqual(round(0.35 * 48 * -n["typo"] / 1000 - 0.088 * 48, 2), 0.7)
        self.assertAlmostEqual(round(0.088 * 48 - 48 * n["subscript"] / 1000, 2), 0.62)
        for drop, want in ((0.35 * 48 * -n["typo"] / 1000, 4.92), (0.088 * 48, 4.22),
                           (48 * n["subscript"] / 1000, 3.60),
                           (0.65 * 48 * n["subscript"] / 1000, 2.34),
                           (0.35 * 48 * n["win"] / 1000, 6.64)):
            self.assertAlmostEqual(round(drop, 2), want)

    def test_a_digital_signature_of_the_source_is_left_out(self):
        script = load()
        signed = script.symbol.tables(self.source)
        signed[b"DSIG"] = struct.pack(">IHH", 1, 0, 0)
        for face in script.FACES:
            with self.subTest(face.family):
                self.assertNotIn(b"DSIG", tables(script.font(script.symbol.sfnt(signed), face)))

    def test_refuses_short_tables_and_an_offset_already_changed(self):
        script = load()
        short = script.symbol.tables(self.source)
        short[b"OS/2"] = short[b"OS/2"][:77]
        with self.assertRaisesRegex(script.FontError, "OS/2 table is too short for a descent"):
            script.font(script.symbol.sfnt(short), script.DESCENT)
        # A version 0 OS/2 table of 78 bytes, which ends with usWinDescent, is taken.
        short[b"OS/2"] = self.source_tables[b"OS/2"][:78]
        self.assertTrue(script.font(script.symbol.sfnt(short), script.DESCENT))
        without = script.symbol.tables(self.source)
        del without[b"hhea"]
        with self.assertRaisesRegex(script.FontError, "no hhea table with a descender"):
            script.font(script.symbol.sfnt(without), script.DESCENT)
        changed = script.symbol.tables(script.font(self.source, script.SUBSCRIPT))
        with self.assertRaisesRegex(script.FontError, "ySubscriptYOffset is 300 already"):
            script.font(script.symbol.sfnt(changed), script.SUBSCRIPT)

    def run_script(self, *args):
        return subprocess.run(
            [sys.executable, "-I", "-B", str(SCRIPT), *args],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_check_refuses_another_or_a_missing_font(self):
        self.assertEqual(self.run_script("--check").returncode, 0)
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            self.assertEqual(self.run_script("--to", str(out)).returncode, 0)
            self.assertEqual((out / "test-descent" / DESCENT.name).read_bytes(),
                             self.fonts["descent"])
            self.assertEqual((out / "test-subscript" / SUBSCRIPT.name).read_bytes(),
                             self.fonts["subscript"])
            self.assertEqual(self.run_script("--check", "--to", str(out)).returncode, 0)
            for path in (out / "test-descent" / DESCENT.name,
                         out / "test-subscript" / SUBSCRIPT.name):
                with self.subTest(path.name):
                    good = path.read_bytes()
                    changed = bytearray(good)
                    changed[-1] ^= 1
                    path.write_bytes(bytes(changed))
                    result = self.run_script("--check", "--to", str(out))
                    self.assertEqual(result.returncode, 1)
                    self.assertIn(f"{path} is not what this script writes", result.stderr)
                    path.unlink()
                    self.assertEqual(self.run_script("--check", "--to", str(out)).returncode, 1)
                    path.write_bytes(good)

    def test_refuses_a_source_that_is_not_truetype(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            result = self.run_script("--from", str(CFF_SOURCE), "--to", str(out))
            self.assertEqual(result.returncode, 1)
            self.assertIn("not a TrueType font", result.stderr)
            self.assertEqual(list(out.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
