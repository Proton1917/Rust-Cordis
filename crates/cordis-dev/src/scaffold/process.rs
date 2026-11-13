//! DOC-15 M18、ADR-0040：进程插件保存当前 SDK、依赖声明和可发现的部署入口。

use super::{Result, registration, write_new};
use std::{fs, path::Path};
use toml_edit::{Array, DocumentMut, Item, value};

mod typescript;
pub use typescript::typescript_plugin;

const MAIN: &str = include_str!("../../templates/python/main.py");
const REQUIREMENTS: &str = include_str!("../../../../sdks/python/requirements.txt");
const SDK: &[(&str, &str)] = &[
    (
        "__init__.py",
        include_str!("../../../../sdks/python/cordis_sdk/__init__.py"),
    ),
    (
        "bindings.py",
        include_str!("../../../../sdks/python/cordis_sdk/bindings.py"),
    ),
    (
        "context.py",
        include_str!("../../../../sdks/python/cordis_sdk/context.py"),
    ),
    (
        "errors.py",
        include_str!("../../../../sdks/python/cordis_sdk/errors.py"),
    ),
    (
        "runtime.py",
        include_str!("../../../../sdks/python/cordis_sdk/runtime.py"),
    ),
    (
        "wire.py",
        include_str!("../../../../sdks/python/cordis_sdk/wire.py"),
    ),
];

/// DOC-15 §12.1：部署入口指向项目内源目录，版本副本由运行时管理。
pub fn python_plugin(project: &Path, name: &str) -> Result<()> {
    create(project, name, write_python)
}

/// DOC-15 §12.1：各进程语言共用项目登记、Cargo 排除项与 watch 源目录。
fn create(project: &Path, name: &str, write: fn(&Path, &str) -> Result<()>) -> Result<()> {
    let project = project.canonicalize()?;
    let (configuration, addition, settings) = registration(&project, name, name.into())?;
    let cargo = project.join("Cargo.toml");
    let mut workspace: DocumentMut = fs::read_to_string(&cargo)?.parse()?;
    exclude(&mut workspace, name)?;
    let directory = project.join("plugins").join(name);
    let sources = settings
        .get("runtime")
        .and_then(|runtime| runtime.get("plugin_dir"))
        .and_then(toml::Value::as_str)
        .unwrap_or("plugins");
    let discoverable = project.join(sources).join(name);
    if discoverable != directory && fs::symlink_metadata(&discoverable).is_ok() {
        return Err(format!("部署入口已存在：{}", discoverable.display()).into());
    }
    write(&directory, name)?;
    if discoverable != directory {
        fs::create_dir_all(discoverable.parent().expect("部署入口父目录"))?;
        let relative = pathdiff::diff_paths(&directory, discoverable.parent().unwrap())
            .ok_or("插件目录与部署目录需要共用路径根")?;
        std::os::unix::fs::symlink(relative, &discoverable)?;
    }
    fs::write(cargo, workspace.to_string())?;
    register_watch(&configuration, name, &addition)
}

/// DOC-15 M18：启动代码与 SDK 一起复制，依赖安装位置归该插件目录所有。
fn write_python(directory: &Path, name: &str) -> Result<()> {
    fs::create_dir(directory)?;
    fs::create_dir(directory.join("cordis_sdk"))?;
    for (filename, source) in SDK {
        write_new(&directory.join("cordis_sdk").join(filename), source)?;
    }
    write_new(&directory.join("main.py"), &MAIN.replace("{{name}}", name))?;
    write_new(&directory.join("requirements.txt"), REQUIREMENTS)?;
    write_new(
        &directory.join(".gitignore"),
        ".dependencies/\n__pycache__/\n",
    )?;
    let manifest = format!(
        "name = {name:?}\nversion = \"0.1.0\"\nsocket = \"process\"\nprotocol = 1\ncommand = [\"python\", \"-B\", \"main.py\"]\n"
    );
    write_new(&directory.join("cordis-plugin.toml"), &manifest)
}

/// ADR-0040：watch 直接观察源目录，配置文件保持原有注释与独立实例内容。
fn register_watch(path: &Path, name: &str, addition: &str) -> Result<()> {
    let mut configuration: DocumentMut = fs::read_to_string(path)?.parse()?;
    if let Some(paths) = configuration
        .get_mut("instance")
        .and_then(|item| item.get_mut("watch"))
        .and_then(|item| item.get_mut("config"))
        .and_then(|item| item.get_mut("paths"))
        .and_then(Item::as_array_mut)
    {
        paths.push(format!("plugins/{name}"));
    }
    fs::write(path, format!("{configuration}{addition}"))?;
    Ok(())
}

/// ADR-0040：Cargo 成员模式保持原样，进程插件目录归 workspace.exclude。
fn exclude(workspace: &mut DocumentMut, name: &str) -> Result<()> {
    let table = workspace["workspace"]
        .as_table_mut()
        .ok_or("项目需要 Cargo workspace")?;
    let entry = table
        .entry("exclude")
        .or_insert_with(|| value(Array::new()));
    let entries = Item::as_array_mut(entry).ok_or("workspace.exclude 需要数组")?;
    let path = format!("plugins/{name}");
    if !entries
        .iter()
        .any(|entry| entry.as_str() == Some(path.as_str()))
    {
        entries.push(path);
    }
    Ok(())
}
