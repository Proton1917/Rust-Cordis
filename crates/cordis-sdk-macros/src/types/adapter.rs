//! ADR-0039：聚合值使用生成的 Serde 适配器，类型化入口保留原有 Option 与 Buf 布局。

use super::Mapping;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Path};

#[derive(Clone)]
pub(crate) struct Adapter {
    pub base: Box<Mapping>,
    pub codec: Path,
}

impl Adapter {
    pub fn new(base: Mapping, codec: Path) -> syn::Result<Self> {
        fn aggregate(mapping: &Mapping) -> bool {
            match mapping {
                Mapping::Cbor(_) => true,
                Mapping::Option(inner) => aggregate(inner),
                _ => false,
            }
        }
        if !aggregate(&base) {
            return Err(syn::Error::new_spanned(
                codec,
                "适配器接受 CBOR 聚合及其外层 Option",
            ));
        }
        Ok(Self {
            base: Box::new(base),
            codec,
        })
    }

    /// DOC-12 §5：适配整值后拆出原有 Option 标志，导出资源由最内层 Buf 持有。
    pub fn prepare(&self, value: TokenStream, environment: TokenStream) -> TokenStream {
        let codec = &self.codec;
        let lowered = lower(
            &self.base,
            quote!(__cordis_adapted),
            quote!(__cordis_encoder),
        );
        quote!({
            let mut __cordis_encoder = ::cordis_sdk::__private::abi::CborEncoder::new(#environment);
            let __cordis_adapted = __cordis_encoder.value(&#codec::Encode(&#value))?;
            #lowered
        })
    }

    /// ADR-0039：输入缓冲保留至整值完成反序列化，随后归还原始导出引用。
    pub fn decode(&self, value: TokenStream) -> TokenStream {
        let codec = &self.codec;
        let recovered = recover(&self.base, quote!(&__cordis_adapted_owned));
        let release = release(&self.base, quote!(__cordis_adapted_owned));
        quote!({
            let __cordis_adapted_owned = #value;
            let __cordis_adapted = #recovered;
            let __cordis_result = ::cordis_sdk::__private::abi::from_value_in::<#codec::Decode>(
                __cordis_adapted, __cordis_environment,
            )?.0;
            #release
            ::cordis_sdk::Result::Ok(__cordis_result)
        })
    }

    pub fn encode_cbor(&self, value: TokenStream, encoder: TokenStream) -> TokenStream {
        let codec = &self.codec;
        quote!(#encoder.value(&#codec::Encode(&#value))?)
    }

    pub fn decode_cbor(&self, value: TokenStream, environment: TokenStream) -> TokenStream {
        let codec = &self.codec;
        quote!(::cordis_sdk::__private::abi::from_value_in::<#codec::Decode>(#value, #environment)
            .map(|__cordis_decoded| __cordis_decoded.0))
    }
}

fn lower(mapping: &Mapping, value: TokenStream, encoder: TokenStream) -> TokenStream {
    match mapping {
        Mapping::Option(inner) => {
            let lowered = lower(inner, quote!(__cordis_item), encoder);
            quote!(match ::cordis_sdk::__private::abi::option_take(#value) {
                Some(__cordis_item) => Some(#lowered),
                None => None,
            })
        }
        Mapping::Cbor(_) => quote!(#encoder.finish(#value)?),
        _ => unreachable!("适配器构造已限定映射结构"),
    }
}

fn recover(mapping: &Mapping, value: TokenStream) -> TokenStream {
    match mapping {
        Mapping::Option(inner) => {
            let recovered = recover(inner, quote!(__cordis_item));
            quote!(match #value {
                Some(__cordis_item) => ::cordis_sdk::__private::abi::option_some(#recovered),
                None => ::cordis_sdk::__private::abi::CborValue::Null,
            })
        }
        Mapping::Cbor(_) => {
            quote!(::cordis_sdk::__private::abi::decode_value((#value).as_slice())?)
        }
        _ => unreachable!("适配器构造已限定映射结构"),
    }
}

fn release(mapping: &Mapping, value: TokenStream) -> TokenStream {
    match mapping {
        Mapping::Option(inner) => {
            let released = release(inner, quote!(__cordis_item));
            quote!(if let Some(__cordis_item) = #value { #released })
        }
        Mapping::Cbor(_) => quote!(#value.release()?;),
        _ => unreachable!("适配器构造已限定映射结构"),
    }
}

/// ADR-0039：仅消费当前位置的 cordis 属性，其余 Rust 属性按原顺序保留。
pub(crate) fn take(attributes: &mut Vec<Attribute>, key: &str) -> syn::Result<Option<Path>> {
    let mut result = None;
    for attribute in std::mem::take(attributes) {
        if !attribute.path().is_ident("cordis") {
            attributes.push(attribute);
            continue;
        }
        attribute.parse_nested_meta(|meta| {
            if !meta.path.is_ident(key) {
                return Err(meta.error(format!("此处接受 {key} 适配器")));
            }
            if result.is_some() {
                return Err(meta.error("同一位置声明一个适配器"));
            }
            result = Some(meta.value()?.parse::<Path>()?);
            Ok(())
        })?;
    }
    Ok(result)
}
