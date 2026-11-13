//! DOC-14 SK-3：组件、接口、提供方与契约套件模板使用同一当前 SDK 编译。

use cordis_testkit::cleanup::TestGuard;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
use toml::{Table, Value};

fn dependency(path: PathBuf) -> Value {
    Value::Table(Table::from_iter([(
        "path".into(),
        Value::String(path.to_string_lossy().into_owned()),
    )]))
}

fn render(skill: &Path, source: &str, name: &str) -> String {
    fs::read_to_string(skill.join("templates").join(source))
        .unwrap()
        .replace("{{name}}", name)
}

fn write_crate(project: &Path, directory: &str, manifest: &str, source: &str) {
    let directory = project.join(directory);
    fs::create_dir_all(directory.join("src")).unwrap();
    let parsed: Table = toml::from_str(manifest).unwrap();
    fs::write(
        directory.join("Cargo.toml"),
        toml::to_string(&parsed).unwrap(),
    )
    .unwrap();
    fs::write(directory.join("src/lib.rs"), source).unwrap();
}

/// SK-3：接口、提供方和契约共用当前 SDK 与契约接口。
fn workspace(root: &Path, project: &Path) {
    let mut workspace: Table = toml::from_str("[workspace]\nmembers=['interfaces/*','plugins/*','contracts/*']\nresolver='3'\n[workspace.dependencies]\n").unwrap();
    let dependencies = workspace["workspace"]["dependencies"]
        .as_table_mut()
        .unwrap();
    dependencies.insert(
        "cordis-sdk".into(),
        dependency(root.join("crates/cordis-sdk")),
    );
    dependencies.insert(
        "cordis-contract".into(),
        dependency(root.join("crates/cordis-contract")),
    );
    fs::write(
        project.join("Cargo.toml"),
        toml::to_string(&workspace).unwrap(),
    )
    .unwrap();
}

/// SK-3：所有源码均取自作者模板，生成与部署工作区一致的 crate 布局。
fn templates(skill: &Path, project: &Path) {
    for (directory, manifest, source) in [
        (
            "interfaces/skill-counter-api",
            "interface.toml",
            "interface.rs",
        ),
        ("plugins/skill-counter", "provider.toml", "provider.rs"),
        (
            "contracts/skill-counter-contract",
            "contract.toml",
            "contract.rs",
        ),
    ] {
        write_crate(
            project,
            directory,
            &render(skill, manifest, "skill-counter"),
            &render(skill, source, "skill-counter"),
        );
    }
    let mut manifest: Table =
        toml::from_str(&render(skill, "plugin.toml", "plain-plugin")).unwrap();
    manifest["package"]["name"] = Value::String("plain-plugin".into());
    write_crate(
        project,
        "plugins/plain-plugin",
        &toml::to_string(&manifest).unwrap(),
        &render(skill, "plugin.rs", "plain-plugin"),
    );
}

#[test]
fn shared_skill_templates_build_with_current_sdk() {
    let guard = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "skill-templates");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let skill = env::var_os("CORDIS_SKILL_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("skills/cordis-plugin"));
    let project = guard.directory("skill-examples");
    fs::create_dir_all(&project).unwrap();
    workspace(&root, &project);
    templates(&skill, &project);
    let _build = cordis_testkit::build::shared_build_lock(&root).unwrap();
    let output = Command::new(env!("CARGO"))
        .current_dir(&project)
        .args(["build", "--workspace", "--target-dir"])
        .arg(cordis_testkit::build::shared_target_dir(&root))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("SK-3模板构建：{}", guard.evidence_for(&project).display());
    guard.success();
}
