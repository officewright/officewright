#!/usr/bin/env python3
"""Keeps the committed corpus small: every file of tests/corpus at most 1 MB, and the whole
folder at most 100 MB, where 1 MB is 1,000,000 bytes.

The sizes are those of the files git tracks under tests/corpus, as they are in the working
tree of the repository the script runs in; files git does not track are not counted, and
a tracked file deleted from the working tree counts as gone. .gitattributes gives text
files LF line ends on every platform, so the sizes are the same everywhere.

    check-corpus-size.py   exit 0 with a line of the sizes, or 1 with what is too large
"""

import os
import subprocess
import sys

CORPUS = "tests/corpus"
MB = 1_000_000
MAX_FILE = 1 * MB
MAX_TOTAL = 100 * MB


def megabytes(size: int) -> str:
    return f"{size / MB:.2f} MB"


def main() -> int:
    top = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], check=True, capture_output=True, text=True
    ).stdout.strip()
    os.chdir(top)
    listed = subprocess.run(
        ["git", "ls-files", "-z", "--", CORPUS], check=True, capture_output=True
    ).stdout
    sizes = {}
    for name in listed.decode("utf-8", "surrogateescape").split("\0"):
        if name and os.path.isfile(name):
            sizes[name] = os.path.getsize(name)
    total = sum(sizes.values())

    failed = False
    # In the order git lists them, by path.
    large = [name for name, size in sizes.items() if size > MAX_FILE]
    if large:
        print(
            f"check-corpus-size: these files of {CORPUS} are larger than its limit of "
            f"1 MB ({MAX_FILE:,} bytes) a file:",
            file=sys.stderr,
        )
        for name in large:
            print(f"  {name}: {sizes[name]:,} bytes", file=sys.stderr)
        failed = True
    if total > MAX_TOTAL:
        print(
            f"check-corpus-size: {CORPUS} holds {total:,} bytes ({megabytes(total)}) in "
            f"its files, more than its limit of 100 MB ({MAX_TOTAL:,} bytes)",
            file=sys.stderr,
        )
        failed = True
    if failed:
        return 1
    largest = max(sizes.values(), default=0)
    print(
        f"{CORPUS}: {megabytes(total)} in {len(sizes)} files, of 100 MB; "
        f"the largest file {megabytes(largest)}, of 1 MB"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
