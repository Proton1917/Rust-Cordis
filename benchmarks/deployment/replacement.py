"""Run prebuilt replacement_scaling binaries in alternating paired order."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import time
from datetime import datetime, timezone


TIMEOUT_SECONDS = 120
SCENARIOS = {(source, target) for source in (100, 1000, 10000) for target in (0, source)}
FIELDS = {
    "scenario", "source_instances", "target_instances", "round",
    "synchronous_replace_ns", "replace_to_settled_ns",
}


def utc_now():
    return datetime.now(timezone.utc).isoformat()


def binary_identity(path):
    path = path.resolve(strict=True)
    if not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError(f"not an executable file: {path}")
    digest = hashlib.sha256()
    with path.open("rb") as binary:
        for block in iter(lambda: binary.read(1024 * 1024), b""):
            digest.update(block)
    stat = path.stat()
    return {"path": str(path), "sha256": digest.hexdigest(),
            "size_bytes": stat.st_size, "mtime_ns": stat.st_mtime_ns}


def machine_identity():
    model = platform.processor()
    cpuinfo = Path("/proc/cpuinfo")
    if not model and cpuinfo.is_file():
        for line in cpuinfo.read_text(encoding="utf-8").splitlines():
            if line.startswith("model name"):
                model = line.partition(":")[2].strip()
                break
    affinity = sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None
    return {"os": platform.platform(), "architecture": platform.machine(),
            "cpu_model": model or None, "cpu_count": os.cpu_count(),
            "cpu_affinity": affinity, "python": platform.python_version()}


def parse_rows(stdout):
    if not re.search(r"^test result: ok\. 1 passed; 0 failed; 0 ignored;", stdout, re.MULTILINE):
        raise ValueError("test harness did not report exactly one successful test")
    rows = []
    seen = set()
    for number, line in enumerate(stdout.splitlines(), 1):
        start = line.find("{")
        if start < 0:
            continue
        prefix = line[:start].strip()
        if prefix not in ("", "test replacement_scaling ..."):
            raise ValueError(f"unexpected text before JSON on stdout line {number}")
        row = json.loads(line[start:])
        if not isinstance(row, dict) or set(row) != FIELDS:
            raise ValueError(f"unexpected JSON fields on stdout line {number}")
        if row["scenario"] != "replacement_scaling":
            raise ValueError(f"unexpected scenario on stdout line {number}")
        for field in FIELDS - {"scenario"}:
            if type(row[field]) is not int:
                raise ValueError(f"{field} must be an integer on stdout line {number}")
        key = row["source_instances"], row["target_instances"]
        if key not in SCENARIOS or key in seen or row["round"] != 0:
            raise ValueError(f"duplicate or unexpected scenario/round on stdout line {number}")
        if not 0 < row["synchronous_replace_ns"] <= row["replace_to_settled_ns"]:
            raise ValueError(f"invalid timing windows on stdout line {number}")
        seen.add(key)
        rows.append(row)
    if len(rows) != 6 or seen != SCENARIOS:
        raise ValueError(f"expected all six scenarios, received {len(rows)} rows")
    return rows


def count(value):
    parsed = int(value)
    if parsed < 0:
        raise argparse.ArgumentTypeError("pair counts must be nonnegative")
    return parsed


def schedule(pairs, warmup_pairs):
    sequence = 0
    for warmup, total in ((True, warmup_pairs), (False, pairs)):
        for pair in range(1, total + 1):
            order = ("baseline", "candidate") if sequence % 2 == 0 else ("candidate", "baseline")
            sequence += 1
            for build in order:
                yield warmup, pair, build


def resume(work, builds, pairs, warmup_pairs):
    metadata = json.loads((work / "metadata.json").read_text(encoding="utf-8"))
    if (metadata["pairs"], metadata["warmup_pairs"]) != (pairs, warmup_pairs):
        raise ValueError("resume pair counts differ from the original run")
    if metadata["environment_override"] != {"CORDIS_REPLACEMENT_ROUNDS": "1"}:
        raise ValueError("resume requires the original single-round environment")
    for build, identity in builds.items():
        if metadata["builds"][build]["sha256"] != identity["sha256"]:
            raise ValueError(f"{build} executable SHA256 differs from the original run")
    planned = list(schedule(pairs, warmup_pairs))
    completed = {}
    accepted_logs = set()
    for run in metadata["runs"]:
        key = run["warmup_pair"], run["pair"], run["build"]
        if type(key[0]) is not bool or type(key[1]) is not int or key not in planned:
            raise ValueError(f"unexpected run identity: {key}")
        if Path(run["stdout"]).name != run["stdout"]:
            raise ValueError("stdout log must be a filename in the output directory")
        eligible = (run["status"] in {"passed", "running", "interrupted"}
                    and run.get("previous_status") not in {"failed", "timeout"}
                    and run.get("returncode") in (None, 0))
        try:
            if not eligible:
                raise ValueError("previous process did not succeed")
            rows = parse_rows((work / run["stdout"]).read_text(encoding="utf-8"))
        except (OSError, ValueError):
            if run["status"] == "passed":
                raise ValueError(f"successful run has invalid or missing stdout: {run['stdout']}") from None
            continue
        if key in completed:
            raise ValueError(f"multiple successful attempts for {key}; preserve and review the evidence")
        completed[key] = rows
        accepted_logs.add(run["stdout"])
    kept = [key for key in planned if key in completed]
    if kept != planned[:len(kept)]:
        raise ValueError("successful runs are not a prefix of the original execution order")
    canonical = [dict(row, build=build, pair=pair, warmup_pair=warmup)
                 for warmup, pair, build in kept for row in completed[warmup, pair, build]]
    samples = work / "samples.jsonl"
    original = samples.read_bytes()
    lines = original.splitlines(keepends=True)
    for number, line in enumerate(lines):
        try:
            row = json.loads(line)
        except (ValueError, UnicodeError):
            if number == len(lines) - 1 and not line.endswith(b"\n"):
                break
            raise ValueError(f"invalid existing JSONL line {number + 1}") from None
        if number >= len(canonical) or row != canonical[number]:
            raise ValueError(f"JSONL line {number + 1} differs from validated stdout")
    recovered = "".join(json.dumps(row) + "\n" for row in canonical).encode("utf-8")
    if recovered != original:
        attempt = 1
        while (work / f"samples.before-resume-{attempt:03d}.jsonl").exists():
            attempt += 1
        with (work / f"samples.before-resume-{attempt:03d}.jsonl").open("xb") as backup:
            backup.write(original)
        temporary = work / "samples.jsonl.tmp"
        temporary.write_bytes(recovered)
        temporary.replace(samples)
    for run in metadata["runs"]:
        if run["stdout"] in accepted_logs and run["status"] != "passed":
            # A harness success footer and six exact rows survive interruption
            # between process completion, JSONL append and metadata commit.
            run["recovered_from_stdout"] = True
            run["status"], run["rows"] = "passed", 6
        elif run["stdout"] not in accepted_logs and run["status"] != "interrupted":
            run["previous_status"] = run["status"]
            run["status"] = "interrupted"
    metadata.setdefault("resumptions", []).append({
        "started_utc": utc_now(), "previous_status": metadata["status"],
        "previous_error": metadata.pop("error", None), "kept_runs": len(kept),
        "machine": machine_identity(), "builds": builds,
    })
    metadata["status"] = "running"
    metadata.pop("finished_utc", None)
    return metadata, set(completed)


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("baseline_executable", type=Path)
    cli.add_argument("candidate_executable", type=Path)
    cli.add_argument("output_directory", type=Path)
    cli.add_argument("--pairs", type=count, default=5)
    cli.add_argument("--warmup-pairs", type=count, default=1)
    cli.add_argument("--resume", action="store_true",
                     help="continue verified results after the previous runner has stopped")
    args = cli.parse_args()
    if args.pairs == 0:
        cli.error("--pairs must be positive")
    try:
        builds = {name: binary_identity(path) for name, path in (
            ("baseline", args.baseline_executable), ("candidate", args.candidate_executable)
        )}
        work = args.output_directory.resolve()
        if args.resume:
            metadata, completed = resume(work, builds, args.pairs, args.warmup_pairs)
        else:
            work.mkdir(parents=True, exist_ok=False)
            metadata = {"started_utc": utc_now(), "status": "running", "machine": machine_identity(),
                        "builds": builds, "pairs": args.pairs, "warmup_pairs": args.warmup_pairs,
                        "process_timeout_seconds": TIMEOUT_SECONDS,
                        "environment_override": {"CORDIS_REPLACEMENT_ROUNDS": "1"}, "runs": []}
            completed = set()
    except (OSError, ValueError, KeyError, TypeError) as problem:
        print(f"replacement benchmark: {problem}", file=sys.stderr)
        return 1

    def save_metadata():
        temporary = work / "metadata.json.tmp"
        temporary.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
        temporary.replace(work / "metadata.json")

    environment = {**os.environ, "CORDIS_REPLACEMENT_ROUNDS": "1"}
    save_metadata()
    try:
        with (work / "samples.jsonl").open("a" if args.resume else "x", encoding="utf-8") as samples:
            for warmup, pair, build in schedule(args.pairs, args.warmup_pairs):
                if (warmup, pair, build) in completed:
                    continue
                label = f"{'warmup' if warmup else 'measured'}-{pair:03d}-{build}"
                attempt = 1 + sum((run["warmup_pair"], run["pair"], run["build"]) ==
                                  (warmup, pair, build) for run in metadata["runs"])
                stem = label if attempt == 1 else f"{label}.attempt-{attempt:03d}"
                while any((work / f"{stem}.{stream}.log").exists() for stream in ("stdout", "stderr")):
                    attempt += 1
                    stem = f"{label}.attempt-{attempt:03d}"
                command = [builds[build]["path"], "--ignored", "--nocapture", "--test-threads=1"]
                stdout = work / f"{stem}.stdout.log"
                stderr = work / f"{stem}.stderr.log"
                run = {"build": build, "pair": pair, "warmup_pair": warmup,
                       "attempt": attempt,
                       "started_utc": utc_now(), "command": command, "status": "running",
                       "stdout": stdout.name, "stderr": stderr.name}
                metadata["runs"].append(run)
                save_metadata()
                print(f"running {label}", file=sys.stderr, flush=True)
                started = time.monotonic()
                try:
                    with stdout.open("xb") as out, stderr.open("xb") as err:
                        process = subprocess.run(command, cwd=work, env=environment,
                                                 stdout=out, stderr=err, check=False,
                                                 timeout=TIMEOUT_SECONDS)
                    run["returncode"] = process.returncode
                    if process.returncode != 0:
                        raise RuntimeError(f"process exited with {process.returncode}")
                    rows = parse_rows(stdout.read_text(encoding="utf-8"))
                    for row in rows:
                        print(json.dumps({**row, "build": build, "pair": pair,
                                          "warmup_pair": warmup}), file=samples)
                    samples.flush()
                    run["status"] = "passed"
                    run["rows"] = len(rows)
                except KeyboardInterrupt:
                    run["status"] = "interrupted"
                    raise
                except Exception as problem:
                    run["status"] = "timeout" if isinstance(problem, subprocess.TimeoutExpired) else "failed"
                    run["error"] = str(problem)
                    raise RuntimeError(f"{label}: {problem}; logs: {stdout}, {stderr}") from problem
                finally:
                    run["elapsed_seconds"] = time.monotonic() - started
                    save_metadata()
        for build, original in builds.items():
            if binary_identity(Path(original["path"]))["sha256"] != original["sha256"]:
                raise RuntimeError(f"{build} executable changed during measurement")
        metadata["status"] = "passed"
        print(work / "samples.jsonl")
        return 0
    except KeyboardInterrupt:
        metadata["status"] = "interrupted"
        print(f"replacement benchmark interrupted; results: {work}", file=sys.stderr)
        return 130
    except Exception as problem:
        metadata["status"] = "failed"
        metadata["error"] = str(problem)
        print(f"replacement benchmark: {problem}", file=sys.stderr)
        return 1
    finally:
        metadata["finished_utc"] = utc_now()
        save_metadata()


if __name__ == "__main__":
    raise SystemExit(main())
