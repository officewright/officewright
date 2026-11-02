"""Tests of the start of scripts/check.sh: what it makes of CORPUS_JOBS.

The script runs in a copy of itself in a throwaway tree, with a fake rustc and a cargo that
always fails, so it stops at its list of missing tools: nothing is built or run.
"""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "check.sh"
BASH = shutil.which("bash")

FAKE_RUSTC = "#!/bin/sh\nprintf 'host: x86_64-unknown-linux-gnu\\n'\n"
FAKE_CARGO = "#!/bin/sh\nexit 1\n"
REFUSED = "CORPUS_JOBS must be a whole number from 1 to 9999"


class CorpusJobs(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(BASH, "bash is needed to run scripts/check.sh")
        self.tree = Path(tempfile.mkdtemp(prefix="check-"))
        self.addCleanup(shutil.rmtree, self.tree)
        for name, text in [
            ("scripts/check.sh", SCRIPT.read_text(encoding="utf-8")),
            ("bin/rustc", FAKE_RUSTC),
            ("bin/cargo", FAKE_CARGO),
        ]:
            path = self.tree / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(text.encode("utf-8"))
            path.chmod(0o755)

    def run_script(self, jobs):
        env = {**os.environ, "PATH": str(self.tree / "bin") + os.pathsep + os.environ["PATH"]}
        env.pop("CORPUS_JOBS", None)
        if jobs is not None:
            env["CORPUS_JOBS"] = jobs
        result = subprocess.run(
            [BASH, "scripts/check.sh"], cwd=self.tree, env=env,
            capture_output=True, encoding="utf-8",
        )
        return result.returncode, result.stdout, result.stderr

    def test_refuses_what_is_not_a_number_of_documents(self):
        # Zero, signs, spaces, leading zeros, more than four digits, digits of other
        # scripts and numbers past the runner's integers.
        for jobs in ["abc", "0", "-1", "+2", " 2", "2 ", "02", "1.5", "10000", "²", "٢",
                     "99999999999999999999999", "2;id"]:
            with self.subTest(jobs=jobs):
                status, out, err = self.run_script(jobs)
                self.assertEqual(status, 1)
                self.assertIn(f"{REFUSED}, not '{jobs}'", err)
                # It stops before the list of missing tools.
                self.assertNotIn("missing tools", err)
                self.assertNotIn("lays out", out)

    def test_takes_a_number_and_says_so(self):
        for jobs in ["1", "2", "16", "9999"]:
            with self.subTest(jobs=jobs):
                status, out, err = self.run_script(jobs)
                self.assertIn(
                    f"check.sh: the corpus runner lays out {jobs} documents at a time", out
                )
                self.assertNotIn(REFUSED, err)
                # It goes on to the tools, and stops there for the fake cargo.
                self.assertEqual(status, 1)
                self.assertIn("missing tools", err)

    def test_unset_or_empty_changes_nothing(self):
        for jobs in [None, ""]:
            with self.subTest(jobs=jobs):
                status, out, err = self.run_script(jobs)
                self.assertNotIn("lays out", out)
                self.assertNotIn(REFUSED, err)
                self.assertEqual(status, 1)
                self.assertIn("missing tools", err)


if __name__ == "__main__":
    unittest.main()
