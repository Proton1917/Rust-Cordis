//! DOC-15 §5.1：标准安装位置通过纯路径检查验证，错误卸载使用工作区文件。

use super::*;

#[test]
fn persistent_installation_uses_standard_service_directories() {
    let home = Path::new("/Users/service-owner");
    assert_eq!(
        installation_path(
            ServicePlatform::Launchd,
            "com.example.demo",
            501,
            home,
            None
        ),
        home.join("Library/LaunchAgents/com.example.demo.plist")
    );
    assert_eq!(
        installation_path(ServicePlatform::Launchd, "com.example.demo", 0, home, None),
        Path::new("/Library/LaunchDaemons/com.example.demo.plist")
    );
    assert_eq!(
        installation_path(ServicePlatform::Systemd, "demo", 1000, home, None),
        home.join(".config/systemd/user/demo.service")
    );
    assert_eq!(
        installation_path(
            ServicePlatform::Systemd,
            "demo",
            1000,
            home,
            Some(Path::new("/config"))
        ),
        Path::new("/config/systemd/user/demo.service")
    );
}

#[test]
fn absent_service_uninstall_preserves_neighboring_configuration() {
    let guard = cordis_testkit::cleanup::TestGuard::for_package(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        "absent-service",
    );
    let directory = guard.directory("service-errors");
    fs::create_dir_all(&directory).unwrap();
    let other = directory.join("other.plist");
    fs::write(&other, b"existing service configuration").unwrap();
    let path = directory.join("absent.plist");
    let error = Location {
        platform: ServicePlatform::Launchd,
        uid: 501,
        path: path.clone(),
    }
    .uninstall()
    .unwrap_err();
    assert!(error.message.contains(&path.display().to_string()));
    assert_eq!(fs::read(&other).unwrap(), b"existing service configuration");
    guard.success();
}
