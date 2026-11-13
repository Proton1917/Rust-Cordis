//! DOC-15 M19、ADR-0040：TypeScript 插件随源码包含 SDK，统一编译并交付普通文件。

use super::{Result, create, write_new};
use std::{fs, path::Path};

const SDK: &[(&str, &str)] = &[
    (
        "index.ts",
        include_str!("../../../../../sdks/typescript/cordis-sdk/src/index.ts"),
    ),
    (
        "types.ts",
        include_str!("../../../../../sdks/typescript/cordis-sdk/src/types.ts"),
    ),
    (
        "budget.ts",
        include_str!("../../../../../sdks/typescript/cordis-sdk/src/budget.ts"),
    ),
    (
        "wire.ts",
        include_str!("../../../../../sdks/typescript/cordis-sdk/src/wire.ts"),
    ),
    (
        "context.ts",
        include_str!("../../../../../sdks/typescript/cordis-sdk/src/context.ts"),
    ),
    (
        "runtime.ts",
        include_str!("../../../../../sdks/typescript/cordis-sdk/src/runtime.ts"),
    ),
    (
        "node.ts",
        include_str!("../../../../../sdks/typescript/cordis-sdk/src/node.ts"),
    ),
    (
        "bindings.ts",
        include_str!("../../../../../sdks/typescript/cordis-sdk/src/bindings.ts"),
    ),
    (
        "cbor-x.d.ts",
        include_str!("../../../../../sdks/typescript/cordis-sdk/src/cbor-x.d.ts"),
    ),
];

pub fn typescript_plugin(project: &Path, name: &str) -> Result<()> {
    create(project, name, write_typescript)
}

/// ADR-0040：模板入口与 SDK 共用 src，依赖安装保持普通文件，构建产物进入 dist。
fn write_typescript(directory: &Path, name: &str) -> Result<()> {
    fs::create_dir(directory)?;
    fs::create_dir_all(directory.join("src/cordis_sdk"))?;
    for (filename, source) in SDK {
        write_new(&directory.join("src/cordis_sdk").join(filename), source)?;
    }
    for (path, source) in [
        (
            "src/main.ts",
            include_str!("../../../templates/typescript/main.ts"),
        ),
        (
            "package.json",
            include_str!("../../../templates/typescript/package.json"),
        ),
        (
            "tsconfig.json",
            include_str!("../../../templates/typescript/tsconfig.json"),
        ),
        (
            "cordis-plugin.toml",
            include_str!("../../../templates/typescript/cordis-plugin.toml"),
        ),
    ] {
        write_new(&directory.join(path), &source.replace("{{name}}", name))?;
    }
    write_new(&directory.join(".npmrc"), "bin-links=false\n")?;
    write_new(&directory.join(".gitignore"), "node_modules/\ndist/\n")
}
