//! DOC-12 §3、§10、DOC-11 SW-2：装载独立库副本并以描述符租约保持代码寿命。

use crate::lease::{Descriptor, Lease, UnloadPolicy};
use crate::verification;
use cordis_abi::{
    ABI_MAJOR, AbiBytes, AbiUnit, Apply, ComponentV1, Entry, Environment, Error, PluginV1,
    RemoteFuture, Resolve, Result,
};
use cordis_types::VerificationReceipt;
use cordis_up::payload::{Component as ComponentInfo, Dependency as DependencyInfo, Plugin};
use libloading::Library;
use semver::Version;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    mem::size_of,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct LibraryOptions {
    pub directory: PathBuf,
    pub verified_dir: PathBuf,
    pub require_verified: bool,
    pub unload: UnloadPolicy,
    pub allow_panic_abort: bool,
}

pub struct LibraryStore {
    options: LibraryOptions,
}

#[derive(Clone, Debug)]
pub struct LibraryInfo {
    pub id: u64,
    pub name: String,
    pub version: String,
    pub sdk_version: String,
    pub build_info: String,
    pub sequence: u64,
    pub path: PathBuf,
    pub verified: bool,
    pub components: Vec<ComponentInfo>,
}

impl LibraryInfo {
    pub fn description(&self) -> Plugin {
        Plugin {
            name: self.name.clone(),
            version: self.version.clone(),
            sdk_version: self.sdk_version.clone(),
            build_info: self.build_info.clone(),
            format: 1,
            library: Some(self.id),
            components: self.components.clone(),
        }
    }
}

pub struct NativeComponent {
    pub info: ComponentInfo,
    descriptor: Arc<Descriptor>,
    apply: Apply,
    resolve: Resolve,
}

impl NativeComponent {
    pub fn resolve(&self, config: &[u8]) -> Result<()> {
        // SAFETY: DOC-12 §10，组件保持描述符租约，配置字节覆盖同步校验调用。
        unsafe {
            self.descriptor
                .environment
                .call_unit(|err| (self.resolve)(AbiBytes::new(config), err))
        }
    }

    pub fn apply(&self, context: u64, config: &[u8]) -> Result<RemoteFuture<AbiUnit>> {
        let environment = self.descriptor.environment;
        // SAFETY: DOC-12 §10，apply输出单位Future，描述符租约覆盖构造调用。
        let raw = unsafe {
            environment.call(|out, err| (self.apply)(context, AbiBytes::new(config), out, err))
        }?;
        // SAFETY: DOC-12 §8、§10，接管apply返回的单位Future和既有租约。
        Ok(unsafe { RemoteFuture::from_raw(raw, environment) })
    }
}

pub struct NativeLibrary {
    info: LibraryInfo,
    pub lease: Arc<Lease>,
    components: HashMap<String, Arc<NativeComponent>>,
}

impl NativeLibrary {
    pub fn name(&self) -> &str {
        &self.info.name
    }

    pub fn info(&self) -> LibraryInfo {
        self.info.clone()
    }

    pub fn component(&self, name: &str) -> Result<Arc<NativeComponent>> {
        self.components
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new(2, format!("组件 {name} 当前不可用")))
    }

    /// 核心完成该版本的实例清理后调用，返回租约以等待代码静止。
    pub fn retire(self) -> Arc<Lease> {
        self.lease.retire();
        let lease = self.lease.clone();
        drop(self);
        lease.reap();
        lease
    }
}

pub struct OpenError {
    pub error: Error,
    pub copy: Option<PathBuf>,
}

impl LibraryStore {
    pub fn new(options: LibraryOptions) -> Result<Self> {
        fs::create_dir_all(&options.directory).map_err(io_error)?;
        Ok(Self { options })
    }

    /// DOC-11 SW-1：目录锁保护全局序号分配，预留文件后释放锁再复制和装载。
    fn reserve_copy(&self, extension: &str) -> Result<(u64, PathBuf, File)> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.options.directory.join(".sequence.lock"))
            .map_err(io_error)?;
        lock.lock().map_err(io_error)?;
        let mut maximum = 0;
        for entry in fs::read_dir(&self.options.directory).map_err(io_error)? {
            let name = entry.map_err(io_error)?.file_name();
            if let Some((stem, "dylib" | "so")) = name.to_string_lossy().rsplit_once('.')
                && let Some((_, sequence)) = stem.rsplit_once('-')
                && let Ok(sequence) = sequence.parse::<u64>()
            {
                maximum = maximum.max(sequence);
            }
        }
        let sequence = maximum
            .checked_add(1)
            .ok_or_else(|| Error::new(2, "库副本序号已达到上限"))?;
        let path = self
            .options
            .directory
            .join(format!("candidate-{sequence}.{extension}"));
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_error)?;
        Ok((sequence, path, output))
    }

    /// DOC-11 SW-1、SW-2：创建唯一副本后读取入口与描述符，失败返回已创建的副本路径。
    pub fn open(
        &self,
        source: &Path,
        environment: Environment,
    ) -> std::result::Result<NativeLibrary, OpenError> {
        let mut copy = None;
        let result = (|| {
            let extension = source
                .extension()
                .and_then(|extension| extension.to_str())
                .filter(|extension| matches!(*extension, "dylib" | "so"))
                .ok_or_else(|| Error::new(2, "原生库扩展名需要为 dylib 或 so"))?;
            let mut input = File::open(source).map_err(io_error)?;
            let (sequence, staged, mut output) = self.reserve_copy(extension)?;
            copy = Some(staged.clone());
            std::io::copy(&mut input, &mut output).map_err(io_error)?;
            output.sync_all().map_err(io_error)?;
            drop(output);
            let receipt = if self.options.require_verified {
                Some(verification::before_open(
                    source,
                    &staged,
                    &self.options.verified_dir,
                )?)
            } else {
                None
            };
            let descriptor = self.load_descriptor(&staged, environment)?;
            let lease = descriptor.lease.clone();
            let decoded = self.decode(descriptor, receipt.as_ref(), sequence, staged.clone());
            match decoded {
                Ok(loaded) => self.publish_copy(loaded, extension, receipt.as_ref(), &mut copy),
                Err(error) => {
                    lease.retire();
                    lease.reap();
                    Err(error)
                }
            }
        })();
        result.map_err(|error| OpenError { error, copy })
    }

    /// SW-1、SW-2：通过握手的副本按逻辑名称发布，保存印记失败时退役本次库。
    fn publish_copy(
        &self,
        mut loaded: NativeLibrary,
        extension: &str,
        receipt: Option<&VerificationReceipt>,
        copy: &mut Option<PathBuf>,
    ) -> Result<NativeLibrary> {
        let path = self.options.directory.join(format!(
            "{}-{}.{extension}",
            loaded.info.name, loaded.info.sequence
        ));
        if let Err(error) = fs::rename(&loaded.info.path, &path) {
            loaded.retire();
            return Err(io_error(error));
        }
        *copy = Some(path.clone());
        if let Some(receipt) = receipt
            && let Err(error) = verification::preserve(&path, receipt)
        {
            loaded.retire();
            return Err(error);
        }
        loaded.info.path = path;
        Ok(loaded)
    }

    /// DOC-12 §3、§10：校验入口协议，描述符构造失败时回收初始库租约。
    fn load_descriptor(&self, staged: &Path, environment: Environment) -> Result<Arc<Descriptor>> {
        // SAFETY: DOC-15 Q0，原生库只在独立宿主进程装载，副本路径属于本次准备操作。
        let library = unsafe { Library::new(staged) };
        let library = library.map_err(|error| Error::new(2, error.to_string()))?;
        // SAFETY: DOC-12 §3，ABI版本符号为只读u32，library覆盖读取期间。
        let major = unsafe {
            **library
                .get::<*const u32>(b"CORDIS_PLUGIN_ABI\0")
                .map_err(|error| Error::new(2, error.to_string()))?
        };
        if major != ABI_MAJOR {
            return Err(Error::new(3, format!("插件 ABI 主版本为 {major}")));
        }
        // SAFETY: DOC-12 §3，入口使用Entry签名，库映像随后交给租约记录。
        let entry = unsafe {
            *library
                .get::<Entry>(b"cordis_plugin_v1\0")
                .map_err(|error| Error::new(2, error.to_string()))?
        };
        let lease = Lease::register(library, self.options.unload);
        match Self::descriptor(entry, lease.clone(), environment) {
            Ok(descriptor) => Ok(descriptor),
            Err(error) => {
                lease.retire();
                crate::lease::release(lease.id)?;
                lease.reap();
                Err(error)
            }
        }
    }

    /// DOC-12 §3、§10：入口交付拥有描述符，长度通过后建立其库租约所有者。
    fn descriptor(
        entry: Entry,
        lease: Arc<Lease>,
        environment: Environment,
    ) -> Result<Arc<Descriptor>> {
        // SAFETY: DOC-12 §3，宿主表具有进程寿命，入口输出描述符并转交所有权。
        let raw: *const PluginV1 = unsafe {
            environment.call(|out, err| entry(environment.raw_host(), lease.id, out, err))
        }?;
        if raw.is_null() {
            return Err(Error::new(2, "插件入口返回空描述符"));
        }
        // SAFETY: DOC-12 §10，先读取可读的size字段确认当前ABI首部范围。
        if unsafe { raw.cast::<u64>().read() } < size_of::<PluginV1>() as u64 {
            return Err(Error::new(3, "插件描述符长度不足"));
        }
        Ok(Arc::new(Descriptor {
            raw,
            lease,
            environment,
        }))
    }

    /// DOC-12 §10：读取固定描述并验证声明契约，错误随候选库准备结果返回。
    fn decode(
        &self,
        descriptor: Arc<Descriptor>,
        receipt: Option<&VerificationReceipt>,
        sequence: u64,
        path: PathBuf,
    ) -> Result<NativeLibrary> {
        // SAFETY: DOC-12 §10，descriptor已确认首部范围并持有库租约。
        let raw = unsafe { &*descriptor.raw };
        self.validate_policy(raw)?;
        // SAFETY: DOC-12 §5、§10，描述符的字符串借用由当前租约保持有效。
        let (name, version, sdk_version, build_info) = unsafe {
            (
                raw.name.as_str()?.to_owned(),
                raw.version.as_str()?.to_owned(),
                raw.sdk_version.as_str()?.to_owned(),
                raw.build_info.as_str()?.to_owned(),
            )
        };
        validate_name(&name)?;
        Version::parse(&version).map_err(|error| Error::new(2, error.to_string()))?;
        if raw.panic_unwind == 0
            && let Err(error) = descriptor.environment.host().log(
                0,
                1,
                &format!("原生库 {name} 声明 panic=abort，宿主配置已允许该策略"),
            )
        {
            descriptor.environment.report(error);
        }
        let components = Self::components(&descriptor)?;
        let mut info = LibraryInfo {
            id: descriptor.lease.id,
            name,
            version,
            sdk_version,
            build_info,
            sequence,
            path,
            verified: receipt.is_some(),
            components: components
                .values()
                .map(|component| component.info.clone())
                .collect(),
        };
        info.components
            .sort_by(|left, right| left.name.cmp(&right.name));
        if let Some(receipt) = receipt {
            verification::coverage(receipt, &info.components)?;
        }
        Ok(NativeLibrary {
            info,
            lease: descriptor.lease.clone(),
            components,
        })
    }

    /// DOC-12 §3、§10：描述符遵循 ABI 主版本、原生格式与配置允许的 panic 策略。
    fn validate_policy(&self, raw: &PluginV1) -> Result<()> {
        if raw.abi_major != ABI_MAJOR || raw.format != 1 {
            return Err(Error::new(
                3,
                "插件描述符的 ABI、格式或 panic 策略与宿主配置不兼容",
            ));
        }
        match raw.panic_unwind {
            1 => {}
            0 if self.options.allow_panic_abort => {}
            0 => return Err(Error::new(3, "插件采用 panic=abort，当前宿主要求 unwind")),
            _ => return Err(Error::new(2, "panic_unwind 标志需要为 0 或 1")),
        }
        Ok(())
    }

    /// DOC-12 §10：各组件共享描述符租约，独立保留声明和调用入口。
    fn components(descriptor: &Arc<Descriptor>) -> Result<HashMap<String, Arc<NativeComponent>>> {
        // SAFETY: DOC-12 §10，descriptor已确认首部范围并持有库租约。
        let raw = unsafe { &*descriptor.raw };
        let mut components = HashMap::new();
        for index in 0..raw.component_count as usize {
            // SAFETY: DOC-12 §10，租约覆盖完整记录表，helper按各记录size前进。
            let component = unsafe { cordis_abi::component_at(raw, index) }?;
            let info = Self::component_info(component)?;
            if components.contains_key(&info.name) {
                return Err(Error::new(2, "插件内组件名称重复"));
            }
            components.insert(
                info.name.clone(),
                Arc::new(NativeComponent {
                    info,
                    descriptor: descriptor.clone(),
                    apply: component.apply,
                    resolve: component.resolve,
                }),
            );
        }
        Ok(components)
    }

    /// DOC-12 §10：组件记录已由 component_at 校验长度，读取其借用声明并复制到宿主。
    fn component_info(component: &ComponentV1) -> Result<ComponentInfo> {
        // SAFETY: DOC-12 §5、§10，组件的声明数组与名称由调用方描述符租约保持。
        unsafe {
            let name = component.name.as_str()?.to_owned();
            validate_name(&name)?;
            let mut inject = Vec::new();
            for index in 0..component.inject_count as usize {
                let dependency = &*component.inject.add(index);
                if dependency.required > 1 {
                    return Err(Error::new(2, "依赖 required 标志无效"));
                }
                let interface = dependency.interface.as_str()?.to_owned();
                validate_interface(&interface)?;
                inject.push(DependencyInfo {
                    interface,
                    required: dependency.required == 1,
                });
            }
            let mut provides = Vec::new();
            for index in 0..component.provides_count as usize {
                let interface = (*component.provides.add(index)).as_str()?.to_owned();
                validate_interface(&interface)?;
                provides.push(interface);
            }
            let mut listens = Vec::new();
            for index in 0..component.listens_count as usize {
                let event = (*component.listens.add(index)).as_str()?.to_owned();
                validate_interface(&event)?;
                listens.push(event);
            }
            Ok(ComponentInfo {
                name,
                inject,
                provides,
                listens,
                state_schema: component.state_schema.as_str()?.to_owned(),
                state_version_min: component.state_version_min,
                state_version_max: component.state_version_max,
            })
        }
    }
}

fn validate_name(value: &str) -> Result<()> {
    if !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        && !matches!(value, "." | "..")
    {
        Ok(())
    } else {
        Err(Error::new(2, format!("插件或组件名称无效：{value}")))
    }
}

fn validate_interface(value: &str) -> Result<()> {
    let (name, version) = value
        .split_once('@')
        .ok_or_else(|| Error::new(2, "接口标识需要名称与版本"))?;
    validate_name(name)?;
    let version = Version::parse(&format!("{version}.0"))
        .map_err(|error| Error::new(2, error.to_string()))?;
    u32::try_from(version.major).map_err(|_| Error::new(2, "接口主版本超出 u32 范围"))?;
    u32::try_from(version.minor).map_err(|_| Error::new(2, "接口次版本超出 u32 范围"))?;
    Ok(())
}

fn io_error(error: std::io::Error) -> Error {
    Error::new(1, error.to_string())
}
