"""Fail-closed regressions for conformance evidence, independent of Cargo success."""

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import conformance
import conformance_run as runner


class ConformanceEvidenceTests(unittest.TestCase):
    def test_duplicate_matrix_id_is_rejected(self):
        row = "| DSP-01 | source:1 | title | M3 | 已改写 | existing.rs::test |\n"
        with tempfile.TemporaryDirectory() as directory:
            matrix = Path(directory) / "matrix.md"
            matrix.write_text(row + row, encoding="utf-8")
            with patch.object(conformance, "MATRIX", matrix):
                with self.assertRaisesRegex(SystemExit, "ID 重复"):
                    conformance.parse_matrix()

    def test_successful_build_without_target_is_rejected(self):
        with self.assertRaisesRegex(runner.VerificationError, "no test executable"):
            runner.compiled_targets('{"reason":"build-finished","success":true}',
                                    ["crates/cordis/tests/dispose.rs"])

    def test_artifact_must_belong_to_selected_source(self):
        item = {"reason": "compiler-artifact", "executable": "/tmp/other-test",
                "target": {"src_path": str(runner.ROOT / "crates/cordis/tests/events.rs"),
                           "kind": ["test"]}}
        with self.assertRaises(runner.VerificationError):
            runner.compiled_targets(json.dumps(item), ["crates/cordis/tests/dispose.rs"])

    def test_zero_selected_and_ignored_are_rejected(self):
        for output in ("test result: ok. 0 passed; 0 failed; 1 filtered out",
                       "test required ... ignored\ntest result: ok. 0 passed; 0 failed; 1 ignored"):
            with self.subTest(output=output):
                with self.assertRaises(runner.VerificationError):
                    runner.require_passed("required", runner.passed_tests(output), "DSP-01")

    def test_inventory_and_result_names_are_exact(self):
        inventory = runner.test_names("required_extra: test\nother: benchmark\n")
        self.assertNotIn("required", inventory)
        outcomes = runner.passed_tests("test required_extra ... ok\n")
        with self.assertRaises(runner.VerificationError):
            runner.require_passed("required", outcomes, "DSP-01")
        runner.require_passed("required_extra", outcomes, "DSP-01")

    def test_doctest_requires_the_compile_fail_item(self):
        ref = {"path": "crates/cordis/src/effect.rs", "name": "EffectScope::defer"}
        valid = "crates/cordis/src/effect.rs - effect::EffectScope::defer (line 122) - compile fail"
        wrong_item = valid.replace("::defer ", "::defer_async ")
        wrong_kind = valid.replace(" - compile fail", "")
        self.assertEqual(runner.matching_doctests(ref, {wrong_item: "ok", wrong_kind: "ok"}), [])
        self.assertEqual(runner.matching_doctests(ref, {valid: "ok"}), [valid])

    def test_duplicate_harness_outcome_is_rejected(self):
        with self.assertRaisesRegex(runner.VerificationError, "duplicate test"):
            runner.passed_tests("test same ... ok\ntest same ... ok\n")

    def test_matrix_cannot_silently_exempt_a_regression(self):
        rows = conformance.parse_matrix()
        rows["DSP-01"][4] = "不适用"
        with self.assertRaisesRegex(runner.VerificationError, "exclusions differ"):
            runner.references(rows)


if __name__ == "__main__":
    unittest.main()
