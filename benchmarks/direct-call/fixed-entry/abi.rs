use cordis_sdk::__private::abi::{AbiBytes, AbiStr};
use std::ffi::c_void;

#[cordis_sdk::interface(name = "benchmark.counter", version = "1.0")]
pub trait Counter: Send + Sync {
    fn value(&self) -> cordis_sdk::Result<u64>;
}

/// 加载方把字节与释放令牌一起移交，插件在返回加载结果前释放输入。
#[repr(C)]
pub struct Inherited {
    pub bytes: AbiBytes,
    pub owner: *mut c_void,
    pub release: unsafe extern "C" fn(*mut c_void),
}

impl Drop for Inherited {
    fn drop(&mut self) {
        // SAFETY: 加载入口消费源侧创建的唯一拥有令牌。
        unsafe { (self.release)(self.owner) }
    }
}

/// 同步调用写入调用方提供的缓冲。返回编码长度，负数为参数、契约或空间错误。
#[repr(C)]
pub struct Entry {
    pub major: u32,
    pub size: usize,
    pub load: unsafe extern "C" fn(Inherited, *mut *mut c_void) -> i32,
    pub bind: unsafe extern "C" fn(AbiStr, AbiStr, AbiStr) -> i32,
    pub invoke: unsafe extern "C" fn(*const c_void, i32, AbiBytes, *mut u8, usize) -> isize,
    pub close: unsafe extern "C" fn(*mut c_void),
}

pub type Bootstrap = unsafe extern "C" fn() -> *const Entry;
