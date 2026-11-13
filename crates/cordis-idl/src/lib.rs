//! DOC-15 §7、ADR-0039：WIT 解析和语言绑定共用接口、类型与传输表示。
#![forbid(unsafe_code)]

mod intrinsics;
mod metadata;
mod model;
mod names;
mod properties;
mod rust;
mod rust_codec;
mod schema;
mod script;

pub use model::{
    Document, InterfaceInfo, MethodInfo, Parameter, ResourceInfo, TypeInfo, ValueType,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// DOC-15 §7：同一解析文档产生三种语言的绑定，错误保留生成位置。
pub fn generate(document: &Document, language: &str) -> Result<String> {
    match language {
        "rust" => rust::generate(document),
        "python" => script::python(document),
        "ts" | "typescript" => script::typescript(document),
        _ => Err(format!("绑定语言 {language} 需要为 rust、python 或 ts").into()),
    }
}
