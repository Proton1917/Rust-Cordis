//! DOC-11 §8、ADR-0028：独立验证核心、开发工具和原生宿主共用的结果与印记数据。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// DOC-11 §8.1：一个接口行为断言的真实结果，顺序与套件报告顺序一致。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContractAssertion {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

/// ADR-0017：两个阶段的合并结果，finished 标记套件完成，error 保存运行故障。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ContractReport {
    pub finished: bool,
    pub passed: bool,
    pub stage: u32,
    pub assertions: Vec<ContractAssertion>,
    pub error: Option<String>,
}

/// DOC-11 §8.2：套件版本、接口主版本覆盖与断言结果；时间以 Unix 秒记录。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VerificationSuite {
    pub plugin: String,
    pub version: String,
    pub interfaces: Vec<String>,
    pub assertions: usize,
    pub results: Vec<ContractAssertion>,
    pub verified_at: u64,
    pub duration_ms: u64,
}

/// SW-2：verified_copy 指向保留的库副本，宿主以完整字节比较确认候选内容。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VerificationReceipt {
    pub verified_copy: PathBuf,
    pub suites: Vec<VerificationSuite>,
    pub verified_at: u64,
}
