"""Tests of scripts/check-added-files.sh in a throwaway git repository."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "check-added-files.sh"
BASH = shutil.which("bash")

# Files that name a home directory, in the forms they take in text, JSON and binaries.
# The user names are filled in when the tests run, so this file passes the check itself.
OFFENDING = {
    "linux.txt": b"fonts in /home/%s/.fonts\n" % b"alice",
    "mac.md": b"see /Users/%s/Library/Fonts\n" % b"bob",
    "windows.ps1": b"$p = 'C:\\Users\\%s\\AppData'\n" % b"carol",
    "escaped.json": b'{"path": "C:\\\\Users\\\\%s\\\\x.docx"}\n' % b"dave",
    "url.xml": b"<r Target='file:///c:/users/%s/Normal.dotm'/>\n" % b"erin",
    "binary.bin": b"\x00\x01PDF C:\\Users\\%s\\file.docx\x00\xff" % b"Frank Smith",
}
# Placeholders and paths that are not home directories.
ALLOWED = {
    "placeholders.txt": b"/home/<name>/.fonts /Users/<name>/Library C:\\Users\\<name>\\x\n",
    "escaped.xml": b"<r Target='file:///C:/Users/&lt;name&gt;/Normal.dotm'/>\n",
    "system.txt": b"/usr/share/fonts /home/ C:\\Windows\\Fonts /Users/\n",
}
BYTECODE = ["__pycache__/tool.cpython-312.pyc", "tool.pyc", "lib/old.pyo"]


class CheckAddedFiles(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(BASH, "bash is needed to run scripts/check-added-files.sh")
        self.repo = Path(tempfile.mkdtemp(prefix="check-added-files-"))
        self.addCleanup(shutil.rmtree, self.repo)
        self.git("init", "-q")

    def git(self, *args):
        subprocess.run(
            ["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", *args],
            cwd=self.repo, check=True, capture_output=True,
        )

    def write(self, files):
        for name, data in files.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        self.git("add", "-f", "--", *files)

    def check(self, *args):
        result = subprocess.run(
            [BASH, SCRIPT.as_posix(), *args], cwd=self.repo, capture_output=True, text=True,
            env={**os.environ, "LC_ALL": "C"},
        )
        return result.returncode, result.stderr

    def test_finds_every_form_of_a_home_directory(self):
        self.write({**OFFENDING, **ALLOWED})
        for args in [(), ("--staged",)]:
            code, err = self.check(*args)
            self.assertEqual(code, 1, err)
            for name in OFFENDING:
                self.assertIn(f"  {name}:1:", err)
            for name in ALLOWED:
                self.assertNotIn(name, err)
            self.assertIn("/home/%s/" % "alice", err)
            self.assertIn("C:\\Users\\%s\\" % "Frank Smith", err)

    def test_placeholders_pass(self):
        self.write(ALLOWED)
        self.assertEqual(self.check(), (0, ""))
        self.assertEqual(self.check("--staged"), (0, ""))

    def test_refuses_python_bytecode(self):
        self.write({name: b"\x00bytecode" for name in BYTECODE})
        code, err = self.check("--staged")
        self.assertEqual(code, 1)
        for name in BYTECODE:
            self.assertIn(f"  {name}\n", err)
        # Removing bytecode that was committed before is what the check asks for.
        self.git("commit", "-q", "--no-verify", "-m", "old")
        self.git("rm", "-q", "--cached", "--", *BYTECODE)
        self.assertEqual(self.check("--staged"), (0, ""))

    def test_staged_checks_only_what_the_commit_adds_or_changes(self):
        self.write({"linux.txt": OFFENDING["linux.txt"]})
        self.git("commit", "-q", "--no-verify", "-m", "old")
        self.write({"placeholders.txt": ALLOWED["placeholders.txt"]})
        self.assertEqual(self.check("--staged"), (0, ""))
        # Every tracked file is checked without --staged.
        self.assertEqual(self.check()[0], 1)
        # A change to the old file is checked, as it will be committed: the index, not
        # the working tree.
        self.write({"linux.txt": b"now /home/%s/x\n" % b"bob"})
        (self.repo / "linux.txt").write_bytes(b"clean again\n")
        code, err = self.check("--staged")
        self.assertEqual(code, 1)
        self.assertIn("linux.txt:1:/home/%s/" % "bob", err)
        # Removing it is fine.
        self.git("rm", "-q", "-f", "--cached", "linux.txt")
        self.assertEqual(self.check("--staged"), (0, ""))

    def test_nothing_staged(self):
        self.assertEqual(self.check("--staged"), (0, ""))


if __name__ == "__main__":
    unittest.main()
