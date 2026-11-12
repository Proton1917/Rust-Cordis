use crate::{
    async_method::lower_signature,
    parse::{Identity, result_type},
    types::Mapping,
};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemTrait, Pat, Signature, TraitItem};

struct Method {
    signature: Signature,
    names: Vec<syn::Ident>,
    inputs: Vec<Mapping>,
    output: Mapping,
    asynchronous: bool,
}

impl Method {
    fn parse(signature: &Signature) -> syn::Result<Self> {
        if !signature.generics.params.is_empty()
            || signature.unsafety.is_some()
            || signature.constness.is_some()
            || signature.variadic.is_some()
        {
            return Err(syn::Error::new_spanned(
                signature,
                "接口方法使用具体类型和安全方法签名",
            ));
        }
        match signature.inputs.first() {
            Some(FnArg::Receiver(receiver))
                if receiver.reference.is_some() && receiver.mutability.is_none() => {}
            _ => {
                return Err(syn::Error::new_spanned(
                    signature,
                    "接口方法使用 &self 接收者",
                ));
            }
        }
        let mut names = Vec::new();
        let mut inputs = Vec::new();
        for argument in signature.inputs.iter().skip(1) {
            let FnArg::Typed(argument) = argument else {
                unreachable!()
            };
            let Pat::Ident(pattern) = argument.pat.as_ref() else {
                return Err(syn::Error::new_spanned(argument, "接口参数使用独立名称"));
            };
            names.push(pattern.ident.clone());
            inputs.push(Mapping::parse(&argument.ty)?);
        }
        let output = Mapping::parse(&result_type(signature)?)?;
        if output.borrowed() {
            return Err(syn::Error::new_spanned(
                &signature.output,
                "返回字符串和字节串使用 String 或 Vec<u8>",
            ));
        }
        Ok(Self {
            signature: signature.clone(),
            names,
            inputs,
            output,
            asynchronous: signature.asyncness.is_some(),
        })
    }

    fn wire_output(&self) -> TokenStream {
        if self.asynchronous {
            quote!(::cordis_sdk::__private::abi::AbiFuture)
        } else {
            self.output.wire()
        }
    }

    fn function_type(&self) -> TokenStream {
        let inputs = self.inputs.iter().map(Mapping::wire);
        let output = self.wire_output();
        // SAFETY: DOC-12 §7.2，固定 C 签名由同一方法声明的参数和返回类型生成。
        quote!(unsafe extern "C" fn(
            *mut ::std::ffi::c_void, #(#inputs,)* *mut #output,
            *mut ::cordis_sdk::__private::abi::AbiError,
        ) -> ::cordis_sdk::__private::abi::AbiStatus)
    }

    fn proxy(&self, index: usize) -> TokenStream {
        let mut signature = self.signature.clone();
        lower_signature(&mut signature);
        let function_type = self.function_type();
        let names = &self.names;
        let prepared =
            self.inputs.iter().zip(names).map(|(mapping, name)| {
                mapping.prepare(quote!(#name), quote!(__cordis_environment))
            });
        let raw_arguments = self
            .inputs
            .iter()
            .zip(names)
            .map(|(mapping, name)| mapping.encode_raw(quote!(#name)));
        let output = self.output.wire();
        let adopted = self
            .output
            .adopt(quote!(__cordis_raw), quote!(__cordis_environment));
        let decoded = self.output.decode(quote!(__cordis_owned));
        let generic = self.generic_proxy();
        let call = quote! {
            // SAFETY: DOC-12 §7.2，方法索引和签名来自同一接口声明，先检查槽长度。
            let __cordis_method: #function_type = unsafe { self.object.typed_slot(#index) }?;
            #(let #names = #prepared;)*
            // SAFETY: DOC-12 §5、§7，代理持有对象租约，全部参数按声明布局转移。
            let __cordis_raw = unsafe {
                __cordis_environment.call(|out, err| {
                    __cordis_method(self.object.data(), #(#raw_arguments,)* out, err)
                })
            }?;
        };
        let response = if self.asynchronous {
            quote! {
                let __cordis_future = {
                    #call
                    // SAFETY: DOC-12 §8，方法按该静态返回类型创建 Future 并转交租约。
                    unsafe {
                        ::cordis_sdk::__private::abi::RemoteFuture::<#output>::from_raw(
                            __cordis_raw, __cordis_environment,
                        )
                    }
                };
                __cordis_future.await?
            }
        } else {
            quote! { #call __cordis_raw }
        };
        let body = quote! {
            let __cordis_environment = self.object.environment();
            if !self.object.has_typed_methods() {
                return { #generic };
            }
            let __cordis_raw = { #response };
            let __cordis_owned = #adopted;
            #decoded
        };
        if self.asynchronous {
            quote! {
                #[doc = "DOC-12 §5、§7、§8：生成代理按 flags 选择类型化表或 CBOR 入口，持有对象租约，并在等待前结束参数的调用期借用和资源导出。"]
                #signature { Box::pin(async move { #body }) }
            }
        } else {
            quote! {
                #[doc = "DOC-12 §5、§7：生成代理按 flags 选择类型化表或 CBOR 入口，持有对象租约，并按接口声明转移参数和返回值的所有权。"]
                #signature { #body }
            }
        }
    }

    fn generic_proxy(&self) -> TokenStream {
        let method = self.signature.ident.to_string();
        let mutable = if self.inputs.iter().any(Mapping::contains_service) {
            quote!(mut)
        } else {
            quote!()
        };
        let arguments =
            self.inputs.iter().zip(&self.names).map(|(mapping, name)| {
                mapping.encode_cbor(quote!(#name), quote!(__cordis_encoder))
            });
        let prepare = quote! {
            let #mutable __cordis_encoder = ::cordis_sdk::__private::abi::CborEncoder::new(__cordis_environment);
            let __cordis_values = ::cordis_sdk::__private::abi::CborValue::Array(vec![#(#arguments),*]);
            let __cordis_arguments = __cordis_encoder.finish(__cordis_values)?;
        };
        let response = if self.asynchronous {
            quote! {
                let __cordis_future = {
                    #prepare
                    let future = self.object.invoke_async(#method, __cordis_arguments.as_slice())?;
                    __cordis_arguments.release()?;
                    future
                };
                let __cordis_output = __cordis_future.await?;
            }
        } else {
            quote! {
                let __cordis_output = {
                    #prepare
                    let output = self.object.invoke(#method, __cordis_arguments.as_slice())?;
                    __cordis_arguments.release()?;
                    output
                };
            }
        };
        let decoded = self
            .output
            .decode_cbor(quote!(__cordis_value), quote!(__cordis_environment));
        let finish = if matches!(&self.output, Mapping::Unit) {
            quote! { #decoded?; __cordis_output.release() }
        } else {
            quote! {
                let __cordis_result = #decoded?;
                __cordis_output.release()?;
                ::cordis_sdk::Result::Ok(__cordis_result)
            }
        };
        quote! {
            #response
            let __cordis_value = ::cordis_sdk::__private::abi::decode_value(__cordis_output.as_slice())?;
            #finish
        }
    }

    fn generic_arm(&self, interface: &syn::Ident, asynchronous: bool) -> TokenStream {
        let method = &self.signature.ident;
        let method_name = method.to_string();
        if self.asynchronous && !asynchronous {
            return quote!(#method_name => Err(::cordis_sdk::Error::new(2, "该方法需要异步调用入口")),);
        }
        let names = &self.names;
        let count = names.len();
        let decoded = self.inputs.iter().zip(names).map(|(mapping, name)| {
            mapping.decode_cbor(quote!(#name), quote!(__cordis_environment))
        });
        let arguments = self
            .inputs
            .iter()
            .zip(names)
            .map(|(mapping, name)| mapping.async_borrow(quote!(#name)));
        let call = if self.asynchronous {
            quote!(__cordis_implementation.#method(#(#arguments),*).await?)
        } else {
            quote!(__cordis_implementation.#method(#(#arguments),*)?)
        };
        let mutable = if self.output.contains_service() {
            quote!(mut)
        } else {
            quote!()
        };
        let result_binding = if matches!(&self.output, Mapping::Unit) {
            quote!()
        } else {
            quote!(let __cordis_result =)
        };
        let encoded = if matches!(&self.output, Mapping::Unit) {
            quote!(::cordis_sdk::__private::abi::CborValue::Null)
        } else {
            self.output
                .encode_cbor(quote!(__cordis_result), quote!(__cordis_encoder))
        };
        let operation = quote! {
            #result_binding #call;
            let #mutable __cordis_encoder = ::cordis_sdk::__private::abi::CborEncoder::new(__cordis_environment);
            let __cordis_value = #encoded;
            __cordis_encoder.finish(__cordis_value)
        };
        let output = if asynchronous {
            quote! {
                let future = ::cordis_sdk::__private::abi::make_invoke_future(async move {
                    #operation
                }, __cordis_environment)?;
                Ok(future.into_raw())
            }
        } else {
            quote! {
                let output = { #operation }?;
                Ok(output.into_raw())
            }
        };
        quote! {
            #method_name => {
                // SAFETY: DOC-12 §5、§7.3，参数字节覆盖当前入口调用，异步入口在返回前完成资源导入。
                let values = ::cordis_sdk::__private::abi::decode_arguments(unsafe { __cordis_args.as_slice() })?;
                let [#(#names),*]: [::cordis_sdk::__private::abi::CborValue; #count] = values.try_into()
                    .map_err(|_| ::cordis_sdk::Error::new(2, concat!(#method_name, " 参数个数不符")))?;
                #(let #names = #decoded?;)*
                // SAFETY: DOC-12 §7，本方法分派与当前接口的 ObjectData 配套。
                let __cordis_implementation = unsafe { ::cordis_sdk::__private::abi::object_arc::<dyn #interface>(__cordis_data) };
                #output
            },
        }
    }

    fn trampoline(&self, index: usize, interface: &syn::Ident) -> TokenStream {
        let name = format_ident!("method_{index}");
        let method = &self.signature.ident;
        let names = &self.names;
        let inputs = self.inputs.iter().map(Mapping::wire);
        let output = self.wire_output();
        let adopted = self
            .inputs
            .iter()
            .zip(names)
            .map(|(mapping, name)| mapping.adopt(quote!(#name), quote!(__cordis_environment)));
        let decoded = self
            .inputs
            .iter()
            .zip(names)
            .map(|(mapping, name)| mapping.decode(quote!(#name)));
        let result_binding = if matches!(&self.output, Mapping::Unit) {
            quote!()
        } else {
            quote!(let __cordis_result =)
        };
        let prepared_output = if matches!(&self.output, Mapping::Unit) {
            quote!(::cordis_sdk::__private::abi::AbiUnit::default())
        } else {
            self.output
                .prepare(quote!(__cordis_result), quote!(__cordis_environment))
        };
        let raw_output = self.output.encode_raw(quote!(__cordis_output));
        let operation = if self.asynchronous {
            let copied = self
                .inputs
                .iter()
                .zip(names)
                .map(|(mapping, name)| mapping.async_copy(quote!(#name)));
            let arguments = self
                .inputs
                .iter()
                .zip(names)
                .map(|(mapping, name)| mapping.async_borrow(quote!(#name)));
            quote! {
                #(let #names = #copied;)*
                let __cordis_future = ::cordis_sdk::__private::abi::make_future(async move {
                    #result_binding __cordis_implementation.#method(#(#arguments),*).await?;
                    let __cordis_output = #prepared_output;
                    Ok(#raw_output)
                }, __cordis_environment)?;
                Ok(__cordis_future.into_raw())
            }
        } else {
            quote! {
                #result_binding __cordis_implementation.#method(#(#names),*)?;
                let __cordis_output = #prepared_output;
                Ok(#raw_output)
            }
        };
        quote! {
            #[doc = "# Safety"]
            #[doc = "DOC-12 §5、§7、§8：data 来自本接口的 Object::new；参数遵守声明的 ABI 映射，拥有参数及租约转入，借用参数覆盖调用期；out 和 err 为有效输出槽。"]
            // SAFETY: DOC-12 §7，数据来自本接口 Object::new，参数与输出遵循生成的固定签名。
            pub(super) unsafe extern "C" fn #name(
                __cordis_data: *mut ::std::ffi::c_void,
                #(#names: #inputs,)*
                __cordis_out: *mut #output,
                __cordis_error: *mut ::cordis_sdk::__private::abi::AbiError,
            ) -> ::cordis_sdk::__private::abi::AbiStatus {
                // SAFETY: DOC-12 §7、§11，对象拥有的环境在本次调用期间有效。
                let __cordis_environment = unsafe { ::cordis_sdk::__private::abi::object_environment(__cordis_data) };
                let __cordis_operation = || {
                    #(let #names = #adopted;)*
                    #(let #names = #decoded?;)*
                    // SAFETY: DOC-12 §7，静态虚表与 ObjectData<dyn 当前接口>配套。
                    let __cordis_implementation = unsafe {
                        ::cordis_sdk::__private::abi::object_arc::<dyn #interface>(__cordis_data)
                    };
                    #operation
                };
                // SAFETY: DOC-12 §5、§6，调用者提供输出槽，边界捕获本实现及参数转换中的 panic。
                unsafe {
                    ::cordis_sdk::__private::abi::boundary(__cordis_environment, __cordis_out, __cordis_error, __cordis_operation)
                }
            }
        }
    }
}

pub fn expand(identity: Identity, mut item: ItemTrait) -> syn::Result<TokenStream> {
    if !item.generics.params.is_empty() || item.unsafety.is_some() || identity.kind.is_some() {
        return Err(syn::Error::new_spanned(
            &item,
            "接口 trait 使用具体类型及 name、version 标识",
        ));
    }
    let mut methods = Vec::new();
    for associated in &mut item.items {
        let TraitItem::Fn(method) = associated else {
            return Err(syn::Error::new_spanned(
                associated,
                "接口 trait 声明方法，数据类型放在 trait 外",
            ));
        };
        methods.push(Method::parse(&method.sig)?);
        if method.sig.asyncness.is_some() {
            lower_signature(&mut method.sig);
            if let Some(body) = &method.default {
                method.default = Some(syn::parse_quote!({ Box::pin(async move #body) }));
            }
        }
    }
    for required in ["Send", "Sync"] {
        let present = item.supertraits.iter().any(|bound| {
            matches!(bound, syn::TypeParamBound::Trait(trait_bound)
                if trait_bound.path.segments.last().is_some_and(|segment| segment.ident == required))
        });
        if !present {
            let bound: syn::TypeParamBound = syn::parse_str(required)?;
            item.supertraits.push(bound);
        }
    }
    let name = &item.ident;
    let visibility = &item.vis;
    let module = format_ident!("__cordis_interface_{name}");
    let proxy = format_ident!("__Cordis{name}Proxy");
    let full_name = identity.full_name();
    let major = identity.major;
    let minor = identity.minor;
    let method_count = methods.len();
    let fields: Vec<_> = (0..method_count)
        .map(|index| format_ident!("method_{index}"))
        .collect();
    let function_types = methods.iter().map(Method::function_type);
    let proxy_methods = methods
        .iter()
        .enumerate()
        .map(|(index, method)| method.proxy(index));
    let trampolines = methods
        .iter()
        .enumerate()
        .map(|(index, method)| method.trampoline(index, name));
    let method_names: Vec<_> = methods
        .iter()
        .map(|method| method.signature.ident.to_string())
        .collect();
    let synchronous = methods.iter().map(|method| method.generic_arm(name, false));
    let asynchronous = methods.iter().map(|method| method.generic_arm(name, true));
    Ok(quote! {
        #item

        #[doc(hidden)]
        #[doc = "DOC-12 §7、§11：对象所有者保持服务实现、虚表及其库租约，直到最后一份代理引用释放。"]
        #visibility struct #proxy { object: ::std::sync::Arc<::cordis_sdk::__private::abi::Object> }

        impl #name for #proxy { #(#proxy_methods)* }

        impl ::cordis_sdk::Interface for dyn #name {
            const INTERFACE: &'static str = #full_name;
            const MAJOR: u32 = #major;
            const MINOR: u32 = #minor;
            const METHOD_COUNT: usize = #method_count;
            fn proxy(object: ::std::sync::Arc<::cordis_sdk::__private::abi::Object>) -> ::cordis_sdk::Result<::std::sync::Arc<Self>> {
                ::cordis_sdk::__private::check_interface(&object, #full_name)?;
                Ok(::std::sync::Arc::new(#proxy { object }))
            }

            #[doc = "DOC-12 §7：生成实现把接口值与完整静态虚表配对，虚表指针保留全部方法槽的有效范围。"]
            fn object(value: ::std::sync::Arc<Self>, environment: ::cordis_sdk::__private::abi::Environment)
                -> ::cordis_sdk::Result<::cordis_sdk::__private::abi::Object>
            {
                // SAFETY: DOC-12 §7，指针由完整静态虚表派生，全部蹦床都读取 Arc<dyn 当前接口>。
                unsafe {
                    ::cordis_sdk::__private::abi::Object::new(
                        value,
                        ::std::ptr::from_ref(&#module::VTABLE).cast::<::cordis_sdk::__private::abi::ObjectHeader>(),
                        environment,
                    )
                }
            }

            #[doc = "DOC-12 §7：原对象保留类型化表，作用域工厂在同一库中构造类型化视图对象。"]
            fn scoped_object(
                value: ::std::sync::Arc<Self>,
                scope: ::std::sync::Arc<dyn Fn(::cordis_sdk::__private::abi::CborValue) -> ::cordis_sdk::Result<::std::sync::Arc<Self>> + Send + Sync>,
                environment: ::cordis_sdk::__private::abi::Environment,
            ) -> ::cordis_sdk::Result<::cordis_sdk::__private::abi::Object> {
                let factory = ::std::sync::Arc::new(move |metadata| Self::object(scope(metadata)?, environment));
                // SAFETY: DOC-12 §7，完整作用域虚表与Arc<dyn当前接口>配套，工厂同属源侧库。
                unsafe {
                    ::cordis_sdk::__private::abi::Object::new_scoped(
                        value,
                        ::std::ptr::from_ref(&#module::SCOPED_VTABLE).cast::<::cordis_sdk::__private::abi::ObjectHeader>(),
                        environment,
                        factory,
                    )
                }
            }
        }

        #[allow(non_snake_case)]
        mod #module {
            use super::*;

            #[repr(C)]
            #[doc = "DOC-12 §7.2：首部之后按接口声明顺序保存固定 C 函数指针，size 覆盖全部方法槽。"]
            pub(super) struct VTable {
                pub(super) header: ::cordis_sdk::__private::abi::ObjectHeader,
                #(#fields: #function_types,)*
            }

            pub(super) static VTABLE: VTable = VTable {
                header: ::cordis_sdk::__private::abi::ObjectHeader::for_type::<dyn #name>(
                    ::std::mem::size_of::<VTable>(),
                    1,
                    #full_name,
                ).with_dispatch(&METHODS, invoke, invoke_async),
                #(#fields,)*
            };

            pub(super) static SCOPED_VTABLE: VTable = VTable {
                header: ::cordis_sdk::__private::abi::ObjectHeader::for_type::<dyn #name>(
                    ::std::mem::size_of::<VTable>(), 1, #full_name,
                ).with_dispatch(&SCOPED_METHODS, invoke, invoke_async),
                #(#fields,)*
            };

            static METHODS: [::cordis_sdk::__private::abi::AbiStr; #method_count] = [
                #(::cordis_sdk::__private::abi::AbiStr::new(#method_names)),*
            ];
            static SCOPED_METHODS: [::cordis_sdk::__private::abi::AbiStr; #method_count + 1] = [
                #(::cordis_sdk::__private::abi::AbiStr::new(#method_names),)*
                ::cordis_sdk::__private::abi::AbiStr::new("cordis.scope"),
            ];

            #[doc = "# Safety"]
            #[doc = "DOC-12 §7：data来自本接口对象，args借用覆盖当前调用；返回视图的资源引用由结果缓冲拥有。"]
            // SAFETY: DOC-12 §7，保留方法读取源侧作用域工厂并编码拥有视图。
            unsafe fn bind_scope(__cordis_data: *mut ::std::ffi::c_void, args: ::cordis_sdk::__private::abi::AbiBytes)
                -> ::cordis_sdk::Result<::cordis_sdk::__private::abi::Buf>
            {
                // SAFETY: DOC-12 §5，args在当前入口期间保持有效。
                let values = ::cordis_sdk::__private::abi::decode_arguments(unsafe { args.as_slice() })?;
                let [metadata]: [::cordis_sdk::__private::abi::CborValue; 1] = values.try_into()
                    .map_err(|_| ::cordis_sdk::Error::new(2, "cordis.scope需要一个元数据参数"))?;
                // SAFETY: DOC-12 §7，数据前缀及其工厂由当前对象保持有效。
                let environment = unsafe { ::cordis_sdk::__private::abi::object_environment(__cordis_data) };
                // SAFETY: DOC-12 §7，工厂调用仍处于invoke的panic边界内。
                let object = unsafe { ::cordis_sdk::__private::abi::object_scope(__cordis_data, metadata) }?;
                let mut encoder = ::cordis_sdk::__private::abi::CborEncoder::new(environment);
                let value = encoder.export(object)?;
                encoder.finish(value)
            }

            #[doc = "# Safety"]
            #[doc = "DOC-12 §5、§7.3：data 来自本接口的对象，method 与 args 覆盖调用期，out 和 err 为有效输出槽；参数资源在返回前导入，结果缓冲持有所导出的资源引用。"]
            // SAFETY: DOC-12 §7.3，名称分派和CBOR转换来自同一接口声明。
            unsafe extern "C" fn invoke(
                __cordis_data: *mut ::std::ffi::c_void,
                __cordis_method: ::cordis_sdk::__private::abi::AbiStr,
                __cordis_args: ::cordis_sdk::__private::abi::AbiBytes,
                __cordis_out: *mut ::cordis_sdk::__private::abi::AbiBuf,
                __cordis_error: *mut ::cordis_sdk::__private::abi::AbiError,
            ) -> ::cordis_sdk::__private::abi::AbiStatus {
                // SAFETY: DOC-12 §7，对象拥有的公共前缀在当前调用期间有效。
                let __cordis_environment = unsafe { ::cordis_sdk::__private::abi::object_environment(__cordis_data) };
                let operation = || {
                    // SAFETY: DOC-12 §5，方法名称借用覆盖当前入口调用。
                    match unsafe { __cordis_method.as_str() }? {
                        "cordis.scope" => {
                            // SAFETY: DOC-12 §7，当前通用入口持有对象和调用期参数。
                            unsafe { bind_scope(__cordis_data, __cordis_args) }.map(::cordis_sdk::__private::abi::Buf::into_raw)
                        }
                        #(#synchronous)*
                        _ => Err(::cordis_sdk::Error::new(3, "接口方法不存在")),
                    }
                };
                // SAFETY: DOC-12 §5、§6，成功返回拥有缓冲，错误及panic经当前边界返回。
                unsafe { ::cordis_sdk::__private::abi::boundary(__cordis_environment, __cordis_out, __cordis_error, operation) }
            }

            #[doc = "# Safety"]
            #[doc = "DOC-12 §5、§7.3、§8：data 与本接口配套，借用参数覆盖入口调用，out 和 err 为有效输出槽；返回前导入参数资源，Future 持有实现及资源对象，成功结果为拥有 CBOR 缓冲。"]
            // SAFETY: DOC-12 §7.3、§8，异步入口为全部已声明方法构造匹配的Future。
            unsafe extern "C" fn invoke_async(
                __cordis_data: *mut ::std::ffi::c_void,
                __cordis_method: ::cordis_sdk::__private::abi::AbiStr,
                __cordis_args: ::cordis_sdk::__private::abi::AbiBytes,
                __cordis_out: *mut ::cordis_sdk::__private::abi::AbiFuture,
                __cordis_error: *mut ::cordis_sdk::__private::abi::AbiError,
            ) -> ::cordis_sdk::__private::abi::AbiStatus {
                // SAFETY: DOC-12 §7，对象拥有的公共前缀在当前调用期间有效。
                let __cordis_environment = unsafe { ::cordis_sdk::__private::abi::object_environment(__cordis_data) };
                let operation = || {
                    // SAFETY: DOC-12 §5，方法名称借用覆盖当前入口调用。
                    match unsafe { __cordis_method.as_str() }? {
                        "cordis.scope" => {
                            // SAFETY: DOC-12 §7，视图在返回Future前完成绑定，结果缓冲保留其资源引用。
                            let result = unsafe { bind_scope(__cordis_data, __cordis_args) }?;
                            ::cordis_sdk::__private::abi::make_invoke_future(async move { Ok(result) }, __cordis_environment)
                                .map(::cordis_sdk::__private::abi::InvokeFuture::into_raw)
                        }
                        #(#asynchronous)*
                        _ => Err(::cordis_sdk::Error::new(3, "接口方法不存在")),
                    }
                };
                // SAFETY: DOC-12 §5、§6、§8，边界转交CBOR结果Future及其租约。
                unsafe { ::cordis_sdk::__private::abi::boundary(__cordis_environment, __cordis_out, __cordis_error, operation) }
            }

            #(#trampolines)*
        }
    })
}
