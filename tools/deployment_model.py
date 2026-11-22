import json
from dataclasses import dataclass, replace
from itertools import permutations


# 四个坐标依次为基础配置、覆盖、程序来源和实例检查点。0 与 1 表示两个部署版本。
@dataclass(frozen=True)
class State:
    committed: int = 0
    running: int = 0
    files: tuple = (0, 0, 0, 0)
    phase: str = "idle"
    written: int = 0


def restored(version):
    return State(version, version, (version,) * 4)


def successors(s):
    old, new = s.committed, 1 - s.committed
    if s.phase == "idle":
        yield "same_target", s
        yield "begin", replace(s, phase="prepared")
    elif s.phase == "prepared":
        yield "activation_fails", restored(old)
        yield "activate", replace(s, running=new, phase="candidate")
    elif s.phase == "candidate":
        yield "confirmation_fails", restored(old)
        yield "journal_synced", replace(s, phase="writing")
    elif s.phase == "writing":
        if s.written < len(s.files):
            files = list(s.files)
            files[s.written] = new
            yield "write_file", replace(s, files=tuple(files), written=s.written + 1)
            yield "write_fails", restored(old)
        else:
            yield "files_sync_fails", restored(old)
            yield "rename_fails", restored(old)
            yield "rename", replace(s, phase="renamed")
    elif s.phase == "renamed":
        yield "directory_synced", replace(s, committed=new, phase="retiring")
        yield "directory_sync_fails", replace(s, phase="uncertain")
    elif s.phase == "retiring":
        yield "cleanup", restored(s.committed)
        yield "cleanup_fails", restored(s.committed)

    # 恢复依据目录中的日志标记。改名至目录同步之间，两个持久结果都可能出现。
    if s.phase in {"renamed", "uncertain"}:
        yield "recover_undo", restored(old)
        yield "recover_staging", restored(s.files[0])
    elif s.phase != "idle":
        yield "crash_recover", restored(s.committed)


def check(depth=20):
    frontier = {State()}
    visited = set(frontier)
    checked = set()
    last_depth = 0
    for length in range(1, depth + 1):
        following = set()
        for state in frontier:
            for event, after in successors(state):
                checked.add((state, event, after))
                if state.phase == "uncertain" and not event.startswith("recover_"):
                    raise AssertionError((state, event))
                if after.phase == "idle" and (
                    len(set(after.files)) != 1 or after.files[0] != after.running
                    or after.running != after.committed
                ):
                    raise AssertionError((state, event, after))
                if event not in {"directory_synced", "recover_staging"} and state.committed != after.committed:
                    raise AssertionError((state, event, after))
                if event == "directory_synced" and after.files != (after.committed,) * 4:
                    raise AssertionError((state, event, after))
                if event == "same_target" and state != after:
                    raise AssertionError((state, event, after))
                following.add(after)
        last_depth = length
        frontier = following - visited
        visited.update(following)
        if not frontier:
            break
    events = sorted({event for _, event, _ in checked})
    return {"versions": 2, "durable_fields": 4, "event_depth_limit": depth,
            "depth_explored": last_depth, "reached_fixed_point": not frontier,
            "reachable_states": len(visited), "transitions": len(checked), "events": events}


def graph_cases():
    # 边已经按作用域解析，方向为提供方到消费者或父实例到子实例。
    nodes = range(3)
    pairs = list(permutations(nodes, 2))
    cases = 0
    for graph in range(1 << len(pairs)):
        edges = {edge for bit, edge in enumerate(pairs) if graph & (1 << bit)}
        closed = [mask for mask in range(8)
                  if all(not mask & (1 << a) or mask & (1 << b) for a, b in edges)]
        for seed in range(8):
            least = 7
            for mask in closed:
                if mask & seed == seed:
                    least &= mask
            # 独立定义：从直接变化的实例出发，三节点图的简单路径至多含两条边。
            reachable = seed
            for a, b in edges:
                if seed & (1 << a):
                    reachable |= 1 << b
                    for c in nodes:
                        if (b, c) in edges:
                            reachable |= 1 << c
            if reachable != least:
                raise AssertionError((edges, seed, least, reachable))
            cases += 1
    return {"nodes": len(nodes), "edge_sets": 1 << len(pairs), "graph_seed_pairs": cases}


if __name__ == "__main__":
    print(json.dumps({"deployment": check(), "selection": graph_cases()}, indent=2))
