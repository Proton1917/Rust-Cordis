use crate::{
    ABI_MAJOR, ABI_MINOR, AbiError, AbiStatus, AbiStr, Apply, ComponentV1, DependencyV1,
    FORMAT_NATIVE, HostV1, LibraryId, PluginV1, Resolve, error::release_boundary,
};
use std::mem::{align_of, size_of};

pub type Entry = unsafe extern "C" fn(
    *const HostV1,
    LibraryId,
    *mut *const PluginV1,
    *mut AbiError,
) -> AbiStatus;

pub struct PluginEntry(Entry);

impl PluginEntry {
    /// # Safety
    /// DOC-12 §3、§10：入口及其描述符遵守ABI契约，代码在本令牌和产生的描述符存活期间有效。
    // SAFETY: DOC-12 §3 的代码寿命与入口契约由构造者保证。
    pub const unsafe fn new(entry: Entry) -> Self {
        Self(entry)
    }
    pub const fn function(&self) -> Entry {
        self.0
    }
}

/// # Safety
/// DOC-12 §10：components的来源覆盖component_count条连续记录，首地址按
/// ComponentV1对齐；每条size字段及其声明的字节有效，存储覆盖plugin的借用期。
// SAFETY: DOC-12 §10 的记录表范围与寿命由描述符持有者保证。
pub unsafe fn component_at(plugin: &PluginV1, index: usize) -> crate::Result<&ComponentV1> {
    if index as u64 >= plugin.component_count {
        return Err(crate::Error::new(
            2,
            "component index is outside the descriptor",
        ));
    }
    let mut record = plugin.components.cast::<u8>();
    for current in 0..=index {
        // SAFETY: DOC-12 §10，当前记录的首个u64字段可读。
        let size = unsafe { record.cast::<u64>().read() };
        if size < size_of::<ComponentV1>() as u64 || size % align_of::<ComponentV1>() as u64 != 0 {
            return Err(crate::Error::new(2, "component record size is invalid"));
        }
        if current < index {
            // SAFETY: DOC-12 §10，记录表覆盖该size范围，步长保持下一条记录对齐。
            record = unsafe { record.add(size as usize) };
        }
    }
    // SAFETY: DOC-12 §10，目标记录已验证最低大小，其前缀覆盖完整ComponentV1。
    Ok(unsafe { &*record.cast::<ComponentV1>() })
}

pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub sdk_version: String,
    pub build_info: String,
    pub panic_unwind: bool,
}
pub struct Dependency {
    pub interface: String,
    pub required: bool,
}
pub struct ComponentDefinition {
    pub name: String,
    pub inject: Vec<Dependency>,
    pub provides: Vec<String>,
    pub listens: Vec<String>,
    pub state_schema: String,
    pub state_version_min: u32,
    pub state_version_max: u32,
    pub apply: Apply,
    pub resolve: Resolve,
}

#[repr(C)]
pub struct PluginDescriptor {
    raw: PluginV1,
    metadata: PluginMetadata,
    definitions: Vec<ComponentDefinition>,
    components: Vec<ComponentV1>,
    inject: Vec<Vec<DependencyV1>>,
    provides: Vec<Vec<AbiStr>>,
    listens: Vec<Vec<AbiStr>>,
}

impl PluginDescriptor {
    pub fn new(metadata: PluginMetadata, definitions: Vec<ComponentDefinition>) -> Self {
        let inject: Vec<Vec<DependencyV1>> = definitions
            .iter()
            .map(|component| {
                component
                    .inject
                    .iter()
                    .map(|dependency| DependencyV1 {
                        interface: AbiStr::new(&dependency.interface),
                        required: u8::from(dependency.required),
                        reserved: [0; 7],
                    })
                    .collect()
            })
            .collect();
        let provides: Vec<Vec<AbiStr>> = definitions
            .iter()
            .map(|component| {
                component
                    .provides
                    .iter()
                    .map(|value| AbiStr::new(value))
                    .collect()
            })
            .collect();
        let listens: Vec<Vec<AbiStr>> = definitions
            .iter()
            .map(|component| {
                component
                    .listens
                    .iter()
                    .map(|value| AbiStr::new(value))
                    .collect()
            })
            .collect();
        let components: Vec<_> = definitions
            .iter()
            .enumerate()
            .map(|(index, component)| ComponentV1 {
                size: size_of::<ComponentV1>() as u64,
                name: AbiStr::new(&component.name),
                inject: inject[index].as_ptr(),
                inject_count: inject[index].len() as u64,
                provides: provides[index].as_ptr(),
                provides_count: provides[index].len() as u64,
                listens: listens[index].as_ptr(),
                listens_count: listens[index].len() as u64,
                state_schema: AbiStr::new(&component.state_schema),
                state_version_min: component.state_version_min,
                state_version_max: component.state_version_max,
                apply: component.apply,
                resolve: component.resolve,
            })
            .collect();
        let raw = PluginV1 {
            size: size_of::<PluginV1>() as u64,
            abi_major: ABI_MAJOR,
            abi_minor: ABI_MINOR,
            panic_unwind: u8::from(metadata.panic_unwind),
            format: FORMAT_NATIVE,
            reserved: [0; 6],
            name: AbiStr::new(&metadata.name),
            version: AbiStr::new(&metadata.version),
            sdk_version: AbiStr::new(&metadata.sdk_version),
            build_info: AbiStr::new(&metadata.build_info),
            components: components.as_ptr(),
            component_count: components.len() as u64,
            release: release_descriptor,
        };
        Self {
            raw,
            metadata,
            definitions,
            components,
            inject,
            provides,
            listens,
        }
    }
    pub fn into_raw(self) -> *const PluginV1 {
        Box::into_raw(Box::new(self)).cast()
    }
}

// SAFETY: DOC-12 §3、§10，指针来自PluginDescriptor::into_raw，唯一所有权转入。
unsafe extern "C" fn release_descriptor(raw: *const PluginV1, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §10，repr(C)保证raw位于PluginDescriptor首部。
    unsafe {
        release_boundary(err, || {
            drop(Box::from_raw(raw.cast_mut().cast::<PluginDescriptor>()))
        })
    }
}
