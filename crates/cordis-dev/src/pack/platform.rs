//! DOC-15 §12.2、ADR-0036：平台身份从实际可执行文件取得，app 使用系统 bundle 格式。

use super::Plan;
use crate::Result;
use object::{Architecture, BinaryFormat, Object};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
pub enum Platform {
    MacArm,
    LinuxX64,
}

impl Platform {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "aarch64-apple-darwin" => Ok(Self::MacArm),
            "x86_64-unknown-linux-gnu" => Ok(Self::LinuxX64),
            _ => Err(format!(
                "发布目标支持 aarch64-apple-darwin、x86_64-unknown-linux-gnu，收到 {value}"
            )
            .into()),
        }
    }

    pub fn local(self) -> Result<()> {
        let matched = match self {
            Self::MacArm => cfg!(all(target_os = "macos", target_arch = "aarch64")),
            Self::LinuxX64 => cfg!(all(target_os = "linux", target_arch = "x86_64")),
        };
        if matched {
            Ok(())
        } else {
            Err("打包需要在所选目标平台运行".into())
        }
    }

    pub fn macos(self) -> bool {
        matches!(self, Self::MacArm)
    }

    pub fn library(self, name: &str) -> String {
        let extension = if self.macos() { "dylib" } else { "so" };
        format!("lib{}.{}", name.replace('-', "_"), extension)
    }

    /// AU-12：二进制格式和处理器架构与发布目标共同匹配。
    pub fn binary(self, path: &Path) -> Result<()> {
        let bytes = fs::read(path)?;
        let object = object::File::parse(bytes.as_slice())?;
        let expected = match self {
            Self::MacArm => (BinaryFormat::MachO, Architecture::Aarch64),
            Self::LinuxX64 => (BinaryFormat::Elf, Architecture::X86_64),
        };
        if (object.format(), object.architecture()) == expected {
            Ok(())
        } else {
            Err(format!("{} 的格式或架构与发布目标失配", path.display()).into())
        }
    }
}

pub fn bundle(plan: &Plan, destination: &Path) -> Result<()> {
    let mut properties = plist::Dictionary::new();
    for (key, value) in [
        ("CFBundleExecutable", "cordis"),
        ("CFBundleIdentifier", &plan.identifier),
        ("CFBundleName", &plan.name),
        ("CFBundleDisplayName", &plan.name),
        ("CFBundleVersion", &plan.version),
        ("CFBundleShortVersionString", &plan.version),
        ("CFBundlePackageType", "APPL"),
    ] {
        properties.insert(key.into(), plist::Value::String(value.into()));
    }
    properties.insert("LSUIElement".into(), plist::Value::Boolean(true));
    properties.insert(
        "NSHighResolutionCapable".into(),
        plist::Value::Boolean(true),
    );
    plist::Value::Dictionary(properties).to_file_xml(destination.join("Contents/Info.plist"))?;
    Ok(())
}

/// ADR-0036/0043：以实际路径收集Mach-O，目录深处的模块先签，外层bundle随后封装。
pub fn code_files(root: &Path, platform: Platform) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if platform.macos() {
        collect(&root.canonicalize()?, &mut files)?;
        files.sort_by_key(|path| (std::cmp::Reverse(path.components().count()), path.clone()));
    }
    Ok(files)
}

fn collect(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(&entry.path(), files)?;
        } else if kind.is_file() {
            let mut prefix = Vec::with_capacity(16);
            fs::File::open(entry.path())?
                .take(16)
                .read_to_end(&mut prefix)?;
            if matches!(
                object::FileKind::parse(prefix.as_slice()),
                Ok(object::FileKind::MachO32
                    | object::FileKind::MachO64
                    | object::FileKind::MachOFat32
                    | object::FileKind::MachOFat64)
            ) {
                files.push(entry.path());
            }
        }
    }
    Ok(())
}
