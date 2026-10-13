"""The Apache POI documents in tests/corpus/poi/ are listed and credited. nightly.sh compares
them with POI's, which it fetches."""

import hashlib
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
POI = ROOT / "tests" / "corpus" / "poi"
SOURCES = ROOT / "tests" / "corpus" / "SOURCES.md"
NOTICES = ROOT / "THIRD-PARTY-NOTICES.md"
MANIFEST = ROOT / "scripts" / "corpus-ext.tsv"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def rows() -> dict[str, tuple[str, str]]:
    """File name -> (commit, SHA-256) from the SOURCES.md table of poi/."""
    found = {}
    for line in SOURCES.read_text(encoding="utf-8").splitlines():
        match = re.match(
            r"\| poi/([^ |]+) \| Apache POI, `test-data/document/\1` at commit `([0-9a-f]{12})`, "
            r"https://github\.com/apache/poi/blob/([0-9a-f]{40})/test-data/document/\1 "
            r"\| Apache-2\.0 \| `([0-9a-f]{64})` \| .+ \|$",
            line,
        )
        if match:
            name, short, commit, digest = match.groups()
            if not commit.startswith(short):
                raise AssertionError(f"{name}: short and full commit differ")
            found[name] = (commit, digest)
    return found


class PoiCorpus(unittest.TestCase):
    def test_every_document_is_listed_with_its_hash(self):
        listed = rows()
        committed = sorted(path.name for path in POI.glob("*.docx"))
        self.assertTrue(committed)
        self.assertEqual(sorted(listed), committed)
        for name, (_, digest) in listed.items():
            with self.subTest(name):
                self.assertEqual(sha256(POI / name), digest)

    def test_the_commit_is_the_pinned_one(self):
        pins = [line.split("\t") for line in MANIFEST.read_text(encoding="utf-8").splitlines()]
        pin = next(fields[3] for fields in pins if fields[0] == "poi")
        self.assertEqual({commit for commit, _ in rows().values()}, {pin})
        self.assertIn(f"`{pin}`", NOTICES.read_text(encoding="utf-8"))

    def test_license_and_notice_are_there_as_credited(self):
        notices = re.sub(r"\s+", " ", NOTICES.read_text(encoding="utf-8"))
        for name in ("LICENSE", "NOTICE"):
            with self.subTest(name):
                self.assertIn(
                    f"`tests/corpus/poi/{name}` (SHA-256 `{sha256(POI / name)}`)", notices
                )
        self.assertIn("Apache License", (POI / "LICENSE").read_text(encoding="utf-8"))
        self.assertTrue((POI / "NOTICE").read_text(encoding="utf-8").startswith("Apache POI\n"))


if __name__ == "__main__":
    unittest.main()
