//! ADR-0039：按已解析类型的拓扑顺序计算借用、缺省值与可空编码属性。

use std::collections::{HashMap, HashSet};
use wit_parser::{Handle, Resolve, Type, TypeDefKind, TypeId};

#[derive(Clone, Copy, Default)]
pub struct Properties {
    pub borrowed: bool,
    pub resource: bool,
    pub default: bool,
    pub nullable: bool,
    pub adapter: bool,
}

pub fn get(properties: &HashMap<TypeId, Properties>, ty: Type) -> Properties {
    match ty {
        Type::Id(id) => properties[&id],
        Type::ErrorContext => Properties {
            resource: true,
            ..Properties::default()
        },
        _ => Properties {
            default: true,
            ..Properties::default()
        },
    }
}

/// ADR-0039：解析器的拓扑顺序使每个组合复用已计算的子类型属性。
pub fn analyze(resolve: &Resolve, defaults: &HashSet<TypeId>) -> HashMap<TypeId, Properties> {
    let mut properties = HashMap::new();
    for (id, definition) in resolve.types.iter() {
        let property = type_properties(&definition.kind, defaults.contains(&id), &properties);
        properties.insert(id, property);
    }
    properties
}

fn type_properties(
    kind: &TypeDefKind,
    default: bool,
    properties: &HashMap<TypeId, Properties>,
) -> Properties {
    match kind {
        TypeDefKind::Type(ty) => get(properties, *ty),
        TypeDefKind::Resource | TypeDefKind::Future(_) | TypeDefKind::Stream(_) => Properties {
            resource: true,
            ..Properties::default()
        },
        TypeDefKind::Handle(handle) => Properties {
            resource: true,
            borrowed: matches!(handle, Handle::Borrow(_)),
            ..Properties::default()
        },
        TypeDefKind::Option(ty) => {
            let inner = get(properties, *ty);
            Properties {
                default: true,
                nullable: true,
                adapter: inner.nullable || inner.adapter,
                ..inner
            }
        }
        TypeDefKind::List(ty) => Properties {
            default: true,
            nullable: false,
            ..get(properties, *ty)
        },
        TypeDefKind::FixedLengthList(ty, _) => Properties {
            nullable: false,
            adapter: true,
            ..get(properties, *ty)
        },
        TypeDefKind::Map(key, value) => {
            let items = [get(properties, *key), get(properties, *value)];
            Properties {
                default: true,
                ..aggregate(&items)
            }
        }
        TypeDefKind::Tuple(tuple) => {
            let items: Vec<_> = tuple.types.iter().map(|ty| get(properties, *ty)).collect();
            Properties {
                nullable: items.is_empty(),
                ..aggregate(&items)
            }
        }
        _ => aggregate_definition(kind, default, properties),
    }
}

/// ADR-0039：记录和变体各自携带字段适配器，数据 result 采用统一分支编码。
fn aggregate_definition(
    kind: &TypeDefKind,
    default: bool,
    properties: &HashMap<TypeId, Properties>,
) -> Properties {
    match kind {
        TypeDefKind::Record(record) => {
            let items: Vec<_> = record
                .fields
                .iter()
                .map(|field| get(properties, field.ty))
                .collect();
            Properties {
                default: default && items.iter().all(|item| item.default),
                adapter: false,
                ..aggregate(&items)
            }
        }
        TypeDefKind::Variant(variant) => {
            let items: Vec<_> = variant
                .cases
                .iter()
                .filter_map(|case| case.ty)
                .map(|ty| get(properties, ty))
                .collect();
            Properties {
                default: false,
                adapter: false,
                ..aggregate(&items)
            }
        }
        TypeDefKind::Result(result) => {
            let items: Vec<_> = [result.ok, result.err]
                .into_iter()
                .flatten()
                .map(|ty| get(properties, ty))
                .collect();
            Properties {
                default: false,
                adapter: true,
                ..aggregate(&items)
            }
        }
        TypeDefKind::Flags(_) => Properties {
            default,
            ..Properties::default()
        },
        TypeDefKind::Enum(_) | TypeDefKind::Unknown => Properties::default(),
        _ => unreachable!("单项与容器类型已计算属性"),
    }
}

fn aggregate(items: &[Properties]) -> Properties {
    Properties {
        borrowed: items.iter().any(|item| item.borrowed),
        resource: items.iter().any(|item| item.resource),
        default: items.iter().all(|item| item.default),
        nullable: false,
        adapter: items.iter().any(|item| item.adapter),
    }
}
