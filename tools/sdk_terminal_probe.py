"""Source-body differential support for protocol_storage_probe.py.

Only synchronous response/claim methods are extracted. Asyncio supplies the real
Future.done/set_result behavior; Node supplies the real Map. Payload accounting,
error construction, parsed-message validation and explicit cleanup are harness
adapters. The probe neither decodes CBOR nor schedules a transport.
"""

from __future__ import annotations

import ast
import asyncio
from pathlib import Path
import subprocess
import textwrap


PYTHON_PATH = "sdks/python/cordis_sdk/wire.py"
TYPESCRIPT_PATH = "sdks/typescript/cordis-sdk/src/wire.ts"


def python_response(source: str) -> str:
    module = ast.parse(source)
    connection, = [item for item in module.body if isinstance(item, ast.ClassDef) and item.name == "Connection"]
    response, = [item for item in connection.body if isinstance(item, ast.FunctionDef) and item.name == "_response"]
    # Include its original indentation on the first line before dedenting.
    return textwrap.dedent("\n".join(source.splitlines()[response.lineno - 1:response.end_lineno]))


class Error(Exception):
    def __init__(self, code, message, detail=None):
        super().__init__(message)


def run_python(method: str, vectors: str) -> str:
    namespace = {"CordisError": Error}
    exec(compile(method, PYTHON_PATH, "exec"), namespace)
    loop = asyncio.new_event_loop()
    output = []
    try:
        for row in vectors.splitlines():
            style, events, *_ = row.split("\t")
            if style != "1":
                continue
            counters = {"settlements": 0, "payloads": 0, "accepted": 0, "closed": 0}

            class Payload:
                def __init__(self):
                    counters["payloads"] += 1
                    self.closed = False

                def close(self):
                    assert not self.closed, "one payload must not close twice"
                    self.closed = True
                    counters["closed"] += 1

            class Future(asyncio.Future):
                def set_result(self, value):
                    counters["settlements"] += 1
                    counters["accepted"] += 1
                    super().set_result(value)

                def set_exception(self, problem):
                    counters["settlements"] += 1
                    super().set_exception(problem)

            class Runtime:
                def payload(self, value):
                    return Payload()

            class Subject:
                _response = namespace["_response"]

            future = Future(loop=loop)
            subject = Subject()
            subject.pending = {1: future}
            subject.runtime = Runtime()
            try:
                for event in events if events != "-" else "":
                    if event == "c":
                        subject.pending.pop(1, None)
                    else:
                        result = {"ok": None} if event == "o" else {"error": {"code": 2, "message": "fixture"}}
                        subject._response({"id": 1, "result": result})
                values = [int(1 in subject.pending), int(future.done()), counters["settlements"],
                          counters["payloads"], counters["accepted"], counters["closed"]]
                output.append("\t".join([style, events, *map(str, values)]))
            finally:
                if future.done() and not future.cancelled():
                    future.exception()
    finally:
        loop.close()
    return "\n".join(output) + "\n"


NODE_PRELUDE = r"""
const fs = require('node:fs');
const identifier = value => value;
const fields = value => value;
class CordisError extends Error { constructor(code, message, detail) { super(message); } }
class Subject {
"""

NODE_HARNESS = r"""
}
const output = [];
for (const row of fs.readFileSync(0, 'utf8').trim().split('\n')) {
    const [style, events] = row.split('\t');
    if (style !== '0') continue;
    const counters = {done: false, settlements: 0, payloads: 0, accepted: 0, closed: 0};
    const subject = new Subject();
    subject.timeouts = 0;
    subject.limits = {consecutiveTimeouts: 1000000};
    subject.fail = () => {throw new Error('unexpected retirement in response-only trace');};
    subject.runtime = {payload() {
        counters.payloads++;
        let closed = false;
        return {close() {
            if (closed) throw new Error('one payload must not close twice');
            closed = true;
            counters.closed++;
        }};
    }};
    subject.pending = new Map([[1, {
        ownsDeadline: true,
        resolve(_payload) { counters.done = true; counters.settlements++; counters.accepted++; },
        reject(_error) { counters.done = true; counters.settlements++; },
    }]]);
    for (const event of events === '-' ? '' : events) {
        if (event === 'c') subject.pending.delete(1);
        else subject.response({id: 1, result: event === 'o' ? {ok: null} : {error: {code: 2, message: 'fixture'}}});
    }
    output.push([style, events, Number(subject.pending.has(1)), Number(counters.done),
                 counters.settlements, counters.payloads, counters.accepted, counters.closed].join('\t'));
}
process.stdout.write(output.join('\n') + '\n');
"""


def check_sdk(root: Path, directory: Path, vectors: str, extract_block, node: str) -> dict:
    python_source = (root / PYTHON_PATH).read_text()
    typescript_source = (root / TYPESCRIPT_PATH).read_text()
    method = python_response(python_source)
    expected_python = "\n".join(row for row in vectors.splitlines() if row.startswith("1\t")) + "\n"
    if run_python(method, vectors) != expected_python:
        raise AssertionError("actual Python _response disagrees with Lean")

    bodies = []
    for marker, signature in (("private claimPending(id: Identifier, timedOut?: boolean): Pending | undefined {", "claimPending(id, timedOut)"),
                              ("private response(body: Fields): void {", "response(body)")):
        original = extract_block(typescript_source, marker)
        bodies.append(signature + " " + original[original.index("{"):])
    methods = "\n".join(bodies)
    expected_node = "\n".join(row for row in vectors.splitlines() if row.startswith("0\t")) + "\n"

    def run_node(name, body):
        script = directory / f"{name}.cjs"
        script.write_text(NODE_PRELUDE + body + NODE_HARNESS)
        return subprocess.run([node, str(script)], input=vectors, text=True, capture_output=True)

    result = run_node("sdk_typescript", methods)
    if result.returncode or result.stdout != expected_node:
        raise AssertionError(f"actual TypeScript response/claimPending disagrees with Lean: {result.stderr[:1000]}")

    old = "if future is None or future.done():"
    if method.count(old) != 1:
        raise AssertionError("Python terminal negative-control injection point changed")
    try:
        changed = run_python(method.replace(old, "if future is None:"), vectors)
    except asyncio.InvalidStateError:
        changed = None
    if changed == expected_python:
        raise AssertionError("Python removed future.done guard escaped detection")
    old = "this.pending.delete(id);"
    if methods.count(old) != 1:
        raise AssertionError("TypeScript terminal negative-control injection point changed")
    changed = run_node("sdk_typescript_mutant", methods.replace(old, ""))
    if changed.returncode == 0 and changed.stdout == expected_node:
        raise AssertionError("TypeScript removed claim deletion escaped detection")
    return {
        "cases": len(vectors.splitlines()),
        "negative_controls": ["python_missing_future_done_guard", "typescript_missing_pending_delete"],
        "extracted_production_functions": ["Connection._response", "Connection.response", "Connection.claimPending"],
        "support_stubs": ["payload lifetime counters", "parsed-message validator identity adapters",
                          "error constructor", "explicit map cleanup event", "TypeScript Promise settlement counters"],
        "runtime_primitives": ["asyncio.Future", "JavaScript Map"],
        "source_paths": [PYTHON_PATH, TYPESCRIPT_PATH],
    }
