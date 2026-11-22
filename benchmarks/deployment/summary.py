import argparse
from collections import defaultdict
import json
from pathlib import Path
from statistics import median


def measurements(row):
    if "calls_per_second" in row:
        yield "calls_per_second", row["calls_per_second"]
        yield "p99_ns", row["p99_ns"]
    if "start_to_verified_ns" in row:
        yield "start_to_verified_ns", row["start_to_verified_ns"]
    hosts = row["hosts"]
    if isinstance(hosts, dict):
        hosts = list(hosts.values())
    for group, processes in (("core", [row["core"]]), ("hosts", hosts)):
        if not processes:
            continue
        for field in processes[0]:
            if field.startswith("peak_") and len(processes) > 1:
                # 各进程的峰值出现时点独立，分别列出具名宿主。
                for name, host in row["hosts"].items() if isinstance(row["hosts"], dict) else ():
                    yield f"{name}.{field}", host[field]
            else:
                yield f"{group}.{field}", sum(p[field] for p in processes)


def summarize(file):
    pairs = defaultdict(dict)
    with file.open() as raw:
        for line in raw:
            row = json.loads(line)
            if row["pair"] == 0:
                continue
            for metric, value in measurements(row):
                key = row["scenario"], metric
                group = pairs[key].setdefault(row["pair"], {})
                if row["build"] in group:
                    raise ValueError(f"{key} 第 {row['pair']} 对的 {row['build']} 样本重复")
                group[row["build"]] = value
    results = {}
    for (scenario, metric), records in sorted(pairs.items()):
        if records.keys() != set(range(1, 11)) or any(r.keys() != {"baseline", "candidate"} for r in records.values()):
            raise ValueError(f"{scenario} 的 {metric} 需要十组完整配对数据")
        old = [r["baseline"] for r in records.values()]
        new = [r["candidate"] for r in records.values()]
        ratios = sorted(b / a for a, b in zip(old, new))
        mid, lo, hi = median(ratios), ratios[1], ratios[-2]
        stats = {"baseline_median": median(old), "candidate_median": median(new),
                 "paired_ratio_median": mid, "ratio_mad": median(abs(r - mid) for r in ratios),
                 "median_interval": [lo, hi]}
        if metric in {"calls_per_second", "p99_ns", "start_to_verified_ns"}:
            if hi - lo > 0.05:
                stats["five_percent_gate"] = "inconclusive"
            elif metric == "calls_per_second":
                stats["five_percent_gate"] = "within" if lo >= 0.95 else "regressed" if hi < 0.95 else "inconclusive"
            else:
                stats["five_percent_gate"] = "within" if hi <= 1.05 else "regressed" if lo > 1.05 else "inconclusive"
        results.setdefault(scenario, {})[metric] = stats
    return results


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("samples", type=Path)
    args = parser.parse_args()
    print(json.dumps(summarize(args.samples), indent=2))
