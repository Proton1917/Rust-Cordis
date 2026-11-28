//! Cargo 工作区成员给出包身份，compiler-artifact 消息给出本次构建的路径与特性。

use crate::{Result, build_paths, tooling};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Build {
    pub workspace: Option<PathBuf>,
    pub plugins: BTreeMap<String, Selection>,
    pub web: Vec<Web>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    package: String,
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Web {
    pub name: String,
    pub directory: PathBuf,
    pub command: Vec<String>,
    pub output: PathBuf,
}

#[derive(Deserialize)]
struct Workspace {
    workspace_root: PathBuf,
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    version: String,
    targets: Vec<Target>,
}

#[derive(Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
    crate_types: Vec<String>,
}

impl Target {
    fn output_kind(&self) -> Option<&'static str> {
        if self.kind.iter().any(|kind| kind == "bin") {
            Some("bin")
        } else if self.crate_types.iter().any(|kind| kind == "cdylib")
            && !self.kind.iter().any(|kind| kind == "example")
        {
            Some("cdylib")
        } else {
            None
        }
    }
}

#[derive(Deserialize)]
struct Compiled {
    package_id: String,
    target: Target,
    features: Vec<String>,
    filenames: Vec<PathBuf>,
    executable: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
enum Message {
    CompilerArtifact(Compiled),
    #[serde(other)]
    Other,
}

#[derive(Serialize)]
pub struct Artifact {
    #[serde(skip)]
    pub path: PathBuf,
    package: String,
    version: String,
    cargo_target: String,
    kind: &'static str,
    platform: String,
    profile: &'static str,
    features: Vec<String>,
}

impl Artifact {
    pub fn publish(&self, directory: &Path) -> Result<()> {
        Ok(std::fs::write(
            directory.join("cordis-build.json"),
            serde_json::to_vec_pretty(self)?,
        )?)
    }
}

#[derive(Default)]
pub struct Artifacts(Vec<Artifact>);

impl Artifacts {
    pub fn libraries(&self) -> impl Iterator<Item = &PathBuf> {
        self.0
            .iter()
            .filter(|item| item.kind == "cdylib")
            .map(|item| &item.path)
    }

    pub fn select(
        &self,
        kind: &str,
        filename: &str,
        selection: Option<&Selection>,
    ) -> Result<Option<&Artifact>> {
        let mut matching = self.0.iter().filter(|item| {
            item.kind == kind
                && match selection {
                    Some(key) => item.package == key.package && item.cargo_target == key.target,
                    None => item.path.file_name().is_some_and(|name| name == filename),
                }
        });
        let first = matching.next();
        if let Some(key) = selection
            && first.is_none()
        {
            return Err(format!(
                "Cargo 包 {} 缺少已构建的 {kind} 目标 {}",
                key.package, key.target
            )
            .into());
        }
        if let Some(second) = matching.next() {
            return Err(format!("发布产物 {filename} 同时匹配包 {} 和 {}，请在 build.plugins 中指定 package 与 target", first.unwrap().package, second.package).into());
        }
        Ok(first)
    }
}

pub async fn build(
    project: &Path,
    build: &Build,
    platform: &str,
    target_dir: &Path,
) -> Result<Artifacts> {
    let Some(manifest) = &build.workspace else {
        if !build.plugins.is_empty() {
            return Err("build.plugins 需要 build.workspace".into());
        }
        return Ok(Artifacts::default());
    };
    let manifest = project.join(manifest).canonicalize()?;
    let cargo = tooling::executable("cargo", None)?
        .to_string_lossy()
        .into_owned();
    let metadata = tooling::command(
        vec![
            cargo.clone(),
            "metadata".into(),
            "--format-version=1".into(),
            "--no-deps".into(),
            "--manifest-path".into(),
            manifest.to_string_lossy().into_owned(),
        ],
        manifest.parent().ok_or("Cargo 清单需要父目录")?,
        Duration::from_secs(60),
    )
    .await?;
    let workspace: Workspace = serde_json::from_slice(&metadata)?;
    let mut seen = BTreeSet::new();
    let mut groups = BTreeMap::<PathBuf, Vec<&Package>>::new();
    for package in &workspace.packages {
        // 同名 bin 或 cdylib 在 Cargo release 目录占用同一路径，分配独立构建目录。
        let outputs: Vec<_> = package
            .targets
            .iter()
            .filter_map(|target| target.output_kind().map(|kind| (kind, &target.name)))
            .collect();
        let directory = if outputs.iter().any(|key| seen.contains(key)) {
            target_dir.join("cordis-pack").join(&package.name)
        } else {
            seen.extend(outputs);
            target_dir.to_owned()
        };
        groups.entry(directory).or_default().push(package);
    }
    let env: Vec<(String, String)> =
        build_paths::environment(&workspace.workspace_root, Some(platform))?
            .into_iter()
            .collect();
    let mut artifacts = Artifacts::default();
    for (directory, members) in groups {
        let mut command = vec![
            cargo.clone(),
            "build".into(),
            "--manifest-path".into(),
            manifest.to_string_lossy().into_owned(),
            "--release".into(),
            "--target".into(),
            platform.into(),
            "--target-dir".into(),
            directory.to_string_lossy().into_owned(),
            "--message-format=json-render-diagnostics".into(),
        ];
        for member in &members {
            command.extend(["--package".into(), member.id.clone()]);
        }
        let messages = tooling::command_with_env(
            command,
            &env,
            &workspace.workspace_root,
            Duration::from_secs(600),
        )
        .await?;
        for message in serde_json::Deserializer::from_slice(&messages).into_iter::<Message>() {
            let Message::CompilerArtifact(record) = message? else {
                continue;
            };
            let Some(kind) = record.target.output_kind() else {
                continue;
            };
            let Some(package) = members.iter().find(|member| member.id == record.package_id) else {
                continue;
            };
            let path = match kind {
                "bin" => record.executable,
                _ => record.filenames.into_iter().find(|path| {
                    matches!(
                        path.extension().and_then(|s| s.to_str()),
                        Some("dylib" | "so" | "dll")
                    )
                }),
            }
            .ok_or_else(|| {
                format!(
                    "Cargo 包 {} 的 {} 消息缺少 {kind} 文件",
                    package.name, record.target.name
                )
            })?;
            artifacts.0.push(Artifact {
                path,
                kind,
                package: package.name.clone(),
                version: package.version.clone(),
                cargo_target: record.target.name,
                platform: platform.into(),
                profile: "release",
                features: record.features,
            });
        }
    }
    Ok(artifacts)
}

pub fn filename(path: &Path) -> Result<String> {
    Ok(path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("产物文件名需要 UTF-8")?
        .to_owned())
}
