"""Every test font in tests/fonts/ is credited in THIRD-PARTY-NOTICES.md with its SHA-256, and
the upstream sources scripts/check-test-fonts.sh checks them against are the ones credited."""

import hashlib
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FONTS = ROOT / "tests" / "fonts"
NOTICES = ROOT / "THIRD-PARTY-NOTICES.md"


# An assignment of a pin in scripts/check-test-fonts.sh.
PIN = r"^(\w+_(?:url|sha256|commit))=(.*)$"

SECTIONS = {
    "liberation": "### Liberation Sans, Serif and Mono",
    "carlito": "### Carlito",
    "noto": "### Noto Sans",
}

EXPECTED = [
    "liberation_url",
    "liberation_sha256",
    "carlito_url",
    "carlito_commit",
    "noto_url",
    "noto_commit",
]

# What the notices put around each kind of pin, and the shape of the pin.
CONTEXTS = {
    "url": ("<{}>", r"https://\S+"),
    "sha256": ("(archive SHA-256 `{}`)", r"[0-9a-f]{64}"),
    "commit": ("commit `{}`", r"[0-9a-f]{40}"),
}


def pin_errors(script, notices):
    """What is wrong with the pins of `script`: each of the six must be assigned once, have
    its shape, and stand in the notices' section of its font with the text around it there,
    whitespace collapsed."""
    errors = []
    assignments = re.findall(PIN, script, re.M)
    names = [name for name, _ in assignments]
    if sorted(names) != sorted(EXPECTED):
        errors.append(f"pins assigned: {sorted(names)}, expected each of {sorted(EXPECTED)} once")
    for name, value in assignments:
        font, kind = name.split("_")
        context, shape = CONTEXTS[kind]
        if not re.fullmatch(shape, value):
            errors.append(f"{name}={value} is not a full {kind}")
            continue
        start = notices.find(SECTIONS.get(font, "\0"))
        if start < 0:
            errors.append(f"{name}: the notices have no section for {font}")
            continue
        end = notices.find("\n#", start + 1)
        section = re.sub(r"\s+", " ", notices[start:end])
        if context.format(value) not in section:
            errors.append(f"{name}={value} is not in the notices as {context.format('…')}")
    return errors


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
        notices = NOTICES.read_text(encoding="utf-8")
        self.assertEqual(pin_errors(script, notices), [])

    def test_wrong_pins_are_found(self):
        script = (ROOT / "scripts" / "check-test-fonts.sh").read_text(encoding="utf-8")
        notices = NOTICES.read_text(encoding="utf-8")
        pins = dict(re.findall(PIN, script, re.M))

        def file_hash(font):
            """The SHA-256 of the first file the notices list for `font`."""
            section = notices[notices.index(SECTIONS[font]) :]
            return re.search(r"\(SHA-256 `([0-9a-f]{64})`\)", section).group(1)

        commit = pins["noto_commit"]
        # The earlier check took the first three, and a pin assigned twice (the review of
        # #208, issue #232).
        wrong = {
            "a shortened URL": ("carlito_url", "https://github.com"),
            "a file's SHA-256 for the archive's": ("liberation_sha256", file_hash("liberation")),
            "a piece of a file's SHA-256 for a commit": ("carlito_commit", file_hash("carlito")[:40]),
            "a URL of another font": ("noto_url", pins["carlito_url"]),
            "another font's commit": ("carlito_commit", commit),
            "a commit with one digit changed": (
                "noto_commit",
                commit[:39] + ("1" if commit[39] == "0" else "0"),
            ),
        }
        for case, (name, value) in wrong.items():
            with self.subTest(case):
                changed = re.sub(f"^{name}=.*$", f"{name}={value}", script, count=1, flags=re.M)
                self.assertNotEqual(changed, script)
                self.assertNotEqual(pin_errors(changed, notices), [])
        with self.subTest("a wrong pin assigned before the right one"):
            twice = script.replace(
                f"noto_commit={commit}", f"noto_commit={'0' * 40}\nnoto_commit={commit}"
            )
            self.assertNotEqual(pin_errors(twice, notices), [])
        with self.subTest("a pin missing"):
            missing = re.sub(r"^carlito_commit=.*\n", "", script, flags=re.M)
            self.assertNotEqual(pin_errors(missing, notices), [])

    def test_every_listed_file_is_there(self):
        notices = NOTICES.read_text(encoding="utf-8")
        listed = set(re.findall(r"`(tests/fonts/[^`]+)`", notices))
        self.assertTrue(listed)
        for name in sorted(listed):
            with self.subTest(name):
                self.assertTrue((ROOT / name).is_file())


if __name__ == "__main__":
    unittest.main()
