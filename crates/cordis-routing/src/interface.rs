//! DOC-11 §3、ADR-0041：协议接口名称、主版本键与次版本要求。

use crate::{Result, error};

#[derive(Clone, Debug)]
pub struct Interface {
    pub full: String,
    pub key: String,
    pub minor: u32,
}

impl Interface {
    /// DOC-11 §3：完整接口与主版本标识归一到同一服务键。
    pub fn key(name: &str) -> Result<String> {
        let version = name
            .rsplit_once('@')
            .ok_or_else(|| error(2, "接口需要名称和版本"))?
            .1;
        if version.contains('.') {
            Self::parse(name).map(|interface| interface.key)
        } else {
            Self::parse(&format!("{name}.0")).map(|interface| interface.key)
        }
    }

    /// DOC-11 §3：格式错误在描述符或请求边界返回，版本解析使用 semver。
    pub fn parse(full: &str) -> Result<Self> {
        let (name, version) = full
            .rsplit_once('@')
            .ok_or_else(|| error(2, "接口需要名称和版本"))?;
        if name.is_empty()
            || !name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
            })
        {
            return Err(error(2, "接口名称需要小写字母、数字、短横线或点"));
        }
        let version = semver::Version::parse(&format!("{version}.0"))
            .map_err(|problem| error(2, problem.to_string()))?;
        let major =
            u32::try_from(version.major).map_err(|problem| error(2, problem.to_string()))?;
        let minor =
            u32::try_from(version.minor).map_err(|problem| error(2, problem.to_string()))?;
        Ok(Self {
            full: full.into(),
            key: format!("{name}@{major}"),
            minor,
        })
    }
}
