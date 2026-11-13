//! DOC-15 §5.1：平台模板共享运行参数，恢复重启节流为一秒。

use super::ServicePlatform;
use std::{collections::BTreeMap, path::Path};

pub(super) fn render(
    platform: ServicePlatform,
    name: &str,
    binary: &Path,
    configuration: &Path,
    project: &Path,
    environment: &BTreeMap<String, String>,
) -> String {
    match platform {
        ServicePlatform::Launchd => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>run</string><string>--config</string><string>{}</string></array>\n<key>WorkingDirectory</key><string>{}</string>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n<key>ThrottleInterval</key><integer>1</integer>\n<key>StandardOutPath</key><string>{}</string>\n<key>StandardErrorPath</key><string>{}</string>\n{}</dict></plist>\n",
            xml(name),
            xml_path(binary),
            xml_path(configuration),
            xml_path(project),
            xml_path(&project.join("service/stdout.log")),
            xml_path(&project.join("service/stderr.log")),
            plist_environment(environment),
        ),
        ServicePlatform::Systemd => format!(
            "[Unit]\nDescription=Cordis {}\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart={} run --config {}\nRestart=always\nRestartSec=1\nKillMode=control-group\n{}\n[Install]\nWantedBy=default.target\n",
            name,
            unit(project),
            unit(binary),
            unit(configuration),
            unit_environment(environment),
        ),
    }
}

fn plist_environment(environment: &BTreeMap<String, String>) -> String {
    if environment.is_empty() {
        return String::new();
    }
    let mut output = String::from("<key>EnvironmentVariables</key><dict>");
    for (name, value) in environment {
        output.push_str(&format!(
            "<key>{}</key><string>{}</string>",
            xml(name),
            xml(value)
        ));
    }
    output.push_str("</dict>\n");
    output
}

fn unit_environment(environment: &BTreeMap<String, String>) -> String {
    environment
        .iter()
        .map(|(name, value)| {
            let value = value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t");
            format!("Environment=\"{name}={value}\"\n")
        })
        .collect()
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
