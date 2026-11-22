import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import subprocess
import tarfile

from tree_sitter import Language, Parser
import tree_sitter_rust


SOURCE = {".c", ".cjs", ".css", ".html", ".js", ".mjs", ".py", ".rs", ".sh", ".ts", ".wit"}
GENERATED = {
    "plugins/http/assets/cordis-web.js",
    "plugins/http-echo-py/bindings.py", "plugins/http-echo-ts/src/bindings.ts",
    "plugins/kv-py/kv_py/bindings.py", "plugins/kv-ts/src/bindings.ts",
    "plugins/counter-py/counter_py/bindings.py", "examples/web/source/web-ui/kv.ts",
    "tests/dynamic-sdk/script/src/bindings.ts", "tests/typescript-sdk/plugin/src/bindings.ts",
    "docs/explainer/cordis-explainer.html",
}
CRATE_ROLES = {
    "cordis": "kernel",
    "cordis-runtime": "deployment", "cordis-supervisor": "deployment",
    "cordis-web-deployment": "deployment",
    "cordis-host": "native_execution", "cordis-abi": "native_execution",
    "cordis-process": "process_and_protocol", "cordis-up": "process_and_protocol",
    "cordis-channels": "process_and_protocol", "cordis-routing": "process_and_protocol",
    "cordis-sdk": "sdk_and_contracts", "cordis-contract": "sdk_and_contracts",
    "cordis-types": "sdk_and_contracts",
    "cordis-idl": "code_generation", "cordis-sdk-macros": "code_generation",
    "cordis-testkit": "test_infrastructure", "cordis-dev": "developer_tools",
}


def test_attribute(node):
    attr = node.named_children[0]
    name, *arguments = attr.named_children
    if name.text in {b"test", b"tokio::test"}:
        return True
    return name.text == b"cfg" and any(
        b"".join(arg.text.split()) in {b"(test)", b'(feature="test-support")'}
        for arg in arguments
    )


def test_rows(parser, source, path):
    root = parser.parse(source).root_node
    if root.has_error:
        raise ValueError(f"{path} 的 Rust 语法树包含错误")
    rows = set()
    # 外部属性修饰同一列表中的下一项；宏的 token_tree 是待展开的输入。
    def visit(parent):
        attributes = []
        for node in parent.named_children:
            if node.type == "attribute_item":
                attributes.append(node)
            elif node.type not in {"line_comment", "block_comment"}:
                if any(test_attribute(attr) for attr in attributes):
                    last = node.end_point.row + int(node.end_point.column > 0)
                    rows.update(range(attributes[0].start_point.row, last))
                elif node.type != "token_tree":
                    visit(node)
                attributes.clear()
    visit(root)
    return len(rows)


def package_role(path):
    parts = path.parts
    if parts[0] == "crates":
        return "/".join(parts[:2]), CRATE_ROLES.get(parts[1], "other_crates")
    if parts[0] == "plugins":
        return "/".join(parts[:2]), "plugins"
    if parts[0] == "sdks":
        depth = 3 if parts[1] == "typescript" and parts[2].startswith("cordis-") else 2
        return "/".join(parts[:depth]), "sdk_and_contracts"
    return (parts[0] if len(parts) > 1 else "repository",
            {"tools": "developer_tools", "benchmarks": "test_infrastructure",
             "tests": "test_infrastructure"}.get(parts[0], "project_material"))


def category(path):
    parts = path.parts
    if path.as_posix() in GENERATED:
        return "generated"
    if path.suffix == ".md" or parts[0] == "docs":
        return "documentation"
    if ({"tests", "fixtures", "fuzz", "benchmarks", "benches"}.intersection(parts)
        or parts[:2] == ("crates", "cordis-testkit")
        or path.as_posix() == "crates/cordis-abi/src/testing.rs"
        or path.name == "tests.rs"):
        return "test_and_benchmark"
    if parts[0] in {"examples", "skills"} or "examples" in parts:
        return "examples"
    if "templates" in parts:
        return "shipped_templates"
    if parts[0] == "tools" or path.name in {"build.rs", "build.mjs"}:
        return "build_and_tools"
    return "production"


def included(path):
    return path.suffix in SOURCE or path.suffix == ".md"


def tracked_files(root, revision):
    if revision:
        with subprocess.Popen(["git", "archive", "--format=tar", revision], cwd=root,
                              stdout=subprocess.PIPE) as archive:
            with tarfile.open(fileobj=archive.stdout, mode="r|") as members:
                for member in members:
                    path = Path(member.name)
                    if member.isfile() and included(path):
                        yield path, members.extractfile(member).read()
            if archive.wait():
                raise RuntimeError(f"git archive {revision} 失败")
        return
    names = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=root
    )
    for name in names.decode().split("\0"):
        path = Path(name)
        file = root / path
        if included(path) and not file.is_symlink() and file.is_file():
            yield path, file.read_bytes()


def measure(root, revision):
    packages, roles, total = defaultdict(Counter), defaultdict(Counter), Counter()
    parser = Parser(Language(tree_sitter_rust.language()))
    for path, source in tracked_files(root, revision):
        package, role = package_role(path)
        kind = category(path)
        lines = len(source.splitlines())
        tests = test_rows(parser, source, path) if kind in {"production", "build_and_tools"} and path.suffix == ".rs" else 0
        for counts in (packages[package], roles[role], total):
            counts[kind] += lines - tests
            if tests:
                counts["test_and_benchmark"] += tests
    implementation = sum(total[name] for name in ("production", "build_and_tools", "shipped_templates", "generated"))
    return {"scope": "source-physical-lines/v2", "revision": revision or "worktree",
            "by_package": packages, "by_responsibility": roles, "total": total,
            "implementation_including_tools_templates_generated": implementation}


if __name__ == "__main__":
    cli = argparse.ArgumentParser()
    cli.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    cli.add_argument("--revision", help="直接统计该 Git 提交，缺省读取当前工作树")
    args = cli.parse_args()
    print(json.dumps(measure(args.root, args.revision), ensure_ascii=False, indent=2, sort_keys=True))
