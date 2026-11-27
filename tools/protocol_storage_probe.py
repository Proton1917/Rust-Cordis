#!/usr/bin/env python3
"""Compare the compiled production lease algorithm with Lean-generated vectors.

The original Rust struct and impl blocks are inserted verbatim in a temporary
crate. A std Mutex adapter supplies the parking_lot-style lock return type; this
experiment executes sequential critical sections. It does not model a scheduler.
No repository production source is rewritten. Two injected incorrect branches
must be detected, so a source hash alone can never satisfy the comparison.
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from sdk_terminal_probe import check_sdk


ROOT = Path(__file__).resolve().parents[1]
SUBJECT = ROOT / "crates/cordis-up/src/rpc/buffer.rs"


def block(source: str, marker: str) -> str:
    if source.count(marker) != 1:
        raise ValueError(f"expected exactly one production block: {marker}")
    start = source.index(marker)
    opening = source.index("{", start)
    depth = 1
    position = opening + 1
    # The selected blocks contain no brace-bearing strings or comments. Fail
    # closed if their shape changes enough that rustc cannot parse this slice.
    while depth:
        char = source[position]
        depth += (char == "{") - (char == "}")
        position += 1
    return source[start:position]


PRELUDE = r"""
use std::sync::Arc;
struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
}
"""

HARNESS = r"""
pub fn run() {
    use std::io::{self, BufRead, Write};
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let fields: Vec<usize> = line.split_whitespace()
            .map(|value| value.parse().unwrap()).collect();
        assert_eq!(fields.len(), 13);
        let [maximum, reserve, used_data, used_control, lease_data, lease_control,
             privileged, target] = fields[..8].try_into().unwrap();
        let budget = Arc::new(Budget {
            maximum, reserve,
            usage: Mutex::new(Usage { data: used_data, control: used_control,
                                     peak: used_data + used_control }),
        });
        let mut lease = Lease { budget: budget.clone(), data: lease_data,
                                control: lease_control, privileged: privileged != 0 };
        let accepted = usize::from(lease.grow(target));
        {
            let usage = budget.usage.lock();
            writeln!(output, "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                maximum, reserve, used_data, used_control, lease_data, lease_control,
                privileged, target, accepted, usage.data, usage.control,
                lease.data, lease.control).unwrap();
        }
        drop(lease);
        let returned = budget.usage.lock();
        assert_eq!(returned.data, used_data - lease_data, "data lease must return exactly once");
        assert_eq!(returned.control, used_control - lease_control, "control lease must return exactly once");
    }
}
"""


def execute(command: list[str], **kwargs) -> subprocess.CompletedProcess:
    result = subprocess.run(command, check=False, text=True, capture_output=True, **kwargs)
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {command}\n{result.stdout}\n{result.stderr}")
    return result


def compiler(value: str, label: str) -> str:
    located = shutil.which(value)
    if not located:
        raise SystemExit(f"{label} compiler unavailable: {value}")
    return located


def write_receipt(path: Path | None, report: dict) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", dir=path.parent, prefix=path.name + ".", delete=False) as handle:
        temporary = Path(handle.name)
        try:
            json.dump(report, handle, indent=2)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        except BaseException:
            temporary.unlink(missing_ok=True)
            raise
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def required_budget_inputs():
    """Coverage contract, independent of the Lean generator's output/results."""
    for maximum, reserve in itertools.product([0, 1, 4, 8], [0, 1, 3]):
        for used_data, used_control in itertools.product(range(maximum + 1), range(reserve + 1)):
            for lease_data, lease_control, privileged in itertools.product(range(used_data + 1), range(used_control + 1), [0, 1]):
                capacity = lease_data + lease_control
                for target in [0, 1, capacity, capacity + 1, maximum + reserve, maximum + reserve + 1]:
                    yield [maximum, reserve, used_data, used_control, lease_data, lease_control, privileged, target]
    for maximum, reserve in itertools.product([2147483647, 4294967292, 9223372036854775807, 18446744073709551612], [0, 1, 3]):
        for used_data, used_control in itertools.product([0, 1, maximum - 1, maximum], [0, reserve]):
            for lease_data, lease_control, privileged, target in itertools.product([0, used_data], [0, used_control], [0, 1], [0, 1, maximum, maximum + reserve]):
                yield [maximum, reserve, used_data, used_control, lease_data, lease_control, privileged, target]


def verify(args: argparse.Namespace) -> dict:
    tracked = ["formal/Protocol.lean", "formal/SdkProtocol.lean", "formal/ProtocolProbe.lean",
               "tools/protocol_storage_probe.py", "tools/sdk_terminal_probe.py",
               str(SUBJECT.relative_to(ROOT)), "sdks/python/cordis_sdk/wire.py",
               "sdks/typescript/cordis-sdk/src/wire.ts"]
    before = {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in tracked}
    lean_name = args.lean or (str(Path(compiler(args.lake, "Lake")).with_name("lean"))
                              if args.lake else "lean")
    lean, rustc = compiler(lean_name, "Lean"), compiler(args.rustc, "Rust")
    node = compiler(args.node, "Node")
    source = SUBJECT.read_text()
    sections = [block(source, marker) for marker in (
        "pub(super) struct Budget {", "struct Usage {", "pub(super) struct Lease {",
        "impl Lease {", "impl Drop for Lease {",
    )]
    production = "\n\n".join(sections)
    with tempfile.TemporaryDirectory(prefix="cordis-protocol-") as temporary:
        directory = Path(temporary)
        execute([lean, "-o", str(directory / "Protocol.olean"), "Protocol.lean"], cwd=ROOT / "formal")
        execute([lean, "-o", str(directory / "SdkProtocol.olean"), "SdkProtocol.lean"], cwd=ROOT / "formal")
        environment = os.environ.copy()
        environment["LEAN_PATH"] = str(directory) + os.pathsep + environment.get("LEAN_PATH", "")
        expected = execute([lean, "--run", "ProtocolProbe.lean"], cwd=ROOT / "formal", env=environment).stdout
        sdk_vectors = execute([lean, "--run", "ProtocolProbe.lean", "--", "--sdk"], cwd=ROOT / "formal", env=environment).stdout
        rows = expected.splitlines()
        if not rows or any(len(row.split("\t")) != 13 for row in rows):
            raise AssertionError("Lean vector generator did not produce the documented 13-column schema")
        actual_inputs = [list(map(int, row.split("\t")[:8])) for row in rows]
        if actual_inputs != list(required_budget_inputs()):
            raise AssertionError("Lean budget vectors changed or omitted the required input domain")
        sdk_rows = [row.split("\t") for row in sdk_vectors.splitlines()]
        required_traces = [[str(style), "".join(events) or "-"]
                           for style in [0, 1] for length in range(7)
                           for events in itertools.product("oec", repeat=length)]
        if any(len(row) != 8 for row in sdk_rows) or [row[:2] for row in sdk_rows] != required_traces:
            raise AssertionError("Lean SDK vectors changed or omitted the required trace domain")

        def run_variant(name: str, algorithm: str) -> subprocess.CompletedProcess:
            path = directory / f"{name}.rs"
            path.write_text("mod subject {\n" + PRELUDE + algorithm + HARNESS + "\n}\nfn main() { subject::run(); }\n")
            binary = directory / name
            execute([rustc, "--edition=2024", "-Dwarnings", "-C", "overflow-checks=yes", str(path), "-o", str(binary)])
            return subprocess.run([str(binary)], input=expected, text=True, capture_output=True, check=False)

        actual = run_variant("production", production)
        if actual.returncode or actual.stdout != expected:
            raise AssertionError(f"compiled production disagrees with Lean vectors: {actual.stderr[:2000]}")
        negative_controls = {
            "normal_frame_spends_control_reserve": ("!self.privileged ||", "!self.privileged &&"),
            "unaccounted_control_bytes": ("let control = needed - data;", "let control = 0;"),
        }
        detected: list[str] = []
        for name, (old, new) in negative_controls.items():
            if production.count(old) != 1:
                raise AssertionError(f"negative control injection point changed: {name}")
            mutant = run_variant(name, production.replace(old, new))
            if mutant.returncode == 0 and mutant.stdout == expected:
                raise AssertionError(f"negative control escaped detection: {name}")
            detected.append(name)
        sdk = check_sdk(ROOT, directory, sdk_vectors, block, node)
        after = {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in tracked}
        if before != after:
            raise AssertionError("protocol probe inputs changed during compilation or comparison")
        report = {
            "status": "passed",
            "domain": "protocol-buffer",
            "evidence": "compiled-production-body-differential",
            "source": str(SUBJECT.relative_to(ROOT)),
            "source_sha256": hashlib.sha256(source.encode()).hexdigest(),
            "lean_module": "Protocol",
            "vector_generator": "formal/ProtocolProbe.lean",
            "vectors": len(rows),
            "compiled_input_cases": len(rows),
            "cases": len(rows) + sdk["cases"],
            "unique_budget_inputs": len({tuple(row) for row in actual_inputs}),
            "source_sha256s": {path: before[path]
                               for path in [str(SUBJECT.relative_to(ROOT)), *sdk["source_paths"]]},
            "checked_sources_sha256": before,
            "model_build": "fresh temporary oleans compiled from the checked sources",
            "sdk": sdk,
            "columns": ["maximum", "reserve", "usage_data", "usage_control", "lease_data",
                        "lease_control", "privileged", "target", "accepted", "next_usage_data",
                        "next_usage_control", "next_lease_data", "next_lease_control"],
            "drop_conservation_checked": True,
            "negative_controls_detected": detected + sdk["negative_controls"],
            "negative_controls": dict.fromkeys(detected + sdk["negative_controls"], "rejected"),
            "extracted_production_functions": ["Lease::capacity", "Lease::grow", "Lease::drop"] + sdk["extracted_production_functions"],
            "support_stubs": ["sequential std::sync::Mutex adapter for parking_lot::Mutex"] + sdk["support_stubs"],
            "limitations": ["sequential critical sections", "finite vectors including 64-bit boundaries",
                            "std mutex adapter; no scheduler or allocator refinement"],
        }
        return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lake", help="Lake executable; use Lean in the same toolchain directory")
    parser.add_argument("--lean", help="explicit Lean executable")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--node", default="node")
    parser.add_argument("--evidence", "--json", dest="evidence", type=Path,
                        help="write the checked evidence report")
    args = parser.parse_args()
    write_receipt(args.evidence, {"status": "running"})
    try:
        report = verify(args)
    except BaseException as problem:
        write_receipt(args.evidence, {"status": "failed", "error": str(problem)})
        raise
    write_receipt(args.evidence, report)
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
