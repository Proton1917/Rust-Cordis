//! ADR-0039：future、stream 与 error-context 使用既有资源引用协议。

use crate::{
    model::{MethodInfo, ValueType},
    names,
};
use std::collections::{BTreeMap, HashMap};
use wit_parser::{Handle, Resolve, Type, TypeDefKind, TypeId, TypeOwner};

pub struct Intrinsics {
    pub services: Vec<Intrinsic>,
    pub by_type: HashMap<TypeId, usize>,
    pub error_context: Option<usize>,
}

pub struct Intrinsic {
    pub name: String,
    pub class: String,
    pub methods: Vec<MethodInfo>,
}

/// 相同载荷类型共享同一资源接口，类型别名沿其目标取得身份。
pub fn analyze(resolve: &Resolve) -> Intrinsics {
    let mut services = Vec::new();
    let mut by_type = HashMap::new();
    let mut known = BTreeMap::new();
    let mut error_context = None;
    for (id, definition) in resolve.types.iter() {
        let (kind, payload) = match &definition.kind {
            TypeDefKind::Future(payload) => ("future", *payload),
            TypeDefKind::Stream(payload) => ("stream", *payload),
            _ => continue,
        };
        let payload_name = payload
            .map(|ty| tag(resolve, ty))
            .unwrap_or_else(|| "unit".into());
        let name = format!("cordis.{kind}.{payload_name}");
        let index = *known.entry(name.clone()).or_insert_with(|| {
            let value = payload.map(ValueType::Wit).unwrap_or(ValueType::Unit);
            let (method, result) = if kind == "future" {
                ("resolve", value)
            } else {
                ("next", ValueType::Option(Box::new(value)))
            };
            let index = services.len();
            let mut class = format!("{}{}", names::class(kind), names::class(&payload_name));
            if services
                .iter()
                .any(|service: &Intrinsic| service.class == class)
            {
                class.push_str(&format!("Type{}", id.index()));
            }
            services.push(Intrinsic {
                name,
                class,
                methods: vec![method_info(method, result, true)],
            });
            index
        });
        by_type.insert(id, index);
    }
    if uses_error_context(resolve) {
        error_context = Some(services.len());
        services.push(Intrinsic {
            name: "cordis.error-context".into(),
            class: "ErrorContext".into(),
            methods: vec![method_info(
                "debug_message",
                ValueType::Wit(Type::String),
                false,
            )],
        });
    }
    Intrinsics {
        services,
        by_type,
        error_context,
    }
}

fn method_info(name: &str, result: ValueType, asynchronous: bool) -> MethodInfo {
    MethodInfo {
        name: name.into(),
        rust: name.into(),
        python: name.into(),
        typescript: names::typescript(name),
        asynchronous,
        params: Vec::new(),
        result,
        error: None,
        documentation: None,
        since: None,
    }
}

/// 结构名称用于资源接口身份，具名数据类型带所属包主版本。
fn tag(resolve: &Resolve, ty: Type) -> String {
    match ty {
        Type::Id(id) => match &resolve.types[id].kind {
            TypeDefKind::Type(ty) => tag(resolve, *ty),
            TypeDefKind::List(ty) => compound("list", &[tag(resolve, *ty)]),
            TypeDefKind::Option(ty) => compound("option", &[tag(resolve, *ty)]),
            TypeDefKind::Map(key, value) => {
                compound("map", &[tag(resolve, *key), tag(resolve, *value)])
            }
            TypeDefKind::FixedLengthList(ty, length) => {
                compound(&format!("array-{length}"), &[tag(resolve, *ty)])
            }
            TypeDefKind::Tuple(tuple) => compound(
                "tuple",
                &tuple
                    .types
                    .iter()
                    .map(|ty| tag(resolve, *ty))
                    .collect::<Vec<_>>(),
            ),
            TypeDefKind::Result(result) => compound(
                "result",
                &[
                    result
                        .ok
                        .map(|ty| tag(resolve, ty))
                        .unwrap_or_else(|| "unit".into()),
                    result
                        .err
                        .map(|ty| tag(resolve, ty))
                        .unwrap_or_else(|| "unit".into()),
                ],
            ),
            TypeDefKind::Future(ty) => compound(
                "future",
                &[ty.map(|ty| tag(resolve, ty))
                    .unwrap_or_else(|| "unit".into())],
            ),
            TypeDefKind::Stream(ty) => compound(
                "stream",
                &[ty.map(|ty| tag(resolve, ty))
                    .unwrap_or_else(|| "unit".into())],
            ),
            TypeDefKind::Handle(handle) => match handle {
                Handle::Own(id) => compound("own", &[named(resolve, *id)]),
                Handle::Borrow(id) => compound("borrow", &[named(resolve, *id)]),
            },
            _ => named(resolve, id),
        },
        Type::Bool => "bool".into(),
        Type::U8 => "u8".into(),
        Type::U16 => "u16".into(),
        Type::U32 => "u32".into(),
        Type::U64 => "u64".into(),
        Type::S8 => "s8".into(),
        Type::S16 => "s16".into(),
        Type::S32 => "s32".into(),
        Type::S64 => "s64".into(),
        Type::F32 => "f32".into(),
        Type::F64 => "f64".into(),
        Type::Char => "char".into(),
        Type::String => "string".into(),
        Type::ErrorContext => "error-context".into(),
    }
}

fn named(resolve: &Resolve, id: TypeId) -> String {
    let definition = &resolve.types[id];
    let name = definition.name.as_deref().expect("WIT 具名数据类型");
    let name = match definition.owner {
        TypeOwner::Interface(interface) => {
            let interface = &resolve.interfaces[interface];
            let package = &resolve.packages[interface.package.expect("WIT 接口所属包")].name;
            format!(
                "{}:{}/{}#{}@{}",
                package.namespace,
                package.name,
                interface.name.as_deref().unwrap_or("inline"),
                name,
                package
                    .version
                    .as_ref()
                    .map(|version| version.major)
                    .unwrap_or(0)
            )
        }
        TypeOwner::World(world) => format!("world.{}.{}", resolve.worlds[world].name, name),
        TypeOwner::None => name.into(),
    };
    let encoded: String = name
        .to_ascii_lowercase()
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || byte == b'.' {
                char::from(byte).to_string()
            } else {
                format!("-x{byte:02x}-")
            }
        })
        .collect();
    compound("named", &[encoded])
}

fn compound(kind: &str, parts: &[String]) -> String {
    format!(
        "{kind}.{}.{}",
        parts.len(),
        parts
            .iter()
            .map(|part| format!("{}.{}", part.len(), part))
            .collect::<Vec<_>>()
            .join(".")
    )
}

fn uses_error_context(resolve: &Resolve) -> bool {
    let direct = |ty: Type| ty == Type::ErrorContext;
    resolve.interfaces.iter().any(|(_, interface)| {
        interface.functions.values().any(|function| {
            function.params.iter().any(|param| direct(param.ty))
                || function.result.is_some_and(direct)
        })
    }) || resolve
        .types
        .iter()
        .any(|(_, definition)| match &definition.kind {
            TypeDefKind::Record(record) => record.fields.iter().any(|field| direct(field.ty)),
            TypeDefKind::Variant(variant) => {
                variant.cases.iter().any(|case| case.ty.is_some_and(direct))
            }
            TypeDefKind::Tuple(tuple) => tuple.types.iter().copied().any(direct),
            TypeDefKind::Result(result) => {
                result.ok.is_some_and(direct) || result.err.is_some_and(direct)
            }
            TypeDefKind::Map(key, value) => direct(*key) || direct(*value),
            TypeDefKind::Option(ty)
            | TypeDefKind::List(ty)
            | TypeDefKind::FixedLengthList(ty, _)
            | TypeDefKind::Type(ty) => direct(*ty),
            TypeDefKind::Future(ty) | TypeDefKind::Stream(ty) => ty.is_some_and(direct),
            _ => false,
        })
}
