//! ADR-0039：语言标识符与运行时方法名称采用确定的大小写及关键字规则。

use heck::{ToLowerCamelCase, ToSnakeCase, ToUpperCamelCase};

pub fn rust(value: &str) -> String {
    let value = value.to_snake_case();
    if matches!(
        value.as_str(),
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "try"
            | "gen"
    ) {
        format!("{value}_")
    } else {
        value
    }
}

pub fn python(value: &str) -> String {
    let value = value.to_snake_case();
    if matches!(
        value.as_str(),
        "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "try"
            | "while"
            | "with"
            | "yield"
    ) {
        format!("{value}_")
    } else {
        value
    }
}

pub fn class(value: &str) -> String {
    let value = value.to_upper_camel_case();
    if matches!(
        value.as_str(),
        "Self" | "String" | "Option" | "Vec" | "Service" | "Result"
    ) {
        format!("{value}Value")
    } else {
        value
    }
}

pub fn typescript(value: &str) -> String {
    value.to_lower_camel_case()
}

pub fn module(namespace: &str, package: &str, interface: &str) -> String {
    namespace
        .split(':')
        .chain(package.split('/'))
        .chain([interface])
        .map(rust)
        .collect::<Vec<_>>()
        .join("__")
}
