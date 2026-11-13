//! ADR-0046 §4：三语言 HTTP 模板共用 skill 源码与官方 WIT 定义。

use super::{
    Backend, Result, Table, Value, append_instance, library, process, registration, write_crate_at,
    write_new,
};
use std::{fs, path::Path};

const RUST: &str = include_str!("../../../../skills/cordis-plugin/templates/http.rs");
const PYTHON: &str = include_str!("../../../../skills/cordis-plugin/templates/http.py");
const TYPESCRIPT: &str = include_str!("../../../../skills/cordis-plugin/templates/http.ts");
const HTTP: &str = include_str!("../../../../plugins/http-api/wit/http.wit");
const STREAM: &str = include_str!("../../../../plugins/stream-api/wit/stream.wit");

pub fn http_plugin(project: &Path, name: &str, backend: Backend) -> Result<()> {
    match backend {
        Backend::Rust => rust(project, name),
        Backend::Python | Backend::TypeScript => {
            process::create_deployment(project, name, false, |directory, name| {
                let (language, output) = if matches!(backend, Backend::Python) {
                    process::python_sources(directory, name, PYTHON)?;
                    ("python", directory.join("http_bindings.py"))
                } else {
                    process::typescript_sources(directory, name, TYPESCRIPT)?;
                    ("ts", directory.join("src/http-bindings.ts"))
                };
                let definitions = directory.join("interfaces");
                fs::create_dir_all(definitions.join("deps"))?;
                write_new(&definitions.join("http.wit"), HTTP)?;
                write_new(&definitions.join("deps/stream.wit"), STREAM)?;
                let source = cordis_idl::generate(
                    &cordis_idl::Document::load(&definitions, &[])?,
                    language,
                )?;
                write_new(&output, &source)?;
                readme(directory, name, language)
            })
        }
    }
}

fn rust(project: &Path, name: &str) -> Result<()> {
    let project = project.canonicalize()?;
    let (configuration, addition, _) = registration(&project, name, library(name))?;
    let workspace: Table = toml::from_str(&fs::read_to_string(project.join("Cargo.toml"))?)?;
    let mut dependency = workspace["workspace"]["dependencies"]["cordis-sdk"]
        .as_table()
        .ok_or("cordis-sdk 依赖需要来源表")?
        .clone();
    dependency.remove("version");
    if let Some(path) = dependency.get("path").and_then(Value::as_str) {
        let sdk = Path::new(path);
        let root = sdk
            .parent()
            .and_then(Path::parent)
            .ok_or("本机 SDK 需要位于源码仓库中")?;
        dependency.insert(
            "path".into(),
            Value::String(
                root.join("plugins/http-api")
                    .canonicalize()?
                    .to_string_lossy()
                    .into_owned(),
            ),
        );
    }
    let directory = project.join("plugins").join(name);
    let mut dependencies = Table::new();
    dependencies.insert("http-api".into(), Value::Table(dependency));
    write_crate_at(
        &directory,
        name,
        &RUST.replace("{{name}}", name),
        false,
        dependencies,
    )?;
    readme(&directory, name, "rust")?;
    append_instance(&configuration, &addition)
}

fn readme(directory: &Path, name: &str, language: &str) -> Result<()> {
    let build = match language {
        "rust" => "在项目根目录运行 `cargo build --target-dir target`。",
        "python" => {
            "在插件目录安装依赖：`python -m pip install --target .dependencies -r requirements.txt`。"
        }
        _ => "在插件目录运行 `npm install`，然后运行 `npm run build`。",
    };
    write_new(
        &directory.join("README.md"),
        &format!(
            "# {name} HTTP 路由\n\n{build}\n\n在运行目录中配置 HTTP 提供方 `cordis.http@1.0`，然后运行 `cordis run`。本插件登记 `/{name}`，返回请求方法与路径。HTTP 插件重建后依赖会重新激活，卸下时效应自动撤回路由。\n"
        ),
    )
}
