use crate::{
    AbiBuf, AbiBytes, AbiError, AbiStatus, Environment, Error, Result, error::release_boundary,
};
use std::mem::ManuallyDrop;

pub struct Buf {
    raw: Option<AbiBuf>,
    env: Environment,
}

// SAFETY: DOC-12 §5，Buf独占字节，源侧free支持任意线程调用。
unsafe impl Send for Buf {}
// SAFETY: DOC-12 §5，共享Buf只开放只读字节借用。
unsafe impl Sync for Buf {}

impl Buf {
    pub fn new(bytes: Vec<u8>, env: Environment) -> Result<Self> {
        if bytes.is_empty() {
            return Ok(Self {
                raw: Some(AbiBuf::empty()),
                env,
            });
        }
        env.acquire(env.library())?;
        let mut bytes = ManuallyDrop::new(bytes);
        Ok(Self {
            raw: Some(AbiBuf {
                ptr: bytes.as_mut_ptr(),
                len: bytes.len() as u64,
                cap: bytes.capacity() as u64,
                free: Some(free_buffer),
                library: env.library(),
            }),
            env,
        })
    }

    /// # Safety
    /// DOC-12 §5、§11：raw包含有效缓冲及一份既有租约，唯一所有权转入。
    // SAFETY: DOC-12 §5、§11 的拥有输入由调用者转交。
    pub unsafe fn from_raw(raw: AbiBuf, env: Environment) -> Self {
        Self {
            raw: Some(raw),
            env,
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        let raw = self.raw.as_ref().expect("buffer is owned");
        // SAFETY: DOC-12 §5，self保持缓冲及租约，借用期受self约束。
        unsafe {
            AbiBytes {
                ptr: raw.ptr,
                len: raw.len,
            }
            .as_slice()
        }
    }
    pub fn into_raw(mut self) -> AbiBuf {
        self.raw.take().expect("buffer is owned")
    }
    pub fn into_vec(self) -> Result<Vec<u8>> {
        let value = self.as_slice().to_vec();
        self.release()?;
        Ok(value)
    }
    pub fn into_string(self) -> Result<String> {
        String::from_utf8(self.into_vec()?).map_err(|e| Error::new(2, e.to_string()))
    }
    pub fn release(mut self) -> Result<()> {
        self.release_inner()
    }
    fn release_inner(&mut self) -> Result<()> {
        let Some(raw) = self.raw.take() else {
            return Ok(());
        };
        let result = match raw.free {
            // SAFETY: DOC-12 §5，原始字段与源侧释放函数配套，唯一所有权在此结束。
            Some(free) => unsafe {
                self.env
                    .call_unit(|err| free(raw.ptr, raw.len, raw.cap, err))
            },
            None => Ok(()),
        };
        let lease = self.env.release(raw.library);
        result.and(lease)
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        if let Err(error) = self.release_inner() {
            self.env.report(error);
        }
    }
}

// SAFETY: DOC-12 §5，字段来自本模块创建的Vec，调用方独占该缓冲。
unsafe extern "C" fn free_buffer(
    ptr: *mut u8,
    len: u64,
    cap: u64,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §5、§6，释放源侧Vec并在边界捕获panic。
    unsafe {
        release_boundary(err, || {
            drop(Vec::from_raw_parts(ptr, len as usize, cap as usize))
        })
    }
}
