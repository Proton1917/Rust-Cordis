//! UPSTREAM logger-console/shared.ts、core/logger.ts：等级过滤、标签、颜色和多行布局。

use crate::config::{Alignment, ConsoleConfig};
use chrono::{DateTime, FixedOffset};
use cordis_sdk::builtin::LogRecord;
use std::fmt::Write;

pub(super) struct Renderer {
    config: ConsoleConfig,
    colors: u8,
    timestamp: i64,
}

impl Renderer {
    pub fn new(config: ConsoleConfig, terminal: bool, timestamp: i64) -> Self {
        let colors = config.colors.map_or(u8::from(terminal), |value| value.0);
        Self {
            config,
            colors,
            timestamp,
        }
    }

    /// ADR-0032：按精确实例名过滤，时间和差值取接收时刻；过滤的消息保持原时间标记。
    pub fn render(
        &mut self,
        message: &LogRecord,
        received: DateTime<FixedOffset>,
    ) -> Option<String> {
        let target = self
            .config
            .levels
            .get(&message.instance)
            .or_else(|| self.config.levels.get("default"))
            .copied()
            .map(crate::config::LevelThreshold::number)
            .unwrap_or(self.config.level as f64);
        if target < message.level as f64 {
            return None;
        }
        let code = color_code(&message.instance, self.colors);
        let mut output = String::new();
        let indent = self.header(&mut output, message, &received, code);
        message_lines(
            &mut output,
            &message.message,
            self.config.max_length,
            indent,
        );
        let timestamp = received.timestamp_millis();
        if self.config.show_diff && self.timestamp != 0 {
            let difference = format!(" +{}", elapsed(timestamp - self.timestamp));
            color(&mut output, self.colors, code, &difference, false);
        }
        self.timestamp = timestamp;
        Some(output)
    }

    /// shared.ts render：写入时间、等级和标签，返回后续行的缩进长度。
    fn header(
        &self,
        output: &mut String,
        message: &LogRecord,
        received: &DateTime<FixedOffset>,
        code: u8,
    ) -> usize {
        let prefix = match message.level {
            0 => "[E]",
            1 => "[W]",
            2 => "[I]",
            _ => "[D]",
        };
        let space = " ".repeat(self.config.label.margin);
        let mut indent = 3 + self.config.label.margin;
        if !self.config.show_time.0.is_empty() {
            let time = received
                .format_with_items(self.config.show_time.0.iter())
                .to_string();
            indent += time.encode_utf16().count();
            color(output, self.colors, 8, &time, false);
        }
        let padding = self
            .config
            .label
            .width
            .saturating_sub(message.instance.encode_utf16().count());
        match self.config.label.align {
            Alignment::Right => {
                output.push_str(&" ".repeat(padding));
                color(output, self.colors, code, &message.instance, true);
                write!(output, "{space}{prefix}{space}").unwrap();
                indent += self.config.label.width + self.config.label.margin;
            }
            Alignment::Left => {
                write!(output, "{prefix}{space}").unwrap();
                color(output, self.colors, code, &message.instance, true);
                output.push_str(&" ".repeat(padding));
                output.push_str(&space);
            }
        }
        indent
    }
}

/// core/logger.ts Logger.format：逐行限制 UTF-16 长度，CRLF 作为一个换行符处理。
fn message_lines(output: &mut String, message: &str, maximum: usize, indent: usize) {
    let mut lines = message.split('\n').peekable();
    let mut spaces = None;
    let mut first = true;
    while let Some(line) = lines.next() {
        if !first {
            output.push('\n');
            output.push_str(spaces.get_or_insert_with(|| " ".repeat(indent)));
        }
        first = false;
        let line = if lines.peek().is_some() {
            line.strip_suffix('\r').unwrap_or(line)
        } else {
            line
        };
        clipped_line(output, line, maximum);
    }
}

fn clipped_line(output: &mut String, line: &str, maximum: usize) {
    if line.len() <= maximum {
        output.push_str(line);
        return;
    }
    let mut units = 0;
    for (index, character) in line.char_indices() {
        let length = character.len_utf16();
        if units + length > maximum {
            output.push_str(&line[..index]);
            if units < maximum {
                output.push('\u{fffd}');
            }
            output.push_str("...");
            return;
        }
        units += length;
    }
    output.push_str(line);
}

/// core/logger.ts Logger.color：等级 1 使用基础色，等级 2、3 保留标签粗体。
fn color(output: &mut String, level: u8, code: u8, text: &str, bold: bool) {
    if level == 0 {
        output.push_str(text);
        return;
    }
    if code < 8 {
        write!(output, "\x1b[3{code}").unwrap();
    } else {
        write!(output, "\x1b[38;5;{code}").unwrap();
    }
    if level >= 2 && bold {
        output.push_str(";1");
    }
    write!(output, "m{text}\x1b[0m").unwrap();
}

fn color_code(name: &str, level: u8) -> u8 {
    if level == 0 {
        return 0;
    }
    const C16: &[u8] = &[6, 2, 3, 4, 5, 1];
    const C256: &[u8] = &[
        20, 21, 26, 27, 32, 33, 38, 39, 40, 41, 42, 43, 44, 45, 56, 57, 62, 63, 68, 69, 74, 75, 76,
        77, 78, 79, 80, 81, 92, 93, 98, 99, 112, 113, 129, 134, 135, 148, 149, 160, 161, 162, 163,
        164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 178, 179, 184, 185, 196, 197, 198, 199,
        200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 214, 215, 220, 221,
    ];
    let number = name.encode_utf16().fold(0i32, |value, unit| {
        value.wrapping_mul(7).wrapping_add(unit as i32 + 13)
    });
    let colors = if level >= 2 { C256 } else { C16 };
    colors[number.unsigned_abs() as usize % colors.len()]
}

/// cosmokit Time.format：按相邻单位的半刻度切换，采用 JavaScript 的向上半值舍入。
fn elapsed(milliseconds: i64) -> String {
    let magnitude = milliseconds.unsigned_abs();
    let (unit, suffix) = if magnitude >= 84_600_000 {
        (86_400_000, "d")
    } else if magnitude >= 3_570_000 {
        (3_600_000, "h")
    } else if magnitude >= 59_500 {
        (60_000, "m")
    } else if magnitude >= 1_000 {
        (1_000, "s")
    } else {
        return format!("{milliseconds}ms");
    };
    format!(
        "{}{suffix}",
        (milliseconds as f64 / unit as f64 + 0.5).floor()
    )
}
