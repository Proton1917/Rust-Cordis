use std::{ffi::c_void, mem::MaybeUninit};

pub type AbiStatus = u32;
pub type LibraryId = u64;
pub const ABI_MAJOR: u32 = 1;
pub const ABI_MINOR: u32 = 0;
pub const FORMAT_NATIVE: u8 = 1;
pub const FORMAT_PROCESS: u8 = 2;
pub const FORMAT_WASM: u8 = 3;
pub const OBJECT_TYPED_METHODS: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiStr {
    pub ptr: *const u8,
    pub len: u64,
}

impl AbiStr {
    pub const fn new(value: &str) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len() as u64,
        }
    }

    /// # Safety
    /// DOC-12 §5：指针在所选借用期内对应 len 字节的有效内存。
    // SAFETY: DOC-12 §5 的调用期借用由调用者保证。
    pub unsafe fn as_str<'a>(self) -> crate::Result<&'a str> {
        // SAFETY: DOC-12 §5，字节借用继承本函数的调用前提。
        let bytes = unsafe {
            AbiBytes {
                ptr: self.ptr,
                len: self.len,
            }
            .as_slice()
        };
        std::str::from_utf8(bytes).map_err(|e| crate::Error::new(2, e.to_string()))
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiBytes {
    pub ptr: *const u8,
    pub len: u64,
}

impl AbiBytes {
    pub const fn new(value: &[u8]) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len() as u64,
        }
    }

    /// # Safety
    /// DOC-12 §5：非空指针对应所选借用期内的有效字节。
    // SAFETY: DOC-12 §5 的调用期借用由调用者保证。
    pub unsafe fn as_slice<'a>(self) -> &'a [u8] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: DOC-12 §2、§5，目标为64位，指针范围由调用者保证。
        unsafe { std::slice::from_raw_parts(self.ptr, self.len as usize) }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AbiUnit {
    pub reserved: u8,
}

pub type FreeBuffer =
    unsafe extern "C" fn(*mut c_void, *mut u8, u64, u64, *mut AbiError) -> AbiStatus;

#[repr(C)]
pub struct AbiBuf {
    pub ptr: *mut u8,
    pub len: u64,
    pub cap: u64,
    pub owner: *mut c_void,
    pub free: Option<FreeBuffer>,
    pub library: LibraryId,
}

impl AbiBuf {
    pub const fn empty() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
            cap: 0,
            owner: std::ptr::null_mut(),
            free: None,
            library: 0,
        }
    }
}

#[repr(C)]
pub struct AbiOption<T> {
    pub is_some: u8,
    pub value: MaybeUninit<T>,
}

impl<T> AbiOption<T> {
    pub const fn none() -> Self {
        Self {
            is_some: 0,
            value: MaybeUninit::uninit(),
        }
    }
    pub const fn some(value: T) -> Self {
        Self {
            is_some: 1,
            value: MaybeUninit::new(value),
        }
    }

    /// # Safety
    /// DOC-12 §5：标志为0或1；1对应初始化的拥有值，其所有权转移给调用者。
    // SAFETY: DOC-12 §5 的结构与所有权前提由调用者保证。
    pub unsafe fn take(self) -> Option<T> {
        if self.is_some == 0 {
            None
        } else {
            // SAFETY: DOC-12 §5，is_some=1 时载荷初始化。
            Some(unsafe { self.value.assume_init() })
        }
    }
}

#[repr(C)]
pub struct AbiError {
    pub code: u32,
    pub message: AbiBuf,
    pub detail: AbiBuf,
}

impl AbiError {
    pub const fn code(code: u32) -> Self {
        Self {
            code,
            message: AbiBuf::empty(),
            detail: AbiBuf::empty(),
        }
    }
}

#[repr(C)]
pub struct AbiState {
    pub schema: AbiBuf,
    pub version: u32,
    pub reserved: u32,
    pub data: AbiBuf,
}

#[repr(C)]
pub struct AbiObject {
    pub data: *mut c_void,
    pub vtable: *const ObjectHeader,
    pub library: LibraryId,
}

#[repr(C)]
pub struct ObjectHeader {
    pub size: u64,
    pub kind: u32,
    pub flags: u32,
    pub reserved: u32,
    pub interface: AbiStr,
    pub clone: unsafe extern "C" fn(*mut c_void, *mut AbiObject, *mut AbiError) -> AbiStatus,
    pub drop: unsafe extern "C" fn(*mut c_void, *mut AbiError) -> AbiStatus,
    pub methods: *const AbiStr,
    pub method_count: u64,
    pub invoke: Invoke,
    pub invoke_async: InvokeAsync,
}

pub type Invoke =
    unsafe extern "C" fn(*mut c_void, AbiStr, AbiBytes, *mut AbiBuf, *mut AbiError) -> AbiStatus;
pub type InvokeAsync =
    unsafe extern "C" fn(*mut c_void, AbiStr, AbiBytes, *mut AbiFuture, *mut AbiError) -> AbiStatus;

#[repr(C)]
pub struct AbiFuture {
    pub data: *mut c_void,
    pub vtable: *const FutureVTable,
    pub library: LibraryId,
}

#[repr(C)]
pub struct FutureVTable {
    pub size: u64,
    pub poll: unsafe extern "C" fn(
        *mut c_void,
        *const AbiWaker,
        *mut c_void,
        *mut u32,
        *mut AbiError,
    ) -> AbiStatus,
    pub drop: unsafe extern "C" fn(*mut c_void, *mut AbiError) -> AbiStatus,
}

#[repr(C)]
pub struct AbiWaker {
    pub data: *mut c_void,
    pub vtable: *const WakerVTable,
    pub library: LibraryId,
}

#[repr(C)]
pub struct WakerVTable {
    pub clone: unsafe extern "C" fn(*mut c_void, *mut AbiWaker, *mut AbiError) -> AbiStatus,
    pub wake: unsafe extern "C" fn(*mut c_void, *mut AbiError) -> AbiStatus,
    pub wake_by_ref: unsafe extern "C" fn(*mut c_void, *mut AbiError) -> AbiStatus,
    pub drop: unsafe extern "C" fn(*mut c_void, *mut AbiError) -> AbiStatus,
}

pub type SyncCall =
    unsafe extern "C" fn(*mut c_void, u64, u64, AbiBytes, *mut AbiError) -> AbiStatus;
pub type AsyncCall = unsafe extern "C" fn(
    *mut c_void,
    u64,
    u64,
    AbiBytes,
    *mut AbiFuture,
    *mut AbiError,
) -> AbiStatus;

#[repr(C)]
pub struct SyncCallbackVTable {
    pub header: ObjectHeader,
    pub call: SyncCall,
}
#[repr(C)]
pub struct AsyncCallbackVTable {
    pub header: ObjectHeader,
    pub call: AsyncCall,
}
#[repr(C)]
pub struct ExporterVTable {
    pub header: ObjectHeader,
    pub export: unsafe extern "C" fn(*mut c_void, *mut AbiBuf, *mut AbiError) -> AbiStatus,
    pub thaw: unsafe extern "C" fn(*mut c_void, *mut AbiError) -> AbiStatus,
}

#[repr(C)]
pub struct HostV1 {
    pub size: u64,
    pub log: unsafe extern "C" fn(u64, u32, AbiStr, *mut AbiError) -> AbiStatus,
    pub now: unsafe extern "C" fn(u64, *mut u64, *mut AbiError) -> AbiStatus,
    pub sleep: unsafe extern "C" fn(u64, u64, *mut AbiFuture, *mut AbiError) -> AbiStatus,
    pub spawn: unsafe extern "C" fn(u64, AbiFuture, *mut AbiError) -> AbiStatus,
    pub effect: unsafe extern "C" fn(u64, AbiStr, AbiObject, *mut u64, *mut AbiError) -> AbiStatus,
    pub effect_async:
        unsafe extern "C" fn(u64, AbiStr, AbiObject, *mut u64, *mut AbiError) -> AbiStatus,
    pub defer: unsafe extern "C" fn(u64, AbiObject, *mut AbiError) -> AbiStatus,
    pub defer_async: unsafe extern "C" fn(u64, AbiObject, *mut AbiError) -> AbiStatus,
    pub is_current: unsafe extern "C" fn(u64, *mut u8, *mut AbiError) -> AbiStatus,
    pub provide: unsafe extern "C" fn(u64, AbiStr, AbiObject, *mut AbiError) -> AbiStatus,
    pub get: unsafe extern "C" fn(u64, AbiStr, *mut AbiObject, *mut AbiError) -> AbiStatus,
    pub on: unsafe extern "C" fn(u64, AbiStr, AbiObject, *mut AbiError) -> AbiStatus,
    pub emit: unsafe extern "C" fn(u64, AbiStr, AbiBytes, *mut AbiError) -> AbiStatus,
    pub emit_async:
        unsafe extern "C" fn(u64, AbiStr, AbiBytes, *mut AbiFuture, *mut AbiError) -> AbiStatus,
    pub export_state: unsafe extern "C" fn(u64, AbiStr, u32, AbiObject, *mut AbiError) -> AbiStatus,
    pub inherited_state:
        unsafe extern "C" fn(u64, *mut AbiOption<AbiState>, *mut AbiError) -> AbiStatus,
    pub config: unsafe extern "C" fn(u64, *mut AbiBuf, *mut AbiError) -> AbiStatus,
    pub spawn_thread: unsafe extern "C" fn(u64, AbiStr, AbiObject, *mut AbiError) -> AbiStatus,
    pub lease_acquire: unsafe extern "C" fn(LibraryId, *mut AbiError) -> AbiStatus,
    pub lease_release: unsafe extern "C" fn(LibraryId, *mut AbiError) -> AbiStatus,
    pub effect_settled: unsafe extern "C" fn(u64, *mut AbiFuture, *mut AbiError) -> AbiStatus,
    pub effect_dispose: unsafe extern "C" fn(u64, *mut AbiFuture, *mut AbiError) -> AbiStatus,
    pub object_export: unsafe extern "C" fn(AbiObject, *mut u64, *mut AbiError) -> AbiStatus,
    pub object_import: unsafe extern "C" fn(u64, *mut AbiObject, *mut AbiError) -> AbiStatus,
    pub object_release: unsafe extern "C" fn(u64, *mut AbiError) -> AbiStatus,
    pub effect_release: unsafe extern "C" fn(u64, *mut AbiError) -> AbiStatus,
}

pub type Apply = unsafe extern "C" fn(u64, AbiBytes, *mut AbiFuture, *mut AbiError) -> AbiStatus;
pub type Resolve = unsafe extern "C" fn(AbiBytes, *mut AbiError) -> AbiStatus;

#[repr(C)]
pub struct PluginV1 {
    pub size: u64,
    pub abi_major: u32,
    pub abi_minor: u32,
    pub panic_unwind: u8,
    pub format: u8,
    pub reserved: [u8; 6],
    pub name: AbiStr,
    pub version: AbiStr,
    pub sdk_version: AbiStr,
    pub build_info: AbiStr,
    pub components: *const ComponentV1,
    pub component_count: u64,
    pub release: unsafe extern "C" fn(*const PluginV1, *mut AbiError) -> AbiStatus,
}

#[repr(C)]
pub struct ComponentV1 {
    pub size: u64,
    pub name: AbiStr,
    pub inject: *const DependencyV1,
    pub inject_count: u64,
    pub provides: *const AbiStr,
    pub provides_count: u64,
    pub listens: *const AbiStr,
    pub listens_count: u64,
    pub state_schema: AbiStr,
    pub state_version_min: u32,
    pub state_version_max: u32,
    pub apply: Apply,
    pub resolve: Resolve,
}

#[repr(C)]
pub struct DependencyV1 {
    pub interface: AbiStr,
    pub required: u8,
    pub reserved: [u8; 7],
}

// SAFETY: DOC-12 §2、§5，AbiStr只携带地址，解引用由显式unsafe借用入口执行。
unsafe impl Send for AbiStr {}
// SAFETY: DOC-12 §7，静态虚表的字符串地址在租约期间保持只读。
unsafe impl Sync for AbiStr {}
// SAFETY: DOC-12 §7，首部携带只读元数据和函数指针，实际访问由拥有对象维持寿命。
unsafe impl Send for ObjectHeader {}
// SAFETY: DOC-12 §7，虚表元数据按契约只读，指针解引用经拥有对象完成。
unsafe impl Sync for ObjectHeader {}
