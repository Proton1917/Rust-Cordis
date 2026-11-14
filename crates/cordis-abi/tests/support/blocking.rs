//! ADR-0051：真实借用回调边界、嵌套、旧表长度及 panic 释放经 Miri 检查。

use super::support;
use cordis_abi::*;
use std::{cell::Cell, ffi::c_void, mem::size_of, rc::Rc};

static EXTENDED: HostV1_1 = HostV1_1 {
    v1: HostV1 {
        size: size_of::<HostV1_1>() as u64,
        ..support::HOST
    },
    block_in_place: invoke,
};

// SAFETY: ADR-0051：同步回调原样使用调用期借用指针和错误槽，在当前线程执行一次。
unsafe extern "C" fn invoke(
    data: *mut c_void,
    callback: AbiBlockingCallback,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: ADR-0051：输入来自 Environment 的有效借用调用，回调返回后结束访问。
    unsafe { callback(data, err) }
}

fn extended() -> Environment {
    let base = support::environment();
    // SAFETY: ADR-0051：指针从完整静态扩展表派生，库编号已由测试租约表登记。
    unsafe { Environment::from_raw(std::ptr::addr_of!(EXTENDED).cast(), base.library()) }.unwrap()
}

#[test]
fn borrowed_callback_keeps_local_values_and_supports_nested_calls() {
    let env = extended();
    let local = Rc::new(Cell::new(7));
    let result = env
        .block_in_place(|| {
            env.block_in_place(|| local.set(local.get() + 5)).unwrap();
            local.get()
        })
        .unwrap();
    assert_eq!(result, 12);
    assert_eq!(local.get(), 12);
}

#[test]
fn old_host_rejects_extension_before_entering_callback() {
    let env = support::environment();
    let entered = Cell::new(false);
    let error = env.block_in_place(|| entered.set(true)).unwrap_err();
    assert_eq!(error.code(), 3);
    assert!(!entered.get());
}

#[test]
fn callback_panic_releases_source_state_and_returns_abi_error() {
    struct Owned(Rc<Cell<u32>>);
    impl Drop for Owned {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let env = extended();
    let released = Rc::new(Cell::new(0));
    let owned = Owned(released.clone());
    let error = env
        .block_in_place(move || {
            let _owned = owned;
            panic!("borrowed callback panic");
        })
        .unwrap_err();
    assert_eq!(error.code(), 5);
    assert_eq!(released.get(), 1);
    assert_eq!(support::counts(env).active, 0);
}
