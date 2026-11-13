//! ADR-0039：WIT 文档中的 cordis 代码块使用 Markdown 与 TOML 库解析。

use crate::Result;
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use serde::Deserialize;

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Metadata {
    pub name: Option<String>,
    pub event: Option<String>,
    pub kind: Option<EventKind>,
    pub default: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Sync,
    Async,
}

/// ADR-0039：每项定义的属性来自同一个代码块，解析错误保留实际诊断。
pub fn read(source: Option<&str>) -> Result<Metadata> {
    let mut active = false;
    let mut document = String::new();
    let mut metadata = None;
    for event in Parser::new(source.unwrap_or_default()) {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(language)))
                if language.as_ref() == "cordis" =>
            {
                if metadata.is_some() {
                    return Err("同一定义的 cordis 属性使用一个代码块".into());
                }
                active = true;
            }
            Event::Text(text) if active => document.push_str(&text),
            Event::End(TagEnd::CodeBlock) if active => {
                metadata = Some(toml::from_str(&document)?);
                active = false;
            }
            _ => {}
        }
    }
    Ok(metadata.unwrap_or_default())
}

pub fn name(declared: Option<&str>, original: &str) -> Result<String> {
    let name = declared
        .map(str::to_owned)
        .unwrap_or_else(|| original.to_ascii_lowercase());
    if name.is_empty()
        || !name.bytes().all(|value| {
            value.is_ascii_lowercase() || value.is_ascii_digit() || matches!(value, b'-' | b'.')
        })
    {
        return Err("Cordis 接口名称使用小写字母、数字、短横线和点".into());
    }
    Ok(name)
}
