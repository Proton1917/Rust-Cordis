import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "verify_semantics.py"
SPEC = importlib.util.spec_from_file_location("verify_semantics", SCRIPT)
verify = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verify)


class SemanticEvidenceFailures(unittest.TestCase):
    def test_failed_attempt_replaces_previous_passed_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory) / "evidence.json"
            evidence.write_text(json.dumps({"status": "passed", "theorems": ["stale"]}))
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--lake", str(Path(directory) / "missing-lake"),
                 "--evidence", str(evidence)], capture_output=True, text=True, timeout=10,
            )
            self.assertNotEqual(result.returncode, 0)
            observed = json.loads(evidence.read_text())
            self.assertEqual(observed["status"], "failed")
            self.assertEqual(observed["current_stage"], "toolchain")
            self.assertIn("must be installed", observed["error"])
            self.assertNotIn("theorems", observed)

    def test_source_mutation_invalidates_bound_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "Lifecycle.lean"
            source.write_text("first decision")
            with patch.object(verify, "ROOT", root):
                before = verify.snapshot([source])
                source.write_text("different decision")
                after = verify.snapshot([source])
            with self.assertRaisesRegex(RuntimeError, "inputs changed"):
                verify.require_same_source(before, after)

    def test_incomplete_or_repeated_compiled_input_domain_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            verify.parse_vectors("idle\t0\tDisposed\n")
        with self.assertRaisesRegex(RuntimeError, "repeated"):
            verify.parse_vectors("idle\t0\tDisposed\nidle\t0\tDisposed\n")


if __name__ == "__main__":
    unittest.main()
