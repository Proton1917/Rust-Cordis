//! DOC-12 §7：分别生成调用代理、对象构造、固定方法表和通用分派入口。

use super::{Identity, Method};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, ItemTrait};

struct Definition<'a> {
    identity: &'a Identity,
    item: &'a ItemTrait,
    methods: &'a [Method],
    module: Ident,
    proxy: Ident,
}

/// DOC-12 §7.2：所有入口沿同一方法列表生成，固定表槽顺序保持接口声明顺序。
pub(super) fn render(identity: &Identity, item: &ItemTrait, methods: &[Method]) -> TokenStream {
    let name = &item.ident;
    let definition = Definition {
        identity,
        item,
        methods,
        module: format_ident!("__cordis_interface_{name}"),
        proxy: format_ident!("__Cordis{name}Proxy"),
    };
    let module = &definition.module;
    let proxy = definition.proxy();
    let implementation = definition.implementation();
    let table = definition.table();
    let scope = scope_binding();
    let synchronous = definition.synchronous();
    let asynchronous = definition.asynchronous();
    let trampolines = methods
        .iter()
        .enumerate()
        .map(|(index, method)| method.trampoline(index, name));
    quote! {
        #item
        #proxy
        #implementation
        #[allow(non_snake_case)]
        mod #module {
            use super::*;
            #table
            #scope
            #synchronous
            #asynchronous
            #(#trampolines)*
        }
    }
}

impl Definition<'_> {
    fn proxy(&self) -> TokenStream {
        let name = &self.item.ident;
        let visibility = &self.item.vis;
        let proxy = &self.proxy;
        let methods = self
            .methods
            .iter()
            .enumerate()
            .map(|(index, method)| method.proxy(index));
        quote! {
            #[doc(hidden)]
            #[doc = "DOC-12 §7、§11：对象所有者保持服务实现、虚表及其库租约，直到最后一份代理引用释放。"]
            #visibility struct #proxy { object: ::std::sync::Arc<::cordis_sdk::__private::abi::Object> }
            impl #name for #proxy { #(#methods)* }
        }
    }

    fn implementation(&self) -> TokenStream {
        let name = &self.item.ident;
        let proxy = &self.proxy;
        let full_name = self.identity.full_name();
        let major = self.identity.major;
        let minor = self.identity.minor;
        let method_count = self.methods.len();
        let constructors = self.constructors();
        quote! {
            impl ::cordis_sdk::Interface for dyn #name {
                const INTERFACE: &'static str = #full_name;
                const MAJOR: u32 = #major;
                const MINOR: u32 = #minor;
                const METHOD_COUNT: usize = #method_count;
                fn proxy(object: ::std::sync::Arc<::cordis_sdk::__private::abi::Object>) -> ::cordis_sdk::Result<::std::sync::Arc<Self>> {
                    ::cordis_sdk::__private::check_interface(&object, #full_name)?;
                    Ok(::std::sync::Arc::new(#proxy { object }))
                }
                #constructors
            }
        }
    }

    /// DOC-12 §7：普通对象与 Scoped 对象分别配对完整静态虚表。
    fn constructors(&self) -> TokenStream {
        let module = &self.module;
        quote! {
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
    }

    fn table(&self) -> TokenStream {
        let name = &self.item.ident;
        let full_name = self.identity.full_name();
        let method_count = self.methods.len();
        let fields: Vec<_> = (0..method_count)
            .map(|index| format_ident!("method_{index}"))
            .collect();
        let function_types = self.methods.iter().map(Method::function_type);
        let method_names: Vec<_> = self
            .methods
            .iter()
            .map(|method| method.signature.ident.to_string())
            .collect();
        quote! {
            #[repr(C)]
            #[doc = "DOC-12 §7.2：首部之后按接口声明顺序保存固定 C 函数指针，size 覆盖全部方法槽。"]
            pub(super) struct VTable {
                pub(super) header: ::cordis_sdk::__private::abi::ObjectHeader,
                #(#fields: #function_types,)*
            }
            pub(super) static VTABLE: VTable = VTable {
                header: ::cordis_sdk::__private::abi::ObjectHeader::for_type::<dyn #name>(
                    ::std::mem::size_of::<VTable>(), 1, #full_name,
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
        }
    }

    /// DOC-12 §7.3：同步分派复用当前对象环境，结果缓冲接管导出的资源。
    fn synchronous(&self) -> TokenStream {
        let name = &self.item.ident;
        let methods = self
            .methods
            .iter()
            .map(|method| method.generic_arm(name, false));
        quote! {
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
                        #(#methods)*
                        _ => Err(::cordis_sdk::Error::new(3, "接口方法不存在")),
                    }
                };
                // SAFETY: DOC-12 §5、§6，成功返回拥有缓冲，错误及panic经当前边界返回。
                unsafe { ::cordis_sdk::__private::abi::boundary(__cordis_environment, __cordis_out, __cordis_error, operation) }
            }
        }
    }

    /// DOC-12 §7.3、§8：异步分派在返回前导入参数，结果 Future 持有实现及资源。
    fn asynchronous(&self) -> TokenStream {
        let name = &self.item.ident;
        let methods = self
            .methods
            .iter()
            .map(|method| method.generic_arm(name, true));
        quote! {
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
                        #(#methods)*
                        _ => Err(::cordis_sdk::Error::new(3, "接口方法不存在")),
                    }
                };
                // SAFETY: DOC-12 §5、§6、§8，边界转交CBOR结果Future及其租约。
                unsafe { ::cordis_sdk::__private::abi::boundary(__cordis_environment, __cordis_out, __cordis_error, operation) }
            }
        }
    }
}

/// DOC-15 §6.4：保留方法导出当前作用域工厂产生的拥有型对象。
fn scope_binding() -> TokenStream {
    quote! {
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
    }
}
