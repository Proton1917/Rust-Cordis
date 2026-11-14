//! ADR-0051：同步借用回调在宿主交出执行线程后执行，结果留在调用方。

use crate::{AbiError, AbiStatus, Environment, Error, HostV1_1, Result, boundary_unit};
use std::{ffi::c_void, mem::size_of};

struct Call<F, T> {
    environment: Environment,
    body: Option<F>,
    output: Option<T>,
}

impl Environment {
    /// ADR-0051：宿主在当前线程执行一次 body，支持借用及嵌套；旧表返回 code 3。
    pub fn block_in_place<F: FnOnce() -> T, T>(self, body: F) -> Result<T> {
        if self.raw_host().size < size_of::<HostV1_1>() as u64 {
            return Err(Error::new(
                3,
                "同步等待需要 cordis-host >= 0.1.1（ABI 1.1）",
            ));
        }
        let mut call = Call {
            environment: self,
            body: Some(body),
            output: None,
        };
        // SAFETY: ADR-0051：原始表指针覆盖已确认的扩展槽，call 在当前同步调用期间独占有效。
        unsafe {
            let host = &*self.raw_host_ptr().cast::<HostV1_1>();
            self.call_unit(|err| {
                (host.block_in_place)((&mut call as *mut Call<F, T>).cast(), invoke::<F, T>, err)
            })?;
        }
        call.output
            .ok_or_else(|| Error::new(4, "宿主同步回调结果缺席"))
    }
}

// SAFETY: ADR-0051：data 借用自匹配的 Call<F,T>，宿主在原线程调用一次并保持错误槽有效。
unsafe extern "C" fn invoke<F: FnOnce() -> T, T>(
    data: *mut c_void,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: ADR-0051：同步入口的借用状态具有独占访问权，覆盖整个回调。
    let call = unsafe { &mut *data.cast::<Call<F, T>>() };
    // SAFETY: DOC-12 §6：源侧边界捕获 body 的 panic，结果在返回前存入借用状态。
    unsafe {
        boundary_unit(call.environment, err, || {
            let body = call
                .body
                .take()
                .ok_or_else(|| Error::new(4, "宿主同步回调已执行"))?;
            call.output = Some(body());
            Ok(())
        })
    }
}
