use crate::{AbiError, AbiStatus, Buf, Environment, HostV1, LibraryId};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
};

pub type Result<T = ()> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct Error {
    code: u32,
    message: String,
    detail: Vec<u8>,
}

impl Error {
    pub fn new(code: u32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: Vec::new(),
        }
    }
    pub fn code(&self) -> u32 {
        self.code
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn detail(&self) -> &[u8] {
        &self.detail
    }
    pub fn with_detail(mut self, detail: Vec<u8>) -> Self {
        self.detail = detail;
        self
    }

    fn into_raw(self, env: Environment) -> AbiError {
        let code = self.code;
        let buffers = (|| {
            let message = Buf::new(self.message.into_bytes(), env)?;
            let detail = Buf::new(self.detail, env)?;
            Ok::<_, Error>((message, detail))
        })();
        match buffers {
            Ok((message, detail)) => AbiError {
                code,
                message: message.into_raw(),
                detail: detail.into_raw(),
            },
            Err(error) => AbiError::code(error.code),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            write!(f, "ABI error {}", self.code)
        } else {
            write!(f, "{} (code {})", self.message, self.code)
        }
    }
}
impl std::error::Error for Error {}

pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).map_err(|e| Error::new(2, e.to_string()))?;
    Ok(bytes)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    ciborium::from_reader(bytes).map_err(|e| Error::new(2, e.to_string()))
}

fn dispose_payload(payload: Box<dyn std::any::Any + Send>) {
    if let Err(nested) = catch_unwind(AssertUnwindSafe(|| drop(payload))) {
        // DOC-12 §6：二次panic的载荷保留，避免其析构继续触发用户代码。
        std::mem::forget(nested);
    }
}

/// # Safety
/// DOC-12 §5、§6：out和err为有效写入槽，operation交出一个拥有结果。
// SAFETY: DOC-12 §5、§6 的输出存储由边界调用者提供。
pub unsafe fn boundary<T>(
    env: Environment,
    out: *mut T,
    err: *mut AbiError,
    operation: impl FnOnce() -> Result<T>,
) -> AbiStatus {
    match catch_unwind(AssertUnwindSafe(|| {
        operation().map_err(|e| e.into_raw(env))
    })) {
        Ok(Ok(value)) => {
            // SAFETY: DOC-12 §5，成功结果初始化调用方输出槽。
            unsafe { out.write(value) };
            0
        }
        Ok(Err(error)) => {
            let status = if error.code == 5 { 2 } else { 1 };
            // SAFETY: DOC-12 §6，失败时初始化错误槽。
            unsafe { err.write(error) };
            status
        }
        Err(payload) => {
            dispose_payload(payload);
            // SAFETY: DOC-12 §6，panic仅交出自足的code5错误。
            unsafe { err.write(AbiError::code(5)) };
            2
        }
    }
}

/// # Safety
/// DOC-12 §6：err是有效写入槽。
// SAFETY: DOC-12 §6 的错误存储由调用者提供。
pub unsafe fn boundary_unit(
    env: Environment,
    err: *mut AbiError,
    operation: impl FnOnce() -> Result<()>,
) -> AbiStatus {
    let mut unit = std::mem::MaybeUninit::uninit();
    // SAFETY: DOC-12 §5，局部单位输出槽有效，err继承调用前提。
    unsafe { boundary(env, unit.as_mut_ptr(), err, operation) }
}

/// # Safety
/// DOC-12 §3：host首字段有效，out和err是有效输出槽。
// SAFETY: DOC-12 §3、§5 的握手输入输出由装载器提供。
pub unsafe fn entry_boundary<T>(
    host: *const HostV1,
    library: LibraryId,
    out: *mut T,
    err: *mut AbiError,
    operation: impl FnOnce(Environment) -> Result<T>,
) -> AbiStatus {
    // SAFETY: DOC-12 §3，宿主提供进程寿命的虚表内存。
    match unsafe { Environment::from_raw(host, library) } {
        // SAFETY: DOC-12 §5、§6，握手输出槽继承调用前提。
        Ok(env) => unsafe { boundary(env, out, err, || operation(env)) },
        Err(error) => {
            // SAFETY: DOC-12 §6，初始化握手错误槽，错误使用自足的code。
            unsafe { err.write(AbiError::code(error.code())) };
            1
        }
    }
}

// SAFETY: DOC-12 §6 的释放函数错误槽由调用者提供。
pub(crate) unsafe fn release_boundary(err: *mut AbiError, operation: impl FnOnce()) -> AbiStatus {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(()) => 0,
        Err(payload) => {
            dispose_payload(payload);
            // SAFETY: DOC-12 §6，释放panic初始化错误槽。
            unsafe { err.write(AbiError::code(5)) };
            2
        }
    }
}
