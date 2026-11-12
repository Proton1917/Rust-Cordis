use crate::parse::Identity;
use quote::quote;

pub fn expand(identity: Identity, item: syn::ItemStruct) -> syn::Result<proc_macro2::TokenStream> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "事件类型使用具体字段类型",
        ));
    }
    let ident = &item.ident;
    let full_name = identity.full_name();
    let major = identity.major;
    let minor = identity.minor;
    let kind = identity
        .kind
        .as_ref()
        .map(syn::LitStr::value)
        .unwrap_or_else(|| "sync".into());
    let family = match kind.as_str() {
        "sync" => quote!(::cordis_sdk::SyncEvent),
        "async" => quote!(::cordis_sdk::AsyncEvent),
        _ => {
            return Err(syn::Error::new_spanned(
                identity.kind,
                "kind 接受 sync 或 async",
            ));
        }
    };
    Ok(quote! {
        #item
        impl ::cordis_sdk::Event for #ident {
            const INTERFACE: &'static str = #full_name;
            const MAJOR: u32 = #major;
            const MINOR: u32 = #minor;
        }
        impl #family for #ident {}
    })
}
