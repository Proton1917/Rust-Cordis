//! DOC-15 §7、ADR-0039：解析后的接口与资源方法共用名称、版本及所属定义。

use crate::{
    Result,
    metadata::{self, Metadata},
    names,
    properties::{self, Properties},
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
use wit_parser::{
    Function, FunctionKind, InterfaceId, PackageId, Resolve, Type, TypeDefKind, TypeId, TypeOwner,
    WorldItem, WorldKey,
};

pub struct Document {
    pub resolve: Resolve,
    pub package: PackageId,
    pub interfaces: Vec<InterfaceInfo>,
    pub types: HashMap<TypeId, TypeInfo>,
    pub(crate) by_interface: HashMap<InterfaceId, usize>,
    pub(crate) properties: HashMap<TypeId, Properties>,
    pub(crate) intrinsics: crate::intrinsics::Intrinsics,
}

pub struct InterfaceInfo {
    pub id: InterfaceId,
    pub name: String,
    pub module: String,
    pub class: String,
    pub version: String,
    pub root: bool,
    pub methods: Vec<MethodInfo>,
    pub resources: Vec<ResourceInfo>,
}

pub struct MethodInfo {
    pub name: String,
    pub rust: String,
    pub python: String,
    pub typescript: String,
    pub asynchronous: bool,
    pub params: Vec<Parameter>,
    pub result: ValueType,
    pub error: Option<Type>,
    pub documentation: Option<String>,
    pub since: Option<String>,
}

pub struct Parameter {
    pub name: String,
    pub ty: ValueType,
}

#[derive(Clone)]
pub enum ValueType {
    Unit,
    Wit(Type),
    Option(Box<ValueType>),
}

pub struct ResourceInfo {
    pub ty: TypeId,
    pub name: String,
    pub class: String,
    pub methods: Vec<MethodInfo>,
}

pub struct TypeInfo {
    pub name: String,
    pub class: String,
    pub interface: Option<InterfaceId>,
    pub(crate) metadata: Metadata,
}

impl Document {
    /// ADR-0020：文件与目录的包解析由 wit-parser 完成，特性按调用者显式选择。
    pub fn load(path: &Path, features: &[String]) -> Result<Self> {
        let mut resolve = Resolve::default();
        resolve.features.extend(features.iter().cloned());
        let (package, _) = resolve.push_path(path)?;
        let labels = interface_names(&resolve);
        let mut interfaces = Vec::new();
        let mut by_interface = HashMap::new();
        for (id, (owner, label)) in labels {
            let info = interface(&resolve, id, owner, &label, owner == package)?;
            by_interface.insert(id, interfaces.len());
            interfaces.push(info);
        }
        let types = type_names(&resolve, &interfaces, &by_interface)?;
        for interface in &mut interfaces {
            for resource in &mut interface.resources {
                let ty = &types[&resource.ty];
                resource.class = ty.class.clone();
                resource.name = metadata::name(ty.metadata.name.as_deref(), &resource.name)?;
            }
        }
        let defaults: HashSet<_> = types
            .iter()
            .filter(|(_, info)| info.metadata.default)
            .map(|(id, _)| *id)
            .collect();
        let properties = properties::analyze(&resolve, &defaults);
        validate(&resolve, &types, &properties)?;
        let intrinsics = crate::intrinsics::analyze(&resolve);
        Ok(Self {
            resolve,
            package,
            interfaces,
            types,
            by_interface,
            properties,
            intrinsics,
        })
    }

    pub fn interface(&self, id: InterfaceId) -> &InterfaceInfo {
        &self.interfaces[self.by_interface[&id]]
    }

    /// ADR-0046：跨包导入资源沿别名取得原定义，接口身份与客户端类型共用此结果。
    pub(crate) fn resource_type(&self, mut id: TypeId) -> TypeId {
        while let TypeDefKind::Type(Type::Id(original)) = self.resolve.types[id].kind {
            id = original;
        }
        id
    }

    pub fn root_interfaces(&self) -> impl Iterator<Item = &InterfaceInfo> {
        self.interfaces.iter().filter(|interface| interface.root)
    }
}

/// ADR-0039：数据类型与资源保留其所属接口，名称冲突按声明种类区分。
fn type_names(
    resolve: &Resolve,
    interfaces: &[InterfaceInfo],
    by_interface: &HashMap<InterfaceId, usize>,
) -> Result<HashMap<TypeId, TypeInfo>> {
    let mut types = HashMap::new();
    for (id, definition) in resolve.types.iter() {
        let Some(name) = &definition.name else {
            continue;
        };
        let interface = match definition.owner {
            TypeOwner::Interface(id) => Some(id),
            _ => None,
        };
        let mut class = names::class(name);
        if interface.is_some_and(|id| interfaces[by_interface[&id]].class == class) {
            class.push_str(
                if matches!(
                    definition.kind,
                    TypeDefKind::Resource | TypeDefKind::Future(_) | TypeDefKind::Stream(_)
                ) {
                    "Resource"
                } else {
                    "Data"
                },
            );
        }
        types.insert(
            id,
            TypeInfo {
                name: name.clone(),
                class,
                interface,
                metadata: metadata::read(definition.docs.contents.as_deref())?,
            },
        );
    }
    Ok(types)
}

/// WIT 的内联 interface 使用所属 world 的具名导入/导出标识。
fn interface_names(resolve: &Resolve) -> Vec<(InterfaceId, (PackageId, String))> {
    let mut labels = HashMap::new();
    for (package, definition) in resolve.packages.iter() {
        for (name, id) in &definition.interfaces {
            labels.insert(*id, (package, name.clone()));
        }
        for (world_name, world) in &definition.worlds {
            for (key, item) in resolve.worlds[*world]
                .imports
                .iter()
                .chain(&resolve.worlds[*world].exports)
            {
                if let (WorldKey::Name(name), WorldItem::Interface { id, .. }) = (key, item) {
                    labels
                        .entry(*id)
                        .or_insert_with(|| (package, format!("{world_name}-{name}")));
                }
            }
        }
    }
    let mut ordered: Vec<_> = labels.into_iter().collect();
    ordered.sort_by_key(|(id, _)| id.index());
    ordered
}

fn interface(
    resolve: &Resolve,
    id: InterfaceId,
    package: PackageId,
    label: &str,
    root: bool,
) -> Result<InterfaceInfo> {
    let declaration = &resolve.interfaces[id];
    let package = &resolve.packages[package].name;
    let version = package
        .version
        .as_ref()
        .ok_or_else(|| format!("WIT 包 {} 需要接口版本", package))?;
    let _: u32 = version.major.try_into()?;
    let _: u32 = version.minor.try_into()?;
    let metadata = metadata::read(declaration.docs.contents.as_deref())?;
    let name = metadata::name(metadata.name.as_deref(), label)?;
    let mut info = InterfaceInfo {
        id,
        name,
        module: names::module(&package.namespace, &package.name, label),
        class: names::class(label),
        version: format!("{}.{}", version.major, version.minor),
        root,
        methods: Vec::new(),
        resources: Vec::new(),
    };
    for (name, ty) in &declaration.types {
        if matches!(resolve.types[*ty].kind, TypeDefKind::Resource)
            && resolve.types[*ty].owner == TypeOwner::Interface(id)
        {
            info.resources.push(ResourceInfo {
                ty: *ty,
                name: format!("{}.{name}", info.name),
                class: names::class(name),
                methods: Vec::new(),
            });
        }
    }
    for function in declaration.functions.values() {
        let method = method(resolve, function);
        match function.kind {
            FunctionKind::Method(resource)
            | FunctionKind::AsyncMethod(resource)
            | FunctionKind::MethodGetter(resource)
            | FunctionKind::MethodSetter(resource) => {
                info.resources
                    .iter_mut()
                    .find(|item| item.ty == resource)
                    .ok_or("资源方法需要所属资源定义")?
                    .methods
                    .push(method);
            }
            _ => info.methods.push(method),
        }
    }
    Ok(info)
}

fn validate(
    resolve: &Resolve,
    types: &HashMap<TypeId, TypeInfo>,
    properties: &HashMap<TypeId, Properties>,
) -> Result<()> {
    for (id, info) in types {
        if info.metadata.event.is_some() {
            if !matches!(resolve.types[*id].kind, TypeDefKind::Record(_)) {
                return Err("事件属性应用于 record".into());
            }
            if properties[id].borrowed {
                return Err("事件载荷使用拥有型资源，借用资源属于调用期".into());
            }
            metadata::name(info.metadata.event.as_deref(), &info.name)?;
        } else if info.metadata.kind.is_some() {
            return Err("kind 属性与事件名称共同声明".into());
        }
        if info.metadata.default && !properties[id].default {
            return Err(format!("类型 {} 的字段需要支持 Default", info.name).into());
        }
    }
    Ok(())
}

fn method(resolve: &Resolve, function: &Function) -> MethodInfo {
    let skip_self = matches!(
        function.kind,
        FunctionKind::Method(_)
            | FunctionKind::AsyncMethod(_)
            | FunctionKind::MethodGetter(_)
            | FunctionKind::MethodSetter(_)
    );
    let name = function.item_name().to_owned();
    let (rust, python, typescript) = method_names(resolve, function, &name, skip_self);
    let params = function
        .params
        .iter()
        .skip(usize::from(skip_self))
        .map(|param| Parameter {
            name: param.name.clone(),
            ty: ValueType::Wit(param.ty),
        })
        .collect();
    let (result, error) = method_result(resolve, function.result);
    let since = match &function.stability {
        wit_parser::Stability::Stable { since, .. } => Some(since.to_string()),
        _ => None,
    };
    MethodInfo {
        name,
        rust,
        python,
        typescript,
        asynchronous: function.kind.is_async(),
        params,
        result: result.map(ValueType::Wit).unwrap_or(ValueType::Unit),
        error,
        documentation: function.docs.contents.clone(),
        since,
    }
}

/// ADR-0039：资源构造器、静态函数和访问器共享确定的方法名称规则。
fn method_names(
    resolve: &Resolve,
    function: &Function,
    name: &str,
    skip_self: bool,
) -> (String, String, String) {
    let mut rust = names::rust(name);
    let mut python = names::python(name);
    let mut typescript = names::typescript(name);
    if let Some(accessor) = function.kind.accessor() {
        let prefix = match accessor {
            wit_parser::AccessorKind::Getter => "get",
            wit_parser::AccessorKind::Setter => "set",
        };
        rust = format!("{prefix}__{rust}");
        python = format!("{prefix}__{python}");
        typescript = format!("{prefix}__{typescript}");
    }
    if let Some(resource) = function.kind.resource()
        && !skip_self
    {
        let resource = resolve.types[resource]
            .name
            .as_deref()
            .expect("WIT 资源名称");
        if matches!(function.kind, FunctionKind::Constructor(_)) {
            rust = format!("new__{}", names::rust(resource));
            python = format!("new__{}", names::python(resource));
            typescript = format!("new__{}", names::typescript(resource));
        } else {
            rust = format!("{}__{rust}", names::rust(resource));
            python = format!("{}__{python}", names::python(resource));
            typescript = format!("{}__{typescript}", names::typescript(resource));
        }
    }
    (rust, python, typescript)
}

fn method_result(resolve: &Resolve, ty: Option<Type>) -> (Option<Type>, Option<Type>) {
    match ty {
        Some(Type::Id(id)) => match &resolve.types[id].kind {
            TypeDefKind::Type(inner) => method_result(resolve, Some(*inner)),
            TypeDefKind::Result(result) => (result.ok, result.err),
            _ => (ty, None),
        },
        _ => (ty, None),
    }
}
