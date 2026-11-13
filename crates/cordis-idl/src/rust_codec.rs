//! ADR-0039：字段适配器保存嵌套 option、result 分支和固定数组的传输形态。

use crate::{Result, ValueType, rust::Generator};
use std::fmt::Write;
use wit_parser::{Type, TypeDefKind};

/// 适配器直接使用 CborValue 的 Serde 转换，继承外层资源编码作用域。
pub fn adapter(
    generator: &Generator<'_>,
    value: &ValueType,
    name: &str,
    method: bool,
    output: &mut String,
) -> Result<()> {
    let (ty, borrowed) = value_type(generator, value, method);
    let ty = ty.replace("super::", "super::super::");
    let lifetime = if borrowed { "'a, " } else { "" };
    let generic = if borrowed { "<'a>" } else { "" };
    let input = slice_type(generator, value, method)
        .map(|value| value.replace("super::", "super::super::"))
        .unwrap_or_else(|| format!("__Value{generic}"));
    let encode = value_expression(generator, value, "value", true, method, 0);
    let decode = value_expression(generator, value, "value", false, method, 0);
    writeln!(
        output,
        "mod {name} {{ #[allow(unused_imports)] use super::*; use serde::{{Serialize, Deserialize}}; use ::cordis_sdk::__private::abi;"
    )?;
    writeln!(output, "type __Value{generic} = {ty};")?;
    writeln!(
        output,
        "fn to_wire{generic}(value: &{input}) -> ::std::result::Result<abi::CborValue, String> {{ {encode} }}"
    )?;
    writeln!(
        output,
        "fn from_wire{generic}(value: abi::CborValue) -> ::std::result::Result<__Value{generic}, String> {{ {decode} }}"
    )?;
    writeln!(
        output,
        "pub fn serialize<{lifetime}S: serde::Serializer>(value: &{input}, serializer: S) -> ::std::result::Result<S::Ok, S::Error> {{ let value = to_wire(value).map_err(serde::ser::Error::custom)?; value.serialize(serializer) }}"
    )?;
    writeln!(
        output,
        "pub fn deserialize<'de, {lifetime}D: serde::Deserializer<'de>>(deserializer: D) -> ::std::result::Result<__Value{generic}, D::Error> {{ let value = abi::CborValue::deserialize(deserializer)?; from_wire(value).map_err(serde::de::Error::custom) }}"
    )?;
    if method {
        let encode_generic = if borrowed { "<'wire, 'a>" } else { "<'wire>" };
        writeln!(
            output,
            "pub struct Encode{encode_generic}(pub &'wire __Value{generic}); impl{encode_generic} Serialize for Encode{encode_generic} {{ fn serialize<S: serde::Serializer>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error> {{ serialize(self.0, serializer) }} }}"
        )?;
        writeln!(
            output,
            "pub struct Decode{generic}(pub __Value{generic}); impl<'de, {lifetime}> Deserialize<'de> for Decode{generic} {{ fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> ::std::result::Result<Self, D::Error> {{ deserialize(deserializer).map(Self) }} }}"
        )?;
    }
    output.push_str("}\n");
    Ok(())
}

/// 列表的序列化借用切片，反序列化继续构造 WIT 声明的拥有型 Vec。
fn slice_type(generator: &Generator<'_>, value: &ValueType, method: bool) -> Option<String> {
    let ValueType::Wit(mut ty) = value.clone() else {
        return None;
    };
    loop {
        let Type::Id(id) = ty else {
            return None;
        };
        match &generator.document.resolve.types[id].kind {
            TypeDefKind::Type(inner) => ty = *inner,
            TypeDefKind::List(inner) => {
                return Some(format!("[{}]", generator.ty(*inner, method, "'a")));
            }
            _ => return None,
        }
    }
}

fn value_type(generator: &Generator<'_>, value: &ValueType, method: bool) -> (String, bool) {
    match value {
        ValueType::Unit => ("()".into(), false),
        ValueType::Wit(ty) => (
            generator.ty(*ty, method, "'a"),
            crate::properties::get(&generator.document.properties, *ty).borrowed,
        ),
        ValueType::Option(inner) => {
            let (ty, borrowed) = value_type(generator, inner, method);
            (format!("Option<{ty}>"), borrowed)
        }
    }
}

fn value_expression(
    generator: &Generator<'_>,
    ty: &ValueType,
    value: &str,
    encoding: bool,
    method: bool,
    depth: usize,
) -> String {
    match ty {
        ValueType::Wit(ty) => expression(generator, *ty, value, encoding, method, depth),
        ValueType::Unit => {
            if encoding {
                "Ok::<_, String>(abi::CborValue::Null)".into()
            } else {
                format!("{value}.deserialized::<()>().map_err(|error| error.to_string())")
            }
        }
        ValueType::Option(inner) => {
            let item = format!("item_{depth}");
            let child = value_expression(generator, inner, &item, encoding, method, depth + 1);
            if encoding {
                format!(
                    "match {value} {{ Some({item}) => Ok::<_, String>(abi::option_some(({child})?)), None => Ok::<_, String>(abi::CborValue::Null) }}"
                )
            } else {
                format!(
                    "match abi::option_take({value}) {{ Some({item}) => Ok::<_, String>(Some(({child})?)), None => Ok::<_, String>(None) }}"
                )
            }
        }
    }
}

/// 聚合只检查一次输入形状；已解析的元素类型决定递归转换。
fn expression(
    generator: &Generator<'_>,
    ty: Type,
    value: &str,
    encoding: bool,
    method: bool,
    depth: usize,
) -> String {
    let fallback = || {
        if encoding {
            format!("abi::CborValue::serialized({value}).map_err(|error| error.to_string())")
        } else {
            format!("{value}.deserialized().map_err(|error| error.to_string())")
        }
    };
    let Type::Id(id) = ty else {
        return fallback();
    };
    let item = format!("item_{depth}");
    let recurse = |ty, input: &str| expression(generator, ty, input, encoding, false, depth + 1);
    match &generator.document.resolve.types[id].kind {
        TypeDefKind::Type(ty) => expression(generator, *ty, value, encoding, method, depth + 1),
        TypeDefKind::Option(ty) => {
            let child = expression(generator, *ty, &item, encoding, method, depth + 1);
            if encoding {
                format!(
                    "match {value} {{ Some({item}) => Ok::<_, String>(abi::option_some(({child})?)), None => Ok::<_, String>(abi::CborValue::Null) }}"
                )
            } else {
                format!(
                    "match abi::option_take({value}) {{ Some({item}) => Ok::<_, String>(Some(({child})?)), None => Ok::<_, String>(None) }}"
                )
            }
        }
        TypeDefKind::List(Type::U8) => {
            if encoding {
                format!("Ok::<_, String>(abi::CborValue::Bytes({value}.to_vec()))")
            } else {
                let converted = if method { ".map(|bytes| bytes.0)" } else { "" };
                format!(
                    "{value}.deserialized::<::cordis_sdk::Bytes>(){converted}.map_err(|error| error.to_string())"
                )
            }
        }
        TypeDefKind::List(ty) | TypeDefKind::FixedLengthList(ty, _) => {
            let length = match generator.document.resolve.types[id].kind {
                TypeDefKind::FixedLengthList(_, length) => Some(length),
                _ => None,
            };
            list_expression(value, &item, &recurse(*ty, &item), length, encoding)
        }
        TypeDefKind::Map(key, ty) => {
            let key_expr = recurse(*key, &format!("key_{depth}"));
            let item_expr = recurse(*ty, &item);
            let operation = format!(
                "map(|(key_{depth}, {item})| Ok::<_, String>((({key_expr})?, ({item_expr})?)))"
            );
            if encoding {
                format!(
                    "{value}.iter().{operation}.collect::<::std::result::Result<Vec<_>, String>>().map(abi::CborValue::Map)"
                )
            } else {
                format!(
                    "match {value} {{ abi::CborValue::Map(items) => items.into_iter().{operation}.collect::<::std::result::Result<_, String>>(), _ => Err(\"映射需要 CBOR 映射\".into()) }}"
                )
            }
        }
        TypeDefKind::Tuple(tuple) => tuple_expression(tuple, value, encoding, recurse),
        TypeDefKind::Result(result) => result_expression(result, value, &item, encoding, recurse),
        _ => fallback(),
    }
}

/// ADR-0039：固定数组先解码全部元素，再执行其长度契约。
fn list_expression(
    value: &str,
    item: &str,
    child: &str,
    length: Option<u32>,
    encoding: bool,
) -> String {
    if encoding {
        return format!(
            "{value}.iter().map(|{item}| {child}).collect::<::std::result::Result<Vec<_>, String>>().map(abi::CborValue::Array)"
        );
    }
    let converted = format!(
        "items.into_iter().map(|{item}| {child}).collect::<::std::result::Result<Vec<_>, String>>()"
    );
    let converted = if let Some(length) = length {
        format!(
            "({converted})?.try_into().map_err(|items: Vec<_>| format!(\"固定数组需要 {length} 个元素，实际 {{}}\", items.len()))"
        )
    } else {
        converted
    };
    format!(
        "match {value} {{ abi::CborValue::Array(items) => {{ {converted} }}, _ => Err(\"列表需要 CBOR 数组\".into()) }}"
    )
}

fn tuple_expression(
    tuple: &wit_parser::Tuple,
    value: &str,
    encoding: bool,
    recurse: impl Fn(Type, &str) -> String,
) -> String {
    if encoding {
        let items: Vec<_> = tuple
            .types
            .iter()
            .enumerate()
            .map(|(index, ty)| format!("({})?", recurse(*ty, &format!("&({value}).{index}"))))
            .collect();
        format!(
            "Ok::<_, String>(abi::CborValue::Array(vec![{}]))",
            items.join(",")
        )
    } else {
        let items: Vec<_> = tuple
            .types
            .iter()
            .map(|ty| {
                format!(
                    "({})?,",
                    recurse(*ty, "items.next().expect(\"已检查 tuple 长度\")")
                )
            })
            .collect();
        let length = tuple.types.len();
        format!(
            "match {value} {{ abi::CborValue::Array(items) if items.len() == {length} => {{ let mut items = items.into_iter(); Ok::<_, String>(({})) }}, _ => Err(\"tuple 长度或表示错误\".into()) }}",
            items.join("")
        )
    }
}

/// ADR-0039：数据中的 result 使用小写分支名称，与方法错误通道独立。
fn result_expression(
    result: &wit_parser::Result_,
    value: &str,
    item: &str,
    encoding: bool,
    recurse: impl Fn(Type, &str) -> String,
) -> String {
    let branch = |ty: Option<Type>, label: &str| {
        let child = ty.map(|ty| recurse(ty, item)).unwrap_or_else(|| {
            if encoding {
                "Ok::<_, String>(abi::CborValue::Null)".into()
            } else {
                format!("{item}.deserialized::<()>().map_err(|error| error.to_string())")
            }
        });
        if encoding {
            format!(
                "Ok::<_, String>(abi::CborValue::Map(vec![(abi::CborValue::Text(\"{label}\".into()), ({child})?)]))"
            )
        } else {
            format!(
                "Ok::<_, String>({}(({child})?))",
                if label == "ok" { "Ok" } else { "Err" }
            )
        }
    };
    let ok = branch(result.ok, "ok");
    let error = branch(result.err, "err");
    if encoding {
        format!("match {value} {{ Ok({item}) => {ok}, Err({item}) => {error} }}")
    } else {
        format!(
            "match {value} {{ abi::CborValue::Map(mut items) if items.len() == 1 => {{ let (key, {item}) = items.pop().expect(\"已检查 result 长度\"); match key.as_text() {{ Some(\"ok\") => {ok}, Some(\"err\") => {error}, _ => Err(\"result 分支需要 ok 或 err\".into()) }} }}, _ => Err(\"result 需要单键映射\".into()) }}"
        )
    }
}
