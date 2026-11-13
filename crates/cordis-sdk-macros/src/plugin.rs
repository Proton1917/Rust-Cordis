//! DOC-12 §3：描述符、组件回调与库入口分别生成，ABI 固定模板保持同一布局。

use crate::parse::Plugin;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Expr, Ident, Type};

/// DOC-12 §3：组合组件描述和入口；组件列表沿输入声明顺序排列。
pub fn expand(plugin: Plugin) -> syn::Result<TokenStream> {
    let Plugin {
        name,
        version,
        components,
        rlib_feature,
    } = plugin;
    let export = match rlib_feature {
        // SAFETY: DOC-12 §3，每个动态库由此固定模板导出唯一入口及 ABI 版本符号。
        Some(feature) => quote!(#[cfg_attr(not(feature = #feature), unsafe(no_mangle))]),
        // SAFETY: DOC-12 §3，每个动态库由此固定模板导出唯一入口及 ABI 版本符号。
        None => quote!(#[unsafe(no_mangle)]),
    };
    let mut definitions = Vec::new();
    let mut callbacks = Vec::new();
    for (index, component) in components.iter().enumerate() {
        let apply = format_ident!("apply_{index}");
        let resolve = format_ident!("resolve_{index}");
        definitions.push(component_definition(component, &apply, &resolve));
        callbacks.push(apply_callback(component, &apply));
        callbacks.push(resolve_callback(component, &resolve));
    }
    let entry = plugin_entry(&name, &version, &export, &definitions);
    Ok(quote! {
        #[doc = "DOC-12 §3：返回由本 crate 固定入口模板构造的进程内插件入口句柄。"]
        pub fn cordis_entry() -> ::cordis_sdk::__private::abi::PluginEntry {
            // SAFETY: DOC-12 §3，固定入口模板遵守ABI，Rust调用方链接的插件代码保持有效。
            unsafe { ::cordis_sdk::__private::abi::PluginEntry::new(cordis_plugin_v1) }
        }

        // SAFETY: DOC-12 §3，每个插件 crate 导出一份固定 ABI 主版本符号。
        #export
        pub static CORDIS_PLUGIN_ABI: u32 = ::cordis_sdk::__private::abi::ABI_MAJOR;

        #entry

        mod __cordis_plugin {
            use super::*;
            pub(super) static ENVIRONMENT: ::std::sync::OnceLock<::cordis_sdk::__private::abi::Environment>
                = ::std::sync::OnceLock::new();
            #(#callbacks)*
        }
    })
}

/// DOC-12 §3：将组件声明的服务、事件、状态模式及回调写入描述符。
fn component_definition(component: &Type, apply: &Ident, resolve: &Ident) -> TokenStream {
    quote!({
        let state = <#component as ::cordis_sdk::Component>::state();
        ::cordis_sdk::__private::abi::ComponentDefinition {
            name: <#component as ::cordis_sdk::Component>::NAME.to_owned(),
            inject: <#component as ::cordis_sdk::Component>::inject().entries().iter()
                .map(|dependency| ::cordis_sdk::__private::abi::Dependency {
                    interface: dependency.interface.to_owned(), required: dependency.required,
                }).collect(),
            provides: <#component as ::cordis_sdk::Component>::provides().entries().iter()
                .map(|name| (*name).to_owned()).collect(),
            listens: <#component as ::cordis_sdk::Component>::listens().entries().iter()
                .map(|name| (*name).to_owned()).collect(),
            state_schema: state.name.to_owned(),
            state_version_min: state.min_version,
            state_version_max: state.max_version,
            apply: __cordis_plugin::#apply,
            resolve: __cordis_plugin::#resolve,
        }
    })
}

/// DOC-12 §3、§8：加载回调在 ABI 边界内解码配置，交回持有环境租约的 Future。
fn apply_callback(component: &Type, apply: &Ident) -> TokenStream {
    quote! {
        #[doc = "# Safety"]
        #[doc = "DOC-12 §3、§5、§10：入口已经成功，context 属于当前实例纪元，config 覆盖调用期，out 与 err 为有效输出槽；返回 Future 的所有权及租约转给调用方。"]
        // SAFETY: DOC-12 §3、§10，入口成功后宿主按描述符调用，配置借用和输出槽有效。
        pub(super) unsafe extern "C" fn #apply(
            context: u64,
            config: ::cordis_sdk::__private::abi::AbiBytes,
            out: *mut ::cordis_sdk::__private::abi::AbiFuture,
            err: *mut ::cordis_sdk::__private::abi::AbiError,
        ) -> ::cordis_sdk::__private::abi::AbiStatus {
            let environment = *ENVIRONMENT.get().expect("插件入口已保存宿主环境");
            // SAFETY: DOC-12 §5、§6，配置借用覆盖反序列化，输出为单位 Future。
            unsafe {
                ::cordis_sdk::__private::abi::boundary(environment, out, err, || {
                    let config = ::cordis_sdk::decode::<<#component as ::cordis_sdk::Component>::Config>(config.as_slice())?;
                    let context = ::cordis_sdk::Context::new(environment, context);
                    let future = ::cordis_sdk::__private::abi::make_future(async move {
                        <#component as ::cordis_sdk::Component>::apply(context, config).await?;
                        Ok(::cordis_sdk::__private::abi::AbiUnit::default())
                    }, environment)?;
                    Ok(future.into_raw())
                })
            }
        }
    }
}

/// DOC-12 §3、§6：配置验证和错误编码共同保持在入口边界内。
fn resolve_callback(component: &Type, resolve: &Ident) -> TokenStream {
    quote! {
        #[doc = "# Safety"]
        #[doc = "DOC-12 §3、§5、§10：入口已经成功，config 对应调用期内有效的 CBOR 字节，err 是有效错误输出槽。"]
        // SAFETY: DOC-12 §3、§10，宿主在成功握手后传入当前调用期的配置借用。
        pub(super) unsafe extern "C" fn #resolve(
            config: ::cordis_sdk::__private::abi::AbiBytes,
            err: *mut ::cordis_sdk::__private::abi::AbiError,
        ) -> ::cordis_sdk::__private::abi::AbiStatus {
            let environment = *ENVIRONMENT.get().expect("插件入口已保存宿主环境");
            // SAFETY: DOC-12 §5、§6，配置验证和问题编码都在边界内执行。
            unsafe {
                ::cordis_sdk::__private::abi::boundary_unit(environment, err, || {
                    ::cordis_sdk::__private::validate_config::<#component>(config.as_slice())?;
                    Ok(())
                })
            }
        }
    }
}

/// DOC-12 §3：入口保存一次宿主环境，并按声明顺序构造拥有型插件描述符。
fn plugin_entry(
    name: &Expr,
    version: &Expr,
    export: &TokenStream,
    definitions: &[TokenStream],
) -> TokenStream {
    quote! {
        // SAFETY: DOC-12 §3，宿主和输出存储有效，入口每个插件库调用一次。
        #export
        #[doc = "# Safety"]
        #[doc = "DOC-12 §3、§5、§6：host 的声明范围及函数具有进程寿命，library 由宿主登记；out 与 err 为有效输出槽；每个插件库至多调用一次入口。"]
        // SAFETY: DOC-12 §3，宿主和输出存储有效，入口每个插件库调用一次。
        pub unsafe extern "C" fn cordis_plugin_v1(
            host: *const ::cordis_sdk::__private::abi::HostV1,
            library: ::cordis_sdk::__private::abi::LibraryId,
            out: *mut *const ::cordis_sdk::__private::abi::PluginV1,
            err: *mut ::cordis_sdk::__private::abi::AbiError,
        ) -> ::cordis_sdk::__private::abi::AbiStatus {
            // SAFETY: DOC-12 §3、§6，entry_boundary 验证宿主长度并捕获描述符构造中的 panic。
            unsafe {
                ::cordis_sdk::__private::abi::entry_boundary(host, library, out, err, |environment| {
                    __cordis_plugin::ENVIRONMENT.set(environment)
                        .map_err(|_| ::cordis_sdk::Error::new(4, "插件入口已调用"))?;
                    let metadata = ::cordis_sdk::__private::abi::PluginMetadata {
                        name: (#name).to_string(),
                        version: (#version).to_string(),
                        sdk_version: ::cordis_sdk::VERSION.to_owned(),
                        build_info: ::cordis_sdk::BUILD_INFO.to_owned(),
                        panic_unwind: cfg!(panic = "unwind"),
                    };
                    Ok(::cordis_sdk::__private::abi::PluginDescriptor::new(metadata, vec![#(#definitions),*]).into_raw())
                })
            }
        }
    }
}
