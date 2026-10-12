#!/usr/bin/env python3
"""Imports reference renders made by scripts/windows/make-reference.ps1 into tests/corpus/.

    corpus-import.py RESULTS_DIR --group word-rules [--dry-run]

RESULTS_DIR is the -OutputDir of make-reference.ps1 (for example /mnt/c/officewright-ref/out
under WSL). For every <name>.json there the script checks that

  - Word rendered the document and found no personal data in its output;
  - the input is the committed tests/corpus/<group>/<name>.docx, byte for byte;
  - the PDF and the re-saved DOCX are the files the JSON describes;
  - neither file names a person, a company or a user profile path in its metadata
    (a second check, independent of the one on Windows);
  - pdfinfo counts the pages Word counted;

and then copies the PDF to tests/corpus/reference/<group>/<name>.pdf, the re-saved DOCX to
tests/corpus/resaved/<group>/<name>.docx, and writes the document's row in the "Reference
renders" table of tests/corpus/SOURCES.md, with the fonts pdffonts reports as not embedded.
Nothing is copied unless every document passes. Needs pdfinfo and pdffonts (poppler-utils).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Callable
from xml.etree import ElementTree

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "tests" / "corpus"

# The make-reference.ps1 output format this script reads.
SCRIPT_VERSION = 1

# Metadata values our own documents carry; anything else in a name field is refused.
ALLOWED_NAMES = {"", "Officewright"}

# A path into a user profile: C:\Users\<name>, /Users/<name>, file:///C:/Users/...
PROFILE_PATH = re.compile(rb"(?i)(\\|/)(users|documents and settings)(\\|/)")

REFERENCE_HEADER = "| File | Rendered from | Application and version | Date | Notes |"

REQUIRED = {
    "script_version": int,
    "file": str,
    "sha256": str,
    "resaved_file": str,
    "resaved_sha256": str,
    "pdf_file": str,
    "pdf_sha256": str,
    "status": str,
    "pages": int,
    "compatibility_mode": int,
    "fonts": list,
    "fonts_missing": list,
    "privacy_findings": list,
    "word_version": str,
    "word_build": str,
    "windows_version": str,
    "rendered": str,
}


class Refused(Exception):
    """A result that must not enter the corpus; the message says why."""


@dataclass(frozen=True)
class Tools:
    """The poppler tools, replaceable in tests."""

    info: Callable[[Path], str]
    xmp: Callable[[Path], str]
    fonts: Callable[[Path], str]


def run_tool(*args: str) -> str:
    try:
        done = subprocess.run(args, check=True, capture_output=True)
    except FileNotFoundError as error:
        raise Refused(f"{args[0]} is not installed (apt install poppler-utils)") from error
    except subprocess.CalledProcessError as error:
        message = error.stderr.decode(errors="replace").strip()
        raise Refused(f"{' '.join(args)} failed: {message}") from error
    return done.stdout.decode("utf-8", errors="replace")


POPPLER = Tools(
    info=lambda path: run_tool("pdfinfo", str(path)),
    xmp=lambda path: run_tool("pdfinfo", "-meta", str(path)),
    fonts=lambda path: run_tool("pdffonts", str(path)),
)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_record(path: Path) -> dict:
    """The JSON of one document, with its fields checked."""
    try:
        # PowerShell may write a byte order mark.
        record = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, ValueError) as error:
        raise Refused(f"{path.name}: cannot read it: {error}") from error
    if not isinstance(record, dict):
        raise Refused(f"{path.name}: not a JSON object")
    for key, kind in REQUIRED.items():
        value = record.get(key)
        # ConvertTo-Json in Windows PowerShell 5.1 writes a one-element array as its element.
        if kind is list and isinstance(value, str):
            value = record[key] = [value]
        if kind is list and value is None:
            value = record[key] = []
        if not isinstance(value, kind) or isinstance(value, bool):
            raise Refused(f"{path.name}: field {key} is missing or not a {kind.__name__}")
    if record["script_version"] != SCRIPT_VERSION:
        raise Refused(
            f"{path.name}: written by make-reference.ps1 format {record['script_version']}, "
            f"this script reads format {SCRIPT_VERSION}"
        )
    if record["status"] != "rendered":
        detail = record.get("error") or ", ".join(record["privacy_findings"])
        raise Refused(f"{record['file']}: status {record['status']}: {detail}")
    if record["privacy_findings"]:
        findings = ", ".join(record["privacy_findings"])
        raise Refused(f"{record['file']}: personal data: {findings}")
    for key in ("file", "resaved_file", "pdf_file"):
        value = record[key]
        if re.search(r"[\\/:]", value) or value in ("", ".", ".."):
            raise Refused(f"{path.name}: {key} must be a file name, not {value!r}")
    return record


def local_name(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def docx_privacy_problems(path: Path) -> list[str]:
    """Personal data in a DOCX package: names in the document properties, profile paths."""
    problems = []
    with zipfile.ZipFile(path) as archive:
        for info in archive.infolist():
            data = archive.read(info)
            if PROFILE_PATH.search(data):
                problems.append(f"{info.filename} contains a user profile path")
            if info.filename in ("docProps/core.xml", "docProps/app.xml"):
                try:
                    root = ElementTree.fromstring(data)
                except ElementTree.ParseError as error:
                    problems.append(f"{info.filename} is not well-formed: {error}")
                    continue
                for element in root:
                    name = local_name(element.tag)
                    value = (element.text or "").strip()
                    if name in ("creator", "lastModifiedBy") and value not in ALLOWED_NAMES:
                        problems.append(f"{info.filename}: {name} is {value!r}")
                    if name in ("Company", "Manager") and value:
                        problems.append(f"{info.filename}: {name} is {value!r}")
    return problems


def pdf_privacy_problems(info: str, xmp: str, data: bytes) -> list[str]:
    """Personal data in a PDF: the Author entry, XMP creator fields, profile paths."""
    problems = []
    for line in info.splitlines():
        key, _, value = line.partition(":")
        if key.strip() == "Author" and value.strip() not in ALLOWED_NAMES:
            problems.append(f"PDF Author is {value.strip()!r}")
    if xmp.strip():
        for tag in ("dc:creator", "pdf:Author", "xmp:Author"):
            for match in re.finditer(rf"<{tag}>(.*?)</{tag}>", xmp, re.S):
                names = re.findall(r"<rdf:li[^>]*>(.*?)</rdf:li>", match.group(1), re.S)
                for name in names or [match.group(1)]:
                    if name.strip() not in ALLOWED_NAMES:
                        problems.append(f"PDF XMP {tag} is {name.strip()!r}")
    if PROFILE_PATH.search(data):
        problems.append("PDF contains a user profile path")
    return problems


def pdf_pages(info: str) -> int:
    match = re.search(r"^Pages:\s+(\d+)\s*$", info, re.M)
    if not match:
        raise Refused("pdfinfo reports no page count")
    return int(match.group(1))


def not_embedded(pdffonts: str) -> list[str]:
    """Fonts pdffonts lists with emb = no.

    pdffonts prints a header, a dashed rule whose dash groups give the column widths, and a
    row per font: name, type, encoding, emb, sub, uni, object, generation.
    """
    lines = pdffonts.splitlines()
    rule = next((i for i, line in enumerate(lines) if line.startswith("---")), None)
    if rule is None:
        return []
    columns = [m.span() for m in re.finditer(r"-+", lines[rule])]
    names = []
    for line in lines[rule + 1 :]:
        if not line.strip() or len(columns) < 4:
            continue
        cells = [line[start:end + 1].strip() for start, end in columns]
        if cells[3] == "no":
            names.append(re.sub(r"^[A-Z]{6}\+", "", cells[0]))
    return sorted(set(names))


def reference_row(group: str, record: dict, missing_embeds: list[str]) -> str:
    base = Path(record["file"]).stem
    windows = record["windows_version"]
    display = record.get("windows_display_version") or ""
    if display:
        windows = f"{windows} ({display})"
    notes = [
        f"compatibility mode {record['compatibility_mode']}",
        f"{record['pages']} pages",
        "fonts Word substituted: " + (", ".join(record["fonts_missing"]) or "none"),
        "fonts not embedded: " + (", ".join(missing_embeds) or "none"),
        "fields as saved",
    ]
    return (
        f"| reference/{group}/{base}.pdf "
        f"| {group}/{record['file']}, re-saved by Word as resaved/{group}/{base}.docx "
        f"| Microsoft Word {record['word_version']}, build {record['word_build']}, "
        f"Windows {windows} "
        f"| {record['rendered']} "
        f"| {'; '.join(notes)} |"
    )


def upsert_reference_row(sources: str, row: str) -> str:
    """SOURCES.md with the row in the reference table, replacing a row for the same file."""
    lines = sources.split("\n")
    try:
        header = lines.index(REFERENCE_HEADER)
    except ValueError as error:
        raise Refused(f"SOURCES.md has no table header {REFERENCE_HEADER!r}") from error
    key = row.split("|")[1].strip()
    end = header + 2  # past the header and the separator
    while end < len(lines) and lines[end].startswith("|"):
        if lines[end].split("|")[1].strip() == key:
            lines[end] = row
            return "\n".join(lines)
        end += 1
    rows = sorted(lines[header + 2 : end] + [row])
    return "\n".join(lines[: header + 2] + rows + lines[end:])


@dataclass(frozen=True)
class Checked:
    record: dict
    pdf: Path
    resaved: Path
    row: str


def check_result(json_path: Path, group: str, corpus: Path, tools: Tools) -> Checked:
    record = load_record(json_path)
    results = json_path.parent
    original = corpus / group / record["file"]
    if not original.is_file():
        raise Refused(f"{record['file']}: tests/corpus/{group}/ has no such document")
    if sha256(original) != record["sha256"]:
        raise Refused(f"{record['file']}: Word rendered other bytes than the committed file")
    pdf = results / record["pdf_file"]
    resaved = results / record["resaved_file"]
    for path, key in ((pdf, "pdf_sha256"), (resaved, "resaved_sha256")):
        if not path.is_file():
            raise Refused(f"{record['file']}: {path.name} is missing")
        if sha256(path) != record[key]:
            raise Refused(f"{record['file']}: {path.name} changed after Word wrote it")
    info = tools.info(pdf)
    problems = docx_privacy_problems(resaved)
    problems += pdf_privacy_problems(info, tools.xmp(pdf), pdf.read_bytes())
    if problems:
        raise Refused(f"{record['file']}: {'; '.join(problems)}")
    pages = pdf_pages(info)
    if pages != record["pages"]:
        raise Refused(
            f"{record['file']}: Word counted {record['pages']} pages, the PDF has {pages}"
        )
    row = reference_row(group, record, not_embedded(tools.fonts(pdf)))
    return Checked(record, pdf, resaved, row)


def import_results(
    results: Path, group: str, corpus: Path, tools: Tools, dry_run: bool
) -> list[Checked]:
    if not (corpus / group).is_dir():
        raise Refused(f"tests/corpus/{group} does not exist")
    records = sorted(p for p in results.glob("*.json") if p.name != "run.json")
    if not records:
        raise Refused(f"no results in {results}")
    checked = []
    errors = []
    for path in records:
        try:
            checked.append(check_result(path, group, corpus, tools))
        except Refused as error:
            errors.append(str(error))
    if errors:
        raise Refused("nothing imported:\n  " + "\n  ".join(errors))
    if dry_run:
        return checked
    sources_path = corpus / "SOURCES.md"
    sources = sources_path.read_text(encoding="utf-8")
    for item in checked:
        base = Path(item.record["file"]).stem
        reference = corpus / "reference" / group / f"{base}.pdf"
        resaved = corpus / "resaved" / group / f"{base}.docx"
        reference.parent.mkdir(parents=True, exist_ok=True)
        resaved.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(item.pdf, reference)
        shutil.copyfile(item.resaved, resaved)
        sources = upsert_reference_row(sources, item.row)
    sources_path.write_text(sources, encoding="utf-8")
    return checked


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("results", type=Path, help="the -OutputDir of make-reference.ps1")
    parser.add_argument(
        "--group", required=True, help="the folder under tests/corpus/, e.g. word-rules"
    )
    parser.add_argument("--dry-run", action="store_true", help="check everything, copy nothing")
    args = parser.parse_args()
    try:
        checked = import_results(args.results, args.group, CORPUS, POPPLER, args.dry_run)
    except Refused as error:
        print(f"corpus-import.py: {error}", file=sys.stderr)
        return 1
    verb = "would import" if args.dry_run else "imported"
    for item in checked:
        print(f"{verb} {item.record['file']}: {item.record['pages']} pages")
    print(f"corpus-import.py: {verb} {len(checked)} reference renders")
    return 0


if __name__ == "__main__":
    sys.exit(main())
