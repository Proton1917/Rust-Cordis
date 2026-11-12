//! ADR-0032：真实配置解码、过滤规则与终端文本格式的契约。

use crate::{Console, ConsoleConfig, format::Renderer};
use chrono::{DateTime, FixedOffset, TimeDelta};
use cordis_sdk::{Component, builtin::LogRecord};

fn configuration(json: &str) -> ConsoleConfig {
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let config = cordis_sdk::decode(&cordis_sdk::encode(&value).unwrap()).unwrap();
    Console::resolve(&config).unwrap();
    config
}

fn received() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-11-13T08:09:10+08:00").unwrap()
}

fn record(level: u32, instance: &str, message: &str) -> LogRecord {
    LogRecord {
        level,
        instance: instance.into(),
        message: message.into(),
    }
}

#[test]
fn default_time_prefix_and_multiline_layout_follow_console_contract() {
    let mut renderer = Renderer::new(configuration("{}"), false, 0);
    let output = renderer
        .render(&record(2, "worker", "first\nsecond"), received())
        .unwrap();
    assert_eq!(
        output,
        format!(
            "2026-11-13 08:09:10 [I] worker first\n{}second",
            " ".repeat(24)
        )
    );
}

#[test]
fn right_label_and_per_line_limits_preserve_upstream_spacing() {
    let config = configuration(
        r#"{"show_time":"","colors":false,"level":3,"max_length":5,"label":{"width":8,"margin":2,"align":"right"}}"#,
    );
    let mut renderer = Renderer::new(config, true, 0);
    let output = renderer
        .render(&record(3, "api", "abcdef\r\nshort\n💡more"), received())
        .unwrap();
    assert_eq!(
        output,
        format!(
            "     api  [D]  abcde...\n{}short\n{}💡mor...",
            " ".repeat(15),
            " ".repeat(15)
        )
    );
}

#[test]
fn exact_instance_levels_override_default_and_filtered_messages_keep_diff_origin() {
    let config = configuration(
        r#"{"show_time":"","show_diff":true,"level":0,"levels":{"default":2,"app":3,"fractional":2.5}}"#,
    );
    let start = received();
    let mut renderer = Renderer::new(config, false, start.timestamp_millis());
    assert_eq!(
        renderer
            .render(
                &record(3, "app", "first"),
                start + TimeDelta::milliseconds(100)
            )
            .unwrap(),
        "[D] app first +100ms"
    );
    assert!(
        renderer
            .render(
                &record(3, "app.child", "filtered"),
                start + TimeDelta::milliseconds(500)
            )
            .is_none()
    );
    assert!(
        renderer
            .render(
                &record(3, "fractional", "filtered"),
                start + TimeDelta::milliseconds(1000)
            )
            .is_none()
    );
    assert_eq!(
        renderer
            .render(
                &record(2, "other", "second"),
                start + TimeDelta::milliseconds(1600)
            )
            .unwrap(),
        "[I] other second +2s"
    );
}

#[test]
fn legacy_level_remains_the_fallback_threshold() {
    let mut renderer = Renderer::new(configuration(r#"{"show_time":"","level":1}"#), false, 0);
    assert_eq!(
        renderer
            .render(&record(1, "api", "warning"), received())
            .unwrap(),
        "[W] api warning"
    );
    assert!(
        renderer
            .render(&record(2, "api", "information"), received())
            .is_none()
    );
}

#[test]
fn explicit_color_depth_controls_label_palette_and_bold_style() {
    for (depth, label) in [
        (1, "\x1b[31mapi\x1b[0m"),
        (2, "\x1b[38;5;40;1mapi\x1b[0m"),
        (3, "\x1b[38;5;40;1mapi\x1b[0m"),
    ] {
        let config = configuration(&format!(r#"{{"show_time":"","colors":{depth}}}"#));
        let mut renderer = Renderer::new(config, false, 0);
        assert_eq!(
            renderer
                .render(&record(2, "api", "payload"), received())
                .unwrap(),
            format!("[I] {label} payload")
        );
    }
}

#[test]
fn unsupported_color_and_date_formats_fail_config_decoding() {
    for json in [
        r#"{"colors":true}"#,
        r#"{"colors":4}"#,
        r#"{"show_time":"%Q"}"#,
    ] {
        assert!(
            serde_json::from_str::<ConsoleConfig>(json).is_err(),
            "配置应在解码时返回错误：{json}"
        );
    }
}
