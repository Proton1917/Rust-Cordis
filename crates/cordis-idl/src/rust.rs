//! ADR-0039：Rust 绑定保留原生类型，接口宏承担两种调用入口与资源所有权。

use crate::{Document, InterfaceInfo, MethodInfo, Result, ValueType, metadata::EventKind, names};
use std::{collections::BTreeMap, fmt::Write};
use wit_parser::{Handle, InterfaceId, Type, TypeDefKind, TypeId};

pub(crate) struct Generator<'a> {
    pub document: &'a Document,
    pub interface: Option<InterfaceId>,
}

/// DOC-15 §7：按 WIT 接口输出模块，单个根接口保留直接导入的使用方式。
pub fn generate(document: &Document) -> Result<String> {
    let mut output = String::from("// DOC-15 §7、ADR-0039：WIT 接口与跨语言数据表示。\n");
    for interface in &document.interfaces {
        interface_module(document, interface, &mut output)?;
    }
    if !document.intrinsics.services.is_empty() {
        intrinsic_module(document, &mut output)?;
    }
    if let [interface] = document.root_interfaces().collect::<Vec<_>>().as_slice() {
        writeln!(output, "pub use {}::*;", interface.module)?;
    }
    let syntax = syn::parse_file(&output)?;
    Ok(prettyplease::unparse(&syntax))
}

/// ADR-0039：一个接口模块先声明数据，再声明普通服务与资源服务。
fn interface_module(
    document: &Document,
    interface: &InterfaceInfo,
    output: &mut String,
) -> Result<()> {
    writeln!(
        output,
        "#[rustfmt::skip] #[allow(non_snake_case)] pub mod {} {{",
        interface.module
    )?;
    let generator = Generator {
        document,
        interface: Some(interface.id),
    };
    for (id, _) in generator.definitions() {
        generator.definition(id, output)?;
    }
    generator.service(
        &interface.class,
        &interface.name,
        &interface.version,
        &interface.methods,
        output,
    )?;
    for resource in &interface.resources {
        generator.service(
            &resource.class,
            &resource.name,
            &interface.version,
            &resource.methods,
            output,
        )?;
    }
    interface_adapters(&generator, interface, output)?;
    output.push_str("}\n");
    Ok(())
}

/// ADR-0039：字段适配与方法适配按各自所属位置生成一次。
fn interface_adapters(
    generator: &Generator<'_>,
    interface: &InterfaceInfo,
    output: &mut String,
) -> Result<()> {
    let mut adapters = BTreeMap::new();
    for (_, definition) in generator.definitions() {
        let fields: Vec<Type> = match &definition.kind {
            TypeDefKind::Record(record) => record.fields.iter().map(|field| field.ty).collect(),
            TypeDefKind::Variant(variant) => {
                variant.cases.iter().filter_map(|case| case.ty).collect()
            }
            _ => Vec::new(),
        };
        for ty in fields {
            if let Type::Id(id) = ty
                && generator.document.properties[&id].adapter
            {
                adapters.insert(
                    format!("__wire_{}", id.index()),
                    (ValueType::Wit(ty), false),
                );
            }
        }
    }
    for method in interface.methods.iter().chain(
        interface
            .resources
            .iter()
            .flat_map(|resource| &resource.methods),
    ) {
        for value in method
            .params
            .iter()
            .map(|parameter| &parameter.ty)
            .chain([&method.result])
        {
            if let Some(name) = generator.codec(value) {
                adapters.insert(name, (value.clone(), true));
            }
        }
    }
    for (name, (value, method)) in adapters {
        crate::rust_codec::adapter(generator, &value, &name, method, output)?;
    }
    Ok(())
}

/// ADR-0039：异步值资源共享稳定接口身份与相同载荷适配器。
fn intrinsic_module(document: &Document, output: &mut String) -> Result<()> {
    output.push_str("#[rustfmt::skip] #[allow(non_snake_case)] pub mod cordis__types {\n");
    let generator = Generator {
        document,
        interface: None,
    };
    let mut adapters = BTreeMap::new();
    for service in &document.intrinsics.services {
        generator.service(
            &service.class,
            &service.name,
            "1.0",
            &service.methods,
            output,
        )?;
        for method in &service.methods {
            if let Some(name) = generator.codec(&method.result) {
                adapters.insert(name, method.result.clone());
            }
        }
    }
    for (name, value) in adapters {
        crate::rust_codec::adapter(&generator, &value, &name, true, output)?;
    }
    output.push_str("}\n");
    Ok(())
}

impl Generator<'_> {
    fn definitions(&self) -> impl Iterator<Item = (TypeId, &wit_parser::TypeDef)> {
        self.document.resolve.types.iter().filter(|(id, _)| {
            self.document
                .types
                .get(id)
                .is_some_and(|info| info.interface == self.interface)
        })
    }
    /// 资源类型使用服务代理；其余具名复合类型使用所属接口的路径。
    pub fn ty(&self, ty: Type, method: bool, lifetime: &str) -> String {
        match ty {
            Type::Bool => "bool".into(),
            Type::U8 => "u8".into(),
            Type::U16 => "u16".into(),
            Type::U32 => "u32".into(),
            Type::U64 => "u64".into(),
            Type::S8 => "i8".into(),
            Type::S16 => "i16".into(),
            Type::S32 => "i32".into(),
            Type::S64 => "i64".into(),
            Type::F32 => "f32".into(),
            Type::F64 => "f64".into(),
            Type::Char => "char".into(),
            Type::String => "String".into(),
            Type::ErrorContext => self.intrinsic(
                self.document
                    .intrinsics
                    .error_context
                    .expect("error-context 接口"),
            ),
            Type::Id(id) => {
                let definition = &self.document.resolve.types[id];
                match &definition.kind {
                    TypeDefKind::Resource => {
                        format!("::cordis_sdk::Service<dyn {}>", self.named(id))
                    }
                    TypeDefKind::Future(_) | TypeDefKind::Stream(_) => {
                        self.intrinsic(self.document.intrinsics.by_type[&id])
                    }
                    TypeDefKind::Handle(Handle::Own(resource)) => {
                        format!("::cordis_sdk::Service<dyn {}>", self.named(*resource))
                    }
                    TypeDefKind::Handle(Handle::Borrow(resource)) => format!(
                        "::cordis_sdk::BorrowedService<{lifetime}, dyn {}>",
                        self.named(*resource)
                    ),
                    _ if !method && self.document.types.contains_key(&id) => {
                        let generic = if self.document.properties[&id].borrowed {
                            format!("<{lifetime}>")
                        } else {
                            String::new()
                        };
                        format!("{}{generic}", self.named(id))
                    }
                    _ => self.structure(id, method, lifetime),
                }
            }
        }
    }

    pub fn structure(&self, id: TypeId, method: bool, lifetime: &str) -> String {
        let child = |ty| self.ty(ty, method, lifetime);
        match &self.document.resolve.types[id].kind {
            TypeDefKind::Resource
            | TypeDefKind::Handle(_)
            | TypeDefKind::Future(_)
            | TypeDefKind::Stream(_) => self.ty(Type::Id(id), method, lifetime),
            TypeDefKind::Type(ty) => child(*ty),
            TypeDefKind::Option(ty) => format!("Option<{}>", child(*ty)),
            TypeDefKind::List(ty) => format!("Vec<{}>", child(*ty)),
            TypeDefKind::FixedLengthList(ty, length) => format!("[{}; {length}]", child(*ty)),
            TypeDefKind::Map(key, value) => format!(
                "::std::collections::BTreeMap<{}, {}>",
                child(*key),
                child(*value)
            ),
            TypeDefKind::Tuple(tuple) => format!(
                "({})",
                tuple
                    .types
                    .iter()
                    .map(|ty| format!("{},", child(*ty)))
                    .collect::<String>()
            ),
            TypeDefKind::Result(result) => format!(
                "::std::result::Result<{}, {}>",
                result.ok.map(child).unwrap_or_else(|| "()".into()),
                result.err.map(child).unwrap_or_else(|| "()".into())
            ),
            _ => {
                let generic = if self.document.properties[&id].borrowed {
                    format!("<{lifetime}>")
                } else {
                    String::new()
                };
                format!("{}{generic}", self.named(id))
            }
        }
    }

    pub fn named(&self, id: TypeId) -> String {
        let info = &self.document.types[&id];
        match info.interface {
            Some(owner) if Some(owner) != self.interface => format!(
                "super::{}::{}",
                self.document.interface(owner).module,
                info.class
            ),
            _ => info.class.clone(),
        }
    }

    fn intrinsic(&self, index: usize) -> String {
        let class = &self.document.intrinsics.services[index].class;
        let prefix = if self.interface.is_some() {
            "super::cordis__types::"
        } else {
            ""
        };
        format!("::cordis_sdk::Service<dyn {prefix}{class}>")
    }

    pub(crate) fn value(&self, value: &ValueType, input: bool) -> String {
        match value {
            ValueType::Unit => "()".into(),
            ValueType::Option(ty) => format!("Option<{}>", self.value(ty, false)),
            ValueType::Wit(ty) => {
                let ty = self.ty(*ty, true, "'_");
                if input && ty == "String" {
                    "&str".into()
                } else {
                    ty
                }
            }
        }
    }

    pub(crate) fn codec(&self, value: &ValueType) -> Option<String> {
        match value {
            ValueType::Wit(Type::Id(id)) if self.method_adapter(Type::Id(*id)) => {
                Some(format!("__method_wire_{}", id.index()))
            }
            ValueType::Option(inner) => {
                if self.codec(inner).is_some() {
                    let suffix = match inner.as_ref() {
                        ValueType::Unit => "unit".into(),
                        ValueType::Wit(Type::Id(id)) => id.index().to_string(),
                        _ => "value".into(),
                    };
                    Some(format!("__method_some_wire_{suffix}"))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn method_adapter(&self, ty: Type) -> bool {
        let Type::Id(id) = ty else {
            return false;
        };
        match &self.document.resolve.types[id].kind {
            TypeDefKind::Type(inner) | TypeDefKind::Option(inner) => self.method_adapter(*inner),
            TypeDefKind::List(Type::U8) => false,
            _ => self.document.properties[&id].adapter,
        }
    }

    /// DOC-12 §7：方法顺序沿解析器声明顺序，构造器与静态方法归属接口服务。
    fn service(
        &self,
        class: &str,
        name: &str,
        version: &str,
        methods: &[MethodInfo],
        output: &mut String,
    ) -> Result<()> {
        writeln!(
            output,
            "/// DOC-15 §7：{name}@{version} 的提供方接口及类型化调用代理。\n#[cordis_sdk::interface(name = {name:?}, version = {version:?})]\npub trait {class}: Send + Sync {{"
        )?;
        for method in methods {
            docs(method.documentation.as_deref(), output)?;
            if let Some(codec) = self.codec(&method.result) {
                writeln!(output, "#[cordis(result_with = {codec})]")?;
            }
            let asynchronous = if method.asynchronous { "async " } else { "" };
            write!(output, "{asynchronous}fn {}(&self", method.rust)?;
            for parameter in &method.params {
                output.push_str(", ");
                if let Some(codec) = self.codec(&parameter.ty) {
                    write!(output, "#[cordis(with = {codec})] ")?;
                }
                write!(
                    output,
                    "{}: {}",
                    names::rust(&parameter.name),
                    self.value(&parameter.ty, true)
                )?;
            }
            writeln!(
                output,
                ") -> ::cordis_sdk::Result<{}>;",
                self.value(&method.result, false)
            )?;
        }
        output.push_str("}\n");
        Ok(())
    }

    /// ADR-0039：记录与分支保留 WIT 字段名，资源字段通过 SDK 的 Serde 作用域处理。
    fn definition(&self, id: TypeId, output: &mut String) -> Result<()> {
        let definition = &self.document.resolve.types[id];
        if matches!(definition.kind, TypeDefKind::Resource) {
            return Ok(());
        }
        let info = &self.document.types[&id];
        let class = &info.class;
        let property = self.document.properties[&id];
        let lifetime = if property.borrowed { "<'a>" } else { "" };
        docs(definition.docs.contents.as_deref(), output)?;
        if let Some(event) = &info.metadata.event {
            let version = &self
                .document
                .interface(self.interface.expect("事件所属接口"))
                .version;
            let kind = if matches!(info.metadata.kind, Some(EventKind::Async)) {
                ", kind = \"async\""
            } else {
                ""
            };
            writeln!(
                output,
                "#[cordis_sdk::event(name = {event:?}, version = {version:?}{kind})]"
            )?;
        }
        match &definition.kind {
            TypeDefKind::Record(record) => {
                derives(property.resource, property.default, output)?;
                self.record(class, lifetime, record, output)?;
            }
            TypeDefKind::Flags(flags) => {
                derives(false, property.default, output)?;
                flags_definition(class, flags, output)?;
            }
            TypeDefKind::Variant(variant) => {
                derives(property.resource, false, output)?;
                self.variant(class, lifetime, variant, output)?;
            }
            TypeDefKind::Enum(enumeration) => {
                derives(false, false, output)?;
                enum_definition(class, enumeration, output)?;
            }
            _ => writeln!(
                output,
                "pub type {class}{lifetime} = {};",
                self.structure(id, false, "'a")
            )?,
        }
        Ok(())
    }

    /// ADR-0039：记录逐字段应用其传输适配器和借用生命周期。
    fn record(
        &self,
        class: &str,
        lifetime: &str,
        record: &wit_parser::Record,
        output: &mut String,
    ) -> Result<()> {
        writeln!(output, "pub struct {class}{lifetime} {{")?;
        for field in &record.fields {
            docs(field.docs.contents.as_deref(), output)?;
            self.field(field.ty, &field.name, output)?;
            writeln!(
                output,
                "pub {}: {},",
                names::rust(&field.name),
                self.ty(field.ty, false, "'a")
            )?;
        }
        output.push_str("}\n");
        Ok(())
    }

    /// ADR-0039：空载荷分支使用 unit 新类型变体，编码为单键 null 映射。
    fn variant(
        &self,
        class: &str,
        lifetime: &str,
        variant: &wit_parser::Variant,
        output: &mut String,
    ) -> Result<()> {
        writeln!(output, "pub enum {class}{lifetime} {{")?;
        for case in &variant.cases {
            docs(case.docs.contents.as_deref(), output)?;
            writeln!(output, "#[serde(rename = {:?})]", case.name)?;
            write!(output, "{}(", names::class(&case.name))?;
            if let Some(ty) = case.ty {
                self.adapter_attribute(ty, output)?;
                write!(output, "{}", self.ty(ty, false, "'a"))?;
            } else {
                output.push_str("()");
            }
            output.push_str("),\n");
        }
        output.push_str("}\n");
        Ok(())
    }

    fn field(&self, ty: Type, name: &str, output: &mut String) -> Result<()> {
        writeln!(output, "#[serde(rename = {name:?})]")?;
        self.adapter_attribute(ty, output)
    }

    fn adapter_attribute(&self, ty: Type, output: &mut String) -> Result<()> {
        if let Type::Id(id) = ty
            && self.document.properties[&id].adapter
        {
            writeln!(output, "#[serde(with = \"__wire_{}\")]", id.index())?;
        }
        Ok(())
    }
}

fn flags_definition(class: &str, flags: &wit_parser::Flags, output: &mut String) -> Result<()> {
    writeln!(output, "pub struct {class} {{")?;
    for flag in &flags.flags {
        writeln!(
            output,
            "#[serde(rename = {:?})] pub {}: bool,",
            flag.name,
            names::rust(&flag.name)
        )?;
    }
    output.push_str("}\n");
    Ok(())
}

fn enum_definition(class: &str, enumeration: &wit_parser::Enum, output: &mut String) -> Result<()> {
    writeln!(output, "pub enum {class} {{")?;
    for case in &enumeration.cases {
        writeln!(
            output,
            "#[serde(rename = {:?})] {},",
            case.name,
            names::class(&case.name)
        )?;
    }
    output.push_str("}\n");
    Ok(())
}

fn derives(resource: bool, default: bool, output: &mut String) -> Result<()> {
    let equality = if resource { "" } else { ", PartialEq" };
    let default = if default { ", Default" } else { "" };
    writeln!(
        output,
        "#[derive(Clone, Debug, serde::Serialize, serde::Deserialize{equality}{default})]"
    )?;
    Ok(())
}

fn docs(source: Option<&str>, output: &mut String) -> Result<()> {
    if let Some(source) = source {
        writeln!(output, "#[doc = {source:?}]")?;
    }
    Ok(())
}
