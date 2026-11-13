#!/bin/sh
set -eu
plugin=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project=$(CDPATH= cd -- "$plugin/../.." && pwd)
cargo build --manifest-path "$project/Cargo.toml" --target-dir "$project/target" -p '{{name}}'
cp "$project/target/debug/{{library}}" "$plugin/{{library}}"
