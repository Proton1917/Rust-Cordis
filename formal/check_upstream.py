#!/usr/bin/env python3
"""Compare pinned TypeScript method bodies with Lean lifecycle observations."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
PIN = "261c5fdae6edb54e7ed4f071024f530acd18903b"
SOURCE = ROOT / "cordis/packages/core/src/fiber.ts"
DOMAINS = {"idle": 8, "set_epoch": 64, "reload": 8, "unload": 2}


def run(command: list[str], cwd: Path = ROOT) -> str:
    result = subprocess.run(command, cwd=cwd, text=True, capture_output=True, timeout=120)
    if result.returncode:
        raise RuntimeError(f"command failed: {' '.join(command)}\n{result.stdout}{result.stderr}")
    return result.stdout


def method_body(source: str, signature: str) -> str:
    if source.count(signature) != 1:
        raise RuntimeError(f"upstream method signature is not unique: {signature}")
    opening = source.index("{", source.index(signature) + len(signature))
    depth, position, start = 1, opening + 1, opening + 1
    quote = None
    while position < len(source):
        char = source[position]
        if quote:
            if char == "\\":
                position += 2
                continue
            if char == quote:
                quote = None
        elif source.startswith("//", position):
            newline = source.find("\n", position)
            if newline < 0:
                break
            position = newline
        elif source.startswith("/*", position):
            ending = source.find("*/", position + 2)
            if ending < 0:
                raise RuntimeError("unterminated source comment")
            position = ending + 2
            continue
        elif char in "\"'`":
            quote = char
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[start:position]
        position += 1
    raise RuntimeError(f"unbalanced upstream method: {signature}")


def parse_vectors(output: str) -> dict[tuple[str, int], str]:
    result = {}
    for line in output.splitlines():
        kind, mask, value = line.split("\t")
        key = kind, int(mask)
        if kind not in DOMAINS or not 0 <= key[1] < DOMAINS[kind] or key in result:
            raise RuntimeError(f"unexpected upstream case: {line}")
        result[key] = value
    if len(result) != sum(DOMAINS.values()):
        raise RuntimeError("incomplete upstream observation domain")
    return result


HARNESS = r'''
const FiberState = Object.fromEntries(["Pending", "Loading", "Active", "Failed", "Disposed", "Unloading"].map(x => [x.toUpperCase(), x]));
const INACTIVE = "__INACTIVE__";
const AsyncFunction = Object.getPrototypeOf(async function(){}).constructor;
const getState = new Function("FiberState", "INACTIVE", methods.getState);
const setEpoch = new Function("epoch", "FiberState", "INACTIVE", methods.setEpoch);
const reload = new AsyncFunction("FiberState", "INACTIVE", methods.reload);
const unload = new AsyncFunction("FiberState", "INACTIVE", methods.unload);
const epochs = [INACTIVE, "0", "1:2", "9007199254740991:9007199254740990"];
const bit = (mask, field) => Boolean(mask & (1 << field));
function state(epoch) {
  const value = {
    _runner: {epoch}, _store: {}, provisions: [], _disposables: {clear: () => []},
    ctx: {logger: {error() {}}}, observation: "Keep",
    _reload() {return Promise.resolve()}, _unload() {return Promise.resolve()},
    _updateState(callback) {
      const next = callback();
      this.observation = next === FiberState.LOADING ? "Load" : next === FiberState.UNLOADING ? "Unload" : "Keep";
    }
  };
  return value;
}
async function main() {
  for (let mask = 0; mask < 8; mask++) {
    const value = state(bit(mask, 2) ? "0" : INACTIVE);
    value.uid = bit(mask, 0) ? 0 : null;
    value._error = bit(mask, 1) ? new Error("recorded") : undefined;
    console.log(`idle\t${mask}\t${getState.call(value, FiberState, INACTIVE)}`);
  }
  for (let mask = 0; mask < 64; mask++) {
    const old = (mask >>> 2) % 4, next = (mask >>> 4) % 4;
    const value = state(epochs[old]);
    value._error = bit(mask, 0) ? new Error("recorded") : undefined;
    value.inertia = bit(mask, 1) ? Promise.resolve() : undefined;
    setEpoch.call(value, epochs[next], FiberState, INACTIVE);
    console.log(`set_epoch\t${mask}\t${value.observation}:${epochs.indexOf(value._runner.epoch)}`);
  }
  for (let mask = 0; mask < 8; mask++) {
    const value = state("old");
    let calls = 0;
    let release;
    const gate = new Promise(resolve => {release = resolve;});
    value._execute = async () => {calls++; await gate; if (bit(mask, 1)) throw new Error("callback");};
    const task = reload.call(value, FiberState, INACTIVE);
    if (!bit(mask, 0)) value._runner.epoch = "next";
    await Promise.resolve();
    if (bit(mask, 2)) value._runner.epoch = "next";
    release();
    await task;
    const epoch = value._runner.epoch === INACTIVE ? "inactive" : value._runner.epoch;
    console.log(`reload\t${mask}\t${value.observation}:${epoch}:${calls}:${Boolean(value._error)}`);
  }
  for (let mask = 0; mask < 2; mask++) {
    const value = state(mask ? "0" : INACTIVE);
    await unload.call(value, FiberState, INACTIVE);
    console.log(`unload\t${mask}\t${value.observation}`);
  }
}
main().catch(error => {console.error(error); process.exitCode = 1;});
'''.replace("\\t", "\t")


def observe(node: str, methods: dict[str, str], temporary: Path) -> dict:
    script = temporary / "upstream.cjs"
    script.write_text("const methods = " + json.dumps(methods) + ";\n" + HARNESS)
    return parse_vectors(run([node, str(script)]))


def check(args: argparse.Namespace) -> dict:
    lake, node = shutil.which(args.lake), shutil.which(args.node)
    if not lake or not node:
        raise RuntimeError("the upstream bridge requires Lean 4.19.0 and Node.js")
    if not SOURCE.is_file():
        raise RuntimeError(f"fetch the pinned Cordis source {PIN} into cordis/ before verification")
    if run(["git", "rev-parse", "HEAD"], ROOT / "cordis").strip() != PIN:
        raise RuntimeError("upstream checkout differs from the conformance pin")
    run(["git", "diff", "--exit-code", PIN, "--", "packages/core/src/fiber.ts"], ROOT / "cordis")
    source = SOURCE.read_text()
    source_hash = hashlib.sha256(source.encode()).hexdigest()
    proof_inputs = [ROOT / "formal" / name for name in ("Lifecycle.lean", "Upstream.lean", "UpstreamVectors.lean", "lakefile.toml", "lake-manifest.json", "lean-toolchain", "check_upstream.py")]
    proof_before = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in proof_inputs}
    run([lake, "build", "Upstream"], ROOT / "formal")
    methods = {
        "getState": method_body(source, "private _getState()"),
        "setEpoch": method_body(source, "private _setEpoch(epoch: string)"),
        "reload": method_body(source, "private async _reload()"),
        "unload": method_body(source, "private async _unload()"),
    }
    # The only in-body TypeScript annotation in these four methods.
    if methods["unload"].count("new Promise<void>") != 1:
        raise RuntimeError("unexpected TypeScript syntax at the unload suspension boundary")
    methods["unload"] = methods["unload"].replace("new Promise<void>", "new Promise")
    expected = parse_vectors(run([lake, "env", "lean", "--run", "UpstreamVectors.lean"], ROOT / "formal"))
    with tempfile.TemporaryDirectory(prefix="cordis-upstream-") as raw:
        temporary = Path(raw)
        observed = observe(node, methods, temporary)
        if observed != expected:
            changed = [key for key in expected if expected[key] != observed[key]]
            raise RuntimeError(f"upstream lifecycle observations disagree: {changed}")
        if methods["setEpoch"].count("if (this.inertia) return") != 1:
            raise RuntimeError("upstream negative-control mutation site is missing")
        corrupted = dict(methods)
        corrupted["setEpoch"] = methods["setEpoch"].replace("if (this.inertia) return", "if (!this.inertia) return")
        if observe(node, corrupted, temporary) == expected:
            raise RuntimeError("upstream bridge accepted an inverted inertia guard")
        stale_guard = "if (this._runner.epoch === oldEpoch) {\n        this._error = reason"
        if methods["reload"].count(stale_guard) != 1:
            raise RuntimeError("upstream stale-failure mutation site is missing")
        corrupted = dict(methods)
        corrupted["reload"] = methods["reload"].replace(stale_guard, "if (true) {\n        this._error = reason")
        if observe(node, corrupted, temporary) == expected:
            raise RuntimeError("upstream bridge accepted an obsolete callback failure")
    if hashlib.sha256(SOURCE.read_bytes()).hexdigest() != source_hash:
        raise RuntimeError("upstream source changed during verification")
    proof_after = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in proof_inputs}
    if proof_after != proof_before:
        raise RuntimeError("upstream proof inputs changed during verification")
    return {
        "status": "passed", "upstream_commit": PIN, "source_sha256": source_hash,
        "proof_inputs_sha256": proof_after,
        "node": run([node, "--version"]).strip(), "compiled_input_cases": len(expected),
        "negative_controls": {"inverted_upstream_inertia_guard": "rejected", "stale_callback_failure_guard_removed": "rejected"},
        "extracted_production_functions": ["Fiber._getState", "Fiber._setEpoch", "Fiber._reload", "Fiber._unload"],
        "support_stubs": ["empty service/cleanup collections", "transition launch observation", "controlled callback completion/error", "logger sink"],
        "syntax_erasure": "new Promise<void> becomes new Promise; method signatures are supplied by Function/AsyncFunction",
        "scope": "shared lifecycle observations at source method boundaries, including a superseded awaited load; full JavaScript/Rust language semantics remain outside this bridge",
    }


def save(path: Path | None, value: dict) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, encoding="utf-8", delete=False) as stream:
        temporary = Path(stream.name)
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lake", default="lake")
    parser.add_argument("--node", default="node")
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()
    save(args.evidence, {"status": "running"})
    try:
        result = check(args)
    except (RuntimeError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        save(args.evidence, {"status": "failed", "error": str(error)})
        print(f"upstream source bridge failed: {error}")
        return 1
    save(args.evidence, result)
    print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
