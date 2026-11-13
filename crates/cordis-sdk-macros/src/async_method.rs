use syn::visit_mut::VisitMut;
use syn::{ImplItem, ItemImpl, Lifetime, Signature, TypeReference};

struct BorrowLifetime(Lifetime);

impl VisitMut for BorrowLifetime {
    fn visit_lifetime_mut(&mut self, lifetime: &mut Lifetime) {
        if lifetime.ident == "_" {
            *lifetime = self.0.clone();
        }
    }
    fn visit_type_reference_mut(&mut self, reference: &mut TypeReference) {
        if reference.lifetime.is_none() {
            reference.lifetime = Some(self.0.clone());
        }
        syn::visit_mut::visit_type_reference_mut(self, reference);
    }
}

pub fn lower_signature(signature: &mut Signature) {
    if signature.asyncness.take().is_none() {
        return;
    }
    let lifetime: Lifetime = syn::parse_quote!('__cordis);
    signature
        .generics
        .params
        .insert(0, syn::parse_quote!('__cordis));
    for argument in &mut signature.inputs {
        match argument {
            syn::FnArg::Receiver(receiver) => {
                if let Some((_, reference_lifetime)) = &mut receiver.reference {
                    *reference_lifetime = Some(lifetime.clone());
                }
            }
            syn::FnArg::Typed(argument) => {
                BorrowLifetime(lifetime.clone()).visit_type_mut(&mut argument.ty);
            }
        }
    }
    let result: syn::Type = match &signature.output {
        syn::ReturnType::Default => syn::parse_quote!(()),
        syn::ReturnType::Type(_, result) => *result.clone(),
    };
    signature.output = syn::parse_quote!(
        -> ::cordis_sdk::BoxFuture<'__cordis, #result>
    );
}

pub fn lower_impl(mut item: ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    if item.trait_.is_none() {
        return Err(syn::Error::new_spanned(
            item,
            "interface 标注接口 trait 的实现",
        ));
    }
    for associated in &mut item.items {
        if let ImplItem::Fn(method) = associated {
            if method.sig.asyncness.is_some() {
                lower_signature(&mut method.sig);
                let body = &method.block;
                method.block = syn::parse_quote!({ Box::pin(async move #body) });
            }
        }
    }
    Ok(quote::quote!(#item))
}
