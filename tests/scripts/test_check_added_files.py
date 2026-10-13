"""Tests of scripts/check-added-files.sh in a throwaway git repository."""

import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "check-added-files.sh"
BASH = shutil.which("bash")

# Files that name a home directory, in the forms they take in text, JSON, URLs and binary
# files. The user names are filled in when the tests run, so this file passes the check.
OFFENDING = {
    "linux.txt": b"fonts in /home/%s/.fonts\n" % b"alice",
    "mac.md": b"see /Users/%s/Library/Fonts\n" % b"bob",
    "windows.ps1": b"$p = 'C:\\Users\\%s\\AppData'\n" % b"carol",
    "escaped.json": b'{"path": "C:\\\\Users\\\\%s\\\\x.docx"}\n' % b"dave",
    "url.xml": b"<r Target='file:///c:/users/%s/Normal.dotm'/>\n" % b"erin",
    "binary.bin": b"\x00\x01PDF C:\\Users\\%s\\file.docx\x00\xff" % b"Frank Smith",
    "unicode.txt": "C:\\Users\\%s\\x and /home/%s/\n".encode() % ("Jürgen".encode(), "Олена".encode()),
    "percent.xml": b"<r Target='file:///C:/Users/%s/Normal.dotm'/>\n" % b"John%20Smith",
    "apostrophe.txt": b"C:\\Users\\%s\\x\n" % b"d'Arcy",
    "wsl.txt": b"\\\\wsl.localhost\\Ubuntu\\home\\%s\\dev\\x.docx\n" % b"gwen",
    "wsl.json": b'"\\\\\\\\wsl$\\\\Ubuntu\\\\home\\\\%s\\\\x"\n' % b"hal",
    "xp.txt": b"C:\\Documents and Settings\\%s\\x\n" % b"ivy",
    # The home directory itself, with nothing after the name.
    "env.sh": b"HOME=/home/%s\n" % b"jo",
    "home.json": b'{"home": "/Users/%s", "profile": "C:\\\\Users\\\\%s"}\n' % (b"kim", b"lee"),
    "home.xml": b"<home>/home/%s</home>\n" % b"mo",
    "spaced.txt": b"cd /home/%s && ls\n" % b"nina",
    # A name ends at each of these, one file each, so every one of them is needed.
    **{
        f"end-{i}.txt": b"x%s/home/%s%sy\n" % (before, b"oz", after)
        for i, (before, after) in enumerate(
            [(b"<p>", b"</p>"), (b"(", b")"), (b"[", b","), (b"a=", b";"), (b"", b":"),
             (b"", b"&amp;"), (b"<", b">"), (b"", b"(")]
        )
    },
    # A name ends at any character no user name can have.
    "bracket.txt": b"/home/%s[1]/x\n" % b"sam",
    "equals.txt": b"x=/home/%s=y\n" % b"tia",
    # \Users without a drive letter, and the profile roots in upper case, as Windows and
    # macOS read them.
    "rooted.txt": b"\\Users\\%s\\AppData and \\\\Users\\\\%s\\\\x\n" % (b"uma", b"vic"),
    "upper.txt": b"/USERS/%s/\n" % b"wes",
    "upper-xp.txt": b"C:\\DOCUMENTS AND SETTINGS\\%s\\\n" % b"xia",
    "lower-xp.txt": b"c:\\documents and settings\\%s\\\n" % b"yan",
    "lower-drive.txt": b"d:/users/%s/x\n" % b"zed",
    # A name ends at a control character: a C string in a binary file.
    "cstring.bin": b"\x00/home/%s\x00" % b"ray",
}
# Placeholders, and paths that are not home directories.
ALLOWED = {
    "placeholders.txt": b"/home/<name>/.fonts /Users/<name>/Library C:\\Users\\<name>\\x\n",
    "escaped.xml": b"<r Target='file:///C:/Users/&lt;name&gt;/Normal.dotm'/>\n",
    "variables.sh": b"/home/$USER/x C:\\Users\\%USERNAME%\\x /home/{user}/x /home/${USER}\n",
    "built.py": b"'C:\\\\Users\\\\' + name + '\\\\x' and `/home/` and /home/ alone\n",
    "system.txt": b"/usr/share/fonts C:\\Windows\\Fonts C:\\Users\\ /Users/\n",
    # Linux home directories are lower case: an upper-case Home is something else.
    "views.txt": b"Views/Home/Index.cshtml /HOME/README\n",
    # A lower-case users folder is not a profile without a drive letter.
    "rest.txt": b"/api/users/42 https://api.github.com/users/octocat docs/users/guide\n",
}
BYTECODE = [
    "__pycache__/tool.cpython-312.pyc",
    "tool.pyc",
    "lib/old.pyo",
    "lib/__pycache__/stale.txt",
    "töol.pyc",
]
# Names Windows does not allow: a newline and a tab, which Git lists quoted.
if os.name != "nt":
    BYTECODE += ["bad\nname.pyc", "tab\tname.pyo"]


def remove_read_only(function, path, _):
    """Git writes its objects read-only, which Windows does not let rmtree delete."""
    try:
        os.chmod(path, stat.S_IWRITE)
        function(path)
    except FileNotFoundError:
        # Gone already, such as a lock of a git process that has just ended.
        pass


class CheckAddedFiles(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(BASH, "bash is needed to run scripts/check-added-files.sh")
        self.repo = Path(tempfile.mkdtemp(prefix="check-added-files-"))
        if sys.version_info >= (3, 12):
            self.addCleanup(shutil.rmtree, self.repo, onexc=remove_read_only)
        else:
            self.addCleanup(shutil.rmtree, self.repo, onerror=remove_read_only)
        self.git("init", "-q")
        # No background maintenance after a commit, whose lock files would come and go
        # while the repository is removed.
        self.git("config", "maintenance.auto", "false")
        self.git("config", "gc.auto", "0")

    def git(self, *args):
        subprocess.run(
            ["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
             "-c", "commit.gpgsign=false", *args],
            cwd=self.repo, check=True, capture_output=True,
        )

    def write(self, files):
        for name, data in files.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        self.git("--literal-pathspecs", "add", "-f", "--", *files)

    def commit(self):
        self.git("commit", "-q", "--no-verify", "-m", "old")

    def check(self, *args, locale="C"):
        result = subprocess.run(
            [BASH, SCRIPT.as_posix(), *args], cwd=self.repo, capture_output=True,
            env={**os.environ, "LC_ALL": locale},
        )
        return result.returncode, result.stderr.decode("utf-8", "replace")

    def test_finds_every_form_of_a_home_directory(self):
        self.write({**OFFENDING, **ALLOWED})
        for args in [(), ("--staged",)]:
            for locale in ["C", "C.UTF-8"]:
                code, err = self.check(*args, locale=locale)
                self.assertEqual(code, 1, err)
                for name in OFFENDING:
                    self.assertIn(f"  {name}:1:", err, (args, locale))
                for name in ALLOWED:
                    self.assertNotIn(name, err, (args, locale))
                self.assertIn("/home/%s/" % "alice", err)
                self.assertIn("C:\\Users\\%s\\" % "Frank Smith", err)

    def test_placeholders_pass(self):
        self.write(ALLOWED)
        self.assertEqual(self.check(), (0, ""))
        self.assertEqual(self.check("--staged"), (0, ""))

    def test_refuses_python_bytecode(self):
        self.write({name: b"\x00bytecode" for name in BYTECODE})
        for args in [(), ("--staged",)]:
            code, err = self.check(*args)
            self.assertEqual(code, 1, args)
            for name in BYTECODE:
                self.assertIn(f"  {name}\n", err, args)
        # Removing bytecode that was committed before is what the check asks for.
        self.commit()
        self.git("rm", "-q", "--cached", "--", *BYTECODE)
        self.assertEqual(self.check("--staged"), (0, ""))

    def test_a_renamed_file_is_checked_under_its_new_name(self):
        self.write({"tool.py": b"print()\n"})
        self.commit()
        self.git("mv", "tool.py", "tool.pyc")
        code, err = self.check("--staged")
        self.assertEqual(code, 1)
        self.assertIn("  tool.pyc\n", err)

    def test_staged_checks_only_what_the_commit_adds_or_changes(self):
        self.write({"linux.txt": OFFENDING["linux.txt"], "other.txt": b"x\n"})
        self.commit()
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

    def test_a_commit_that_only_removes_files_passes(self):
        self.write({"linux.txt": OFFENDING["linux.txt"], "other.txt": b"x\n"})
        self.commit()
        self.git("rm", "-q", "other.txt")
        self.assertEqual(self.check("--staged"), (0, ""))

    def test_file_names_are_not_pathspecs(self):
        # `[ab].txt` would be a pattern for a.txt, and where file names may have a colon,
        # `:(x)` would be pathspec magic and `:notes.txt` the pathspec of notes.txt.
        colons = os.name != "nt"
        old = {":(x)": b"x\n", ":notes.txt": OFFENDING["linux.txt"]} if colons else {}
        self.write({**old, "a.txt": b"x\n"})
        self.commit()
        self.write({"[ab].txt": b"x\n", "b.txt": OFFENDING["mac.md"]})
        (self.repo / "a.txt").write_bytes(OFFENDING["linux.txt"])
        code, err = self.check("--staged")
        self.assertEqual(code, 1, err)
        self.assertIn("  b.txt:1:", err)
        self.assertNotIn("a.txt", err.replace("[ab].txt", ""))
        code, err = self.check()
        self.assertEqual(code, 1, err)
        self.assertIn("  a.txt:1:", err)
        if colons:
            self.assertIn("  :notes.txt:1:", err)

    def test_a_git_error_fails_the_check(self):
        # A git that fails in grep, as it would on a broken repository.
        real = Path(shutil.which("git")).as_posix()
        bin_dir = self.repo / ".fake-bin"
        bin_dir.mkdir()
        fake = bin_dir / "git"
        fake.write_text(
            "#!/bin/sh\n"
            'for a; do [ "$a" = grep ] && { echo "fatal: broken" >&2; exit 2; }; done\n'
            f'exec "{real}" "$@"\n'
        )
        fake.chmod(0o755)
        self.write(ALLOWED)
        env_path = f"{bin_dir}{os.pathsep}{os.environ['PATH']}"
        for args in [(), ("--staged",)]:
            result = subprocess.run(
                [BASH, SCRIPT.as_posix(), *args], cwd=self.repo, capture_output=True,
                text=True, env={**os.environ, "PATH": env_path},
            )
            self.assertEqual(result.returncode, 1, args)
            self.assertIn("git grep failed", result.stderr, args)

    def test_nothing_staged(self):
        self.assertEqual(self.check("--staged"), (0, ""))


if __name__ == "__main__":
    unittest.main()
