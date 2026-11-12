use proc_macro2::TokenStream;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, Ident, LitStr, Result, Token, Type, bracketed};

pub struct Identity {
    pub name: LitStr,
    pub major: u32,
    pub minor: u32,
    pub kind: Option<LitStr>,
}

impl Parse for Identity {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut name = None;
        let mut version = None;
        let mut kind = None;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let value: LitStr = input.parse()?;
            match key.to_string().as_str() {
                "name" => name = Some(value),
                "version" => version = Some(value),
                "kind" => kind = Some(value),
                _ => return Err(syn::Error::new_spanned(key, "接受 name、version 与 kind")),
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        let name = name.ok_or_else(|| input.error("请声明 name"))?;
        let version = version.ok_or_else(|| input.error("请声明 version"))?;
        let fields: Vec<_> = version.value().split('.').map(str::to_owned).collect();
        if fields.len() != 2 {
            return Err(syn::Error::new_spanned(version, "版本格式为 主版本.次版本"));
        }
        let major = fields[0]
            .parse()
            .map_err(|_| syn::Error::new_spanned(&version, "主版本使用 u32"))?;
        let minor = fields[1]
            .parse()
            .map_err(|_| syn::Error::new_spanned(&version, "次版本使用 u32"))?;
        Ok(Self {
            name,
            major,
            minor,
            kind,
        })
    }
}

impl Identity {
    pub fn full_name(&self) -> LitStr {
        LitStr::new(
            &format!("{}@{}.{}", self.name.value(), self.major, self.minor),
            self.name.span(),
        )
    }
}

pub struct Plugin {
    pub name: Expr,
    pub version: Expr,
    pub components: Vec<Type>,
    pub rlib_feature: Option<LitStr>,
}

impl Parse for Plugin {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut name = None;
        let mut version = None;
        let mut components = None;
        let mut rlib_feature = None;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "name" => name = Some(input.parse()?),
                "version" => version = Some(input.parse()?),
                "rlib_feature" => rlib_feature = Some(input.parse()?),
                "components" => {
                    let content;
                    bracketed!(content in input);
                    components = Some(
                        content
                            .parse_terminated(Type::parse, Token![,])?
                            .into_iter()
                            .collect(),
                    );
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        key,
                        "接受 name、version、components 与 rlib_feature",
                    ));
                }
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self {
            name: name.ok_or_else(|| input.error("请声明 name"))?,
            version: version.ok_or_else(|| input.error("请声明 version"))?,
            components: components.ok_or_else(|| input.error("请声明 components"))?,
            rlib_feature,
        })
    }
}

pub fn result_type(signature: &syn::Signature) -> Result<Type> {
    let syn::ReturnType::Type(_, result) = &signature.output else {
        return Err(syn::Error::new_spanned(signature, "接口方法返回 Result<T>"));
    };
    let Type::Path(path) = result.as_ref() else {
        return Err(syn::Error::new_spanned(result, "接口方法返回 Result<T>"));
    };
    let segment = path.path.segments.last().unwrap();
    if segment.ident != "Result" {
        return Err(syn::Error::new_spanned(result, "接口方法返回 Result<T>"));
    }
    match &segment.arguments {
        syn::PathArguments::None => Ok(syn::parse_quote!(())),
        syn::PathArguments::AngleBracketed(arguments) => match arguments.args.first() {
            Some(syn::GenericArgument::Type(value)) if arguments.args.len() == 1 => {
                Ok(value.clone())
            }
            _ => Err(syn::Error::new_spanned(result, "Result 使用 SDK 错误类型")),
        },
        _ => Err(syn::Error::new_spanned(result, "接口方法返回 Result<T>")),
    }
}

pub fn finish(result: Result<TokenStream>) -> proc_macro::TokenStream {
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}
