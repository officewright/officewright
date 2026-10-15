"""Tests of scripts/nightly.sh's parts, with a fake cargo and stub helper scripts.

The script runs in a copy of itself in a throwaway tree, so nothing is fuzzed, built or
downloaded: each fake command writes its arguments to a log and exits as the test asks.
"""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "nightly.sh"
BASH = shutil.which("bash")

# Answers what nightly.sh asks cargo; FAKE_* variables choose the exit statuses.
FAKE_CARGO = r"""#!/bin/sh
printf 'cargo %s\n' "$*" >> "$FAKE_LOG"
case "$*" in
"+nightly fuzz --version") [ -z "$FAKE_NO_FUZZ" ] || exit 101 ;;
"+nightly fuzz list") printf '%b' "$FAKE_TARGETS"; exit "${FAKE_LIST_STATUS:-0}" ;;
"+nightly fuzz run "*)
	set -- $*
	case " $FAKE_CRASH " in *" $4 "*) exit 1 ;; esac
	;;
"+nightly miri test -p officewright-ffi") exit "${FAKE_MIRI_STATUS:-0}" ;;
"build --release -p corpus-runner") exit 0 ;;
*) echo "fake cargo: unexpected arguments: $*" >&2; exit 99 ;;
esac
"""
# A stub that logs its name and arguments and exits with the status in variable $2.
STUB = r"""#!/bin/sh
printf '%%s %%s\n' %s "$*" >> "$FAKE_LOG"
exit "${%s:-0}"
"""
LIMITS = "-max_total_time=1800 -timeout=30 -malloc_limit_mb=1024 -rss_limit_mb=2560"
USAGE = "usage: scripts/nightly.sh"


class Nightly(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(BASH, "bash is needed to run scripts/nightly.sh")
        self.tree = Path(tempfile.mkdtemp(prefix="nightly-"))
        self.addCleanup(shutil.rmtree, self.tree)
        self.log = self.tree / "log.txt"
        self.write("scripts/nightly.sh", SCRIPT.read_text(encoding="utf-8"))
        self.write("bin/cargo", FAKE_CARGO)
        self.write("scripts/fetch-corpus.sh", STUB % ("fetch-corpus", "FAKE_FETCH_STATUS"))
        self.write("scripts/check-test-fonts.sh", STUB % ("test-fonts", "FAKE_FONTS_STATUS"))
        self.write(
            "target/release/corpus-runner", STUB % ("corpus-runner", "FAKE_RUNNER_STATUS")
        )
        # The committed POI documents and the fetched ones they must equal.
        self.write("tests/corpus/poi/a.docx", "same")
        self.write("corpus-ext/poi/test-data/document/a.docx", "same")

    def write(self, name, text):
        path = self.tree / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode("utf-8"))
        path.chmod(0o755)

    def run_script(self, *args, **fake):
        env = {
            **os.environ,
            "PATH": str(self.tree / "bin") + os.pathsep + os.environ["PATH"],
            "FAKE_LOG": str(self.log),
            "FAKE_TARGETS": "xml\\npng_decode\\n",
        }
        env.update({f"FAKE_{key.upper()}": value for key, value in fake.items()})
        result = subprocess.run(
            [BASH, "scripts/nightly.sh", *args], cwd=self.tree, env=env,
            capture_output=True, encoding="utf-8",
        )
        return result.returncode, result.stdout, result.stderr

    def ran(self):
        """The commands the script ran, in order."""
        if not self.log.exists():
            return []
        return self.log.read_text(encoding="utf-8").splitlines()

    def fuzzed(self):
        runs = [line for line in self.ran() if line.startswith("cargo +nightly fuzz run ")]
        return [line.split()[4] for line in runs]

    def test_fuzz_targets_prints_one_line_of_json(self):
        code, out, err = self.run_script("fuzz-targets", targets="docx_open\\nshape-font\\n")
        self.assertEqual((code, out, err), (0, '["docx_open","shape-font"]\n', ""))

    def test_fuzz_targets_fails_when_the_list_does(self):
        # Names printed before the failure must not reach the matrix.
        code, out, _ = self.run_script(
            "fuzz-targets", targets="docx_open\\n", list_status="3"
        )
        self.assertEqual((code, out), (1, ""))

    def test_fuzz_targets_refuses_an_empty_list(self):
        code, out, err = self.run_script("fuzz-targets", targets="")
        self.assertEqual((code, out), (1, ""))
        self.assertIn("cargo fuzz list found no targets", err)

    def test_fuzz_targets_refuses_names_that_json_would_escape(self):
        for name in ['a"b', "a\\\\b", "a.b", "naïve"]:
            code, out, err = self.run_script("fuzz-targets", targets=f"xml\\n{name}\\n")
            self.assertEqual((code, out), (1, ""), name)
            self.assertIn("is not letters, digits, - and _", err, name)

    def test_fuzz_runs_each_target_with_its_seeds_and_limits(self):
        self.assertEqual(
            self.run_script("fuzz", "png_decode", "docx_open"),
            (0, "==> fuzzing (30 min per target)\n", ""),
        )
        runs = [line for line in self.ran() if " fuzz run " in line]
        self.assertEqual(runs, [
            "cargo +nightly fuzz run png_decode fuzz/corpus/png_decode fuzz/seeds/png "
            f"tests/corpus/reference -- {LIMITS}",
            "cargo +nightly fuzz run docx_open fuzz/corpus/docx_open tests/corpus/word-rules "
            "tests/corpus/resaved tests/corpus/poi crates/officewright-doc/tests/data "
            f"-- {LIMITS}",
        ])
        self.assertTrue((self.tree / "fuzz/corpus/docx_open").is_dir())

    def test_fuzz_without_targets_runs_every_listed_one(self):
        self.assertEqual(self.run_script("fuzz")[0], 0)
        self.assertEqual(self.fuzzed(), ["xml", "png_decode"])

    def test_a_crash_fails_the_run_but_not_the_other_targets(self):
        # nightly.yml uploads a job's crash inputs only when this exits non-zero.
        code, _, err = self.run_script(
            "fuzz", "xml", "png_decode", "jpeg_header", crash="xml jpeg_header"
        )
        self.assertEqual(code, 1)
        self.assertEqual(self.fuzzed(), ["xml", "png_decode", "jpeg_header"])
        self.assertIn("fuzzing failed in: xml jpeg_header", err)

    def test_a_target_without_seeds_stops_the_run(self):
        code, _, err = self.run_script("fuzz", "new_target", "xml")
        self.assertEqual(code, 1)
        self.assertIn("no seed directories for fuzz target new_target", err)
        self.assertEqual(self.fuzzed(), [])

    def test_wrong_arguments_print_the_usage(self):
        for args in [("bogus",), ("checks", "x"), ("fuzz-targets", "x"), ("all", "x")]:
            code, _, err = self.run_script(*args)
            self.assertEqual(code, 2, args)
            self.assertIn(USAGE, err, args)
            self.assertEqual(self.ran(), [], args)

    def test_a_missing_cargo_fuzz_stops_the_run_before_anything_else(self):
        for args in [(), ("all",), ("fuzz", "xml"), ("fuzz-targets",)]:
            self.log.unlink(missing_ok=True)
            code, out, err = self.run_script(*args, no_fuzz="1")
            self.assertEqual((code, out), (1, ""), args)
            self.assertIn("cargo fuzz is missing", err, args)
            self.assertEqual(self.ran(), ["cargo +nightly fuzz --version"], args)

    def test_checks_run_every_check_and_nothing_else(self):
        self.assertEqual(self.run_script("checks")[0], 0)
        self.assertEqual(self.ran(), [
            "cargo +nightly miri test -p officewright-ffi",
            "fetch-corpus --check all",
            "fetch-corpus poi libreoffice",
            "cargo build --release -p corpus-runner",
            "corpus-runner run --enforce-memory-limit "
            "--expected tools/corpus-runner/expected-errors.tsv "
            "--baseline tools/corpus-runner/baseline-ext.json --allow-better "
            "--report target/corpus-report.json corpus-ext/poi corpus-ext/libreoffice",
            "test-fonts ",
        ])

    def test_miri_and_the_pins_stop_the_checks_at_once(self):
        cases = [
            ({"miri_status": "1"}, "cargo +nightly miri"),
            ({"fetch_status": "1"}, "fetch-corpus --check"),
        ]
        for fake, last in cases:
            for args in [("checks",), ()]:
                self.log.unlink(missing_ok=True)
                code, _, _ = self.run_script(*args, **fake)
                self.assertEqual(code, 1, (fake, args))
                self.assertTrue(self.ran()[-1].startswith(last), (fake, args, self.ran()))

    def test_corpus_and_font_failures_fail_the_run_at_the_end(self):
        cases = [
            ({"runner_status": "1"}, "the corpus checks failed"),
            ({"fonts_status": "1"}, "the test fonts differ from their upstream files"),
        ]
        for fake, message in cases:
            self.log.unlink(missing_ok=True)
            code, _, err = self.run_script("checks", **fake)
            self.assertEqual(code, 1, fake)
            self.assertIn(message, err, fake)
            self.assertEqual(self.ran()[-1], "test-fonts ", fake)

    def test_a_poi_document_that_differs_fails_the_corpus_checks(self):
        self.write("corpus-ext/poi/test-data/document/a.docx", "changed")
        code, _, err = self.run_script("checks")
        self.assertEqual(code, 1)
        self.assertIn("tests/corpus/poi/a.docx is not POI's file of that name", err)
        self.assertIn("the corpus checks failed", err)

    def test_without_arguments_it_runs_the_checks_then_fuzzes_every_target(self):
        code, _, err = self.run_script(runner_status="1", crash="png_decode")
        self.assertEqual(code, 1)
        self.assertEqual(self.ran()[1], "cargo +nightly miri test -p officewright-ffi")
        self.assertEqual(self.fuzzed(), ["xml", "png_decode"])
        self.assertIn("fuzzing failed in: png_decode", err)
        self.assertIn("the corpus checks failed", err)
        self.assertEqual(self.run_script("all")[0], 0)


if __name__ == "__main__":
    unittest.main()
