#!/usr/bin/env python3
"""Keeps the committed corpus small: every file of tests/corpus at most 1 MB, and the whole
folder at most 100 MB, where 1 MB is 1,000,000 bytes.

The sizes are those of the files git tracks under tests/corpus, as they are in the working
tree of the repository the script runs in; files git does not track are not counted, and
a tracked file deleted from the working tree counts as gone. A symbolic link counts with
its own size, the length of the path it holds, as on a system that checks it out as a
file with that path. .gitattributes gives text files LF line ends on every platform, so
the sizes are the same everywhere.

    check-corpus-size.py   exit 0 with a line of the sizes, or 1 with what is too large
"""

import os
import stat
import subprocess
import sys

CORPUS = "tests/corpus"
MB = 1_000_000
MAX_FILE = 1 * MB
MAX_TOTAL = 100 * MB


def megabytes(size: int) -> str:
    return f"{size / MB:.2f} MB"


def git(*args: str) -> bytes:
    result = subprocess.run(["git", *args], capture_output=True)
    if result.returncode != 0:
        message = result.stderr.decode("utf-8", "replace").strip()
        sys.exit(f"check-corpus-size: git {args[0]} failed: {message}")
    return result.stdout


def main() -> int:
    os.chdir(git("rev-parse", "--show-toplevel").decode("utf-8", "surrogateescape").strip())
    listed = git("ls-files", "-z", "--", CORPUS).decode("utf-8", "surrogateescape")
    sizes = {}
    for name in filter(None, listed.split("\0")):
        try:
            status = os.lstat(name)
        except (FileNotFoundError, NotADirectoryError):
            continue  # deleted from the working tree, or its folder replaced by a file
        if stat.S_ISLNK(status.st_mode):
            sizes[name] = len(os.fsencode(os.readlink(name)))
        elif stat.S_ISREG(status.st_mode):
            sizes[name] = status.st_size
        # Anything else is a folder: a submodule.
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
