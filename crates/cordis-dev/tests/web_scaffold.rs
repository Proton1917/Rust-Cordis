//! DOC-15 §12.1：组合脚手架通过 Cargo 成员解析与真实宿主读取目录内动态库。

use cordis_testkit::cleanup::TestGuard;
use std::{fs, path::Path, process::Command};

fn checked(command: &mut Command) -> Vec<u8> {
    let result = command.output().expect("执行网页脚手架命令");
    assert!(
        result.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
    result.stdout
}

fn generate(root: &Path, project: &Path) {
    let dev = env!("CARGO_BIN_EXE_cordis-dev");
    checked(
        Command::new(dev)
            .arg("new")
            .arg(project)
            .arg("--sdk-path")
            .arg(root.join("crates/cordis-sdk")),
    );
    for (name, language) in [
        ("page", None),
        ("native-page", Some("--rust")),
        ("python-page", Some("--python")),
        ("node-page", Some("--ts")),
    ] {
        let mut command = Command::new(dev);
        command
            .args(["new-plugin", name, "--web", "--project"])
            .arg(project);
        if let Some(language) = language {
            command.arg(language);
        }
        checked(&mut command);
    }
}

/// DOC-15 §9：生成目录同时承载清单、资源与实际库，watch 使用此目录作为替换单位。
#[test]
fn web_variants_share_deployment_directory_and_native_build_output() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "web-scaffold-layout");
    let root = case.workspace();
    let project = case.directory("web-scaffold");
    generate(root, &project);
    checked(
        Command::new("sh")
            .current_dir(&project)
            .arg("plugins/native-page/build.sh"),
    );
    let configuration: toml::Table =
        toml::from_str(&fs::read_to_string(project.join("cordis.toml")).unwrap()).unwrap();
    for name in ["page", "native-page", "python-page", "node-page"] {
        let reference = configuration["instance"][name]["plugin"].as_str().unwrap();
        let deployed = project
            .join("target/debug")
            .join(reference)
            .canonicalize()
            .unwrap();
        let manifest: toml::Table =
            toml::from_str(&fs::read_to_string(deployed.join("cordis-plugin.toml")).unwrap())
                .unwrap();
        let assets = manifest["web"]["assets"].as_str().unwrap();
        let entry = manifest["web"]["entry"].as_str().unwrap();
        assert!(
            deployed.join(assets).join(entry).is_file(),
            "部署清单需要可读取的浏览器入口"
        );
        assert_eq!(deployed, project.join("plugins").join(name));
    }
    describe_native(root, &project, &case);
    case.success();
}

fn describe_native(root: &Path, project: &Path, case: &TestGuard) {
    let binaries = {
        let _lock = cordis_testkit::build::shared_build_lock(root).unwrap();
        checked(
            Command::new(env!("CARGO"))
                .current_dir(root)
                .args(["build", "-p", "cordis-host", "--target-dir"])
                .arg(cordis_testkit::build::shared_target_dir(root)),
        );
        cordis_testkit::build::snapshot_artifacts(
            case,
            "web-scaffold-host",
            "debug",
            &["cordis-host"],
        )
        .unwrap()
    };
    let plugin = project.join("plugins/native-page");
    let manifest: toml::Table =
        toml::from_str(&fs::read_to_string(plugin.join("cordis-plugin.toml")).unwrap()).unwrap();
    let library = plugin.join(manifest["library"].as_str().unwrap());
    let bytes = checked(
        Command::new(binaries.join("cordis-host"))
            .current_dir(project)
            .arg("--describe")
            .arg(library),
    );
    let description: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(description["name"], "native-page");
    assert_eq!(description["components"][0]["name"], "native-page");
    fs::write(project.join("native-description.json"), bytes).unwrap();
}
