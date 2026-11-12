use crate::{AbiError, AbiStatus, AbiStr, Buf, Error, HostClient, HostV1, LibraryId, Result};
use std::mem::{MaybeUninit, size_of};

#[derive(Clone, Copy)]
pub struct Environment {
    host: &'static HostV1,
    library: LibraryId,
}

impl Environment {
    /// # Safety
    /// DOC-12 §5、§6、§9：host全部函数遵守输出、所有权及panic边界契约。
    // SAFETY: DOC-12 §9 的宿主实现契约由构造者保证。
    pub const unsafe fn new(host: &'static HostV1, library: LibraryId) -> Self {
        Self { host, library }
    }

    /// # Safety
    /// DOC-12 §3、§9：host首字段可读，size覆盖的内存与函数在进程期间有效。
    // SAFETY: DOC-12 §3 的宿主虚表寿命由握手调用者保证。
    pub unsafe fn from_raw(host: *const HostV1, library: LibraryId) -> Result<Self> {
        // SAFETY: DOC-12 §9，首字段为可读的u64。
        if unsafe { host.cast::<u64>().read() } < size_of::<HostV1>() as u64 {
            return Err(Error::new(3, "HostV1 size is below ABI v1"));
        }
        // SAFETY: DOC-12 §3、§9，完整v1表经长度确认且具有进程寿命。
        Ok(unsafe { Self::new(&*host, library) })
    }
    pub const fn library(self) -> LibraryId {
        self.library
    }
    pub const fn raw_host(self) -> &'static HostV1 {
        self.host
    }
    pub const fn host(self) -> HostClient {
        HostClient::new(self)
    }

    /// # Safety
    /// DOC-12 §5、§6：call成功初始化out，失败初始化err，并遵守结果类型T。
    // SAFETY: DOC-12 §5、§6 的写入契约由call实现保证。
    pub unsafe fn call<T>(
        self,
        call: impl FnOnce(*mut T, *mut AbiError) -> AbiStatus,
    ) -> Result<T> {
        let mut output = MaybeUninit::uninit();
        let mut error = MaybeUninit::uninit();
        let status = call(output.as_mut_ptr(), error.as_mut_ptr());
        if status == 0 {
            // SAFETY: DOC-12 §5，成功状态初始化了T。
            Ok(unsafe { output.assume_init() })
        } else {
            // SAFETY: DOC-12 §6，失败状态初始化了AbiError并转交所有权。
            Err(unsafe { self.read_error(error.assume_init()) })
        }
    }

    /// # Safety
    /// DOC-12 §6：call失败时初始化err。
    // SAFETY: DOC-12 §6 的错误写入契约由call实现保证。
    pub unsafe fn call_unit(self, call: impl FnOnce(*mut AbiError) -> AbiStatus) -> Result<()> {
        let mut error = MaybeUninit::uninit();
        if call(error.as_mut_ptr()) == 0 {
            Ok(())
        } else {
            // SAFETY: DOC-12 §6，失败状态交出初始化的错误。
            Err(unsafe { self.read_error(error.assume_init()) })
        }
    }

    /// # Safety
    /// DOC-12 §5：error和其缓冲的唯一所有权及租约转交给本函数。
    // SAFETY: DOC-12 §5 的拥有输入由调用者转交。
    pub unsafe fn read_error(self, error: AbiError) -> Error {
        // SAFETY: DOC-12 §5，先接管全部拥有字段，再读取内容。
        let message = unsafe { Buf::from_raw(error.message, self) };
        // SAFETY: DOC-12 §5，detail是第二个独立拥有缓冲。
        let detail = unsafe { Buf::from_raw(error.detail, self) };
        Error::new(
            error.code,
            String::from_utf8_lossy(message.as_slice()).into_owned(),
        )
        .with_detail(detail.as_slice().to_vec())
    }

    pub fn acquire(self, library: LibraryId) -> Result<()> {
        if library == 0 {
            return Ok(());
        }
        // SAFETY: DOC-12 §11，宿主表有效，错误槽由call_unit提供。
        unsafe { self.call_unit(|err| (self.host.lease_acquire)(library, err)) }
    }

    pub fn release(self, library: LibraryId) -> Result<()> {
        if library == 0 {
            return Ok(());
        }
        // SAFETY: DOC-12 §11，所有者在资源回调返回后归还既有租约。
        unsafe { self.call_unit(|err| (self.host.lease_release)(library, err)) }
    }

    pub fn report(self, error: Error) {
        let message = format!("library {}: {error}", self.library);
        let result =
            // SAFETY: DOC-12 §6、§9，ctx0是诊断入口，借用字符串覆盖调用期。
            unsafe { self.call_unit(|err| (self.host.log)(0, 0, AbiStr::new(&message), err)) };
        if let Err(logging) = result {
            eprintln!("{message}; ABI diagnostic failed: {logging}");
        }
    }
}
