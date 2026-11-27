#!/usr/bin/env python3
"""Check supervisor provenance certificates and complete extracted methods.

The restricted parser certifies the metadata-key predicate and origin field
projection for arbitrary inputs. Complete methods run over real std Arc, Weak,
Any, atomics and Mutex with explicit context/connection observation adapters.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FORMAL = ROOT / "formal"
sys.path.insert(0, str(FORMAL))
from check_kernel_refinement import braced, unique

SOURCES = {
    "SCOPED_METADATA": ("crates/cordis-supervisor/src/manager/trusted.rs", "pub(super) fn scoped_metadata("),
    "GENERATION": ("crates/cordis-supervisor/src/manager.rs", "fn generation(&self,"),
    "REQUEST_ORIGIN": ("crates/cordis-supervisor/src/manager/routing.rs", "pub fn request_origin("),
    "IS_CURRENT": ("crates/cordis-supervisor/src/manager/generation.rs", "pub fn is_current("),
    "ACTIVE": ("crates/cordis-supervisor/src/manager/generation.rs", "pub fn active("),
    "CALL_ORIGIN": ("crates/cordis-supervisor/src/manager/routing.rs", "fn call_origin("),
}


def execute(command: list[str], *, success: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, cwd=FORMAL, capture_output=True, text=True, timeout=180)
    if success and result.returncode:
        raise RuntimeError(f"{' '.join(command)} failed:\n{result.stdout}{result.stderr}")
    return result


def parse_filter(body: str) -> tuple[bool, str]:
    statements = re.findall(r"fields\.retain\([^\n]*", body)
    if len(statements) != 1:
        raise RuntimeError("metadata filter must have exactly one recognized statement")
    match = re.fullmatch(
        r'fields\.retain\(\|\(key,\s*_\)\|\s*(!?)matches!\(key,\s*Value::Text\(key\)\s*if\s*'
        r'key\.starts_with\(("(?:\\.|[^"\\])*")\)\)\);\s*', statements[0])
    if not match:
        raise RuntimeError("unsupported metadata-filter syntax")
    return bool(match[1]), json.loads(match[2])


def parse_origin(body: str) -> dict[str, str]:
    inner = body[body.index("{") + 1:body.rindex("}")]
    initializer = re.fullmatch(r"\s*CallContext\s*\{([\s\S]*)\}\s*", inner)
    if not initializer:
        raise RuntimeError("origin body must contain only the supported field initializer")
    content = initializer[1]
    fields = {}
    allowed = {"view.component.connection.id()": "connection", "view.id": "context",
               "view.instance.name.clone()": "instanceId", "view.instance.group.clone()": "group",
               "view.instance.plugin.clone()": "plugin"}
    for entry in content.split(","):
        if not entry.strip():
            continue
        name, expression = (part.strip() for part in entry.split(":", 1))
        if name in fields or expression not in allowed:
            raise RuntimeError("unsupported or repeated origin field")
        fields[name] = allowed[expression]
    if set(fields) != {"connection", "context", "instance", "group", "plugin"}:
        raise RuntimeError("origin field domain changed")
    return fields


def parse_context_bindings(body: str) -> tuple[str, str, str]:
    """Recognize the complete scoped-metadata body, including receiver binding."""
    code = re.sub(r"\s+", "", body[body.index("{") + 1:body.rindex("}")])
    retain, = re.findall(r"fields\.retain\([^\n]*", body)
    retain = re.sub(r"\s+", "", retain)
    receiver = r'(view|self\.0|view\.generation)'
    interface = r'(interface|"(?:\\.|[^"\\])*")'
    pattern = (
        r'letmetadata=' + receiver + r'\.context\.named_meta\(' + interface +
        r'\)\.and_then\(\|meta\|meta\.downcast::<Value>\(\)\.ok\(\)\);'
        r'letmutfields=matchmetadata\.as_deref\(\)\{'
        r'Some\(Value::Map\(fields\)\)=>fields\.clone\(\),'
        r'Some\(Value::Null\)\|None=>Vec::new\(\),'
        r'Some\(_\)=>returnErr\(error\(2,"[^"\\]*"\)\),\};' + re.escape(retain) +
        r'letcaller=' + receiver + r'\.context\.extension::<TrustedCall>\(\)\.ok_or_else\(\|\|\{'
        r'letfailure=error\(4,"[^"\\]*"\);'
        r'self\.0\.context\.logger\(\)\.error\(failure\.message\.clone\(\)\);failure\}\)\?;'
        r'fields\.extend\(caller\.fields\.clone\(\)\);Ok\(Value::Map\(fields\)\)'
    )
    matched = re.fullmatch(pattern, code)
    if not matched:
        raise RuntimeError("unsupported scoped-metadata receiver/control-flow structure")
    metadata, requested, trusted = matched.groups()
    names = {"view": "view", "self.0": "manager", "view.generation": "generation"}
    interface_expression = "requested" if requested == "interface" else json.dumps(json.loads(requested), ensure_ascii=False)
    return names[metadata], names[trusted], interface_expression


def parse_request_context(body: str) -> str:
    code = re.sub(r"\s+", "", body[body.index("{") + 1:body.rindex("}")])
    matched = re.fullmatch(
        r'letview=self\.generation\(connection\.id\(\),(context|\d+)\)\?;'
        r'view\.active\(\)\?;Ok\(call_origin\(&view\)\)', code)
    if not matched:
        raise RuntimeError("unsupported request-origin lookup/control-flow structure")
    return "requested" if matched[1] == "context" else matched[1]


def certificate(fragments: dict[str, str]) -> str:
    negate, prefix = parse_filter(fragments["SCOPED_METADATA"])
    projection = parse_origin(fragments["CALL_ORIGIN"])
    metadata_context, trusted_context, requested_interface = parse_context_bindings(fragments["SCOPED_METADATA"])
    requested_context = parse_request_context(fragments["REQUEST_ORIGIN"])
    text_branch = ("!" if negate else "") + f"(key.startsWith {json.dumps(prefix, ensure_ascii=False)})"
    other_branch = "true" if negate else "false"
    values = ", ".join("generation." + projection[field]
                       for field in ["connection", "context", "instance", "group", "plugin"])
    return f"""import Architecture
namespace Cordis.ArchitectureSourceCertificate
open Architecture
def parsedFilter : MetadataKey → Bool
  | .text key => {text_branch}
  | .other _ => {other_branch}
theorem source_filter_all_keys (key : MetadataKey) :
    parsedFilter key = !reservedKey key := by cases key <;> rfl
def parsedOrigin (generation : Generation) : Origin := ⟨{values}⟩
theorem source_origin_all_generations (generation : Generation) :
    parsedOrigin generation = originOf generation := by rfl
theorem source_filter_all_fields (untrusted trusted : Metadata) :
    untrusted.filter (fun field => parsedFilter field.1) ++ trusted =
      scopedMetadata untrusted trusted := by
  simp only [source_filter_all_keys, scopedMetadata]
def parsedMetadataContext (view manager generation : Metadata) : Metadata := {metadata_context}
def parsedTrustedContext (view manager generation : Metadata) : Metadata := {trusted_context}
def parsedInterface (requested : String) : String := {requested_interface}
def parsedRequestContext (requested : Nat) : Nat := {requested_context}
theorem source_uses_view_metadata (view manager generation : Metadata) :
    parsedMetadataContext view manager generation = view := by rfl
theorem source_uses_view_trusted_identity (view manager generation : Metadata) :
    parsedTrustedContext view manager generation = view := by rfl
theorem source_passes_requested_interface (requested : String) :
    parsedInterface requested = requested := by rfl
theorem source_passes_requested_context (requested : Nat) :
    parsedRequestContext requested = requested := by rfl
theorem source_scope_uses_view_fields (view manager generation trustedView trustedManager trustedGeneration : Metadata) :
    (parsedMetadataContext view manager generation).filter (fun field => parsedFilter field.1) ++
      parsedTrustedContext trustedView trustedManager trustedGeneration = scopedMetadata view trustedView := by
  simp only [source_uses_view_metadata, source_uses_view_trusted_identity,
    source_filter_all_keys, scopedMetadata]
end Cordis.ArchitectureSourceCertificate
"""


def observations(text: str) -> dict[tuple[str, int], str]:
    result = {}
    for line in text.splitlines():
        domain, raw_id, value = line.split("\t", 2)
        key = domain, int(raw_id)
        if key in result:
            raise RuntimeError(f"duplicate architecture case {key}")
        result[key] = value
    expected = {("metadata", case) for case in range(7680)} | {("origin", case) for case in range(6144)}
    if set(result) != expected:
        raise RuntimeError("architecture probe case domain is incomplete")
    return result


def write_receipt(path: Path | None, evidence: dict) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        json.dump(evidence, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def verify(args: argparse.Namespace) -> dict:
    inputs = {ROOT / path for path, _ in SOURCES.values()} | {
        Path(__file__).resolve(), FORMAL / "Architecture.lean", FORMAL / "ArchitectureVectors.lean",
        FORMAL / "architecture_probe.rs", FORMAL / "check_kernel_refinement.py",
        FORMAL / "lakefile.toml", FORMAL / "lake-manifest.json", FORMAL / "lean-toolchain",
    }
    def snapshot() -> dict[str, str]:
        return {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(inputs)}
    before = snapshot()
    fragments = {name: braced((ROOT / path).read_text(), anchor) for name, (path, anchor) in SOURCES.items()}
    template = (FORMAL / "architecture_probe.rs").read_text()
    for name in fragments:
        unique(template, f"// @SOURCE:{name}@")
    execute([args.lake, "env", "lean", "-o", ".lake/build/lib/lean/Architecture.olean", "Architecture.lean"])
    expected = observations(execute([args.lake, "env", "lean", "--run", "ArchitectureVectors.lean"]).stdout)
    controls = {}
    with tempfile.TemporaryDirectory(prefix="cordis-architecture-") as directory:
        temporary = Path(directory)
        cert = temporary / "Certificate.lean"
        certificate_source = certificate(fragments)
        cert.write_text(certificate_source)
        execute([args.lake, "env", "lean", "--trust=0", str(cert)])

        def probe(parts: dict[str, str], name: str) -> dict[tuple[str, int], str]:
            source = template
            for key, body in parts.items():
                source = source.replace(f"// @SOURCE:{key}@", body)
            program, binary = temporary / f"{name}.rs", temporary / name
            program.write_text(source)
            execute([args.rustc, "--edition=2024", "-O", str(program), "-o", str(binary)])
            return observations(execute([str(binary)]).stdout)

        actual = probe(fragments, "source")
        differences = [key for key in expected if expected[key] != actual[key]]
        if differences:
            key = differences[0]
            raise RuntimeError(f"architecture projection differs at {key}: Lean={expected[key]!r}, Rust={actual[key]!r}")
        mutations = {
            "reserved_prefix": ("SCOPED_METADATA", 'starts_with("cordis.")', 'starts_with("cordix.")'),
            "filter_negation": ("SCOPED_METADATA", "!matches!", "matches!"),
            "trusted_append": ("SCOPED_METADATA", "fields.extend(caller.fields.clone());", "fields.clear();"),
            "connection_identity": ("GENERATION", "id() != connection", "id() == connection"),
            "exported_guard": ("ACTIVE", "&& !core.exported", "|| !core.exported"),
            "origin_projection": ("CALL_ORIGIN", "connection: view.component.connection.id()", "connection: view.id"),
            "metadata_context_identity": ("SCOPED_METADATA", "let metadata = view", "let metadata = self.0"),
            "trusted_context_identity": ("SCOPED_METADATA", "let caller = view.context", "let caller = self.0.context"),
            "metadata_interface_identity": ("SCOPED_METADATA", ".named_meta(interface)", '.named_meta("wrong@9")'),
            "request_context_identity": ("REQUEST_ORIGIN", "self.generation(connection.id(), context)?", "self.generation(connection.id(), 23)?"),
        }
        for name, (part, old, new) in mutations.items():
            unique(fragments[part], old)
            mutant = dict(fragments, **{part: fragments[part].replace(old, new)})
            if probe(mutant, name) == expected:
                raise RuntimeError(f"architecture negative control passed: {name}")
            controls[name] = "rejected"
            if name in {"reserved_prefix", "filter_negation", "origin_projection",
                         "metadata_context_identity", "trusted_context_identity",
                         "metadata_interface_identity", "request_context_identity"}:
                cert.write_text(certificate(mutant))
                rejected = execute([args.lake, "env", "lean", "--trust=0", str(cert)], success=False)
                if rejected.returncode == 0 or "error:" not in rejected.stdout + rejected.stderr:
                    raise RuntimeError(f"source certificate accepted mutation: {name}")
                controls["certificate_" + name] = "rejected"
        try:
            parse_filter(fragments["SCOPED_METADATA"].replace('key.starts_with("cordis.")', "arbitrary(key)"))
        except RuntimeError:
            controls["unsupported_source_syntax"] = "rejected"
        else:
            raise RuntimeError("source filter parser accepted an unrecognized operation")
        retain, = re.findall(r"fields\.retain\([^\n]*", fragments["SCOPED_METADATA"])
        guarded = fragments["SCOPED_METADATA"].replace(retain, f"if fields.len() < 9 {{\n{retain}\n}}")
        try:
            parse_context_bindings(guarded)
        except RuntimeError:
            controls["unsupported_metadata_branch"] = "rejected"
        else:
            raise RuntimeError("source context parser accepted a conditional metadata filter")
    after = snapshot()
    if before != after:
        raise RuntimeError("architecture proof or source changed while verification was running")
    return {"status": "passed", "compiled_input_cases": len(expected),
            "metadata_cases": 7680, "origin_cases": 6144,
            "negative_controls": controls, "source_sha256": after, "source_before_sha256": before,
            "extracted_functions": {key: hashlib.sha256(value.encode()).hexdigest() for key, value in fragments.items()},
            "source_certificate_lean": certificate_source,
            "source_certificate_sha256": hashlib.sha256(certificate_source.encode()).hexdigest(),
            "source_certificates": ["reserved metadata predicate for every key and field list",
                                    "origin field projection for every generation",
                                    "view metadata and trusted identity receivers for arbitrary metadata lists",
                                    "unchanged requested interface and context for arbitrary identities"],
            "support_contracts": [
                "Context lookup uses a real interface-indexed HashMap with separate view, generation and manager identities; Any downcast is real std Arc",
                "Context current and Connection alive supply separately established lifecycle observations",
                "Mutex acquisition, Arc/Weak, atomic loads and string cloning use std; the probe schedules these observations sequentially",
                "the restricted source parser and Rust compiler form the certificate extraction boundary",
            ]}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lake", default="lake")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()
    write_receipt(args.evidence, {"status": "running"})
    try:
        evidence = verify(args)
    except (RuntimeError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        write_receipt(args.evidence, {"status": "failed", "error": str(error)})
        print(error, file=sys.stderr)
        return 1
    write_receipt(args.evidence, evidence)
    print(json.dumps(evidence, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
