//! ADR-0046 §3.10/3.11：插件读取密钥文件，并按核心给出的实例身份执行自身授权策略。
#![cfg_attr(unix, forbid(unsafe_code))]

use cordis_sdk::prelude::*;
use secrets_api::Secrets;
use serde::Deserialize;
use std::{collections::BTreeMap, fs::File, io::Read, path::PathBuf};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub file: PathBuf,
    #[serde(default)]
    pub grants: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(rename = "cordis.caller")]
    caller: Option<Caller>,
}
#[derive(Deserialize)]
struct Caller {
    instance: Option<String>,
}

struct Store {
    values: Arc<BTreeMap<String, String>>,
    grants: Arc<BTreeMap<String, Vec<String>>>,
    caller: Option<String>,
}

#[cordis_sdk::interface]
impl Secrets for Store {
    async fn get(&self, name: String) -> Result<Option<String>> {
        let allowed = self
            .caller
            .as_ref()
            .and_then(|caller| self.grants.get(caller));
        if !allowed.is_some_and(|allowed| allowed.contains(&name)) {
            return Err(Error::new(9, "当前实例缺少该密钥的读取授权"));
        }
        Ok(self.values.get(&name).cloned())
    }
}

struct Provider;
impl Component for Provider {
    const NAME: &'static str = "secrets";
    type Config = Config;
    fn provides() -> Provides {
        Provides::new().with::<dyn Secrets>()
    }

    /// ADR-0046 §3.10：同一打开文件上检查属主和权限，再由 dotenv-parser 读取字面值。
    async fn apply(context: Context, config: Config) -> Result<()> {
        let path = config.file.display();
        let failure = |problem| Error::new(1, format!("读取密钥文件 {path}：{problem}"));
        let mut file = File::open(&config.file).map_err(failure)?;
        check_secret_permissions(&file, &config.file)?;
        let mut source = String::new();
        file.read_to_string(&mut source).map_err(failure)?;
        let values = Arc::new(
            dotenv_parser::parse_dotenv(&source)
                .map_err(|_| Error::new(2, format!("密钥文件 {path} 的 NAME=VALUE 格式错误")))?,
        );
        let grants = Arc::new(config.grants);
        let base: Arc<dyn Secrets> = Arc::new(Store {
            values: values.clone(),
            grants: grants.clone(),
            caller: None,
        });
        context.provide_scoped::<dyn Secrets>(Scoped::new(base, move |meta: Metadata| {
            Ok(Arc::new(Store {
                values: values.clone(),
                grants: grants.clone(),
                caller: meta.caller.and_then(|caller| caller.instance),
            }) as Arc<dyn Secrets>)
        }))
    }
}

cordis_sdk::export_plugin!(
    name = "secrets",
    version = env!("CARGO_PKG_VERSION"),
    components = [Provider]
);

#[cfg(unix)]
fn check_secret_permissions(file: &File, path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let info = file
        .metadata()
        .map_err(|problem| Error::new(1, format!("读取密钥文件 {}：{problem}", path.display())))?;
    if info.uid() != rustix::process::geteuid().as_raw() || info.mode() & 0o7777 & !0o600 != 0 {
        return Err(Error::new(
            9,
            format!(
                "密钥文件 {} 属主 {}、权限 {:04o}；要求当前用户及至多 0600",
                path.display(),
                info.uid(),
                info.mode() & 0o7777
            ),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn check_secret_permissions(file: &File, path: &std::path::Path) -> Result<()> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
        GetTokenInformation, OWNER_SECURITY_INFORMATION, PSID, SECURITY_MAX_SID_SIZE, TOKEN_QUERY,
        TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

    fn current_user_sid() -> Result<Vec<u8>> {
        let mut token = null_mut();
        // SAFETY: 获取当前进程查询令牌句柄。
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if ok == 0 {
            return Err(Error::new(
                1,
                format!("读取当前进程令牌失败：{}", std::io::Error::last_os_error()),
            ));
        }
        let mut needed = 0u32;
        // SAFETY: 首次调用探测缓冲区大小。
        unsafe {
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed);
        }
        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: buffer 尺寸与 needed 匹配。
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        // SAFETY: 关闭本次打开的令牌句柄。
        unsafe {
            CloseHandle(token);
        }
        if ok == 0 {
            return Err(Error::new(
                1,
                format!("读取当前用户 SID 失败：{}", std::io::Error::last_os_error()),
            ));
        }
        Ok(buffer)
    }

    fn well_known(kind: i32) -> Result<Vec<u8>> {
        let mut size = SECURITY_MAX_SID_SIZE;
        let mut buffer = vec![0u8; size as usize];
        let ok = {
            // SAFETY: 创建系统预定义 SID（SYSTEM / Administrators）。
            unsafe { CreateWellKnownSid(kind, null_mut(), buffer.as_mut_ptr().cast(), &mut size) }
        };
        if ok == 0 {
            return Err(Error::new(
                1,
                format!("构造系统 SID 失败：{}", std::io::Error::last_os_error()),
            ));
        }
        buffer.truncate(size as usize);
        Ok(buffer)
    }

    let token_buf = current_user_sid()?;
    // SAFETY: token_buf 来自成功的 GetTokenInformation(TokenUser)。
    let user_sid: PSID = unsafe { (*(token_buf.as_ptr() as *const TOKEN_USER)).User.Sid };
    let system_sid = well_known(WinLocalSystemSid)?;
    let admins_sid = well_known(WinBuiltinAdministratorsSid)?;

    let mut owner_sid: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut sd: *mut c_void = null_mut();
    // SAFETY: 在已打开的文件句柄上读取属主与 DACL，避免路径 TOCTOU 竞态。
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner_sid,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(Error::new(
            1,
            format!(
                "读取密钥文件 {} 安全描述符失败（错误码 {status}）",
                path.display()
            ),
        ));
    }

    struct SdGuard(*mut c_void);
    impl Drop for SdGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: sd 由 GetSecurityInfo 经 LocalAlloc 分配。
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }
    let _guard = SdGuard(sd);

    let is_allowed_sid = |sid: PSID| -> bool {
        // SAFETY: 各 SID 指针在 _guard 与局部缓冲生命周期内有效。
        unsafe {
            EqualSid(sid, user_sid) != 0
                || EqualSid(sid, system_sid.as_ptr() as PSID) != 0
                || EqualSid(sid, admins_sid.as_ptr() as PSID) != 0
        }
    };

    if owner_sid.is_null() || !is_allowed_sid(owner_sid) {
        return Err(Error::new(
            9,
            format!("密钥文件 {} 属主不是当前用户或管理员", path.display()),
        ));
    }
    if dacl.is_null() {
        return Err(Error::new(
            9,
            format!(
                "密钥文件 {} 缺少显式 DACL（允许所有用户访问）",
                path.display()
            ),
        ));
    }

    let mut size_info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: dacl 非空且有效，查询 ACE 数量。
    let ok = unsafe {
        GetAclInformation(
            dacl,
            (&raw mut size_info).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    if ok == 0 {
        return Err(Error::new(
            1,
            format!(
                "读取密钥文件 {} DACL 失败：{}",
                path.display(),
                std::io::Error::last_os_error()
            ),
        ));
    }

    for index in 0..size_info.AceCount {
        let mut ace_ptr: *mut c_void = null_mut();
        // SAFETY: index < AceCount，读取对应 ACE。
        if unsafe { GetAce(dacl, index, &mut ace_ptr) } == 0 || ace_ptr.is_null() {
            return Err(Error::new(
                1,
                format!("读取密钥文件 {} ACE 失败", path.display()),
            ));
        }
        // SAFETY: 所有 ACE 头部均为 ACE_HEADER。
        let header = unsafe { &*(ace_ptr as *const ACE_HEADER) };
        if header.AceType == ACCESS_ALLOWED_ACE_TYPE {
            let sid = {
                // SAFETY: ACCESS_ALLOWED_ACE 的 SidStart 字段起始即为受托人 SID。
                unsafe { &raw const (*(ace_ptr as *const ACCESS_ALLOWED_ACE)).SidStart }
            } as PSID;
            if !is_allowed_sid(sid) {
                return Err(Error::new(
                    9,
                    format!(
                        "密钥文件 {} 的 ACL 向当前用户、SYSTEM、Administrators 之外的主体授予了访问权限",
                        path.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}
