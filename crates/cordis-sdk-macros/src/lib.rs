use proc_macro::TokenStream;

mod async_method;
mod event;
mod interface;
mod parse;
mod plugin;
mod types;

#[proc_macro_attribute]
pub fn interface(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let result = (|| {
        let item: syn::Item = syn::parse(item)?;
        match item {
            syn::Item::Trait(item) => {
                let identity = syn::parse::<parse::Identity>(attributes)?;
                interface::expand(identity, item)
            }
            syn::Item::Impl(item) => {
                if !attributes.is_empty() {
                    return Err(syn::Error::new_spanned(
                        &item,
                        "接口实现直接标注 #[interface]",
                    ));
                }
                async_method::lower_impl(item)
            }
            _ => Err(syn::Error::new_spanned(
                item,
                "interface 用于接口 trait 或其 impl",
            )),
        }
    })();
    parse::finish(result)
}

#[proc_macro_attribute]
pub fn event(attributes: TokenStream, item: TokenStream) -> TokenStream {
    parse::finish((|| {
        event::expand(syn::parse(attributes)?, syn::parse(item)?)
    })())
}

#[proc_macro]
pub fn export_plugin(input: TokenStream) -> TokenStream {
    parse::finish(syn::parse(input).and_then(plugin::expand))
}
