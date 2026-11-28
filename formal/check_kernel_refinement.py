#!/usr/bin/env python3
"""Compile exact production fragments and compare them with Lean observations.

The extraction fails on ambiguous or missing anchors. The generated Rust file is
temporary. Function bodies are copied byte-for-byte; the supporting arena,
locking, task, callback, and identity representations are explicitly recorded.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FORMAL = ROOT / "formal"
EXPECTED_CASES = {"refresh": 24576, "finish": 12288, "register": 7680, "epoch": 5362,
                  "commit": 484, "locate": 1024, "snapshot": 512, "counter": 576, "release": 7744,
                  "get_value": 4096, "typed_handle": 48, "identity": 12, "notification": 12,
                  "metadata": 16384}


def run(command: list[str], *, input_text: str | None = None) -> str:
    result = subprocess.run(command, cwd=FORMAL, input=input_text, text=True,
                            capture_output=True, timeout=180)
    if result.returncode:
        raise RuntimeError(f"command failed: {' '.join(command)}\n{result.stdout}{result.stderr}")
    return result.stdout


def unique(text: str, anchor: str) -> int:
    count = text.count(anchor)
    if count != 1:
        raise RuntimeError(f"source anchor must occur exactly once, found {count}: {anchor!r}")
    return text.index(anchor)


def braced(text: str, anchor: str) -> str:
    """Extract one Rust item/block, respecting comments and ordinary strings.

    Selected source blocks contain no raw or byte strings. Rejecting those
    constructs keeps the accepted extraction language explicit.
    """
    start = unique(text, anchor)
    opening = text.index("{", start + len(anchor))
    depth, comments, index = 0, 0, opening
    quoted = False
    while index < len(text):
        if quoted:
            if text[index] == "\\":
                index += 2
                continue
            if text[index] == '"':
                quoted = False
            index += 1
            continue
        if comments:
            if text.startswith("/*", index):
                comments += 1
                index += 2
            elif text.startswith("*/", index):
                comments -= 1
                index += 2
            else:
                index += 1
            continue
        if text.startswith("//", index):
            newline = text.find("\n", index)
            index = len(text) if newline < 0 else newline + 1
            continue
        if text.startswith("/*", index):
            comments = 1
            index += 2
            continue
        if text.startswith(('r"', 'r#', 'br"', 'br#'), index):
            raise RuntimeError("raw strings require extending the reviewed extractor")
        char = text[index]
        if char == '"':
            quoted = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return text[start:index + 1]
        index += 1
    raise RuntimeError(f"unterminated source block: {anchor!r}")


def sha256(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def extract_source() -> tuple[str, list[dict[str, object]]]:
    sources = {name: (ROOT / f"crates/cordis/src/{name}.rs").read_text()
               for name in ["fiber", "context", "effect", "reflect", "service"]}
    fragments: list[dict[str, object]] = []

    def item(file: str, anchor: str, entrypoint: str) -> str:
        code = braced(sources[file], anchor)
        fragments.append({"entrypoint": entrypoint, "path": f"crates/cordis/src/{file}.rs",
                          "anchor": anchor, "sha256": sha256(code), "bytes": len(code.encode())})
        return code

    epoch = item("fiber", "#[derive(Clone, Debug, PartialEq, Eq)]\npub enum Epoch", "Epoch")
    fiber_methods = "\n".join([
        item("fiber", "fn idle_state(&self)", "FiberData::idle_state"),
        item("fiber", "pub(crate) fn is_current(&self)", "FiberData::is_current"),
    ])
    context_methods = "\n".join([
        item("context", "pub(crate) fn derive(&self", "Context::derive"),
        item("context", "pub(crate) fn permits_effect(&self", "Context::permits_effect"),
        item("context", "pub(crate) fn resolve_realm(&self", "Context::resolve_realm"),
        item("context", "pub(crate) fn merge_meta<K:", "Context::merge_meta"),
        item("reflect", "fn get_value(&self", "Context::get_value"),
        item("effect", "fn create_effect(&self", "Context::create_effect"),
    ])
    reflect_functions = "\n".join([
        item("reflect", "pub(crate) fn epoch_ids(", "reflect::epoch_ids"),
        item("reflect", "pub(crate) fn commit_locked(", "reflect::commit_locked"),
        item("reflect", "pub(crate) fn locate(", "reflect::locate"),
        item("reflect", "fn scope_realm(", "reflect::scope_realm"),
        item("reflect", "pub(crate) fn wait_dependents(", "reflect::wait_dependents"),
        item("reflect", "pub(crate) fn release_committed(", "reflect::release_committed"),
        item("reflect", "pub(crate) fn injected_value(", "reflect::injected_value"),
        item("reflect", "pub(crate) fn injected_meta<K:", "reflect::injected_meta"),
    ])
    candidate = item("reflect", "fn current_candidate(", "reflect::current_candidate")
    from_binding = item("service", "pub(crate) fn from_binding(", "Scoped::from_binding")
    scoped_meta = item("service", "pub fn meta(&self)", "Scoped::meta")
    provision = braced(sources["reflect"], "fn provide_value(")
    start = unique(provision, "let uid = state\n                    .impl_counter")
    end = unique(provision, "let id = state.impls.insert(ImplData {")
    reservation = provision[start:end]
    fragments.append({"entrypoint": "provide_value::checked_identity_reservation",
                      "path": "crates/cordis/src/reflect.rs", "sha256": sha256(reservation),
                      "bytes": len(reservation.encode()), "scope": "checked allocation before registry mutations"})
    notification = braced(braced(sources["reflect"], "fn notify("),
                          "if let Some(id) = state.store.get(&(key.clone(), realm)).copied()")
    fragments.append({"entrypoint": "notify::revision_advance", "path": "crates/cordis/src/reflect.rs",
                      "sha256": sha256(notification), "bytes": len(notification.encode())})

    def core(function_anchor: str, start_anchor: str, end_anchor: str, name: str) -> str:
        function = braced(sources["fiber"], function_anchor)
        start, end = unique(function, start_anchor), unique(function, end_anchor)
        if end <= start:
            raise RuntimeError(f"incorrect block ordering: {name}")
        code = function[start:end]
        fragments.append({"entrypoint": name, "path": "crates/cordis/src/fiber.rs",
                          "start_anchor": start_anchor, "end_anchor": end_anchor,
                          "sha256": sha256(code), "bytes": len(code.encode()),
                          "scope": "lock-held field updates; callbacks outside this block are excluded"})
        return code

    refresh = core("pub(crate) fn drain(",
                   "let data = &state.fibers[id];\n                    let target =",
                   "(fiber, old_state, new_state, transition)", "drain::locked_boundary")
    finish = core("fn finish_transition(",
                  "let data = &mut state.fibers[fiber.id];\n            let next = match lifecycle::finish(",
                  "            (old, new, next)\n", "finish_transition::locked_boundary")
    snapshot_function = braced(sources["reflect"], "fn refresh_keys(")
    snapshot = braced(snapshot_function,
                      "for (index, pair, binding, revision, active, available, predicate, value) in &checked")
    fragments.append({"entrypoint": "refresh_keys::checked_snapshot_installation",
                      "path": "crates/cordis/src/reflect.rs", "sha256": sha256(snapshot),
                      "bytes": len(snapshot.encode()),
                      "scope": "entire revalidation and update loop; callback availability is an input"})
    template = (FORMAL / "kernel_refinement_probe.rs").read_text()
    replacements = {"EPOCH": epoch, "FIBER_METHODS": fiber_methods,
                    "CONTEXT_METHODS": context_methods, "REFLECT_FUNCTIONS": reflect_functions,
                    "REFRESH_CORE": refresh, "FINISH_CORE": finish, "SNAPSHOT_LOOP": snapshot,
                    "CURRENT_CANDIDATE": candidate, "FROM_BINDING": from_binding,
                    "RESERVE_IDENTITY": reservation, "ADVANCE_NOTIFICATION": notification,
                    "SCOPED_META": scoped_meta}
    for name, body in replacements.items():
        marker = f"// @SOURCE:{name}@"
        unique(template, marker)
        template = template.replace(marker, body)
    lifecycle = ROOT / "crates/cordis/src/lifecycle.rs"
    unique(template, "@LIFECYCLE_PATH@")
    template = template.replace("@LIFECYCLE_PATH@", str(lifecycle))
    if "@SOURCE:" in template:
        raise RuntimeError("unresolved production fragment marker")
    return template, fragments


def compare(expected: str, actual: str) -> list[str]:
    left, right = expected.splitlines(), actual.splitlines()
    if len(left) != len(right):
        raise RuntimeError(f"probe line count differs: Lean={len(left)}, Rust={len(right)}")
    return [f"Lean: {a}\nRust: {b}" for a, b in zip(left, right) if a != b]


def in_domain(family: str, values: list[int]) -> bool:
    if any(value < 0 for value in values):
        return False
    if family in {"refresh", "finish"}:
        return (len(values) == 5 and values[0] < 6 and values[1] < 64
                and values[2] < 4 and values[3] < 4 and values[4] < (4 if family == "refresh" else 2))
    if family == "register":
        return (len(values) == 6 and all(value < 4 for value in values[:3]) and values[3] < 6
                and values[4] in {0, 1, 2, 17, 129} and values[5] < 4)
    if family == "epoch":
        if len(values) != 4:
            return False
        length, code, required, declarations = values
        return (length < 5 and code < 4 ** length and
                ((declarations == length and required < 2 ** length)
                 or (declarations == length + 1 and required == 0)
                 or (length > 0 and declarations == length - 1 and required == 0)))
    if family == "commit":
        return len(values) == 3 and values[0] < 5 and values[1] < 3 ** values[0] and values[2] < 4
    if family in {"locate", "snapshot", "get_value"}:
        return len(values) == 1 and values[0] < {"locate": 1024, "snapshot": 512, "get_value": 4096}[family]
    if family == "typed_handle":
        return len(values) == 3 and values[0] < 4 and values[1] < 3 and values[2] < 4
    if family == "metadata":
        return len(values) == 2 and values[0] < 1024 and values[1] < 16
    if family in {"identity", "notification"}:
        return len(values) == 2 and values[0] < 6 and values[1] < 2
    if family == "counter":
        return (len(values) == 6 and values[0] < 6 and values[1] in {1, 5, 35}
                and values[2] < 4 and values[3] == values[2] and values[4] < 4 and values[5] < 2)
    if family == "release":
        return (len(values) == 4 and values[0] < 5 and values[1] < 3 ** values[0]
                and values[2] < 16 and values[3] < 4)
    return False


def validate_vectors(text: str) -> Counter[str]:
    families: Counter[str] = Counter()
    seen: set[tuple[str, tuple[int, ...]]] = set()
    for line in text.splitlines():
        fields = line.split("\t")
        if len(fields) != 3:
            raise RuntimeError("Lean vector stream is malformed")
        try:
            values = [int(value) for value in fields[1].split(",")]
        except ValueError as error:
            raise RuntimeError("Lean vector input is not an integer tuple") from error
        if not in_domain(fields[0], values):
            raise RuntimeError(f"input outside the declared finite domain: {fields[:2]}")
        key = fields[0], tuple(values)
        if key in seen:
            raise RuntimeError(f"duplicate vector input: {fields[:2]}")
        seen.add(key)
        families[fields[0]] += 1
    if dict(families) != EXPECTED_CASES:
        raise RuntimeError(f"finite input domain incomplete: expected {EXPECTED_CASES}, observed {dict(families)}")
    return families


def write_evidence(path: Path | None, evidence: dict[str, object]) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        json.dump(evidence, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def verify(args: argparse.Namespace) -> dict[str, object]:
    tracked = [FORMAL / path for path in ["Lifecycle.lean", "Admission.lean", "Scopes.lean",
                                         "KernelRefinement.lean", "Bindings.lean", "KernelSemantics.lean", "KernelVectors.lean",
                                         "lakefile.toml", "lake-manifest.json", "lean-toolchain",
                                         "kernel_refinement_probe.rs", "check_kernel_refinement.py"]]
    tracked += [ROOT / f"crates/cordis/src/{path}.rs"
                for path in ["fiber", "context", "effect", "reflect", "service", "lifecycle", "lifecycle_generated"]]
    before = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in tracked}
    run([args.lake, "build", "KernelRefinement", "Bindings"])
    run([args.lake, "env", "lean", "-o", ".lake/build/lib/lean/KernelSemantics.olean", "KernelSemantics.lean"])
    code, fragments = extract_source()
    expected = run([args.lake, "env", "lean", "--run", "KernelVectors.lean"])
    lines = expected.splitlines()
    families = validate_vectors(expected)
    negative_controls = {}
    malformed = {"missing_vector": "\n".join(lines[1:]),
                 "duplicate_vector": expected + lines[0] + "\n",
                 "out_of_domain_vector": "refresh\t6,0,0,0,0\tunused\n" + "\n".join(lines[1:])}
    for name, invalid in malformed.items():
        try:
            validate_vectors(invalid)
        except RuntimeError:
            negative_controls[name] = "rejected"
        else:
            raise RuntimeError(f"invalid vector negative control escaped: {name}")

    with tempfile.TemporaryDirectory(prefix="cordis-kernel-refinement-") as temporary:
        directory = Path(temporary)
        source, executable = directory / "probe.rs", directory / "probe"
        source.write_text(code)
        run([args.rustc, "--edition=2024", "-O", str(source), "-o", str(executable)])
        actual = run([str(executable)], input_text=expected)
        mismatches = compare(expected, actual)
        if mismatches:
            raise RuntimeError(f"{len(mismatches)} compiled production observations differ\n{mismatches[0]}")

        # Mutate the extracted production guards, not expected results or stubs.
        # Missing or duplicated mutation targets fail closed.
        details = []
        mutations = [
            ("inverted_production_admission_liveness", "register",
             "if !data.alive {\n            return false;\n        }",
             "if data.alive {\n            return false;\n        }"),
            ("one_sided_production_dependency_length_guard", "epoch",
             "fiber.bindings.len() != fiber.inject.entries.len()",
             "fiber.bindings.len() < fiber.inject.entries.len()"),
            ("exhausted_notification_candidate_reused", "snapshot",
             "!state.impls[*id].notification_exhausted", "true"),
            ("optional_absence_classified_as_required", "get_value",
             "entry.key.as_ref() == key && !entry.required", "entry.key.as_ref() == key && entry.required"),
            ("typed_definition_origin_replaced", "typed_handle",
             "definition: definition(),", "definition: caller.clone(),"),
            ("identity_wraparound", "identity",
             'let uid = state\n                    .impl_counter\n                    .checked_add(1)\n                    .ok_or_else(|| Error::new("service identity exhausted").at(location))?;',
             "let uid = state.impl_counter.wrapping_add(1);"),
            ("notification_wraparound", "notification",
             "binding.notification_revision.checked_add(1)",
             "Some(binding.notification_revision.wrapping_add(1))"),
            ("metadata_layer_order_reversed", "metadata",
             ".rev()\n            .fold(initial,", ".fold(initial,"),
            ("metadata_declaration_uses_caller", "metadata",
             "crate::reflect::injected_meta::<K>(&self.requester)",
             "crate::reflect::injected_meta::<K>(&self.caller)"),
        ]
        for name, family, mutation, replacement in mutations:
            unique(code, mutation)
            mutant = code.replace(mutation, replacement)
            mutant_source, mutant_executable = directory / f"{name}.rs", directory / name
            mutant_source.write_text(mutant)
            run([args.rustc, "--edition=2024", "-O", str(mutant_source), "-o", str(mutant_executable)])
            control_input = "\n".join(line for line in lines if line.startswith(family + "\t")) + "\n"
            control_actual = run([str(mutant_executable)], input_text=control_input)
            controls = compare(control_input, control_actual)
            if not controls:
                raise RuntimeError(f"mutated production guard escaped: {name}")
            negative_controls[name] = "rejected"
            details.append({"mutation": name, "detected": True,
                            "differing_cases": len(controls), "witness": controls[0]})

    after = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in tracked}
    if before != after:
        raise RuntimeError("source inputs changed while kernel refinement checks were running")

    return {
        "status": "passed", "compiled_input_cases": len(lines), "cases": dict(sorted(families.items())),
        "vectors_sha256": sha256(expected), "observations_sha256": sha256(actual),
        "source_sha256": before,
        "extracted_production_functions": fragments,
        "support_stubs": [
            "Arena uses Vec with stable valid usize identities; SlotMap allocation, generation reuse, removal, and allocator failure are outside the bridge",
            "DisposableList observation is represented by an ordered Vec; insert/iter behavior and fresh effect identities are the tested interface",
            "Kernel State uses std::sync::Mutex with an infallible lock adapter; OS scheduling, parking_lot implementation, reentrancy, and poisoning are outside this bridge",
            "Lifecycle task/error payloads are opaque unit/error values; snapshot availability is an input. wait_dependents uses std::future::poll_fn and real no-op Wakers with explicit single polls",
            "Implementation::is_active is an input Boolean; predicate/value identities use actual Arc::ptr_eq on constructed pointer pools",
            "Typed handles use real std Any downcasts for u32, String, dyn Counter and wrong wrapper types, preserving actual Arc identity and lazy definition construction",
            "Metadata uses a noncommutative Vec concatenation Merge implementation; arbitrary user Merge code remains an environment contract. Real wrong-type declaration metadata defaults and wrong-type matching interception panics are observed",
            "Root/child scope constructors use explicit distinct owner identities; Context::derive is the exact production body",
            "Source lock-held update blocks are wrapped as single-step functions; callback dispatch, lifetime disposal, and transition-loop scheduling remain separate obligations",
        ],
        "negative_controls": negative_controls,
        "negative_control_details": details,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lake", default="lake")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()
    write_evidence(args.evidence, {"status": "running"})
    try:
        evidence = verify(args)
    except BaseException as error:
        write_evidence(args.evidence, {"status": "failed", "error": f"{type(error).__name__}: {error}"})
        raise
    write_evidence(args.evidence, evidence)
    print(json.dumps(evidence, ensure_ascii=False))


if __name__ == "__main__":
    main()
