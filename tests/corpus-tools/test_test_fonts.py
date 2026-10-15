"""Every test font in tests/fonts/ and tests/fonts-word/ is credited in THIRD-PARTY-NOTICES.md
with its SHA-256, and the upstream sources scripts/check-test-fonts.sh checks them against are
the ones credited."""

import hashlib
import re
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FONTS = ROOT / "tests" / "fonts"
FONTS_WORD = ROOT / "tests" / "fonts-word"
NOTICES = ROOT / "THIRD-PARTY-NOTICES.md"


# An assignment of a pin in scripts/check-test-fonts.sh as the first word of its line:
# indented (inside an `if`, say), after `export`, `readonly`, `declare`, `local` or `typeset`
# and their options, or appended to with `+=`. The value is the rest of the line, a comment
# included. `pin_errors` also counts a pin's name assigned later in a line; what it does not
# see (`printf -v`, `read`) is left to the nightly run, which compares the files themselves.
PIN = (
    r"^[ \t]*(?:(?:export|readonly|declare|local|typeset)(?:[ \t]+-\w+)*[ \t]+)?"
    r"(\w+_(?:url|sha256|commit))\+?=(.*)$"
)

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


def section(notices, font):
    """The notices' section of `font`, up to the next heading, whitespace collapsed; None if
    there is none."""
    start = notices.find(SECTIONS[font]) if font in SECTIONS else -1
    if start < 0:
        return None
    end = notices.find("\n#", start + 1)
    return re.sub(r"\s+", " ", notices[start:] if end < 0 else notices[start:end])


def pin_errors(script, notices):
    """What is wrong with the pins of `script`: each of the six must be assigned once, have
    its shape, and stand in the notices' section of its font with the text around it there,
    whitespace collapsed."""
    errors = []
    assignments = re.findall(PIN, script, re.M)
    names = [name for name, _ in assignments]
    if sorted(names) != sorted(EXPECTED):
        errors.append(f"pins assigned: {sorted(names)}, expected each of {sorted(EXPECTED)} once")
    # Each name assigned once anywhere outside comments: also after `then`, `&&`, `;`, `{`,
    # another assignment or `declare --`, to an index, or in `eval`.
    code = "\n".join(line for line in script.splitlines() if not line.lstrip().startswith("#"))
    for name in EXPECTED:
        count = len(re.findall(rf"(?<![\w$]){name}(?:\[[^\]]*\])?\+?=", code))
        if count != 1:
            errors.append(f"{name} is assigned {count} times, not once")
    for name, value in assignments:
        font, kind = name.rsplit("_", 1)
        context, shape = CONTEXTS[kind]
        if not re.fullmatch(shape, value):
            errors.append(f"{name}={value} is not a full {kind}")
            continue
        text = section(notices, font)
        if text is None:
            errors.append(f"{name}: the notices have no section for {font}")
            continue
        if context.format(value) not in text:
            errors.append(f"{name}={value} is not in the notices as {context.format('…')}")
    return errors


def reflowed(notices, width):
    """The notices with the lines between headings wrapped again at `width` columns, as an
    edit might leave them; unchanged for None."""
    if width is None:
        return notices
    def wrap(text):
        # Words, URLs among them, stay whole, as an editor leaves them.
        return textwrap.fill(text, width, break_long_words=False, break_on_hyphens=False)

    parts = re.split(r"(^#.*$)", notices, flags=re.M)
    return "".join(
        part if part.startswith("#") else "\n" + wrap(" ".join(part.split())) + "\n"
        for part in parts
    )


def wrong_pins(script, notices):
    """Scripts with a wrong pin, each with the notices to check it against, by what is wrong."""
    pins = dict(re.findall(PIN, script, re.M))

    def file_hash(font):
        """The SHA-256 of the first file the notices list for `font`."""
        return re.search(r"\(SHA-256 `([0-9a-f]{64})`\)", section(notices, font)).group(1)

    def assigned(name, value):
        return re.sub(f"^{name}=.*$", f"{name}={value}", script, count=1, flags=re.M)

    commit = pins["noto_commit"]
    other = "1" if commit[39] == "0" else "0"
    cases = {
        # The earlier check took these three, and a pin assigned twice (the review of #208,
        # issue #232).
        "a shortened URL": assigned("carlito_url", "https://github.com"),
        "a file's SHA-256 for the archive's": assigned(
            "liberation_sha256", file_hash("liberation")
        ),
        "a piece of a file's SHA-256 for a commit": assigned(
            "carlito_commit", file_hash("carlito")[:40]
        ),
        "a wrong pin assigned before the right one": script.replace(
            f"noto_commit={commit}", f"noto_commit={'0' * 40}\nnoto_commit={commit}"
        ),
        "a URL of another font": assigned("noto_url", pins["carlito_url"]),
        "another font's commit": assigned("carlito_commit", commit),
        "a commit with one digit changed": assigned("noto_commit", commit[:39] + other),
        "a pin missing": re.sub(r"^carlito_commit=.*\n", "", script, flags=re.M),
    }
    # A wrong pin assigned again after the right one, in the other forms bash takes.
    for again in (
        f"noto_commit={'0' * 40} # an old pin",
        f"  noto_commit={'0' * 40}",
        f"export noto_commit={'0' * 40}",
        f"readonly noto_commit={'0' * 40}",
        f"declare -r noto_commit={'0' * 40}",
        f"typeset noto_commit={'0' * 40}",
        f"local noto_commit={'0' * 40}",
        f"noto_commit+={'0' * 40}",
        # Later in a line, which the second count finds.
        f"if true; then noto_commit={'0' * 40}; fi",
        f"true && noto_commit={'0' * 40}",
        f"true; noto_commit={'0' * 40}",
        f"{{ noto_commit={'0' * 40}; }}",
        f"foo=1 noto_commit={'0' * 40}",
        f"declare -- noto_commit={'0' * 40}",
        f"declare +x noto_commit={'0' * 40}",
        f"noto_commit[0]={'0' * 40}",
        f"eval noto_commit={'0' * 40}",
    ):
        cases[f"assigned again: {again}"] = script.replace(
            f"noto_commit={commit}", f"noto_commit={commit}\n{again}"
        )
    poi = re.search(r"## Apache POI.*?commit\s+`([0-9a-f]{40})`", notices, re.S).group(1)
    cases["another section's commit (POI's) for Noto's"] = assigned("noto_commit", poi)
    found = {case: (changed, notices) for case, changed in cases.items()}
    # A short commit, which git would take, written in the notices too.
    short = commit[:7]
    found["a short commit in the script and the notices"] = (
        assigned("noto_commit", short),
        notices.replace(f"`{commit}`", f"`{short}`"),
    )
    # Pins of the wrong shape, written in the notices too.
    archive = pins["liberation_sha256"]
    found["a short archive SHA-256 in the script and the notices"] = (
        assigned("liberation_sha256", archive[:12]),
        notices.replace(f"`{archive}`", f"`{archive[:12]}`"),
    )
    url = pins["carlito_url"]
    plain = url.replace("https://", "http://", 1)
    found["an http URL in the script and the notices"] = (
        assigned("carlito_url", plain),
        notices.replace(f"<{url}>", f"<{plain}>"),
    )
    return found


class TestFonts(unittest.TestCase):
    def test_every_file_is_listed_with_its_hash(self):
        notices = re.sub(r"\s+", " ", NOTICES.read_text(encoding="utf-8"))
        listed = dict(
            re.findall(r"`(tests/fonts(?:-word)?/[^`]+)` \(SHA-256 `([0-9a-f]{64})`\)", notices)
        )
        files = sorted(
            path for root in (FONTS, FONTS_WORD) for path in root.rglob("*") if path.is_file()
        )
        self.assertGreaterEqual(len(files), 24)
        for path in files:
            name = path.relative_to(ROOT).as_posix()
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            with self.subTest(name):
                self.assertEqual(listed.get(name), digest, "listed SHA-256 of the file")

    def test_the_upstream_pins_are_the_noticed_ones(self):
        # scripts/check-test-fonts.sh compares the files with these sources at night. Each
        # pin must be the full one the notices give in the section of its font, however
        # the notices wrap their lines.
        script = (ROOT / "scripts" / "check-test-fonts.sh").read_text(encoding="utf-8")
        notices = NOTICES.read_text(encoding="utf-8")
        for width in (None, 40, 72, 1000):
            with self.subTest(width=width):
                self.assertEqual(pin_errors(script, reflowed(notices, width)), [])
        # A comment may name an old pin.
        commented = re.sub(
            r"^(noto_commit=.*)$", f"# was noto_commit={'0' * 40}\n\\1", script, flags=re.M
        )
        self.assertNotEqual(commented, script)
        self.assertEqual(pin_errors(commented, notices), [])

    def test_wrong_pins_are_found(self):
        script = (ROOT / "scripts" / "check-test-fonts.sh").read_text(encoding="utf-8")
        for width in (None, 40, 72, 1000):
            notices = reflowed(NOTICES.read_text(encoding="utf-8"), width)
            for case, (changed, changed_notices) in wrong_pins(script, notices).items():
                with self.subTest(case, width=width):
                    self.assertNotEqual(changed, script)
                    self.assertNotEqual(pin_errors(changed, changed_notices), [])

    def test_every_listed_file_is_there(self):
        notices = NOTICES.read_text(encoding="utf-8")
        listed = set(re.findall(r"`(tests/fonts(?:-word)?/[^`]+)`", notices))
        self.assertTrue(listed)
        for name in sorted(listed):
            with self.subTest(name):
                self.assertTrue((ROOT / name).is_file())


if __name__ == "__main__":
    unittest.main()
