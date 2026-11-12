//! ADR-0032、UPSTREAM logger-console/shared.ts：配置解码和格式参数的默认值。

use chrono::format::{Item, StrftimeItems};
use serde::{Deserialize, Deserializer, de::Error};
use std::collections::BTreeMap;

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ConsoleConfig {
    pub level: u32,
    pub colors: Option<ColorSupport>,
    pub max_length: usize,
    pub levels: BTreeMap<String, LevelThreshold>,
    pub show_diff: bool,
    pub show_time: TimeFormat,
    pub label: LabelStyle,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum LevelThreshold {
    Integer(i64),
    Float(f64),
}

impl LevelThreshold {
    pub(super) fn number(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Float(value) => value,
        }
    }
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            level: 2,
            colors: None,
            max_length: 10240,
            levels: BTreeMap::new(),
            show_diff: false,
            show_time: TimeFormat::default(),
            label: LabelStyle::default(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct ColorSupport(pub(super) u8);

impl<'de> Deserialize<'de> for ColorSupport {
    /// shared.ts Config.colors：接收 false 或 0–3，解码后的值直接用于颜色选择。
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Value {
            Boolean(bool),
            Depth(u8),
        }
        match Value::deserialize(deserializer)? {
            Value::Boolean(false) => Ok(Self(0)),
            Value::Depth(depth @ 0..=3) => Ok(Self(depth)),
            _ => Err(D::Error::custom("colors取值为false或0到3")),
        }
    }
}

#[derive(Clone)]
pub struct TimeFormat(pub(super) Vec<Item<'static>>);

impl TimeFormat {
    fn parse(value: &str) -> Result<Self, chrono::ParseError> {
        StrftimeItems::new(value).parse_to_owned().map(Self)
    }
}

impl Default for TimeFormat {
    fn default() -> Self {
        Self::parse("%Y-%m-%d %H:%M:%S ").expect("默认strftime格式有效")
    }
}

impl<'de> Deserialize<'de> for TimeFormat {
    /// ADR-0032：strftime 由 chrono 在配置边界解析，渲染复用拥有的格式项。
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct LabelStyle {
    pub width: usize,
    pub margin: usize,
    pub align: Alignment,
}

impl Default for LabelStyle {
    fn default() -> Self {
        Self {
            width: 0,
            margin: 1,
            align: Alignment::Left,
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Alignment {
    Left,
    Right,
}
