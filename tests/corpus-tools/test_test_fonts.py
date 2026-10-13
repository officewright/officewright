"""Every test font in tests/fonts/ is credited in THIRD-PARTY-NOTICES.md with its SHA-256."""

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

    def test_every_listed_file_is_there(self):
        notices = NOTICES.read_text(encoding="utf-8")
        listed = set(re.findall(r"`(tests/fonts/[^`]+)`", notices))
        self.assertTrue(listed)
        for name in sorted(listed):
            with self.subTest(name):
                self.assertTrue((ROOT / name).is_file())


if __name__ == "__main__":
    unittest.main()
