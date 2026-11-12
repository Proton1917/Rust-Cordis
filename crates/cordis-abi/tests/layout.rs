use cordis_abi::*;
use std::{
    fmt::Write,
    mem::{align_of, offset_of, size_of},
    path::Path,
};

fn layout() -> String {
    let mut output = String::from("Cordis ABI 1.0; 64-bit little-endian\n");
    macro_rules! record {
        ($ty:ty, $($field:ident),+ $(,)?) => {{
            writeln!(output, "{} size={} align={}", stringify!($ty), size_of::<$ty>(), align_of::<$ty>()).unwrap();
            $(writeln!(output, "  {}={}", stringify!($field), offset_of!($ty, $field)).unwrap();)+
        }};
    }
    writeln!(
        output,
        "AbiStatus size={} align={}",
        size_of::<AbiStatus>(),
        align_of::<AbiStatus>()
    )
    .unwrap();
    writeln!(
        output,
        "LibraryId size={} align={}",
        size_of::<LibraryId>(),
        align_of::<LibraryId>()
    )
    .unwrap();
    record!(AbiStr, ptr, len);
    record!(AbiBytes, ptr, len);
    record!(AbiBuf, ptr, len, cap, owner, free, library);
    record!(AbiUnit, reserved);
    record!(AbiError, code, message, detail);
    record!(AbiState, schema, version, reserved, data);
    record!(AbiObject, data, vtable, library);
    record!(
        ObjectHeader,
        size,
        kind,
        flags,
        reserved,
        interface,
        clone,
        drop,
        methods,
        method_count,
        invoke,
        invoke_async
    );
    record!(AbiFuture, data, vtable, library);
    record!(FutureVTable, size, poll, drop);
    record!(AbiWaker, data, vtable, library);
    record!(WakerVTable, clone, wake, wake_by_ref, drop);
    record!(SyncCallbackVTable, header, call);
    record!(AsyncCallbackVTable, header, call);
    record!(ExporterVTable, header, export, thaw);
    record!(
        HostV1,
        size,
        log,
        now,
        sleep,
        spawn,
        effect,
        effect_async,
        defer,
        defer_async,
        is_current,
        provide,
        get,
        on,
        emit,
        emit_async,
        export_state,
        inherited_state,
        config,
        spawn_thread,
        lease_acquire,
        lease_release,
        effect_settled,
        effect_dispose,
        object_export,
        object_import,
        object_release
    );
    record!(
        PluginV1,
        size,
        abi_major,
        abi_minor,
        panic_unwind,
        format,
        reserved,
        name,
        version,
        sdk_version,
        build_info,
        components,
        component_count,
        release
    );
    record!(
        ComponentV1,
        size,
        name,
        inject,
        inject_count,
        provides,
        provides_count,
        listens,
        listens_count,
        state_schema,
        state_version_min,
        state_version_max,
        apply,
        resolve
    );
    record!(DependencyV1, interface, required, reserved);
    record!(AbiOption<u8>, is_some, value);
    record!(AbiOption<i8>, is_some, value);
    record!(AbiOption<u16>, is_some, value);
    record!(AbiOption<i16>, is_some, value);
    record!(AbiOption<u32>, is_some, value);
    record!(AbiOption<i32>, is_some, value);
    record!(AbiOption<u64>, is_some, value);
    record!(AbiOption<i64>, is_some, value);
    record!(AbiOption<f32>, is_some, value);
    record!(AbiOption<f64>, is_some, value);
    record!(AbiOption<AbiStr>, is_some, value);
    record!(AbiOption<AbiBytes>, is_some, value);
    record!(AbiOption<AbiBuf>, is_some, value);
    record!(AbiOption<AbiObject>, is_some, value);
    record!(AbiOption<AbiState>, is_some, value);
    record!(AbiOption<AbiUnit>, is_some, value);
    output
}

#[test]
fn abi_v1_matches_layout_record() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/abi/v1-layout.txt");
    let actual = layout();
    if std::env::var_os("CORDIS_ABI_WRITE_LAYOUT").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, actual).unwrap();
    } else {
        let expected = include_str!("../../../docs/abi/v1-layout.txt");
        assert_eq!(
            actual, expected,
            "ABI 布局与记录存在差异，按 DOC-12 更新记录"
        );
    }
}
