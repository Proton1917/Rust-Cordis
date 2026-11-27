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
    def test_source_bridge_requires_named_rejected_controls(self):
        valid = {"status": "passed", "compiled_input_cases": 12,
                 "negative_controls": {"mutated_guard": "rejected",
                                       "source_certificate": {"status": "rejected", "disagreements": 3}}}
        verify.validate_source_bridge("source", valid)
        for changes in [dict(status="running"), dict(compiled_input_cases=True),
                        dict(compiled_input_cases=0), dict(negative_controls=4),
                        dict(negative_controls={}),
                        dict(negative_controls={"mutated_guard": "passed"}),
                        dict(negative_controls={"mutated_guard": {"status": "unknown"}})]:
            with self.subTest(changes=changes), self.assertRaises(RuntimeError):
                verify.validate_source_bridge("source", dict(valid, **changes))

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

    def test_new_lean_module_cannot_escape_closed_inventory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Proof.lean").write_text("import Std\nnamespace Cordis.Proof\ntheorem valid : True := by trivial\nend Cordis.Proof\n")
            (root / "Audit.lean").write_text("import Lean\n")
            (root / "lakefile.toml").write_text(
                'defaultTargets = ["Proof", "Audit"]\n'
                '[[lean_lib]]\nname = "Proof"\n[[lean_lib]]\nname = "Audit"\n')
            (root / "lake-manifest.json").write_text('{"packages": []}')
            with patch.object(verify, "FORMAL", root), \
                    patch.object(verify, "PROOFS", {"Proof.lean": "Cordis.Proof"}), \
                    patch.object(verify, "PROGRAMS", {"Audit.lean"}):
                verify.check_module_inventory()
                (root / "Unregistered.lean").write_text("import Std\n")
                with self.assertRaisesRegex(RuntimeError, "unregistered.*Unregistered"):
                    verify.check_module_inventory()

    def test_private_helper_axiom_is_audited_even_when_not_in_public_theorem_list(self):
        declarations = [
            {"name": "Cordis.Proof.valid", "module": "Proof", "theorem": True, "compiler_auxiliary": False, "partial_definition": False, "axioms": []},
            {"name": "_private.Proof.0.hidden", "module": "Proof", "theorem": False,
             "compiler_auxiliary": False, "partial_definition": False, "axioms": ["sorryAx"]},
        ]
        with patch.object(verify, "PROOFS", {"Proof.lean": "Cordis.Proof"}):
            with self.assertRaisesRegex(RuntimeError, "unapproved axiom dependency.*hidden"):
                verify.parse_declaration_audit("CORDIS_AUDIT\t" + json.dumps(declarations), ["Cordis.Proof.valid"])

    def test_kernel_census_must_include_every_proof_module_and_public_theorem(self):
        declaration = {"name": "Cordis.Proof.valid", "module": "Proof", "theorem": True, "compiler_auxiliary": False, "partial_definition": False, "axioms": []}
        output = "CORDIS_AUDIT\t" + json.dumps([declaration])
        with patch.object(verify, "PROOFS", {"Proof.lean": "Cordis.Proof", "Missing.lean": "Cordis.Missing"}):
            with self.assertRaisesRegex(RuntimeError, "module mismatch"):
                verify.parse_declaration_audit(output, ["Cordis.Proof.valid"])
        with patch.object(verify, "PROOFS", {"Proof.lean": "Cordis.Proof"}):
            with self.assertRaisesRegex(RuntimeError, "omitted source theorems"):
                verify.parse_declaration_audit(output, ["Cordis.Proof.unchecked"])

    def test_kernel_census_rejects_duplicate_or_missing_receipts(self):
        with self.assertRaisesRegex(RuntimeError, "exactly one census"):
            verify.parse_declaration_audit("", [])
        row = {"name": "Cordis.Proof.valid", "module": "Proof", "theorem": True, "compiler_auxiliary": False, "partial_definition": False, "axioms": []}
        with self.assertRaisesRegex(RuntimeError, "duplicate kernel declaration"):
            verify.parse_declaration_audit("CORDIS_AUDIT\t" + json.dumps([row, row]), [])

    def test_compiler_erasure_is_never_admitted_as_a_logical_axiom(self):
        proof = {"name": "Cordis.Proof.valid", "module": "Proof", "theorem": True,
                 "compiler_auxiliary": False, "partial_definition": False, "axioms": []}
        generated = {"name": "_private.Proof.compiler_stage", "module": "Proof", "theorem": False,
                     "compiler_auxiliary": True, "partial_definition": False, "axioms": ["lcProof"]}
        with patch.object(verify, "PROOFS", {"Proof.lean": "Cordis.Proof"}):
            verify.parse_declaration_audit("CORDIS_AUDIT\t" + json.dumps([proof, generated]), [proof["name"]])
            for malformed in [dict(proof, axioms=["lcProof"]),
                              dict(generated, compiler_auxiliary=False),
                              dict(proof, axioms=["untrusted"]),
                              dict(generated, theorem=True)]:
                with self.assertRaises(RuntimeError):
                    verify.parse_declaration_audit("CORDIS_AUDIT\t" + json.dumps([malformed]), [])


if __name__ == "__main__":
    unittest.main()
