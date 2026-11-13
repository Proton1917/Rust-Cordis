//! ADR-0039：脚本绑定共用可检索的方法描述和递归类型图。

use crate::{Document, MethodInfo, ValueType, names};
use serde_json::{Value, json};
use wit_parser::{Handle, Type, TypeDefKind, TypeId};

pub fn document(document: &Document) -> Value {
    let types: serde_json::Map<_, _> = document
        .resolve
        .types
        .iter()
        .map(|(id, _)| (id.index().to_string(), definition(document, id)))
        .collect();
    let mut services = Vec::new();
    for interface in &document.interfaces {
        services.push(service(
            &interface.class,
            &interface.name,
            &interface.version,
            &interface.methods,
        ));
        for resource in &interface.resources {
            services.push(service(
                &class(document, resource.ty),
                &resource.name,
                &interface.version,
                &resource.methods,
            ));
        }
    }
    for intrinsic in &document.intrinsics.services {
        services.push(service(
            &intrinsic.class,
            &intrinsic.name,
            "1.0",
            &intrinsic.methods,
        ));
    }
    json!({"package": document.resolve.packages[document.package].name.to_string(), "types": types, "services": services})
}

fn service(class: &str, name: &str, version: &str, methods: &[MethodInfo]) -> Value {
    let methods: Vec<_> = methods.iter().map(|method| json!({
        "name": method.rust, "python": method.python, "typescript": method.typescript,
        "async": method.asynchronous, "since": method.since,
        "params": method.params.iter().map(|param| json!({"name": param.name,"type": value(&param.ty)})).collect::<Vec<_>>(),
        "result": value(&method.result), "error": method.error.map(ty),
    })).collect();
    json!({"class": class, "interface": format!("{name}@{version}"), "methods": methods})
}

pub fn value(input: &ValueType) -> Value {
    match input {
        ValueType::Unit => json!({"kind":"unit"}),
        ValueType::Wit(value) => ty(*value),
        ValueType::Option(inner) => json!({"kind":"option", "item":value(inner)}),
    }
}

pub fn ty(ty: Type) -> Value {
    let kind = match ty {
        Type::Bool => "bool",
        Type::U8 => "u8",
        Type::U16 => "u16",
        Type::U32 => "u32",
        Type::U64 => "u64",
        Type::S8 => "s8",
        Type::S16 => "s16",
        Type::S32 => "s32",
        Type::S64 => "s64",
        Type::F32 => "f32",
        Type::F64 => "f64",
        Type::Char => "char",
        Type::String => "string",
        Type::ErrorContext => "error-context",
        Type::Id(id) => return json!({"ref":id.index().to_string()}),
    };
    json!({"kind":kind})
}

/// 同名类型使用所属接口限定脚本名称，源 WIT 名称进入字段描述。
pub fn class(document: &Document, id: TypeId) -> String {
    let info = &document.types[&id];
    if document
        .types
        .values()
        .filter(|other| other.class == info.class)
        .count()
        > 1
    {
        let prefix = info
            .interface
            .map(|id| document.interface(id).module.clone())
            .unwrap_or_else(|| "world".into());
        format!("{}{}", names::class(&prefix), info.class)
    } else {
        info.class.clone()
    }
}

fn definition(document: &Document, id: TypeId) -> Value {
    let definition = &document.resolve.types[id];
    let mut result = match &definition.kind {
        TypeDefKind::Type(inner) => json!({"kind":"alias", "item":ty(*inner)}),
        TypeDefKind::Option(inner) => json!({"kind":"option", "item":ty(*inner)}),
        TypeDefKind::List(inner) => json!({"kind":"list", "item":ty(*inner)}),
        TypeDefKind::FixedLengthList(inner, length) => {
            json!({"kind":"list", "item":ty(*inner), "length":length})
        }
        TypeDefKind::Map(key, value) => json!({"kind":"map", "key":ty(*key), "item":ty(*value)}),
        TypeDefKind::Tuple(tuple) => {
            json!({"kind":"tuple", "items":tuple.types.iter().copied().map(ty).collect::<Vec<_>>()})
        }
        TypeDefKind::Result(result) => {
            json!({"kind":"result", "ok":result.ok.map(ty).unwrap_or(json!({"kind":"unit"})), "err":result.err.map(ty).unwrap_or(json!({"kind":"unit"}))})
        }
        TypeDefKind::Record(record) => {
            json!({"kind":"record", "fields":record.fields.iter().map(|field| json!({"name":field.name,"python":names::python(&field.name),"typescript":names::typescript(&field.name),"type":ty(field.ty)})).collect::<Vec<_>>()})
        }
        TypeDefKind::Variant(variant) => {
            json!({"kind":"variant", "cases":variant.cases.iter().map(|case| json!({"name":case.name,"class":format!("{}{}",class(document,id),names::class(&case.name)),"type":case.ty.map(ty)})).collect::<Vec<_>>()})
        }
        TypeDefKind::Enum(enumeration) => {
            json!({"kind":"enum", "cases":enumeration.cases.iter().map(|case| case.name.clone()).collect::<Vec<_>>()})
        }
        TypeDefKind::Flags(flags) => {
            json!({"kind":"flags", "fields":flags.flags.iter().map(|flag| json!({"name":flag.name,"python":names::python(&flag.name),"typescript":names::typescript(&flag.name),"type":{"kind":"bool"}})).collect::<Vec<_>>()})
        }
        TypeDefKind::Resource => resource(document, id, false),
        TypeDefKind::Handle(Handle::Own(resource_id)) => resource(document, *resource_id, false),
        TypeDefKind::Handle(Handle::Borrow(resource_id)) => resource(document, *resource_id, true),
        TypeDefKind::Future(_) | TypeDefKind::Stream(_) => {
            let service = &document.intrinsics.services[document.intrinsics.by_type[&id]];
            json!({"kind":"resource", "service":format!("{}@1.0",service.name), "class":service.class, "borrowed":false})
        }
        TypeDefKind::Unknown => unreachable!("已解析 WIT 类型"),
    };
    if document.types.contains_key(&id) && result["kind"] != "resource" {
        result["class"] = class(document, id).into();
    }
    result
}

fn resource(document: &Document, id: TypeId, borrowed: bool) -> Value {
    let info = &document.types[&id];
    let interface = document.interface(info.interface.expect("资源所属接口"));
    let resource = interface
        .resources
        .iter()
        .find(|resource| resource.ty == id)
        .expect("资源定义");
    json!({"kind":"resource", "service":format!("{}@{}",resource.name,interface.version),"class":class(document,id), "borrowed":borrowed})
}
