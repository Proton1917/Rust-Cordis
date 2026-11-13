//! DOC-15 §7、ADR-0039：Python 和 TypeScript 使用同一类型图与显式传输接口。

use crate::{Document, MethodInfo, Result, ValueType, names, schema};
use std::fmt::Write;
use wit_parser::{Type, TypeDefKind, TypeId};

pub fn python(document: &Document) -> Result<String> {
    let mut output = String::from(include_str!("templates/runtime.py"));
    writeln!(
        output,
        "\nSCHEMA = json.loads({:?})\n",
        serde_json::to_string(&schema::document(document))?
    )?;
    let generator = Generator {
        document,
        python: true,
    };
    generator.services(&mut output)?;
    for (id, _) in document
        .resolve
        .types
        .iter()
        .filter(|(id, _)| document.types.contains_key(id))
    {
        generator.definition(id, &mut output)?;
    }
    let names = generator.runtime_classes();
    writeln!(
        output,
        "\n_CLASSES = {{{}}}",
        names
            .iter()
            .map(|name| format!("{name:?}: {name}"))
            .collect::<Vec<_>>()
            .join(", ")
    )?;
    writeln!(
        output,
        "_CLIENTS = {{{}}}",
        generator
            .service_names()
            .iter()
            .map(|name| format!("{name:?}: {name}Client"))
            .collect::<Vec<_>>()
            .join(", ")
    )?;
    Ok(output)
}

pub fn typescript(document: &Document) -> Result<String> {
    let mut output = String::from(include_str!("templates/runtime.ts"));
    writeln!(
        output,
        "\nexport const SCHEMA: Schema = {};\n",
        serde_json::to_string_pretty(&schema::document(document))?
    )?;
    let generator = Generator {
        document,
        python: false,
    };
    generator.services(&mut output)?;
    for (id, _) in document
        .resolve
        .types
        .iter()
        .filter(|(id, _)| document.types.contains_key(id))
    {
        generator.definition(id, &mut output)?;
    }
    writeln!(
        output,
        "\nconst CLIENTS: Record<string, new (transport: Transport) => unknown> = {{{}}};",
        generator
            .service_names()
            .join(", ")
            .split(", ")
            .filter(|name| !name.is_empty())
            .map(|name| format!("{name}: {name}Client"))
            .collect::<Vec<_>>()
            .join(", ")
    )?;
    Ok(output)
}

struct Generator<'a> {
    document: &'a Document,
    python: bool,
}

impl Generator<'_> {
    fn service_names(&self) -> Vec<String> {
        self.document
            .interfaces
            .iter()
            .flat_map(|interface| {
                std::iter::once(interface.class.clone()).chain(
                    interface
                        .resources
                        .iter()
                        .map(|resource| schema::class(self.document, resource.ty)),
                )
            })
            .chain(
                self.document
                    .intrinsics
                    .services
                    .iter()
                    .map(|service| service.class.clone()),
            )
            .collect()
    }

    fn runtime_classes(&self) -> Vec<String> {
        let mut classes = Vec::new();
        for (id, definition) in self
            .document
            .resolve
            .types
            .iter()
            .filter(|(id, _)| self.document.types.contains_key(id))
        {
            match &definition.kind {
                TypeDefKind::Record(_) | TypeDefKind::Flags(_) | TypeDefKind::Enum(_) => {
                    classes.push(schema::class(self.document, id))
                }
                TypeDefKind::Variant(variant) => classes.extend(variant.cases.iter().map(|case| {
                    format!(
                        "{}{}",
                        schema::class(self.document, id),
                        names::class(&case.name)
                    )
                })),
                _ => {}
            }
        }
        classes
    }

    fn ty(&self, ty: Type, method: bool) -> String {
        match ty {
            Type::Bool => if self.python { "bool" } else { "boolean" }.into(),
            Type::String | Type::Char => if self.python { "str" } else { "string" }.into(),
            Type::F32 | Type::F64 => if self.python { "float" } else { "number" }.into(),
            Type::U64 | Type::S64 if !self.python => "bigint".into(),
            Type::ErrorContext => "ErrorContextClient".into(),
            Type::Id(id) => match &self.document.resolve.types[id].kind {
                TypeDefKind::Resource => format!("{}Client", schema::class(self.document, id)),
                TypeDefKind::Handle(handle) => format!(
                    "{}Client",
                    schema::class(
                        self.document,
                        match handle {
                            wit_parser::Handle::Own(id) | wit_parser::Handle::Borrow(id) => *id,
                        }
                    )
                ),
                TypeDefKind::Future(_) | TypeDefKind::Stream(_) => format!(
                    "{}Client",
                    self.document.intrinsics.services[self.document.intrinsics.by_type[&id]].class
                ),
                TypeDefKind::Type(inner) if method => self.ty(*inner, method),
                TypeDefKind::Option(_) | TypeDefKind::List(_) if method => {
                    self.structure(id, method)
                }
                _ if self.document.types.contains_key(&id) => schema::class(self.document, id),
                _ => self.structure(id, false),
            },
            _ => if self.python { "int" } else { "number" }.into(),
        }
    }

    fn structure(&self, id: TypeId, method: bool) -> String {
        let child = |ty| self.ty(ty, false);
        match &self.document.resolve.types[id].kind {
            TypeDefKind::Type(inner) => self.ty(*inner, method),
            TypeDefKind::Option(inner) => {
                let value = self.ty(*inner, method);
                if self.python {
                    format!("{value} | None | Some[{value}]")
                } else {
                    format!("Option<{value}>")
                }
            }
            TypeDefKind::List(Type::U8) if method => {
                if self.python { "bytes" } else { "Uint8Array" }.into()
            }
            TypeDefKind::List(inner) | TypeDefKind::FixedLengthList(inner, _) => {
                if self.python {
                    format!("list[{}]", child(*inner))
                } else {
                    format!("Array<{}>", child(*inner))
                }
            }
            TypeDefKind::Tuple(tuple) => {
                let values = tuple
                    .types
                    .iter()
                    .map(|ty| child(*ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                if self.python {
                    if values.is_empty() {
                        "tuple[()]".into()
                    } else {
                        format!("tuple[{values}]")
                    }
                } else {
                    format!("[{values}]")
                }
            }
            TypeDefKind::Map(key, value) => {
                if self.python {
                    format!("dict[{}, {}]", child(*key), child(*value))
                } else {
                    format!("Map<{}, {}>", child(*key), child(*value))
                }
            }
            TypeDefKind::Result(result) => {
                let unit = if self.python { "None" } else { "null" };
                let ok = result.ok.map(child).unwrap_or_else(|| unit.into());
                let error = result.err.map(child).unwrap_or_else(|| unit.into());
                if self.python {
                    format!("Ok[{ok}] | Err[{error}]")
                } else {
                    format!("ResultValue<{ok}, {error}>")
                }
            }
            _ => self.ty(Type::Id(id), false),
        }
    }

    fn value(&self, value: &ValueType) -> String {
        match value {
            ValueType::Unit => if self.python { "None" } else { "void" }.into(),
            ValueType::Wit(ty) => self.ty(*ty, true),
            ValueType::Option(inner) => {
                let inner = self.value(inner);
                if self.python {
                    format!("{inner} | None | Some[{inner}]")
                } else {
                    format!("Option<{inner}>")
                }
            }
        }
    }

    /// 提供方保留源异步声明；远程代理始终通过异步传输取得结果。
    fn services(&self, output: &mut String) -> Result<()> {
        for interface in &self.document.interfaces {
            self.service(
                &interface.class,
                &interface.name,
                &interface.version,
                &interface.methods,
                output,
            )?;
            for resource in &interface.resources {
                self.service(
                    &schema::class(self.document, resource.ty),
                    &resource.name,
                    &interface.version,
                    &resource.methods,
                    output,
                )?;
            }
        }
        for intrinsic in &self.document.intrinsics.services {
            self.service(
                &intrinsic.class,
                &intrinsic.name,
                "1.0",
                &intrinsic.methods,
                output,
            )?;
        }
        Ok(())
    }

    fn service(
        &self,
        class: &str,
        name: &str,
        version: &str,
        methods: &[MethodInfo],
        output: &mut String,
    ) -> Result<()> {
        if self.python {
            writeln!(output, "class {class}(Protocol):")?;
            if methods.is_empty() {
                output.push_str("    pass\n");
            }
            for method in methods {
                self.signature(method, false, output)?;
            }
            writeln!(
                output,
                "\nclass {class}Client:\n    def __init__(self, transport: Transport):\n        self._transport = transport"
            )?;
        } else {
            writeln!(output, "export interface {class} {{")?;
            for method in methods {
                self.signature(method, false, output)?;
            }
            writeln!(
                output,
                "}}\nexport class {class}Client {{\n    constructor(private readonly transport: Transport) {{}}"
            )?;
        }
        for (index, method) in methods.iter().enumerate() {
            self.signature(method, true, output)?;
            let args = method
                .params
                .iter()
                .map(|param| {
                    if self.python {
                        names::python(&param.name)
                    } else {
                        names::typescript(&param.name)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            if self.python {
                writeln!(
                    output,
                    "        return await _invoke(self._transport, {name:?} + \"@{version}\", {index}, [{args}])"
                )?;
            } else {
                writeln!(
                    output,
                    "        return await invoke(this.transport, \"{name}@{version}\", {index}, [{args}]) as {};\n    }}",
                    self.value(&method.result)
                )?;
            }
        }
        if !self.python {
            output.push_str("}\n");
        }
        output.push('\n');
        Ok(())
    }

    fn signature(&self, method: &MethodInfo, client: bool, output: &mut String) -> Result<()> {
        let params = method
            .params
            .iter()
            .map(|param| {
                format!(
                    "{}: {}",
                    if self.python {
                        names::python(&param.name)
                    } else {
                        names::typescript(&param.name)
                    },
                    self.value(&param.ty)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let result = self.value(&method.result);
        if self.python {
            let asynchronous = if client || method.asynchronous {
                "async "
            } else {
                ""
            };
            let comma = if params.is_empty() { "" } else { ", " };
            let ending = if client { "" } else { " ..." };
            writeln!(
                output,
                "    {asynchronous}def {}(self{comma}{params}) -> {result}:{ending}",
                method.python
            )?;
        } else {
            let result = if client || method.asynchronous {
                format!("Promise<{result}>")
            } else {
                result
            };
            let asynchronous = if client { "async " } else { "" };
            let ending = if client { " {" } else { ";" };
            writeln!(
                output,
                "    {asynchronous}{}({params}): {result}{ending}",
                method.typescript
            )?;
        }
        Ok(())
    }

    fn definition(&self, id: TypeId, output: &mut String) -> Result<()> {
        let definition = &self.document.resolve.types[id];
        if matches!(definition.kind, TypeDefKind::Resource) {
            return Ok(());
        }
        let class = schema::class(self.document, id);
        match &definition.kind {
            TypeDefKind::Record(record) => {
                let fields: Vec<_> = record
                    .fields
                    .iter()
                    .map(|field| (field.name.as_str(), field.ty))
                    .collect();
                self.record(&class, &fields, output)?;
            }
            TypeDefKind::Flags(flags) => {
                let fields: Vec<_> = flags
                    .flags
                    .iter()
                    .map(|flag| (flag.name.as_str(), Type::Bool))
                    .collect();
                self.record(&class, &fields, output)?;
            }
            TypeDefKind::Variant(variant) => {
                self.variant(&class, variant, output)?;
            }
            TypeDefKind::Enum(enumeration) => {
                self.enumeration(&class, enumeration, output)?;
            }
            _ => {
                let ty = self.structure(id, false);
                if self.python {
                    writeln!(output, "{class} = {ty}\n")?;
                } else {
                    writeln!(output, "export type {class} = {ty};")?;
                }
            }
        }
        Ok(())
    }

    /// DOC-15 §7：Python 使用具名变体，TypeScript 使用 kind 判别联合。
    fn variant(
        &self,
        class: &str,
        variant: &wit_parser::Variant,
        output: &mut String,
    ) -> Result<()> {
        let mut variants = Vec::new();
        for case in &variant.cases {
            if self.python {
                let branch = format!("{class}{}", names::class(&case.name));
                writeln!(
                    output,
                    "@dataclass\nclass {branch}:\n    kind: ClassVar[str] = {:?}",
                    case.name
                )?;
                if let Some(ty) = case.ty {
                    writeln!(output, "    value: {}", self.ty(ty, false))?;
                }
                variants.push(branch);
            } else {
                variants.push(if let Some(ty) = case.ty {
                    format!("{{ kind: {:?}; value: {} }}", case.name, self.ty(ty, false))
                } else {
                    format!("{{ kind: {:?} }}", case.name)
                });
            }
        }
        if self.python {
            writeln!(output, "{class} = {}\n", variants.join(" | "))?;
        } else {
            writeln!(output, "export type {class} = {};", variants.join(" | "))?;
        }
        Ok(())
    }

    fn enumeration(
        &self,
        class: &str,
        enumeration: &wit_parser::Enum,
        output: &mut String,
    ) -> Result<()> {
        if self.python {
            writeln!(output, "class {class}(Enum):")?;
            for case in &enumeration.cases {
                writeln!(
                    output,
                    "    {} = {:?}",
                    names::python(&case.name).to_uppercase(),
                    case.name
                )?;
            }
            output.push('\n');
        } else {
            let values = enumeration
                .cases
                .iter()
                .map(|case| format!("{:?}", case.name))
                .collect::<Vec<_>>()
                .join(" | ");
            writeln!(output, "export type {class} = {values};")?;
        }
        Ok(())
    }

    fn record(&self, class: &str, fields: &[(&str, Type)], output: &mut String) -> Result<()> {
        if self.python {
            writeln!(output, "@dataclass\nclass {class}:")?;
        } else {
            writeln!(output, "export interface {class} {{")?;
        }
        if self.python && fields.is_empty() {
            output.push_str("    pass\n");
        }
        for (name, ty) in fields {
            let name = if self.python {
                names::python(name)
            } else {
                names::typescript(name)
            };
            let ending = if self.python { "" } else { ";" };
            writeln!(output, "    {name}: {}{ending}", self.ty(*ty, false))?;
        }
        if !self.python {
            output.push_str("}\n");
        }
        output.push('\n');
        Ok(())
    }
}
