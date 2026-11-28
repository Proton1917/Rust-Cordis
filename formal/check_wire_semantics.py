#!/usr/bin/env python3
"""Reflect the real UP integer writer and exercise whole SDK request coroutines.

The restricted Rust grammar is a translation boundary. Generated Lean theorems
quantify over all u64 inputs; compiled Rust and async SDK cases are independent
finite checks. Every negative control must fail before evidence is published.
"""
from __future__ import annotations

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
FORMAL = ROOT / "formal"
RPC = ROOT / "crates/cordis-up/src/rpc.rs"


def run(argv, *, cwd=FORMAL, env=None, check=True):
    result = subprocess.run([str(x) for x in argv], cwd=cwd, env=env,
                            text=True, capture_output=True, timeout=180)
    if check and result.returncode:
        raise RuntimeError(f"{' '.join(map(str, argv))}\n{result.stdout}{result.stderr}")
    return result


def function(source, name):
    found = re.search(r"(?m)^fn " + re.escape(name) + r"\(", source)
    if not found:
        raise ValueError(f"missing production function {name}")
    begin = source.index("{", found.start())
    depth = 1
    end = begin + 1
    while depth and end < len(source):
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    if depth:
        raise ValueError("unbalanced production function")
    return source[found.start():end]


def reflect(source):
    width = function(source, "integer_width")
    update = function(source, "update_budget")
    compact = re.sub(r"\s+", "", width)
    match = re.fullmatch(r"fninteger_width\(value:u64\)->usize\{matchvalue\{"
                         r"(\d+)\.\.=(\d+)=>(\d+),"
                         r"(\d+)\.\.=(\d+)=>(\d+),"
                         r"(\d+)\.\.=(\d+)=>(\d+),"
                         r"(\d+)\.\.=(\d+)=>(\d+),_=>(\d+),\}\}", compact)
    if not match:
        raise ValueError("integer_width leaves the reviewed match grammar")
    numbers = list(map(int, match.groups()))
    arms = [numbers[i:i+3] for i in range(0, 12, 3)]
    previous = -1
    for lower, upper, _ in arms:
        if lower != previous + 1 or upper < lower:
            raise ValueError("width arms must be contiguous and ordered")
        previous = upper
    skeleton = re.sub(r"\s+", "", update)
    pattern = (
        r"fnupdate_budget\(bytes:&mutVec<u8>,offset:usize,value:u64\)\{"
        r"letwidth=integer_width\(value\);bytes.truncate\(offset\);matchwidth\{"
        r"1=>bytes.push\(valueasu8\),"
        r"2=>\{bytes.push\((\d+)\);bytes.push\(valueasu8\);\}"
        r"3=>\{bytes.push\((\d+)\);bytes.extend_from_slice\(&\(valueasu16\).to_be_bytes\(\)\);\}"
        r"5=>\{bytes.push\((\d+)\);bytes.extend_from_slice\(&\(valueasu32\).to_be_bytes\(\)\);\}"
        r"_=>\{bytes.push\((\d+)\);bytes.extend_from_slice\(&value.to_be_bytes\(\)\);\}\}"
        r"letlength=\(bytes.len\(\)-4\)asu32;bytes\[\.\.4\].copy_from_slice\(&length.to_be_bytes\(\)\);\}"
    )
    tags = re.fullmatch(pattern, skeleton)
    if not tags:
        raise ValueError("update_budget leaves the reviewed writer grammar")
    return width, update, arms, numbers[12], list(map(int, tags.groups()))


def certificate(source):
    _, _, arms, fallback, tags = reflect(source)
    arm_text = ", ".join(f"⟨{upper}, {width}⟩" for _, upper, width in arms)
    return f'''import WireSemantics
namespace SourceWireCertificate
open Cordis.WireSemantics
def sourceWidth (n : Nat) := evalWidth {fallback} [{arm_text}] n
def sourceEncode (n : Nat) : List Byte :=
  match sourceWidth n with
  | 1 => [octet n]
  | 2 => octet {tags[0]} :: [octet n]
  | 3 => octet {tags[1]} :: word16 n
  | 5 => octet {tags[2]} :: header n
  | _ => octet {tags[3]} :: word64 n
theorem width_matches_for_every_input (n : Nat) : sourceWidth n = integerWidth n := by rfl
theorem encoding_matches_for_every_input (n : Nat) : sourceEncode n = integerEncode n := by
  change encodeByWidth (integerWidth n) n = integerEncode n
  exact encoding_selected_by_width n
theorem actual_unsigned_roundtrip (n : Nat) (tail : List Byte)
    (bound : n < 18446744073709551616) :
    readUnsigned (sourceEncode n ++ tail) = some (n, tail) := by
  rw [encoding_matches_for_every_input]
  exact unsigned_integer_roundtrip n tail bound
#print axioms width_matches_for_every_input
#print axioms encoding_matches_for_every_input
#print axioms actual_unsigned_roundtrip
end SourceWireCertificate
'''


def allocator_program(source):
    if "next: AtomicU64::new(1)" not in source:
        raise ValueError("UP request frontier initialization changed")
    match = re.search(r"\.fetch_update\(Ordering::Relaxed,\s*Ordering::Relaxed,\s*\|id\|\s*id\.checked_add\((\d+)\)\)", source)
    if not match:
        raise ValueError("UP allocator left the checked atomic-increment grammar")
    return match.group(0), int(match.group(1))


def allocator_certificate(source):
    _, amount = allocator_program(source)
    return f'''import WireSemantics
namespace SourceRequestAllocator
open Cordis.WireSemantics
def sourceAllocate (maximum frontier : Nat) : Option (Nat × Nat) :=
  if frontier + {amount} ≤ maximum then some (frontier, frontier + {amount}) else none
theorem checked_allocator_matches (maximum frontier : Nat) :
    sourceAllocate maximum frontier = allocateId maximum frontier := by
  by_cases next : frontier < maximum
  · simp [sourceAllocate, allocateId, next, show frontier + 1 ≤ maximum by omega]
  · simp [sourceAllocate, allocateId, next, show ¬frontier + 1 ≤ maximum by omega]
#print axioms checked_allocator_matches
end SourceRequestAllocator
'''


def allocator_probe(source, temp, rustc):
    expression, _ = allocator_program(source)
    path = temp / "allocator.rs"
    path.write_text('use std::sync::atomic::{AtomicU64, Ordering};\nfn main(){\n' +
        'for initial in [1_u64,u64::MAX-2,u64::MAX-1,u64::MAX]{\n' +
        'let frontier=AtomicU64::new(initial);\nlet actual=frontier' + expression + ';\n' +
        '''if initial==u64::MAX {assert!(actual.is_err());assert_eq!(frontier.load(Ordering::Relaxed),initial);}
        else {assert_eq!(actual,Ok(initial));assert_eq!(frontier.load(Ordering::Relaxed),initial+1);}
        }}''')
    binary = temp / "allocator"
    run([rustc, "--edition=2024", path, "-o", binary])
    run([binary])
    return 4


def integer_bytes(value):
    if value <= 23:
        return bytes([value])
    for maximum, tag, width in [(255, 24, 1), (65535, 25, 2), (2**32-1, 26, 4), (2**64-1, 27, 8)]:
        if value <= maximum:
            return bytes([tag]) + value.to_bytes(width, "big")
    raise ValueError(value)


PYTHON_HARNESS = r'''
import asyncio, json
from cordis_sdk.wire import Connection, _invoke_deadline
from cordis_sdk.errors import CordisError
class Payload:
    def __init__(self): self.closed = 0
    def close(self): self.closed += 1
class Runtime:
    def __init__(self): self.payloads=[]; self.released=0
    def payload(self, value):
        p=Payload(); self.payloads.append(p); return p
    def release_exports(self, offers): self.released += len(offers)
    def close(self): pass
class Writer:
    def write(self, data): pass
    async def drain(self): pass
    def close(self): pass
async def scenario(kind):
    rt=Runtime(); connection=Connection(rt); connection.writer=Writer()
    ready=asyncio.Event(); gate=asyncio.Event()
    async def send(*args, **kwargs):
        ready.set()
        if kind=='send_failure':
            await gate.wait(); raise OSError('injected transport failure')
    connection.send=send
    task=asyncio.create_task(connection.request('invoke', {}, [91]))
    await ready.wait()
    if kind=='late':
        task.cancel()
        try: await task
        except asyncio.CancelledError: pass
        connection._response({'id':1,'result':{'ok':None}})
        returned=0
    else:
        connection._response({'id':1,'result':{'ok':None}})
        if kind=='cancel': task.cancel()
        if kind=='send_failure': gate.set()
        try: returned=int(await task is rt.payloads[0])
        except (asyncio.CancelledError, OSError): returned=0
    return [kind,returned,sum(p.closed for p in rt.payloads),len(connection.pending),rt.released]
async def main():
    print(json.dumps([await scenario(k) for k in ['return','cancel','send_failure','late']]))
asyncio.run(main())
'''


def python_probe(temp):
    environment = dict(os.environ, PYTHONPATH=str(ROOT / "sdks/python"))
    harness = temp / "python_request.py"
    harness.write_text(PYTHON_HARNESS)
    actual = json.loads(run(["python3", harness], env=environment).stdout)
    expected = [["return", 1, 0, 0, 0], ["cancel", 0, 1, 0, 0],
                ["send_failure", 0, 1, 0, 1], ["late", 0, 1, 0, 0]]
    if actual != expected:
        raise AssertionError((actual, expected))
    tree = ast.parse((ROOT / "sdks/python/cordis_sdk/wire.py").read_text())
    cls = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "Connection")
    request = next(n for n in cls.body if isinstance(n, ast.AsyncFunctionDef) and n.name == "request")
    removed = 0
    for node in ast.walk(request):
        if isinstance(node, ast.Try):
            for child in list(node.finalbody):
                if isinstance(child, ast.If) and "future.result().close()" in ast.unparse(child):
                    node.finalbody.remove(child); removed += 1
    if removed != 1:
        raise AssertionError("request ownership negative control did not remove exactly one guard")
    # Compile the complete source method, with only the receiving-owner cleanup removed.
    injection = ast.unparse(request) + "\nConnection.request = request\n"
    negative = PYTHON_HARNESS.replace("async def scenario(kind):", injection + "\nasync def scenario(kind):")
    harness.write_text(negative)
    mutant = json.loads(run(["python3", harness], env=environment).stdout)
    if mutant == expected or mutant[1][2] != 0:
        raise AssertionError("payload leak mutation survived")
    return {"cases": actual, "negative_control": "settled payload cleanup removal exposes leak"}


TYPESCRIPT_HARNESS = r'''
const fs = require('node:fs');
const vm = require('node:vm');
const {stripTypeScriptTypes} = require('node:module');
let activeSignal;
class CordisError extends Error {constructor(code,message,detail){super(message);this.code=code;this.detail=detail;}}
const source = fs.readFileSync(process.argv[2], 'utf8').replace(/^import .*;\r?\n/gm, '').replace(/^export /gm, '');
const context = {Map,WeakMap,Uint8Array,DataView,AbortController,performance,setTimeout,clearTimeout,
    CordisError,fields:value=>value,identifier:value=>value,checkMessage:()=>{},setSizeLimits:()=>{},
    currentSignal:()=>activeSignal,supportsInvocationBudget:()=>true,
    Decoder:class {},Encoder:class {encode(){return new Uint8Array([1]);}}};
vm.runInNewContext(stripTypeScriptTypes(source,{mode:'strip'})+'\nglobalThis.Subject = Connection;',context);
async function scenario(kind){
    const payloads=[];let released=0,writes=0,firstCallback;
    const controller=new AbortController();activeSignal=controller.signal;
    if(kind==='pre_cancelled')controller.abort(new CordisError(7,'cancel'));
    let ready;const started=new Promise(resolve=>ready=resolve);
    const runtime={releaseExports(offers){released+=offers.length;},close(){},
        payload(){const payload={closed:0,close(){this.closed++;}};payloads.push(payload);return payload;}};
    const output={write(bytes,callback){writes++;if(writes===1){firstCallback=callback;ready();}else callback();}};
    const subject=new context.Subject(runtime,{destroy(){}},output,{timeoutMs:kind==='timeout'?0:10000});
    const promise=subject.request('invoke',{},[91n]).then(payload=>({payload}),error=>({error}));
    if(kind==='pre_cancelled'){}
    else if(kind==='early_cancel'){controller.abort(new CordisError(7,'cancel'));}
    else if(kind==='timeout'){}
    else {
        await started;
        if(kind==='cancel'){controller.abort(new CordisError(7,'cancel'));firstCallback();}
        else{
            subject.response({id:1n,result:{ok:null}});
            if(kind==='response_then_abort')controller.abort(new CordisError(7,'cancel'));
            firstCallback(kind==='send_failure'?new Error('injected write failure'):undefined);
        }
    }
    const result=await promise;
    if(['cancel','early_cancel','timeout'].includes(kind))subject.response({id:1n,result:{ok:null}});
    return [kind,Number(result.payload===payloads[0]&&!!result.payload),payloads.reduce((a,p)=>a+p.closed,0),subject.pending.size,released];
}
(async()=>{const result=[];for(const kind of ['return','send_failure','cancel','early_cancel','timeout','response_then_abort','pre_cancelled'])result.push(await scenario(kind));console.log(JSON.stringify(result));})().catch(error=>{console.error(error);process.exit(1);});
'''


def typescript_probe(temp, node):
    source = (ROOT / "sdks/typescript/cordis-sdk/src/wire.ts").read_text()
    original = temp / "wire.ts"
    harness = temp / "typescript_request.cjs"
    original.write_text(source)
    harness.write_text(TYPESCRIPT_HARNESS)
    actual = json.loads(run([node, harness, original]).stdout)
    expected = [["return", 1, 0, 0, 0], ["send_failure", 0, 1, 0, 1],
                ["cancel", 0, 1, 0, 0], ["early_cancel", 0, 1, 0, 1],
                ["timeout", 0, 1, 0, 1], ["response_then_abort", 1, 0, 0, 0],
                ["pre_cancelled", 0, 0, 0, 1]]
    if actual != expected:
        raise AssertionError((actual, expected))
    if source.count("received?.close();") != 1:
        raise AssertionError("TypeScript receiving-owner negative-control site changed")
    original.write_text(source.replace("received?.close();", ""))
    mutant = json.loads(run([node, harness, original]).stdout)
    if mutant == expected or mutant[1][2] != 0:
        raise AssertionError("TypeScript received-payload leak mutation survived")
    return {"cases": actual, "negative_control": "received.close removal exposes failed-send payload leak",
            "executed": "complete Connection class after TypeScript's native type erasure",
            "adapters": ["CBOR encode placeholder", "message validation", "payload and export counters", "transport callback"]}


def compiled_writer(source, temp, rustc):
    width, update, _, _, _ = reflect(source)
    values = {0,1,22,23,24,25,254,255,256,257,65534,65535,65536,65537,
              2**32-2,2**32-1,2**32,2**32+1,2**63-1,2**63,2**64-2,2**64-1}
    seed = 0xA6F73C25790DF128
    for _ in range(1024):
        seed = (seed * 6364136223846793005 + 1442695040888963407) % 2**64
        values.add(seed)
    program = temp / "writer.rs"
    program.write_text(width + "\n" + update + '\nfn main() {\n' +
        'let values: &[u64] = &[' + ','.join(map(str, sorted(values))) + '];\n' +
        '''for &value in values { for count in 0..9 {
        let mut bytes = vec![0; 4]; bytes.extend(std::iter::repeat_n(0x81, count));
        bytes.extend_from_slice(&[27,255,255,255,255,255,255,255,255]);
        update_budget(&mut bytes, 4+count, value);
        print!("{value}:{count}:{}:", integer_width(value));
        for byte in bytes { print!("{byte:02x}"); } println!();
        }} }''')
    binary = temp / "writer"
    run([rustc, "--edition=2024", program, "-o", binary])
    lines = run([binary]).stdout.splitlines()
    for line in lines:
        value, count, width, encoded = line.split(":")
        value, count, width = int(value), int(count), int(width)
        integer = integer_bytes(value)
        body = b"\x81" * count + integer
        if bytes.fromhex(encoded) != len(body).to_bytes(4, "big") + body or width != len(integer):
            raise AssertionError(line)
    if len(lines) != len(values) * 9:
        raise AssertionError("compiled probe omitted cases")
    return len(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--lake", default="lake")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--node", default="node")
    parser.add_argument("--evidence", type=Path, default=ROOT / "target/wire-semantics.json")
    args = parser.parse_args()
    paths = [RPC, *[FORMAL / (name + ".lean") for name in
             ["Protocol", "SdkProtocol", "WireSemantics", "RequestSemantics"]],
             ROOT / "sdks/python/cordis_sdk/wire.py", ROOT / "sdks/python/cordis_sdk/budget.py",
             ROOT / "sdks/python/cordis_sdk/errors.py", ROOT / "sdks/typescript/cordis-sdk/src/wire.ts",
             Path(__file__).resolve()]
    snapshots = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}
    evidence = {"schema": "cordis-wire-semantics/v1", "status": "running", "source_sha256": snapshots}
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    def publish():
        temporary = args.evidence.with_suffix(".tmp")
        temporary.write_text(json.dumps(evidence, indent=2) + "\n")
        temporary.replace(args.evidence)
    publish()
    try:
        source = RPC.read_text()
        for module in ["Protocol", "SdkProtocol", "WireSemantics", "RequestSemantics"]:
            run([args.lake, "env", "lean", "-o", f".lake/build/lib/lean/{module}.olean", f"{module}.lean"])
        with tempfile.TemporaryDirectory(prefix="cordis-wire-") as directory:
            temp = Path(directory)
            cert = temp / "SourceWireCertificate.lean"
            cert.write_text(certificate(source))
            verified = run([args.lake, "env", "lean", cert])
            for changed in [source.replace("0..=23 => 1", "0..=23 => 2", 1),
                            source.replace("bytes.push(24)", "bytes.push(25)", 1)]:
                if changed == source:
                    raise AssertionError("source mutation missed")
                cert.write_text(certificate(changed))
                if run([args.lake, "env", "lean", cert], check=False).returncode == 0:
                    raise AssertionError("source certificate mutation survived")
            cert.write_text(allocator_certificate(source))
            allocator_verified = run([args.lake, "env", "lean", cert])
            cert.write_text(allocator_certificate(source.replace("|id| id.checked_add(1)", "|id| id.checked_add(2)", 1)))
            if run([args.lake, "env", "lean", cert], check=False).returncode == 0:
                raise AssertionError("allocator increment mutation survived all-input certificate")
            evidence["source_certificate"] = verified.stdout.strip()
            evidence["allocator_certificate"] = allocator_verified.stdout.strip()
            evidence["compiled_writer_cases"] = compiled_writer(source, temp, args.rustc)
            evidence["compiled_allocator_cases"] = allocator_probe(source, temp, args.rustc)
            evidence["python_request"] = python_probe(temp)
            evidence["typescript_request"] = typescript_probe(temp, args.node)
        for relative, digest in snapshots.items():
            if hashlib.sha256((ROOT / relative).read_bytes()).hexdigest() != digest:
                raise AssertionError(f"source changed during verification: {relative}")
        evidence["compiled_input_cases"] = evidence["compiled_writer_cases"] + evidence["compiled_allocator_cases"] + len(evidence["python_request"]["cases"]) + len(evidence["typescript_request"]["cases"])
        evidence["negative_controls"] = {
            "rust_integer_width_change": "rejected", "rust_cbor_unsigned_tag_change": "rejected",
            "rust_checked_allocator_increment_change": "rejected",
            "python_received_payload_cleanup_removal": "rejected",
            "typescript_received_payload_cleanup_removal": "rejected"}
        evidence["trust_boundary"] = [
            "closed-grammar source extractor and Lean parser/compiler",
            "Rust Vec operations, integer casts and compiled toolchain",
            "asyncio scheduling and payload adapters for finite whole-method executions",
            "full CBOR dependency parsing is verified separately from the integer writer"]
        evidence["status"] = "passed"
        publish()
        print(json.dumps({k: v for k, v in evidence.items() if k != "source_sha256"}, indent=2))
    except Exception as error:
        evidence.update(status="failed", error=str(error))
        publish()
        raise


if __name__ == "__main__":
    main()
