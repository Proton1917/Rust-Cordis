#!/usr/bin/env python3
"""Check the source predicate certificate and execute the complete graph builder.

The accepted predicate language consists of three bound comparison atoms,
parentheses, &&, and ||. Unknown syntax fails closed. The Lean kernel checks
the source-derived AST against the program with an arbitrary-input theorem.
The whole production select_instances body is additionally compiled without
rewriting over explicitly documented stable snapshot APIs.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import re
import subprocess
import tempfile
from pathlib import Path
from check_kernel_refinement import braced, unique, write_evidence

ROOT = Path(__file__).resolve().parents[1]
FORMAL = ROOT / "formal"
SOURCE = ROOT / "crates/cordis-supervisor/src/manager/transition.rs"


def run(command: list[str]) -> str:
    result = subprocess.run(command, cwd=FORMAL, text=True, capture_output=True, timeout=180)
    if result.returncode:
        raise RuntimeError(f"{' '.join(command)} failed:\n{result.stdout}{result.stderr}")
    return result.stdout


def tokens(text: str) -> list[str]:
    pattern = r"[A-Za-z_][A-Za-z_0-9]*|>=|==|&&|\|\||[.()*&,]"
    remaining = re.sub(pattern, "", text)
    if remaining.strip():
        raise RuntimeError(f"unrecognized predicate syntax: {remaining!r}")
    return re.findall(pattern, text)


def parse_candidate(text: str) -> str:
    """Return a Lean Predicate expression, preserving parsed boolean structure."""
    items, offset = tokens(text), 0
    atoms = [(tokens("provided.minor >= *minor"), ".minorAtLeast"),
             (tokens("*realm == inherited"), ".sameRealm"),
             (tokens("named_realms.get(realm) == Some(&namespace)"), ".namedNamespace")]

    def primary() -> str:
        nonlocal offset
        for atom, value in atoms:
            if items[offset:offset + len(atom)] == atom:
                offset += len(atom)
                return value
        if offset < len(items) and items[offset] == "(":
            offset += 1
            result = disjunction()
            if offset >= len(items) or items[offset] != ")":
                raise RuntimeError("predicate parentheses do not close")
            offset += 1
            return result
        raise RuntimeError(f"unsupported predicate atom at {items[offset:]}")

    def conjunction() -> str:
        nonlocal offset
        result = primary()
        while offset < len(items) and items[offset] == "&&":
            offset += 1
            result = f"(.and {result} {primary()})"
        return result

    def disjunction() -> str:
        nonlocal offset
        result = conjunction()
        while offset < len(items) and items[offset] == "||":
            offset += 1
            result = f"(.or {result} {conjunction()})"
        return result

    result = disjunction()
    if offset != len(items):
        raise RuntimeError(f"trailing predicate tokens: {items[offset:]}")
    return result


def certificate(ast: str) -> str:
    return f"""import BindingGraph
namespace Cordis.BindingSourceCertificate
open BindingGraph
def parsedPredicate : Predicate := {ast}
theorem source_ast_matches : parsedPredicate = candidateProgram := by decide
theorem all_parameters (named : Nat → Option Nat) (p : Provider) (c : Consumer) :
    parsedPredicate.eval named p c = true ↔
      c.minor ≤ p.minor ∧ (c.realm = p.inherited ∨ named c.realm = some p.space) := by
  rw [source_ast_matches]
  exact candidate_program_spec named p c
end Cordis.BindingSourceCertificate
"""


def validate(output: str) -> dict[int, str]:
    result = {}
    for line in output.splitlines():
        key, value = line.split("\t", 1)
        mask = int(key)
        if mask in result or not 0 <= mask < 32768:
            raise RuntimeError(f"invalid/duplicate case {mask}")
        result[mask] = value
    if set(result) != set(range(32768)):
        raise RuntimeError("incomplete graph construction domain")
    return result


def verify(args: argparse.Namespace) -> dict:
    inputs = ["BindingGraph.lean", "BindingGraphVectors.lean", "Deployment.lean",
              "check_binding_graph.py", "binding_graph_probe.rs", "check_kernel_refinement.py",
              "lakefile.toml", "lake-manifest.json", "lean-toolchain"]
    tracked = [SOURCE] + [FORMAL / name for name in inputs]
    before = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
              for path in tracked}
    source = SOURCE.read_text()
    body = braced(source, "pub(super) fn select_instances(")
    closure = braced(body, ".filter(|(realm, minor, _)|")
    predicate = closure[closure.index("{") + 1:-1]
    ast = parse_candidate(predicate)
    template = (FORMAL / "binding_graph_probe.rs").read_text()
    marker = "// @SOURCE:SELECT_INSTANCES@"
    unique(template, marker)
    run([args.lake, "env", "lean", "-o", ".lake/build/lib/lean/BindingGraph.olean", "BindingGraph.lean"])
    expected = validate(run([args.lake, "env", "lean", "--run", "BindingGraphVectors.lean"]))
    controls = {}
    with tempfile.TemporaryDirectory(prefix="cordis-binding-graph-") as directory:
        path = Path(directory)
        cert = path / "Certificate.lean"
        cert.write_text(certificate(ast))
        run([args.lake, "env", "lean", str(cert)])

        def execute(fragment: str, name: str) -> dict[int, str]:
            program, binary = path / f"{name}.rs", path / name
            program.write_text(template.replace(marker, fragment))
            run([args.rustc, "--edition=2024", "-O", str(program), "-o", str(binary)])
            return validate(run([str(binary)]))

        actual = execute(body, "source")
        differences = [mask for mask in expected if expected[mask] != actual[mask]]
        if differences:
            mask = differences[0]
            raise RuntimeError(f"graph construction disagrees for {mask}: Lean={expected[mask]!r}, Rust={actual[mask]!r}; {len(differences)} disagreements")
        parent = braced(body, "if let Some(config) = fiber.config::<Configuration>()")
        mutants = {
            "minor_direction": body.replace("provided.minor >= *minor", "provided.minor <= *minor"),
            "named_realm_conjunction": body.replace("|| named_realms.get(realm)", "&& named_realms.get(realm)"),
            "replaced_provider_retained": body.replace("if !replaced.contains(name)", "if replaced.contains(name)"),
            "committed_minor_ignored": body.replace("binding.interface.minor >= interface.minor", "true"),
            "parent_edge_reversed": body.replace(parent, parent.replace(".entry(config.instance.name.clone())", ".entry(name.clone())").replace(".push(name.clone())", ".push(config.instance.name.clone())")),
        }
        root_block = braced(body, "if let Some(provider) = &binding.generation")
        mutants["bound_root_falls_through"] = body.replace(root_block, root_block + " else { waiting.entry(interface.key.clone()).or_default().push((view.realm_named(&interface.key),interface.minor,name.clone())); }")
        for name, mutant in mutants.items():
            if mutant == body:
                raise RuntimeError(f"negative control {name} did not mutate source")
            observed = execute(mutant, name)
            rejected = sum(expected[mask] != observed[mask] for mask in expected)
            if rejected == 0:
                raise RuntimeError(f"negative control {name} passed")
            controls[name] = {"status": "rejected", "disagreements": rejected}
        cert.write_text(certificate(parse_candidate(predicate.replace("||", "&&"))))
        try:
            run([args.lake, "env", "lean", str(cert)])
        except RuntimeError:
            controls["source_ast_operator_mutation"] = {"status": "rejected"}
        else:
            raise RuntimeError("source-derived AST mutation passed kernel certificate")
        for text in ["unsafe { true }", predicate + " || arbitrary()"]:
            try:
                parse_candidate(text)
            except RuntimeError:
                continue
            raise RuntimeError("predicate parser accepted unsupported syntax")
        controls["unsupported_source_syntax"] = {"status": "rejected"}
    after = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
             for path in tracked}
    if before != after:
        raise RuntimeError("binding graph proof/source inputs changed during verification")
    evidence = {
        "status": "passed", "compiled_input_cases": len(expected),
        "source_path": str(SOURCE.relative_to(ROOT)),
        "source_sha256": hashlib.sha256(source.encode()).hexdigest(),
        "extracted_function": "Manager::select_instances",
        "extracted_body_sha256": hashlib.sha256(body.encode()).hexdigest(),
        "predicate_ast": ast,
        "source_certificate": "Lean checked parsed AST equality plus candidate_program_spec for arbitrary inputs",
        "proof_inputs_sha256": {"formal/" + name: before["formal/" + name] for name in inputs},
        "negative_controls": controls,
        "support_contracts": [
            "locks expose an immutable finite catalog snapshot; Arc clone and Weak upgrade preserve referenced identity",
            "context lookup returns the constructed live or generation-bound dynamic value; real Arc Any downcast is executed",
            "interface parsing supplies key and u32 minor from valid declarations; parser grammar is covered separately",
            "realm resolution and namespace lookup return constructed scope facts; their production implementations are separately tested",
            "parent config lookup returns the nearest configured ancestor through a finite parent chain"
        ],
        "scope": "whole source graph construction is differential evidence; predicate AST certificate is unbounded in its restricted comparison language; Rust/stdlib/compiler and extraction/parser implementation are trusted",
    }
    return evidence


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--lake", default="lake")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()
    write_evidence(args.evidence, {"status": "running"})
    try:
        evidence = verify(args)
    except BaseException as error:
        write_evidence(args.evidence, {"status": "failed", "error": str(error)})
        raise
    write_evidence(args.evidence, evidence)
    print(json.dumps(evidence, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
