import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import time
import tomllib

from build_paths import build_environment


ROOT = Path(__file__).resolve().parent.parent
PROGRAMS = {
    "cordis": "bin/cordis",
    "cordis-host": "bin/cordis-host",
    "cordis-dev": "bin/cordis-dev",
    "libcordis_http.dylib": "plugins/libcordis_http.dylib",
    "libcordis_watch.dylib": "plugins/libcordis_watch.dylib",
    "libcordis_secrets.dylib": "plugins/libcordis_secrets.dylib",
    "cordis-window": "plugins/window/cordis-window",
}
PACKAGES = ["cordis-runtime", "cordis-host", "cordis-dev", "cordis-http", "cordis-watch", "cordis-secrets", "cordis-window"]


def build(evidence):
    """ADR-0036：使用当前源码与共享编译目录，映射编译器保留的本机路径。"""
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    environment = os.environ.copy()
    environment.update(build_environment(ROOT))
    environment["CORDIS_SOURCE_REVISION"] = revision
    environment["TMPDIR"] = str(ROOT / "target/tmp")
    target = Path(environment.get("CARGO_TARGET_DIR", ROOT / "target")) / "shared"
    command = ["cargo", "build", "--locked", "--release", "--target-dir", str(target)]
    for package in PACKAGES:
        command.extend(["-p", package])
    started = time.monotonic()
    with (evidence / "build.log").open("w") as log:
        subprocess.run(command, cwd=ROOT, env=environment, stdout=log, stderr=subprocess.STDOUT, check=True)
    return target / "release", {"revision": revision, "build_seconds": time.monotonic() - started}


def stage(artifacts, package, version):
    """发布目录按显式清单复制程序，应用配置使用相对路径。"""
    for name, relative in PROGRAMS.items():
        destination = package / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(artifacts / name, destination)
    shutil.copy2(ROOT / "plugins/window/cordis-plugin.toml", package / "plugins/window/cordis-plugin.toml")
    shutil.copy2(ROOT / "LICENSE", package / "LICENSE")
    template = (ROOT / "tools/release/README.md").read_text()
    (package / "README.md").write_text(template.replace("{{version}}", version).replace("{{components}}", component_versions()))
    shutil.copy2(ROOT / "tools/release/cordis.toml", package / "cordis.toml")


def component_versions():
    """ADR-0047：发行组合保留各包独立版本，表格从对应包的版本声明生成。"""
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    typescript_version = json.loads((ROOT / "sdks/typescript/cordis-sdk/package.json").read_text())["version"]
    web_version = json.loads((ROOT / "sdks/typescript/cordis-web/package.json").read_text())["version"]
    rows = []
    for label, directory in [
        ("cordis", "crates/cordis-runtime"), ("cordis-host", "crates/cordis-host"),
        ("cordis-dev", "crates/cordis-dev"), ("Rust SDK", "crates/cordis-sdk"),
        ("cordis-types", "crates/cordis-types"),
        ("cordis-supervisor", "crates/cordis-supervisor"),
        ("cordis-routing", "crates/cordis-routing"),
        ("cordis-process", "crates/cordis-process"),
        ("cordis-testkit", "crates/cordis-testkit"),
        ("HTTP", "plugins/http"), ("watch", "plugins/watch"),
        ("window", "plugins/window"), ("secrets", "plugins/secrets"),
    ]:
        value = tomllib.loads((ROOT / directory / "Cargo.toml").read_text())["package"]["version"]
        version = workspace if isinstance(value, dict) else value
        if label == "HTTP":
            version += f"（内置网页 SDK {web_version}）"
        rows.append((label, version))
    rows.extend([
        ("Python SDK", tomllib.loads((ROOT / "sdks/python/pyproject.toml").read_text())["project"]["version"]),
        ("TypeScript SDK", typescript_version),
        ("网页宿主", web_version),
        ("ABI", "v1.2"), ("UP/1", "1"),
    ])
    return "| 组件 | 版本 |\n|---|---|\n" + "\n".join(f"| {name} | {version} |" for name, version in rows)


def inspect(package, evidence):
    """发布隐私约束：签名后的完整文件字节匹配 HOME 和用户名，命中时停止装包。"""
    records = []
    with (evidence / "signatures.log").open("w") as signatures:
        for relative in PROGRAMS.values():
            binary = package / relative
            subprocess.run(["codesign", "--force", "--sign", "-", str(binary)], stdout=signatures, stderr=signatures, check=True)
            subprocess.run(["codesign", "--verify", "--strict", str(binary)], stdout=signatures, stderr=signatures, check=True)
            contents = binary.read_bytes()
            username = subprocess.check_output(["id", "-un"], text=True).strip()
            counts = {"HOME": contents.count(os.environ["HOME"].encode()), "username": contents.count(username.encode())}
            (evidence / (binary.name + ".path-matches.txt")).write_text(json.dumps(counts) + "\n")
            if any(counts.values()):
                raise RuntimeError(f"路径信息检查失败：{relative}")
            architecture = subprocess.check_output(["lipo", "-archs", str(binary)], text=True).strip()
            if architecture != "arm64":
                raise RuntimeError(f"发布架构应为 arm64：{relative} 为 {architecture}")
            records.append({"file": relative, "architecture": architecture, "path_matches": 0, "signature": "valid"})
    return records


def archive_package(package, archive):
    """发布存档使用中性属主，成员范围来自已装配的发布目录。"""
    def neutral_owner(info):
        info.uid = info.gid = 0
        info.uname = info.gname = "root"
        info.pax_headers = {}
        return info

    with tarfile.open(archive, "w:gz") as output:
        output.add(package, arcname=package.name, filter=neutral_owner)


def main():
    parser = argparse.ArgumentParser(description="构建并装配 macOS arm64 发布包，保存路径检查与签名记录。")
    parser.add_argument("version")
    options = parser.parse_args()
    name = f"cordis-{options.version}-aarch64-apple-darwin"
    work = ROOT / "target/releases" / options.version
    evidence = work / "evidence"
    evidence.mkdir(parents=True, exist_ok=True)
    (ROOT / "target/tmp").mkdir(exist_ok=True)
    artifacts, result = build(evidence)
    package = work / "package" / name
    package.mkdir(parents=True)
    stage(artifacts, package, options.version)
    result["binaries"] = inspect(package, evidence)
    archive = work / (name + ".tar.gz")
    archive_package(package, archive)
    result.update({"version": options.version, "asset": archive.name, "bytes": archive.stat().st_size})
    (evidence / "package.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print(archive)


if __name__ == "__main__":
    main()
