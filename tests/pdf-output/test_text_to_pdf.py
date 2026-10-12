"""Checks PDFs made by `officewright text-to-pdf` with tools that share no code with it:
qpdf validates the file structure (cross-reference offsets, stream lengths, objects)
and poppler's pdftotext must read the input text back.

OW_CLI must point at the officewright binary (scripts/check.sh sets it).
"""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SAMPLE = Path(__file__).resolve().parent / "sample.txt"

# The default layout, and a narrow one that wraps more and fills more pages.
OPTIONS = ([], ["--page-size", "letter", "--font-size", "16"])


def normalized(text):
    # ISO 32000-1, Annex D: WinAnsiEncoding shows U+00A0 with the space glyph and U+00AD
    # with the hyphen glyph, so that is what a reader extracts.
    return text.replace("\N{NO-BREAK SPACE}", " ").replace("\N{SOFT HYPHEN}", "-")


def words(text):
    return normalized(text).split()


def without_character_table(text):
    """The sample without its table of single characters set one space apart, which
    pdftotext joins into words; what remains is prose whose words must survive."""
    lines = text.split("\n")
    start = lines.index("Every character of WinAnsiEncoding, separated by spaces:") + 1
    end = lines.index("", start)
    return "\n".join(lines[:start] + lines[end:])


def characters(text):
    """The text without whitespace, for the character table, where pdftotext's guesses
    about spaces differ from the input; the prose test checks the spaces between words.
    """
    return "".join(words(text))


class TextToPdfTest(unittest.TestCase):
    def convert(self, directory, options, source=SAMPLE):
        pdf = Path(directory) / "sample.pdf"
        command = [os.environ["OW_CLI"], "text-to-pdf", *options, str(source), str(pdf)]
        subprocess.run(command, check=True)
        return pdf

    def pdftotext(self, pdf):
        result = subprocess.run(["pdftotext", "-enc", "UTF-8", str(pdf), "-"], capture_output=True, check=True)
        return result.stdout.decode("utf-8")

    def test_qpdf_finds_no_errors_or_warnings(self):
        for options in OPTIONS:
            with self.subTest(options=options), tempfile.TemporaryDirectory() as directory:
                pdf = self.convert(directory, options)
                # Exit status 0 means no errors and no warnings (2 and 3 otherwise).
                result = subprocess.run(["qpdf", "--check", str(pdf)], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_pdftotext_reads_back_the_input(self):
        expected = characters(SAMPLE.read_text(encoding="utf-8"))
        for options in OPTIONS:
            with self.subTest(options=options), tempfile.TemporaryDirectory() as directory:
                pdf = self.convert(directory, options)
                self.assertEqual(characters(self.pdftotext(pdf)), expected)

    def test_pdftotext_keeps_the_words_of_prose(self):
        prose = without_character_table(SAMPLE.read_text(encoding="utf-8"))
        for options in OPTIONS:
            with self.subTest(options=options), tempfile.TemporaryDirectory() as directory:
                source = Path(directory) / "prose.txt"
                source.write_bytes(prose.encode("utf-8"))
                pdf = self.convert(directory, options, source)
                self.assertEqual(words(self.pdftotext(pdf)), words(prose))


if __name__ == "__main__":
    unittest.main()
