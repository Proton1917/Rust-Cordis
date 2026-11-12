//! DOC-11 §8、SW-2：验证印记保存实际库副本与套件结果，内容身份采用完整字节比较。

use crate::Result;
use cordis_types::{ContractAssertion, VerificationReceipt, VerificationSuite};
use cordis_up::{Limits, payload};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// SW-1：目录锁内接续现有最大序号，预留会话目录后交回调用方。
pub fn session(root: &Path) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    let (_lock, sequence) = next_sequence(root, |name| name.parse().ok())?;
    let path = root.join(sequence.to_string());
    fs::create_dir(&path)?;
    Ok(path)
}

fn next_sequence(root: &Path, parse: impl Fn(&str) -> Option<u64>) -> Result<(File, u64)> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".sequence.lock"))?;
    lock.lock()?;
    let mut maximum = 0;
    for entry in fs::read_dir(root)? {
        if let Some(sequence) = parse(&entry?.file_name().to_string_lossy()) {
            maximum = maximum.max(sequence);
        }
    }
    let next = maximum.checked_add(1).ok_or("验证副本序号已耗尽")?;
    Ok((lock, next))
}

/// CP-2：报告与印记输入沿 UP/1 的文件字节上限读取，超额在反序列化前报告。
pub fn read(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(Limits::DEFAULT.max_frame as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > Limits::DEFAULT.max_frame {
        return Err(format!("文件超过 16 MiB：{}", path.display()).into());
    }
    Ok(bytes)
}

pub fn suite(
    plugin: &payload::Plugin,
    interfaces: Vec<String>,
    results: Vec<ContractAssertion>,
    duration: Duration,
) -> Result<VerificationSuite> {
    Ok(VerificationSuite {
        plugin: plugin.name.clone(),
        version: plugin.version.clone(),
        interfaces,
        assertions: results.len(),
        results,
        verified_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        duration_ms: duration.as_millis().try_into()?,
    })
}

/// DOC-11 §8.2：相同库内容合并套件结果，新内容取得新的验证副本和序号。
pub fn save(
    project: &Path,
    source: &Path,
    staged: &Path,
    plugin: &str,
    suite: VerificationSuite,
) -> Result<PathBuf> {
    let root = project.join(".cordis/verified");
    fs::create_dir_all(&root)?;
    let root = root.canonicalize()?;
    let path = stamp_path(source);
    let previous = previous(&path, staged, &root)?;
    let mut receipt = match previous {
        Some(receipt) => receipt,
        None => VerificationReceipt {
            verified_copy: copy_version(&root, staged, plugin)?,
            suites: Vec::new(),
            verified_at: suite.verified_at,
        },
    };
    receipt.suites.retain(|entry| {
        !(entry.plugin == suite.plugin
            && entry.version == suite.version
            && entry.interfaces == suite.interfaces)
    });
    receipt.verified_at = suite.verified_at;
    receipt.suites.push(suite);
    let data = serde_json::to_vec_pretty(&receipt)?;
    if data.len() > Limits::DEFAULT.max_frame {
        return Err("验证印记超过 16 MiB".into());
    }
    let pending = path.with_extension("json.next");
    let mut output = File::create(&pending)?;
    output.write_all(&data)?;
    output.sync_all()?;
    drop(output);
    fs::rename(pending, &path)?;
    Ok(path)
}

/// SW-2：旧套件结果继续有效的依据是同一保留副本与当前被测副本的完整字节相等。
fn previous(path: &Path, staged: &Path, root: &Path) -> Result<Option<VerificationReceipt>> {
    match fs::metadata(path) {
        Ok(_) => {}
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(problem) => return Err(problem.into()),
    }
    let receipt: VerificationReceipt = serde_json::from_slice(&read(path)?)?;
    let copy = match receipt.verified_copy.canonicalize() {
        Ok(copy) => copy,
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(problem) => return Err(problem.into()),
    };
    if copy.starts_with(root) && files_equal(&copy, staged)? {
        Ok(Some(receipt))
    } else {
        Ok(None)
    }
}

fn copy_version(root: &Path, source: &Path, plugin: &str) -> Result<PathBuf> {
    let (lock, sequence) = next_sequence(root, |name| {
        let (stem, extension) = name.rsplit_once('.')?;
        if !matches!(extension, "dylib" | "so") {
            return None;
        }
        stem.rsplit_once('-')?.1.parse().ok()
    })?;
    let path = root.join(format!(
        "{plugin}-{sequence}{}",
        std::env::consts::DLL_SUFFIX
    ));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    drop(lock);
    std::io::copy(&mut File::open(source)?, &mut output)?;
    output.sync_all()?;
    Ok(path)
}

fn stamp_path(source: &Path) -> PathBuf {
    let mut path = source.as_os_str().to_owned();
    path.push(".verified.json");
    PathBuf::from(path)
}

/// SW-2：分块读取完整字节；文件大小和每个字节都参与相等判定。
fn files_equal(first: &Path, second: &Path) -> Result<bool> {
    let mut first = File::open(first)?;
    let mut second = File::open(second)?;
    let size = first.metadata()?.len();
    if size != second.metadata()?.len() {
        return Ok(false);
    }
    let mut left = [0_u8; 64 * 1024];
    let mut right = [0_u8; 64 * 1024];
    let mut remaining = size;
    while remaining != 0 {
        let count = remaining.min(left.len() as u64) as usize;
        first.read_exact(&mut left[..count])?;
        second.read_exact(&mut right[..count])?;
        if left[..count] != right[..count] {
            return Ok(false);
        }
        remaining -= count as u64;
    }
    Ok(true)
}
