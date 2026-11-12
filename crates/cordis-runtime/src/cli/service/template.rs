//! DOC-15 §5.1：平台模板共享运行参数，恢复重启节流为一秒。

use super::ServicePlatform;
use std::path::Path;

pub(super) fn render(
    platform: ServicePlatform,
    name: &str,
    binary: &Path,
    configuration: &Path,
    project: &Path,
) -> String {
    match platform {
        ServicePlatform::Launchd => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>run</string><string>--config</string><string>{}</string></array>\n<key>WorkingDirectory</key><string>{}</string>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n<key>ThrottleInterval</key><integer>1</integer>\n<key>StandardOutPath</key><string>{}</string>\n<key>StandardErrorPath</key><string>{}</string>\n</dict></plist>\n",
            xml(name),
            xml_path(binary),
            xml_path(configuration),
            xml_path(project),
            xml_path(&project.join("service/stdout.log")),
            xml_path(&project.join("service/stderr.log"))
        ),
        ServicePlatform::Systemd => format!(
            "[Unit]\nDescription=Cordis {}\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart={} run --config {}\nRestart=always\nRestartSec=1\nKillMode=control-group\n\n[Install]\nWantedBy=default.target\n",
            name,
            unit(project),
            unit(binary),
            unit(configuration)
        ),
    }
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn xml_path(path: &Path) -> String {
    xml(&path.to_string_lossy())
}
fn unit(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    )
}
