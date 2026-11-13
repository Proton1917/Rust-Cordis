//! DOC-15 §11、§12.1：网页模板共享部署目录，前端组件与后端使用同一版本。

use super::{Result, library, process, write_crate_at, write_new};
use std::{fs, path::Path};

#[derive(Clone, Copy)]
pub enum Backend {
    Rust,
    Python,
    TypeScript,
}

/// DOC-15 §11.1：纯网页和三种组合模板共用清单、状态继承与清理示例。
pub fn web_plugin(project: &Path, name: &str, backend: Option<Backend>) -> Result<()> {
    match backend {
        Some(Backend::Python) => super::python_plugin(project, name)?,
        Some(Backend::TypeScript) => super::typescript_plugin(project, name)?,
        Some(Backend::Rust) => process::create_deployment(project, name, true, write_native)?,
        None => process::create_deployment(project, name, false, write_web_manifest)?,
    }
    let directory = project.join("plugins").join(name);
    let component = if backend.is_some() {
        format!("{name}/web")
    } else {
        name.into()
    };
    add_web(&directory, name, &component)?;
    let instructions = match backend {
        Some(Backend::Rust) => format!("运行 `sh plugins/{name}/build.sh` 编译后端并复制动态库。"),
        Some(Backend::Python) => format!(
            "运行 `python -m pip install --target plugins/{name}/.dependencies -r plugins/{name}/requirements.txt` 安装 Python 依赖。"
        ),
        Some(Backend::TypeScript) => format!(
            "运行 `npm --prefix plugins/{name} install` 和 `npm --prefix plugins/{name} run build` 安装依赖并编译后端。"
        ),
        None => "浏览器入口为普通 ES 模块，可直接编辑。".into(),
    };
    write_new(
        &directory.join("README.md"),
        &format!(
            "# {name}\n\n{instructions}\n\n网页由提供 `cordis.web@1.1` 的 HTTP 插件发布，其实例设置 `forwarder = true`。运行 `cordis run` 后打开该服务的入口，网页计数保存在当前页面的组件状态中，替换时自动继承。`watch` 观察本插件目录，修改 `web/index.js` 会提交替换。\n\n`cordis-plugin.toml` 的 `web.entry` 相对 `web.assets` 解析。后端与网页部分由同一清单登记，共用替换与回退事务。界面和事件监听器归组件效应管理。\n"
        ),
    )
}

fn write_web_manifest(directory: &Path, name: &str) -> Result<()> {
    fs::create_dir(directory)?;
    write_new(
        &directory.join("cordis-plugin.toml"),
        &format!("name = {name:?}\nversion = \"0.1.0\"\nsocket = \"web\"\n"),
    )
}

/// DOC-15 §9：原生清单引用目录内的库，构建脚本把 Cargo 产物放到此处。
fn write_native(directory: &Path, name: &str) -> Result<()> {
    write_crate_at(
        directory,
        name,
        &super::PLUGIN.replace("{{name}}", name),
        false,
    )?;
    write_new(
        &directory.join("cordis-plugin.toml"),
        &format!(
            "name = {name:?}\nversion = \"0.1.0\"\nsocket = \"native\"\nlibrary = {:?}\n",
            library(name)
        ),
    )?;
    let script = include_str!("../../templates/web/build.sh")
        .replace("{{name}}", name)
        .replace("{{library}}", &library(name));
    write_new(&directory.join("build.sh"), &script)?;
    write_new(&directory.join(".gitignore"), "*.dylib\n*.so\n")
}

/// DOC-15 §11.4：导出计数状态与 DOM 清理同时进入组件生命周期。
fn add_web(directory: &Path, name: &str, component: &str) -> Result<()> {
    let manifest = directory.join("cordis-plugin.toml");
    let mut configuration: toml_edit::DocumentMut = fs::read_to_string(&manifest)?.parse()?;
    configuration["web"]["assets"] = toml_edit::value("web");
    configuration["web"]["mount"] = toml_edit::value(format!("/{name}"));
    configuration["web"]["entry"] = toml_edit::value("index.js");
    fs::write(manifest, configuration.to_string())?;
    fs::create_dir(directory.join("web"))?;
    for (file, source) in [
        ("index.js", include_str!("../../templates/web/index.js")),
        ("style.css", include_str!("../../templates/web/style.css")),
        (
            "LICENSE.open-claude-ui",
            include_str!("../../templates/web/LICENSE.open-claude-ui"),
        ),
    ] {
        write_new(
            &directory.join("web").join(file),
            &source
                .replace("{{name}}", name)
                .replace("{{component}}", component),
        )?;
    }
    Ok(())
}
