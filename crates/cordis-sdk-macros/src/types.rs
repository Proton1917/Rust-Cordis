use proc_macro2::TokenStream;
use quote::quote;
use syn::{GenericArgument, PathArguments, Type};

pub(crate) mod adapter;

#[derive(Clone)]
pub enum Mapping {
    Unit,
    Primitive(Type),
    Bool,
    Str,
    Bytes,
    String,
    VecBytes,
    Service(Type),
    Option(Box<Mapping>),
    Cbor(Type),
    Adapted(adapter::Adapter),
}

impl Mapping {
    pub fn adapted(self, codec: Option<syn::Path>) -> syn::Result<Self> {
        match codec {
            Some(codec) => adapter::Adapter::new(self, codec).map(Self::Adapted),
            None => Ok(self),
        }
    }
    pub fn parse(ty: &Type) -> syn::Result<Self> {
        match ty {
            Type::Tuple(tuple) if tuple.elems.is_empty() => return Ok(Self::Unit),
            Type::Reference(reference) => {
                if reference.mutability.is_some() {
                    return Err(syn::Error::new_spanned(ty, "接口借用参数使用共享引用"));
                }
                return match reference.elem.as_ref() {
                    Type::Path(path) if path.path.is_ident("str") => Ok(Self::Str),
                    Type::Slice(slice) if is_u8(&slice.elem) => Ok(Self::Bytes),
                    _ => Err(syn::Error::new_spanned(ty, "借用参数支持 &str 与 &[u8]")),
                };
            }
            _ => {}
        }
        let Type::Path(path) = ty else {
            return Ok(Self::Cbor(ty.clone()));
        };
        let segment = path.path.segments.last().unwrap();
        let name = segment.ident.to_string();
        match name.as_str() {
            "bool" => Ok(Self::Bool),
            "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64" => {
                Ok(Self::Primitive(ty.clone()))
            }
            "String" => Ok(Self::String),
            "Option" => Ok(Self::Option(Box::new(Self::parse(type_argument(
                segment,
            )?)?))),
            "Vec" if is_u8(type_argument(segment)?) => Ok(Self::VecBytes),
            "Service" => Ok(Self::Service(type_argument(segment)?.clone())),
            _ => Ok(Self::Cbor(ty.clone())),
        }
    }

    pub fn wire(&self) -> TokenStream {
        match self {
            Self::Adapted(adapter) => adapter.base.wire(),
            Self::Unit => quote!(::cordis_sdk::__private::abi::AbiUnit),
            Self::Primitive(ty) => quote!(#ty),
            Self::Bool => quote!(u8),
            Self::Str => quote!(::cordis_sdk::__private::abi::AbiStr),
            Self::Bytes => quote!(::cordis_sdk::__private::abi::AbiBytes),
            Self::String | Self::VecBytes | Self::Cbor(_) => {
                quote!(::cordis_sdk::__private::abi::AbiBuf)
            }
            Self::Service(_) => quote!(::cordis_sdk::__private::abi::AbiObject),
            Self::Option(inner) => {
                let value = inner.wire();
                quote!(::cordis_sdk::__private::abi::AbiOption<#value>)
            }
        }
    }

    pub fn borrowed(&self) -> bool {
        match self {
            Self::Str | Self::Bytes => true,
            Self::Option(inner) => inner.borrowed(),
            _ => false,
        }
    }

    pub fn contains_service(&self) -> bool {
        match self {
            Self::Service(_) | Self::Cbor(_) | Self::Adapted(_) => true,
            Self::Option(inner) => inner.contains_service(),
            _ => false,
        }
    }

    pub fn async_borrow(&self, value: TokenStream) -> TokenStream {
        match self {
            Self::Str => quote!(#value.as_str()),
            Self::Bytes => quote!(#value.as_slice()),
            Self::Option(inner) if inner.borrowed() => {
                let borrowed = inner.async_borrow(quote!(__cordis_value));
                quote!(#value.as_ref().map(|__cordis_value| #borrowed))
            }
            _ => value,
        }
    }

    pub fn async_copy(&self, value: TokenStream) -> TokenStream {
        match self {
            Self::Str | Self::Bytes => quote!(#value.to_owned()),
            Self::Option(inner) if inner.borrowed() => {
                let owned = inner.async_copy(quote!(__cordis_value));
                quote!(#value.map(|__cordis_value| #owned))
            }
            _ => value,
        }
    }

    pub fn prepare(&self, value: TokenStream, environment: TokenStream) -> TokenStream {
        match self {
            Self::Adapted(adapter) => adapter.prepare(value, environment),
            Self::Unit => {
                quote!({ let () = #value; ::cordis_sdk::__private::abi::AbiUnit::default() })
            }
            Self::Primitive(_) => value,
            Self::Bool => quote!(u8::from(#value)),
            Self::Str => quote!(::cordis_sdk::__private::abi::AbiStr::new(#value)),
            Self::Bytes => quote!(::cordis_sdk::__private::abi::AbiBytes::new(#value)),
            Self::String => {
                quote!(::cordis_sdk::__private::abi::Buf::new(#value.into_bytes(), #environment)?)
            }
            Self::VecBytes => quote!(::cordis_sdk::__private::abi::Buf::new(#value, #environment)?),
            Self::Cbor(_) => {
                quote!(::cordis_sdk::__private::abi::CborEncoder::new(#environment).encode(&#value)?)
            }
            Self::Service(_) => quote!(#value.clone_object()?),
            Self::Option(inner) => {
                let prepared = inner.prepare(quote!(__cordis_value), environment);
                quote!(match #value { Some(__cordis_value) => Some(#prepared), None => None })
            }
        }
    }

    pub fn encode_raw(&self, value: TokenStream) -> TokenStream {
        match self {
            Self::Adapted(adapter) => adapter.base.encode_raw(value),
            Self::String | Self::VecBytes | Self::Cbor(_) | Self::Service(_) => {
                quote!(#value.into_raw())
            }
            Self::Option(inner) => {
                let raw = inner.encode_raw(quote!(__cordis_value));
                quote!(match #value {
                    Some(__cordis_value) => ::cordis_sdk::__private::abi::AbiOption::some(#raw),
                    None => ::cordis_sdk::__private::abi::AbiOption::none(),
                })
            }
            _ => value,
        }
    }

    pub fn adopt(&self, value: TokenStream, environment: TokenStream) -> TokenStream {
        match self {
            Self::Adapted(adapter) => adapter.base.adopt(value, environment),
            Self::String | Self::VecBytes | Self::Cbor(_) => quote!({
                // SAFETY: DOC-12 §5，调用方转入缓冲的唯一所有权及既有租约。
                unsafe { ::cordis_sdk::__private::abi::Buf::from_raw(#value, #environment) }
            }),
            Self::Service(_) => quote!({
                // SAFETY: DOC-12 §5、§11，服务对象与既有租约一同转入。
                unsafe { ::cordis_sdk::__private::abi::Object::from_raw(#value, #environment) }
            }),
            Self::Option(inner) => {
                let adopted = inner.adopt(quote!(__cordis_value), environment);
                quote!({
                    // SAFETY: DOC-12 §5，宏按同一 Option 映射产生标志与载荷。
                    match unsafe { #value.take() } {
                        Some(__cordis_value) => Some(#adopted),
                        None => None,
                    }
                })
            }
            _ => value,
        }
    }

    pub fn decode(&self, value: TokenStream) -> TokenStream {
        match self {
            Self::Adapted(adapter) => adapter.decode(value),
            Self::Unit => quote!({ let _ = #value; ::cordis_sdk::Result::Ok(()) }),
            Self::Primitive(_) => quote!(::cordis_sdk::Result::Ok(#value)),
            Self::Bool => quote!(::cordis_sdk::__private::decode_bool(#value)),
            Self::Str => quote!({
                // SAFETY: DOC-12 §5，字符串借用覆盖当前方法调用。
                unsafe { #value.as_str() }
            }),
            Self::Bytes => quote!({
                // SAFETY: DOC-12 §5，字节借用覆盖当前方法调用。
                ::cordis_sdk::Result::Ok(unsafe { #value.as_slice() })
            }),
            Self::String => quote!(#value.into_string()),
            Self::VecBytes => quote!(#value.into_vec()),
            Self::Service(interface) => {
                quote!(::cordis_sdk::Service::<#interface>::from_object(#value))
            }
            Self::Cbor(ty) => quote!({
                let __cordis_decoded = ::cordis_sdk::__private::abi::decode_in::<#ty>(#value.as_slice(), __cordis_environment)?;
                #value.release()?;
                ::cordis_sdk::Result::Ok(__cordis_decoded)
            }),
            Self::Option(inner) => {
                let decoded = inner.decode(quote!(__cordis_value));
                quote!(match #value {
                    Some(__cordis_value) => #decoded.map(Some),
                    None => ::cordis_sdk::Result::Ok(None),
                })
            }
        }
    }

    pub fn encode_cbor(&self, value: TokenStream, encoder: TokenStream) -> TokenStream {
        match self {
            Self::Adapted(adapter) => adapter.encode_cbor(value, encoder),
            Self::Unit => {
                quote!({ let () = #value; ::cordis_sdk::__private::abi::CborValue::Null })
            }
            Self::Primitive(_) | Self::Bool => {
                quote!(::cordis_sdk::__private::abi::to_value(&#value)?)
            }
            Self::Cbor(_) => quote!(#encoder.value(&#value)?),
            Self::Str => quote!(::cordis_sdk::__private::abi::CborValue::Text(#value.to_owned())),
            Self::String => quote!(::cordis_sdk::__private::abi::CborValue::Text(#value)),
            Self::Bytes => {
                quote!(::cordis_sdk::__private::abi::CborValue::Bytes(#value.to_owned()))
            }
            Self::VecBytes => quote!(::cordis_sdk::__private::abi::CborValue::Bytes(#value)),
            Self::Service(_) => quote!(#encoder.export(#value.clone_object()?)?),
            Self::Option(inner) => {
                let encoded = inner.encode_cbor(quote!(__cordis_value), encoder);
                quote!(match #value {
                    Some(__cordis_value) => ::cordis_sdk::__private::abi::option_some(#encoded),
                    None => ::cordis_sdk::__private::abi::CborValue::Null,
                })
            }
        }
    }

    pub fn decode_cbor(&self, value: TokenStream, environment: TokenStream) -> TokenStream {
        match self {
            Self::Adapted(adapter) => adapter.decode_cbor(value, environment),
            Self::Unit => quote!(::cordis_sdk::__private::abi::from_value::<()>(#value)),
            Self::Primitive(ty) => {
                quote!(::cordis_sdk::__private::abi::from_value::<#ty>(#value))
            }
            Self::Cbor(ty) => {
                quote!(::cordis_sdk::__private::abi::from_value_in::<#ty>(#value, #environment))
            }
            Self::Bool => quote!(::cordis_sdk::__private::abi::from_value::<bool>(#value)),
            Self::Str | Self::String => {
                quote!(::cordis_sdk::__private::abi::from_value::<::std::string::String>(#value))
            }
            Self::Bytes | Self::VecBytes => quote!(match #value {
                ::cordis_sdk::__private::abi::CborValue::Bytes(bytes) => ::cordis_sdk::Result::Ok(bytes),
                value => ::cordis_sdk::__private::abi::from_value::<::cordis_sdk::Bytes>(value).map(|bytes| bytes.0),
            }),
            Self::Service(interface) => quote!(#environment.host().import_resource(&#value)
                .and_then(::cordis_sdk::Service::<#interface>::from_object)),
            Self::Option(inner) => {
                let decoded = inner.decode_cbor(quote!(__cordis_value), environment);
                quote!(match ::cordis_sdk::__private::abi::option_take(#value) {
                    Some(__cordis_value) => #decoded.map(Some),
                    None => ::cordis_sdk::Result::Ok(None),
                })
            }
        }
    }
}

fn is_u8(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.path.is_ident("u8"))
}

fn type_argument(segment: &syn::PathSegment) -> syn::Result<&Type> {
    match &segment.arguments {
        PathArguments::AngleBracketed(arguments) => match arguments.args.first() {
            Some(GenericArgument::Type(ty)) if arguments.args.len() == 1 => Ok(ty),
            _ => Err(syn::Error::new_spanned(
                segment,
                "该类型接受一个具体类型参数",
            )),
        },
        _ => Err(syn::Error::new_spanned(segment, "请声明具体类型参数")),
    }
}
