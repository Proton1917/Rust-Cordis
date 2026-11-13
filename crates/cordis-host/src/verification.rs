//! DOC-11 SW-2、§8：原生库打开前确认验证副本，握手后核对接口覆盖范围。

use cordis_abi::{Error, Result};
use cordis_types::VerificationReceipt;
use cordis_up::payload::Component;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

/// SW-2、ADR-0036：绝对引用属于配置根，相对引用按各根的完整字节选择副本。
pub(crate) fn before_open(
    source: &Path,
    candidate: &Path,
    verified_dirs: &[PathBuf],
) -> Result<VerificationReceipt> {
    let receipt: VerificationReceipt =
        serde_json::from_reader(File::open(receipt_path(source)).map_err(io_error)?)
            .map_err(|error| Error::new(2, format!("验证印记读取失败：{error}")))?;
    let roots = verification_roots(verified_dirs)?;
    if receipt.verified_copy.is_absolute() {
        let verified = receipt.verified_copy.canonicalize().map_err(io_error)?;
        if !roots.iter().any(|root| verified.starts_with(root)) {
            return Err(Error::new(2, "验证副本位于配置目录之外"));
        }
        if files_equal(candidate, &verified)? {
            return Ok(receipt);
        }
    } else {
        for root in roots {
            let verified = match root.join(&receipt.verified_copy).canonicalize() {
                Ok(path) => path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(io_error(error)),
            };
            if verified.starts_with(&root) && files_equal(candidate, &verified)? {
                return Ok(receipt);
            }
        }
    }
    Err(Error::new(2, "配置验证目录中缺少与候选完整字节相同的副本"))
}

/// ADR-0036：可写验证目录在首次验证或更新时创建，缺席目录保留为后续来源。
fn verification_roots(directories: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    for directory in directories {
        match directory.canonicalize() {
            Ok(root) => roots.push(root),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(roots)
}

/// DOC-15 §8.7、SW-2：已接受缓存库携带同一印记，分组重启继续执行完整准入检查。
pub(crate) fn preserve(library: &Path, receipt: &VerificationReceipt) -> Result<()> {
    let destination = receipt_path(library);
    let mut staged = destination.as_os_str().to_owned();
    staged.push(".pending");
    let staged = PathBuf::from(staged);
    let mut file = File::create(&staged).map_err(io_error)?;
    serde_json::to_writer(&mut file, receipt)
        .map_err(|error| Error::new(2, format!("验证印记保存失败：{error}")))?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    fs::rename(staged, destination).map_err(io_error)
}

fn receipt_path(library: &Path) -> PathBuf {
    let mut path = library.as_os_str().to_owned();
    path.push(".verified.json");
    PathBuf::from(path)
}

/// SW-2：描述符提供的接口已经完成格式检查，此处按接口主版本匹配成功套件。
pub(crate) fn coverage(receipt: &VerificationReceipt, components: &[Component]) -> Result<()> {
    let mut covered = HashSet::new();
    for suite in &receipt.suites {
        if suite.assertions != suite.results.len()
            || suite.results.iter().any(|result| !result.passed)
        {
            return Err(Error::new(
                2,
                format!("契约套件 {} 的断言记录失败或数量失配", suite.plugin),
            ));
        }
        covered.extend(suite.interfaces.iter().map(String::as_str));
    }
    for interface in components.iter().flat_map(|component| &component.provides) {
        let (major, _) = interface
            .rsplit_once('.')
            .expect("descriptor interface has a minor version");
        if !covered.contains(major) {
            return Err(Error::new(
                2,
                format!("验证印记缺少接口 {major} 的契约套件"),
            ));
        }
    }
    Ok(())
}

/// DOC-11 §8.3：有界缓冲逐字节比较两份文件，任一读取错误直接返回。
fn files_equal(first: &Path, second: &Path) -> Result<bool> {
    let mut first = BufReader::with_capacity(65536, File::open(first).map_err(io_error)?);
    let mut second = BufReader::with_capacity(65536, File::open(second).map_err(io_error)?);
    loop {
        let left = first.fill_buf().map_err(io_error)?;
        let right = second.fill_buf().map_err(io_error)?;
        if left.is_empty() || right.is_empty() {
            return Ok(left.is_empty() && right.is_empty());
        }
        let count = left.len().min(right.len());
        if left[..count] != right[..count] {
            return Ok(false);
        }
        first.consume(count);
        second.consume(count);
    }
}

fn io_error(error: std::io::Error) -> Error {
    Error::new(1, error.to_string())
}
