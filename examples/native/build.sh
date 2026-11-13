#!/usr/bin/env bash
set -euo pipefail

deployment=$(cd "$(dirname "$0")" && pwd)
workspace=$(cd "$deployment/../.." && pwd)
build_root="${CARGO_TARGET_DIR:-$workspace/target}"
case "$build_root" in
    /*) ;;
    *) build_root="$workspace/$build_root" ;;
esac
build="$build_root/shared"
cargo build --manifest-path "$workspace/Cargo.toml" --target-dir "$build" --release \
    -p cordis-runtime -p cordis-host -p kv-file -p counter -p console-log
mkdir -p "$deployment/plugins"
case $(uname -s) in
    Darwin) extension=dylib ;;
    Linux) extension=so ;;
esac
cp "$build/release/cordis" "$deployment/cordis"
cp "$build/release/cordis-host" "$deployment/cordis-host"
for plugin in kv_file counter console_log; do
    cp "$build/release/lib$plugin.$extension" "$deployment/plugins/"
done
