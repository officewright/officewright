"""Every test font in tests/fonts/ is credited in THIRD-PARTY-NOTICES.md with its SHA-256, and
the upstream sources scripts/check-test-fonts.sh checks them against are the ones credited."""

import hashlib
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FONTS = ROOT / "tests" / "fonts"
NOTICES = ROOT / "THIRD-PARTY-NOTICES.md"


class TestFonts(unittest.TestCase):
    def test_every_file_is_listed_with_its_hash(self):
        notices = re.sub(r"\s+", " ", NOTICES.read_text(encoding="utf-8"))
        listed = dict(re.findall(r"`(tests/fonts/[^`]+)` \(SHA-256 `([0-9a-f]{64})`\)", notices))
        files = sorted(path for path in FONTS.rglob("*") if path.is_file())
        self.assertGreaterEqual(len(files), 20)
        for path in files:
            name = path.relative_to(ROOT).as_posix()
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            with self.subTest(name):
                self.assertEqual(listed.get(name), digest, "listed SHA-256 of the file")

    def test_the_upstream_pins_are_the_noticed_ones(self):
        # scripts/check-test-fonts.sh compares the files with these sources at night. Each
        # pin must be the full one the notices give in the section of its font.
        script = (ROOT / "scripts" / "check-test-fonts.sh").read_text(encoding="utf-8")
        pins = dict(re.findall(r"^(\w+_(?:url|sha256|commit))=(\S+)$", script, re.M))
        notices = NOTICES.read_text(encoding="utf-8")
        sections = {
            "liberation": "### Liberation Sans, Serif and Mono",
            "carlito": "### Carlito",
            "noto": "### Noto Sans",
        }
        shapes = {"url": r"https://\S+", "sha256": r"[0-9a-f]{64}", "commit": r"[0-9a-f]{40}"}
        self.assertEqual(len(pins), 6)
        for name, value in pins.items():
            font, kind = name.split("_")
            with self.subTest(name):
                self.assertRegex(value, f"^{shapes[kind]}$")
                start = notices.index(sections[font])
                end = notices.find("\n#", start + 1)
                self.assertIn(value, notices[start:end])

    def test_every_listed_file_is_there(self):
        notices = NOTICES.read_text(encoding="utf-8")
        listed = set(re.findall(r"`(tests/fonts/[^`]+)`", notices))
        self.assertTrue(listed)
        for name in sorted(listed):
            with self.subTest(name):
                self.assertTrue((ROOT / name).is_file())


if __name__ == "__main__":
    unittest.main()
