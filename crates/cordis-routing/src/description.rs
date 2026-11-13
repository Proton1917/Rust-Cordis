//! ADR-0041/0046：协议描述符的名称、接口与保留字节解析，生命周期定义由监管器构造。

use crate::{Interface, Result, error, metadata};
use cordis_up::payload;
use std::{
    collections::HashSet,
    path::{Component, Path},
};

/// SW-3、CP-2/CP-8：保留描述符次序，名称和状态版本范围在登记边界检查。
pub fn components(
    descriptor: &payload::Describe,
) -> Result<Vec<(&payload::Plugin, &payload::Component)>> {
    let mut plugins = HashSet::new();
    let mut components = HashSet::new();
    let mut output = Vec::new();
    for plugin in &descriptor.plugins {
        validate_name(&plugin.name)?;
        semver::Version::parse(&plugin.version).map_err(|problem| error(2, problem.to_string()))?;
        if !matches!(plugin.format, 1 | 2) {
            return Err(error(3, "插件格式需要为原生或通用进程"));
        }
        if plugin.format == 1 && plugin.library.is_none_or(|library| library == 0) {
            return Err(error(2, "原生插件描述需要正库编号"));
        }
        if !plugins.insert(plugin.name.as_str()) {
            return Err(error(2, "插件名称重复"));
        }
        for info in &plugin.components {
            info.name.split('/').try_for_each(validate_name)?;
            if !info.state_schema.is_empty() && info.state_version_min > info.state_version_max {
                return Err(error(2, "继承状态版本范围无效"));
            }
            if !components.insert((plugin.name.as_str(), info.name.as_str())) {
                return Err(error(2, "组件名称重复"));
            }
            output.push((plugin, info));
        }
    }
    Ok(output)
}

pub struct ComponentTypes {
    pub inject: Vec<(Interface, bool)>,
    pub provides: Vec<Interface>,
    pub bytes: usize,
}

/// SW-3、ADR-0016：解析组件接口，并计入复制的名称和展开的版本标识。
pub fn component_types(
    group: &str,
    plugin: &payload::Plugin,
    info: &payload::Component,
) -> Result<ComponentTypes> {
    let inject = info
        .inject
        .iter()
        .map(|dependency| {
            Ok((
                Interface::parse(&dependency.interface)?,
                dependency.required,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let provides = info
        .provides
        .iter()
        .map(|interface| Interface::parse(interface))
        .collect::<Result<Vec<_>>>()?;
    for event in &info.listens {
        Interface::parse(event)?;
    }
    let copied = metadata::component_bytes(info)?
        .checked_add(metadata::strings([group, plugin.name.as_str()])?)
        .ok_or_else(|| error(8, "组件元数据长度超出地址空间"))?;
    let parsed = metadata::strings(
        inject
            .iter()
            .map(|(interface, _)| interface)
            .chain(provides.iter())
            .flat_map(|interface| [interface.full.as_str(), interface.key.as_str()]),
    )?;
    let bytes = copied
        .checked_add(parsed)
        .ok_or_else(|| error(8, "组件元数据长度超出地址空间"))?;
    Ok(ComponentTypes {
        inject,
        provides,
        bytes,
    })
}

pub fn validate_name(name: &str) -> Result<()> {
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(part)), None) if part == name => Ok(()),
        _ => Err(error(2, "名称需要为一个完整名称段")),
    }
}
