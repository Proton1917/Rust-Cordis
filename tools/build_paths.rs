//! DOC-15 §12.2：发布构建从同一规则表生成路径映射，用户环境参数保持后置优先。

use std::{env, io, path::Path, process::Command};

/// ADR-0049：在 Cargo 构建边界合并共享映射与已有环境参数。
pub fn encoded_flags(project: &Path) -> io::Result<String> {
    let home = env::var("HOME").map_err(io::Error::other)?;
    let cargo =
        std::path::absolute(env::var("CARGO_HOME").unwrap_or_else(|_| format!("{home}/.cargo")))?;
    let output = Command::new(env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args(["--print", "sysroot"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("读取 Rust sysroot 失败"));
    }
    let sysroot = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let project = project.canonicalize()?;
    let definitions: Vec<[String; 2]> =
        serde_json::from_str(include_str!("path-remapping.json")).map_err(io::Error::other)?;
    let mut flags = Vec::new();
    for [source, destination] in definitions {
        if source == "flag" {
            flags.push(destination);
            continue;
        }
        let prefix = match source.as_str() {
            "home" => Path::new(&home),
            "cargo_home" => &cargo,
            "sysroot" => Path::new(sysroot.trim()),
            "project" => &project,
            _ => return Err(io::Error::other("路径映射规则来源无效")),
        };
        flags.push(format!(
            "--remap-path-prefix={}={destination}",
            prefix.display()
        ));
    }
    if let Ok(existing) = env::var("CARGO_ENCODED_RUSTFLAGS") {
        flags.extend(
            existing
                .split('\u{1f}')
                .filter(|flag| !flag.is_empty())
                .map(str::to_owned),
        );
    } else if let Ok(existing) = env::var("RUSTFLAGS") {
        flags.extend(existing.split_whitespace().map(str::to_owned));
    }
    Ok(flags.join("\u{1f}"))
}
