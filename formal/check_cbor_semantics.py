#!/usr/bin/env python3
"""Check CBOR grammar proofs, source-reflected guards, and actual codec executions.

The grammar theorem applies to arbitrary recursive values. Rust codec executions
are finite; their counts never stand in for a proof of ciborium's parser. The
source certificates cover the resource-tag predicate and guarded float width
selection, and fail on independently injected production-source changes.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import tempfile

from check_wire_semantics import ROOT, FORMAL, function, run


def balanced_block(source, marker):
    begin = source.index(marker)
    opening = source.index("{", begin)
    depth = 1
    end = opening + 1
    while depth and end < len(source):
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    if depth:
        raise ValueError("unbalanced source block")
    return source[begin:end]


def resource_certificate(codec, payload):
    constant = re.search(r"pub const OBJECT_TAG: u64 = (\d+);", payload)
    if not constant:
        raise ValueError("OBJECT_TAG constant left the checked grammar")
    body = re.sub(r"\s+", "", function(codec, "resource_tag"))
    pattern = (r'fnresource_tag\(tag:u64,value:&Value\)->Result<\(\),String>\{'
               r'iftag==OBJECT_TAG&&value.as_integer\(\).and_then\(\|id\|u64::try_from\(id\).ok\(\)\)'
               r'.is_none_or\(\|id\|id==(\d+)\)\{Err\("resourcetagrequiresanonzerou64".into\(\)\)\}'
               r'else\{Ok\(\(\)\)\}\}')
    matched = re.fullmatch(pattern, body)
    if not matched:
        raise ValueError("resource_tag left the checked scalar-expression grammar")
    return f'''import CborSemantics
namespace SourceCborResource
open Cordis.CborSemantics
def sourceTag (number : Nat) (value : Data) : Bool :=
  if number = {constant.group(1)} then
    match value with
    | .unsigned n => n < 18446744073709551616 && decide (n ≠ {matched.group(1)})
    | _ => false
  else true
theorem source_resource_matches (number : Nat) (value : Data) :
    sourceTag number value = resourceTag number value := by
  cases value <;> simp [sourceTag, resourceTag, Nat.pos_iff_ne_zero, Bool.and_comm]
#print axioms source_resource_matches
end SourceCborResource
'''


def float_certificate(source):
    block = re.sub(r"\s+", "", balanced_block(source, "Header::Float(n64) => {"))
    pattern = (r'Header::Float\(n64\)=>\{letn16=f16::from_f64\(n64\);letn32=n64asf32;'
               r'Title\(Major::Other,iff64::from\(n16\).to_bits\(\)(==|!=)n64.to_bits\(\)'
               r'\{Minor::Next2\(n16.to_be_bytes\(\)\)\}'
               r'elseif f64::from\(n32\).to_bits\(\)(==|!=)n64.to_bits\(\)'
               r'\{Minor::Next4\(n32.to_be_bytes\(\)\)\}'
               r'else\{Minor::Next8\(n64.to_be_bytes\(\)\)\},\)\}')
    pattern = pattern.replace("elseif f64", "elseiff64")
    matched = re.fullmatch(pattern, block)
    if not matched:
        raise ValueError("ciborium float selection left the checked conditional grammar")
    first, second = ["=" if operator == "==" else "≠" for operator in matched.groups()]
    return f'''import CborSemantics
namespace SourceCborFloat
open Cordis.CborSemantics
def sourceShorten (codec : FloatCodec) (bits : Nat) : FloatEncoding :=
  if codec.expandHalf (codec.half bits) {first} bits then .half (codec.half bits)
  else if codec.expandSingle (codec.single bits) {second} bits then .single (codec.single bits)
  else .double bits
theorem source_float_matches (codec : FloatCodec) (bits : Nat) :
    sourceShorten codec bits = shorten codec bits := by rfl
theorem every_source_float_preserves_bits (codec : FloatCodec) (bits : Nat) :
    expand codec (sourceShorten codec bits) = bits := by
  rw [source_float_matches]
  exact every_float_bit_pattern_survives_shortening codec bits
#print axioms every_source_float_preserves_bits
end SourceCborFloat
'''


def value_float_certificate(source):
    block = re.sub(r"\s+", "", balanced_block(source, "Value::Float(x) => {"))
    matched = re.fullmatch(
        r'Value::Float\(x\)=>\{lety=\*xasf32;if\(yasf64\).to_bits\(\)(==|!=)x.to_bits\(\)'
        r'\{serializer.serialize_f32\(y\)\}else\{serializer.serialize_f64\(\*x\)\}\}', block)
    if not matched:
        raise ValueError("Value float preparation left the checked conditional grammar")
    operator = "=" if matched.group(1) == "==" else "≠"
    return f'''import CborSemantics
namespace SourceCborValueFloat
open Cordis.CborSemantics
def sourcePrepare (codec : FloatCodec) (bits : Nat) : Nat :=
  if codec.expandSingle (codec.single bits) {operator} bits then codec.expandSingle (codec.single bits) else bits
theorem value_float_matches (codec : FloatCodec) (bits : Nat) :
    sourcePrepare codec bits = prepareFloat codec bits := by rfl
theorem every_value_float_preserves_bits (codec : FloatCodec) (bits : Nat) :
    expand codec (shorten codec (sourcePrepare codec bits)) = bits := by
  rw [value_float_matches]
  exact both_float_serialization_layers_preserve_bits codec bits
#print axioms every_value_float_preserves_bits
end SourceCborValueFloat
'''


def bignum_certificate(source, constants):
    positive = re.search(r"pub const BIGPOS: u64 = (\d+);", constants)
    negative = re.search(r"pub const BIGNEG: u64 = (\d+);", constants)
    if not positive or not negative:
        raise ValueError("bignum tag constants left the checked grammar")
    block = re.sub(r"\s+", "", balanced_block(source, "match (tag, len) {"))
    pattern = (
        r'match\(tag,len\)\{\(tag::BIGPOS,Some\(len\)\)\|\(tag::BIGNEG,Some\(len\)\)iflen<=(\d+)'
        r'=>\{letresult=matchself.integer\(Some\(Header::Tag\(tag\)\)\)\?\{'
        r'\(false,raw\)=>returnvisitor.visit_u128\(raw\),'
        r'\(true,raw\)=>i(\d+)::try_from\(raw\).map\(\|x\|x\^!0\),\};'
        r'matchresult\{Ok\(x\)=>visitor.visit_i128\(x\),Err\(\.\.\)=>Err\(de::Error::custom\("integertoolarge"\)\),\}\}'
        r'_=>self.recurse\(\|me\|\{letaccess=crate::tag::TagAccess::new\(me,Some\(tag\)\);visitor.visit_enum\(access\)\}\),\}')
    matched = re.fullmatch(pattern, block)
    if not matched:
        raise ValueError("ciborium bignum dispatch left the checked conditional grammar")
    width, signed_bits = map(int, matched.groups())
    return f'''import CborSemantics
namespace SourceCborBignum
open Cordis.CborSemantics
def sourceInterpret (number : Nat) (value : Data) : TagMeaning :=
  match value with
  | .bytes bytes =>
    if bytes.length ≤ {width} then
      if number = {positive.group(1)} then .positiveBig (magnitude bytes)
      else if number = {negative.group(1)} then
        if magnitude bytes < {2 ** (signed_bits - 1)} then .negativeBig (magnitude bytes)
        else .rejectedNegativeOverflow
      else .ordinary number value
    else .ordinary number value
  | _ => .ordinary number value
theorem every_bignum_dispatch_matches (number : Nat) (value : Data) :
    sourceInterpret number value = interpretTag number value := by rfl
#print axioms every_bignum_dispatch_matches
end SourceCborBignum
'''


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--lake", default="lake")
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--evidence", type=Path, default=ROOT / "target/cbor-semantics.json")
    args = parser.parse_args()
    paths = [ROOT / "Cargo.lock", ROOT / "Cargo.toml", ROOT / "crates/cordis-up/Cargo.toml",
             *sorted((ROOT / "crates/cordis-up/src").rglob("*.rs")),
             FORMAL / "WireSemantics.lean", FORMAL / "CborSemantics.lean",
             FORMAL / "cbor_probe.rs", FORMAL / "check_wire_semantics.py", Path(__file__).resolve()]
    snapshots = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}
    evidence = {"schema": "cordis-cbor-semantics/v1", "status": "running", "source_sha256": snapshots}
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    def publish():
        temporary = args.evidence.with_suffix(".tmp")
        temporary.write_text(json.dumps(evidence, indent=2) + "\n")
        temporary.replace(args.evidence)
    publish()
    try:
        for module in ["WireSemantics", "CborSemantics"]:
            run([args.lake, "env", "lean", "-o", f".lake/build/lib/lean/{module}.olean", f"{module}.lean"])
        built = run([args.cargo, "build", "-p", "cordis-up", "--locked", "--message-format=json"], cwd=ROOT)
        artifacts = [json.loads(line) for line in built.stdout.splitlines() if line.startswith("{")]
        up = next(item for item in artifacts if item.get("reason") == "compiler-artifact" and item["target"]["name"] == "cordis_up")
        library = Path(next(path for path in up["filenames"] if path.endswith(".rlib")))
        dependency_dir = library.parent if library.parent.name == "deps" else library.parent / "deps"
        ll = next(item for item in artifacts if item.get("reason") == "compiler-artifact" and item["target"]["name"] == "ciborium_ll")
        float_source = Path(ll["manifest_path"]).parent / "src/hdr.rs"
        ll_text = float_source.read_text()
        ciborium = next(item for item in artifacts if item.get("reason") == "compiler-artifact" and item["target"]["name"] == "ciborium")
        dependency_paths = {"ciborium-ll/src/hdr.rs": float_source,
            "ciborium-ll/src/lib.rs": Path(ll["manifest_path"]).parent / "src/lib.rs",
            "ciborium/src/value/ser.rs": Path(ciborium["manifest_path"]).parent / "src/value/ser.rs",
            "ciborium/src/de/mod.rs": Path(ciborium["manifest_path"]).parent / "src/de/mod.rs",
            "ciborium/src/value/mod.rs": Path(ciborium["manifest_path"]).parent / "src/value/mod.rs"}
        value_text = dependency_paths["ciborium/src/value/ser.rs"].read_text()
        decode_text = dependency_paths["ciborium/src/de/mod.rs"].read_text()
        constants = dependency_paths["ciborium-ll/src/lib.rs"].read_text()
        evidence["dependency_source_sha256"] = {name: hashlib.sha256(path.read_bytes()).hexdigest() for name,path in dependency_paths.items()}
        codec = (ROOT / "crates/cordis-up/src/codec.rs").read_text()
        payload = (ROOT / "crates/cordis-up/src/payload.rs").read_text()
        with tempfile.TemporaryDirectory(prefix="cordis-cbor-") as directory:
            temp = Path(directory)
            cert = temp / "SourceCborCertificate.lean"
            certificates = [resource_certificate(codec, payload), float_certificate(ll_text),
                            value_float_certificate(value_text), bignum_certificate(decode_text, constants)]
            cert.write_text("import CborSemantics\n" + "\n".join(text.replace("import CborSemantics\n", "") for text in certificates))
            evidence["source_certificate"] = run([args.lake, "env", "lean", cert]).stdout.strip()
            for altered in [resource_certificate(codec.replace("|id| id == 0", "|id| id == 1", 1), payload),
                            float_certificate(ll_text.replace("f64::from(n16).to_bits() == n64.to_bits()", "f64::from(n16).to_bits() != n64.to_bits()", 1)),
                            value_float_certificate(value_text.replace("(y as f64).to_bits() == x.to_bits()", "(y as f64).to_bits() != x.to_bits()", 1)),
                            bignum_certificate(decode_text.replace("if len <= 16", "if len <= 15", 1), constants)]:
                cert.write_text(altered)
                if run([args.lake, "env", "lean", cert], check=False).returncode == 0:
                    raise AssertionError("CBOR source mutation survived all-input certificate")
            binary = temp / "codec"
            run([args.rustc, "--edition=2024", FORMAL / "cbor_probe.rs", "--extern", f"cordis_up={library}",
                 "-L", f"dependency={dependency_dir}", "-o", binary])
            evidence.update(json.loads(run([binary]).stdout))
            # Execute the exact private source function as well as the public codec.
            scalar = function(codec, "resource_tag")
            prelude = "use cordis_up::{Value,OBJECT_TAG};\n"
            checks = '''\nfn main(){
                assert!(resource_tag(OBJECT_TAG,&0_u64.into()).is_err());
                assert!(resource_tag(OBJECT_TAG,&1_u64.into()).is_ok());
                assert!(resource_tag(42,&Value::Null).is_ok());
            }'''
            source = temp / "resource.rs"
            source.write_text(prelude + scalar + checks)
            run([args.rustc, "--edition=2024", source, "--extern", f"cordis_up={library}",
                 "-L", f"dependency={dependency_dir}", "-o", binary])
            run([binary])
            source.write_text(prelude + scalar.replace("|id| id == 0", "|id| id == 1", 1) + checks)
            run([args.rustc, "--edition=2024", source, "--extern", f"cordis_up={library}",
                 "-L", f"dependency={dependency_dir}", "-o", binary])
            if run([binary], check=False).returncode == 0:
                raise AssertionError("compiled resource-tag negative control survived")
        for relative, digest in snapshots.items():
            if hashlib.sha256((ROOT / relative).read_bytes()).hexdigest() != digest:
                raise AssertionError(f"source changed during verification: {relative}")
        for name, path in dependency_paths.items():
            if hashlib.sha256(path.read_bytes()).hexdigest() != evidence["dependency_source_sha256"][name]:
                raise AssertionError(f"dependency source changed during verification: {name}")
        evidence["compiled_input_cases"] += 3
        evidence["negative_controls"] = {"resource_zero_guard_certificate": "rejected",
            "float_lossless_guard_certificate": "rejected", "value_float_lossless_guard_certificate": "rejected",
            "bignum_dispatch_length_certificate": "rejected", "compiled_resource_zero_guard": "rejected"}
        evidence["trust_boundary"] = [
            "Inductive CBOR grammar is a specification; no theorem equates the entire ciborium parser to it.",
            "The source-derived resource guard and float branch selection are proved for arbitrary arguments.",
            "UTF-8 library, IEEE conversion primitives, Rust/serde/ciborium parser, compiler and allocator remain dependency boundaries.",
            "Public codec executions cover all constructors, recursive graphs, duplicate map keys, malformed encodings, limits, special tags, NaNs and signed zero as finite checks."]
        evidence["status"] = "passed"
        publish()
        print(json.dumps({key: value for key,value in evidence.items() if key != "source_sha256"}, indent=2))
    except Exception as error:
        evidence.update(status="failed", error=str(error))
        publish()
        raise


if __name__ == "__main__":
    main()
