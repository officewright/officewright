"""Tests of scripts/check-corpus-size.py in a throwaway git repository."""

import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "check-corpus-size.py"
MB = 1_000_000


def remove_read_only(function, path, _):
    """Git writes its objects read-only, which Windows does not let rmtree delete."""
    try:
        os.chmod(path, stat.S_IWRITE)
        function(path)
    except FileNotFoundError:
        # Gone already, such as a lock of a git process that has just ended.
        pass


class CheckCorpusSize(unittest.TestCase):
    def setUp(self):
        self.repo = Path(tempfile.mkdtemp(prefix="check-corpus-size-"))
        if sys.version_info >= (3, 12):
            self.addCleanup(shutil.rmtree, self.repo, onexc=remove_read_only)
        else:
            self.addCleanup(shutil.rmtree, self.repo, onerror=remove_read_only)
        self.git("init", "-q")
        # No background maintenance, whose lock files would come and go while the
        # repository is removed.
        self.git("config", "maintenance.auto", "false")
        self.git("config", "gc.auto", "0")

    def git(self, *args):
        subprocess.run(["git", *args], cwd=self.repo, check=True, capture_output=True)

    def write(self, sizes, add=True):
        """Writes files of the given sizes in bytes, and adds them to the index."""
        for name, size in sizes.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"x" * size)
        if add:
            self.git("add", "-f", "--", *sizes)

    def check(self, cwd=None):
        # Git looks for a repository no higher than the throwaway one's parent, and speaks
        # English (LANGUAGE is ignored in the C locale).
        env = {**os.environ, "GIT_CEILING_DIRECTORIES": str(self.repo.parent), "LC_ALL": "C"}
        result = subprocess.run(
            [sys.executable, "-B", str(SCRIPT)], cwd=cwd or self.repo, capture_output=True,
            text=True, env=env,
        )
        return result.returncode, result.stdout, result.stderr

    def test_a_small_corpus_passes_and_its_sizes_are_printed(self):
        self.write({"tests/corpus/a.docx": 250_000, "tests/corpus/reference/a/1.png": 500_000})
        code, out, err = self.check()
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(
            out,
            "tests/corpus: 0.75 MB in 2 files, of 100 MB; the largest file 0.50 MB, of 1 MB\n",
        )

    def test_a_file_may_take_1_mb_and_no_byte_more(self):
        self.write({"tests/corpus/full.docx": MB})
        self.assertEqual(self.check()[0], 0)
        # Listed by path, not by size either way.
        self.write({
            "tests/corpus/b/over.png": MB + 1,
            "tests/corpus/a/over.json": MB + 2,
            "tests/corpus/c/over.xml": MB + 3,
        })
        code, out, err = self.check()
        self.assertEqual((code, out), (1, ""))
        self.assertEqual(
            err,
            "check-corpus-size: these files of tests/corpus are larger than its limit of "
            "1 MB (1,000,000 bytes) a file:\n"
            "  tests/corpus/a/over.json: 1,000,002 bytes\n"
            "  tests/corpus/b/over.png: 1,000,001 bytes\n"
            "  tests/corpus/c/over.xml: 1,000,003 bytes\n",
        )

    def test_the_corpus_may_take_100_mb_and_no_byte_more(self):
        self.write({f"tests/corpus/{i:03}.png": MB for i in range(100)})
        code, out, err = self.check()
        self.assertEqual((code, err), (0, ""))
        self.assertIn("100.00 MB in 100 files", out)
        self.write({"tests/corpus/one-more.txt": 1})
        code, out, err = self.check()
        self.assertEqual((code, out), (1, ""))
        self.assertEqual(
            err,
            "check-corpus-size: tests/corpus holds 100,000,001 bytes (100.00 MB) in its "
            "files, more than its limit of 100 MB (100,000,000 bytes)\n",
        )

    def test_a_file_over_1_mb_counts_in_the_total_too(self):
        self.write({f"tests/corpus/{i:02}.png": MB for i in range(99)})
        self.write({"tests/corpus/large.docx": 2 * MB})
        code, _, err = self.check()
        self.assertEqual(code, 1)
        self.assertIn("  tests/corpus/large.docx: 2,000,000 bytes\n", err)
        self.assertIn("tests/corpus holds 101,000,000 bytes (101.00 MB) in its files", err)

    def test_only_the_files_git_tracks_in_tests_corpus_count(self):
        self.write({"tests/corpus/a.docx": 100})
        # Untracked files, and tracked ones outside tests/corpus, even in a folder whose
        # name starts the same.
        self.write({"tests/corpus/untracked.png": 2 * MB}, add=False)
        self.write({"tests/corpus-tools/big.bin": 2 * MB, "corpus-ext/big.docx": 2 * MB})
        code, out, err = self.check()
        self.assertEqual((code, err), (0, ""))
        self.assertIn("0.00 MB in 1 files", out)

    def test_a_tracked_file_deleted_from_the_working_tree_counts_as_gone(self):
        self.write({"tests/corpus/kept.docx": 10, "tests/corpus/gone.png": 2 * MB})
        (self.repo / "tests/corpus/gone.png").unlink()
        code, out, err = self.check()
        self.assertEqual((code, err), (0, ""))
        self.assertIn("in 1 files", out)

    def test_a_tracked_file_whose_folder_became_a_file_counts_as_gone(self):
        self.write({"tests/corpus/kept.docx": 10, "tests/corpus/d/gone.png": 2 * MB})
        shutil.rmtree(self.repo / "tests/corpus/d")
        (self.repo / "tests/corpus/d").write_bytes(b"x")
        code, out, err = self.check()
        self.assertEqual((code, err), (0, ""))
        self.assertIn("in 1 files", out)

    def test_a_submodule_is_not_counted(self):
        self.write({"tests/corpus/kept.docx": 10})
        (self.repo / "tests/corpus/sub").mkdir()
        (self.repo / "tests/corpus/sub/large.bin").write_bytes(b"x" * (2 * MB))
        # Any object id will do; the empty tree's has the length of the repository's hash.
        tree = subprocess.run(
            ["git", "hash-object", "-t", "tree", "--stdin"], cwd=self.repo, input=b"",
            check=True, capture_output=True,
        ).stdout.decode().strip()
        self.git("update-index", "--add", "--cacheinfo", f"160000,{tree},tests/corpus/sub")
        code, out, err = self.check()
        self.assertEqual((code, err), (0, ""))
        self.assertIn("in 1 files", out)

    def test_the_sizes_are_those_of_the_working_tree(self):
        self.write({"tests/corpus/a.docx": 10})
        (self.repo / "tests/corpus/a.docx").write_bytes(b"x" * (MB + 1))
        self.assertEqual(self.check()[0], 1)

    @unittest.skipIf(os.name == "nt", "symbolic links need extra rights on Windows")
    def test_a_symbolic_link_counts_with_its_own_size(self):
        self.write({"outside/large.bin": 3 * MB}, add=False)
        link = self.repo / "tests/corpus/link.bin"
        link.parent.mkdir(parents=True)
        link.symlink_to("../../outside/large.bin")
        self.git("add", "--", "tests/corpus/link.bin")
        code, out, err = self.check()
        self.assertEqual((code, err), (0, ""))
        self.assertIn("0.00 MB in 1 files", out)
        self.assertIn("the largest file 0.00 MB", out)
        # The link holds 23 bytes, which take files of 100 MB less 22 bytes one byte past
        # the limit.
        self.write({f"tests/corpus/{i:02}.png": MB for i in range(99)})
        self.write({"tests/corpus/last.png": MB - 22})
        code, _, err = self.check()
        self.assertEqual(code, 1)
        self.assertIn("tests/corpus holds 100,000,001 bytes", err)

    def test_outside_a_repository_it_says_what_git_said(self):
        # A folder of its own next to the repository, below the ceiling, so that git finds
        # no repository even when the temporary folder is inside one.
        outside = Path(tempfile.mkdtemp(prefix="check-corpus-size-outside-"))
        self.addCleanup(shutil.rmtree, outside)
        self.assertEqual(outside.parent, self.repo.parent)
        code, out, err = self.check(cwd=outside)
        self.assertEqual((code, out), (1, ""))
        self.assertTrue(err.startswith("check-corpus-size: git rev-parse failed: "), err)
        self.assertIn("not a git repository", err)

    def test_it_runs_from_any_folder_of_the_repository(self):
        self.write({"tests/corpus/big.docx": MB + 1})
        code, _, err = self.check(cwd=self.repo / "tests")
        self.assertEqual(code, 1)
        self.assertIn("  tests/corpus/big.docx: 1,000,001 bytes\n", err)


if __name__ == "__main__":
    unittest.main()
