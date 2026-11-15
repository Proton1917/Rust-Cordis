//! DOC-15 §12.2：Rust、C/C++ 与 Apple 调试映射共用路径规则，保留用户参数。

use std::{collections::BTreeMap, env, io, path::Path, process::Command};

#[derive(serde::Deserialize)]
struct Rules {
    rust_flags: Vec<String>,
    prefixes: Vec<[String; 2]>,
    native_flags: Vec<String>,
    apple_link_flags: Vec<String>,
}

fn sources(project: &Path) -> io::Result<BTreeMap<&'static str, String>> {
    let home = env::var("HOME").map_err(io::Error::other)?;
    let cargo =
        std::path::absolute(env::var("CARGO_HOME").unwrap_or_else(|_| format!("{home}/.cargo")))?;
    let output = Command::new(env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args(["--print", "sysroot"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("读取 Rust sysroot 失败"));
    }
    Ok(BTreeMap::from([
        ("home", home),
        ("cargo_home", cargo.to_string_lossy().into_owned()),
        (
            "sysroot",
            String::from_utf8(output.stdout)
                .map_err(io::Error::other)?
                .trim()
                .to_owned(),
        ),
        (
            "project",
            project.canonicalize()?.to_string_lossy().into_owned(),
        ),
    ]))
}

/// ADR-0049：在实际 Cargo 命令边界生成完整环境，目标平台决定 Apple 链接参数。
pub fn environment(project: &Path, target: Option<&str>) -> io::Result<BTreeMap<String, String>> {
    let rules: Rules =
        serde_json::from_str(include_str!("path-remapping.json")).map_err(io::Error::other)?;
    let sources = sources(project)?;
    let mut rust = rules.rust_flags;
    let mut native = Vec::new();
    for [source, destination] in rules.prefixes {
        let prefix = sources
            .get(source.as_str())
            .ok_or_else(|| io::Error::other("路径映射来源无效"))?;
        rust.push(format!("--remap-path-prefix={prefix}={destination}"));
        for flag in &rules.native_flags {
            native.push(flag.replace("{from}", prefix).replace("{to}", &destination));
        }
    }
    if target.map_or(cfg!(target_os = "macos"), |target| {
        target.contains("-apple-")
    }) {
        rust.extend(
            rules
                .apple_link_flags
                .iter()
                .map(|flag| flag.replace("{home}", &sources["home"])),
        );
    }
    if let Ok(existing) = env::var("CARGO_ENCODED_RUSTFLAGS") {
        rust.extend(
            existing
                .split('\u{1f}')
                .filter(|flag| !flag.is_empty())
                .map(str::to_owned),
        );
    } else if let Ok(existing) = env::var("RUSTFLAGS") {
        rust.extend(existing.split_whitespace().map(str::to_owned));
    }
    let mut result = native_environment(native)?;
    result.insert("CARGO_ENCODED_RUSTFLAGS".into(), rust.join("\u{1f}"));
    Ok(result)
}

/// cc 的文档化 FLAGS 分层保持原 token 语义，统一引号后支持含空格的目录。
fn native_environment(defaults: Vec<String>) -> io::Result<BTreeMap<String, String>> {
    let shell = env::var("CC_SHELL_ESCAPED_FLAGS")
        .is_ok_and(|value| !matches!(value.as_str(), "" | "0" | "false" | "no"));
    let mut flags = BTreeMap::from([
        ("CFLAGS".into(), defaults.clone()),
        ("CXXFLAGS".into(), defaults),
    ]);
    for (name, value) in env::vars_os() {
        let Some(name) = name.to_str() else {
            continue;
        };
        if !["CFLAGS", "CXXFLAGS", "ARFLAGS", "RANLIBFLAGS"]
            .iter()
            .any(|base| {
                name == *base
                    || name.starts_with(&format!("{base}_"))
                    || name.ends_with(&format!("_{base}"))
            })
        {
            continue;
        }
        let value = value
            .into_string()
            .map_err(|_| io::Error::other("编译参数需要 UTF-8"))?;
        let existing = if shell {
            shlex::split(&value).ok_or_else(|| io::Error::other("编译参数引号失配"))?
        } else {
            value.split_ascii_whitespace().map(str::to_owned).collect()
        };
        flags.entry(name.to_owned()).or_default().extend(existing);
    }
    let mut result = BTreeMap::new();
    for (name, values) in flags {
        result.insert(
            name,
            shlex::try_join(values.iter().map(String::as_str)).map_err(io::Error::other)?,
        );
    }
    result.insert("CC_SHELL_ESCAPED_FLAGS".into(), "1".into());
    Ok(result)
}
