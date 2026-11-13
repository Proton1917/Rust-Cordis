import json
import tarfile
import time
from urllib.request import Request, urlopen


DISTRIBUTIONS = (
    {
        "name": "python",
        "version": "3.12.16",
        "archive": "cpython-3.12.16+20261110-aarch64-apple-darwin-install_only_stripped.tar.gz",
        "root": "python",
        "bytes": 25_237_581,
        "url": "https://github.com/astral-sh/python-build-standalone/releases/download/20261110/cpython-3.12.16%2B20261110-aarch64-apple-darwin-install_only_stripped.tar.gz",
    },
    {
        "name": "node",
        "version": "22.23.5",
        "archive": "node-v22.23.5-darwin-arm64.tar.xz",
        "root": "node-v22.23.5-darwin-arm64",
        "bytes": 25_919_840,
        "url": "https://nodejs.org/download/release/v22.23.5/node-v22.23.5-darwin-arm64.tar.xz",
    },
)


def download(distribution, destination):
    """AU-12：固定来源单次获取，读取量受发行字节数约束，超时保留失败现场。"""
    deadline = time.monotonic() + 180
    received = 0
    request = Request(distribution["url"], headers={"User-Agent": "Cordis-runtime-acceptance"})
    with urlopen(request, timeout=30) as response, destination.open("wb") as output:
        while received <= distribution["bytes"]:
            if time.monotonic() > deadline:
                raise TimeoutError(f"运行时获取超时：{distribution['name']}")
            chunk = response.read(min(1024 * 1024, distribution["bytes"] + 1 - received))
            if not chunk:
                break
            output.write(chunk)
            received += len(chunk)
    if received != distribution["bytes"]:
        raise ValueError(f"运行时归档长度与固定发行记录不同：{distribution['name']}，实际{received}")
    return received


def acquire(workspace, evidence):
    """AU-12：成熟tar解包保留目录内链接及许可证，完整来源作为工作区缓存发布。"""
    cache = workspace / "target/pack-runtime-inputs/python-3.12.16-node-22.23.5"
    if cache.is_dir():
        return cache, {"source": "workspace-cache"}
    prepared = workspace / "target/phase3-acceptance/m21/runtimes"
    if (prepared / "python/bin/python3").is_file() and (prepared / "node/bin/node").is_file():
        return prepared, {"source": "verified-preparation"}
    staging = evidence / "runtime-inputs"
    staging.mkdir()
    sources = []
    for distribution in DISTRIBUTIONS:
        archive = staging / distribution["archive"]
        received = download(distribution, archive)
        with tarfile.open(archive, "r:*") as bundle:
            bundle.extractall(staging, filter="data")
        source = staging / distribution["root"]
        destination = staging / distribution["name"]
        if source != destination:
            source.rename(destination)
        archive.unlink()
        sources.append({"name": distribution["name"], "version": distribution["version"],
                        "url": distribution["url"], "bytes": received})
    cache.parent.mkdir(parents=True, exist_ok=True)
    staging.rename(cache)
    return cache, {"source": "official-distributions", "archives": sources}


def prepare(options):
    """项目规则9：显式运行时根由调用方提供，默认来源在当前工作区准备并缓存。"""
    if options.runtimes is not None:
        root, observation = options.runtimes, {"source": "explicit"}
    else:
        root, observation = acquire(options.workspace, options.evidence)
    observation["root"] = str(root)
    (options.evidence / "runtime-inputs.json").write_text(
        json.dumps(observation, ensure_ascii=False, indent=2) + "\n"
    )
    return root
