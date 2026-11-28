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
        ServicePlatform::WindowsTask => {
            render_windows_task(name, binary, configuration, project, environment)
        }
    }
}

fn render_windows_task(
    name: &str,
    binary: &Path,
    configuration: &Path,
    project: &Path,
    environment: &BTreeMap<String, String>,
) -> String {
    let strip = |path: &Path| -> String {
        let text = path.to_string_lossy();
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    };
    let project_str = strip(project);
    let binary_str = strip(binary);
    let config_str = strip(configuration);
    let stdout = strip(&project.join("service").join("stdout.log"));
    let stderr = strip(&project.join("service").join("stderr.log"));
    let (command, arguments) = if environment.is_empty() {
        // 直接把 cordis.exe 作为顶层任务进程，schtasks /End 终止核心时由其 Job Object 回收全部插件子进程。
        (binary_str, format!("run --config \"{config_str}\""))
    } else {
        let mut inner = String::new();
        for (key, value) in environment {
            inner.push_str(&format!(
                "set \"{}={}\" && ",
                cmd_escape(key),
                cmd_escape(value)
            ));
        }
        inner.push_str(&format!(
            "\"{binary_str}\" run --config \"{config_str}\" 1>>\"{stdout}\" 2>>\"{stderr}\""
        ));
        ("cmd.exe".to_owned(), format!("/d /c \"{inner}\""))
    };
    format!(
        "<?xml version=\"1.0\"?>\n<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n  <RegistrationInfo>\n    <Description>Cordis {}</Description>\n  </RegistrationInfo>\n  <Triggers>\n    <LogonTrigger>\n      <Enabled>true</Enabled>\n    </LogonTrigger>\n  </Triggers>\n  <Settings>\n    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n    <RestartOnFailure>\n      <Interval>PT1M</Interval>\n      <Count>999</Count>\n    </RestartOnFailure>\n  </Settings>\n  <Actions Context=\"Author\">\n    <Exec>\n      <Command>{}</Command>\n      <Arguments>{}</Arguments>\n      <WorkingDirectory>{}</WorkingDirectory>\n    </Exec>\n  </Actions>\n</Task>\n",
        xml(name),
        xml(&command),
        xml(&arguments),
        xml(&project_str),
    )
}

fn cmd_escape(value: &str) -> String {
    value.replace('"', "\"\"")
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
