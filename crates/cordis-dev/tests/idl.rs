//! DOC-15 §7、M17：开发命令生成三语言绑定，经真实 SDK 与 CBOR 往返执行。

use cordis_testkit::cleanup::TestGuard;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

/// 每一步保留实际输出，失败时停止其后依赖步骤并报告命令与日志位置。
fn run(command: &mut Command, log: &Path) {
    let output = command.output().expect("运行 IDL 验收命令");
    let mut contents = output.stdout.clone();
    contents.extend_from_slice(&output.stderr);
    fs::write(log, &contents).unwrap();
    assert!(
        output.status.success(),
        "命令失败 {command:?}\n日志：{}\n{}",
        log.display(),
        String::from_utf8_lossy(&contents)
    );
}

struct Acceptance {
    root: PathBuf,
    fixture: PathBuf,
    directory: PathBuf,
    generated: PathBuf,
}

impl Acceptance {
    /// 本次生成物与依赖属于独立目录，Rust 依赖编译复用项目内 Cargo 缓存。
    fn new(guard: &TestGuard) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let fixture = root.join("crates/cordis-dev/tests/fixtures/idl");
        let directory = guard.track(root.join("target/idl-acceptance").join(format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
        let generated = directory.join("generated");
        fs::create_dir_all(generated.join("js")).unwrap();
        fs::create_dir_all(directory.join("tmp")).unwrap();
        Self {
            root,
            fixture,
            directory,
            generated,
        }
    }

    fn execute(&self, command: &mut Command, name: &str) {
        run(
            command
                .current_dir(&self.root)
                .env("TMPDIR", self.directory.join("tmp")),
            &self.directory.join(format!("{name}.log")),
        );
    }

    /// DOC-15 §7：全部绑定通过公开命令从源 WIT 生成，直接供后续编译读取。
    fn generate(&self) {
        for (name, input) in [
            ("kv", self.root.join("plugins/kv-api/kv.wit")),
            ("types", self.fixture.join("types.wit")),
        ] {
            for (language, extension) in [("rust", "rs"), ("python", "py"), ("ts", "ts")] {
                self.execute(
                    Command::new(env!("CARGO_BIN_EXE_cordis-dev"))
                        .args(["idl", "gen", "--lang", language, "--output"])
                        .arg(self.generated.join(format!("{name}.{extension}")))
                        .arg(&input),
                    &format!("generate-{name}-{language}"),
                );
            }
        }
    }

    /// 脚本包安装在本次验收目录，版本由 requirements 与 npm lock 固定。
    fn prepare_scripts(&self) {
        self.execute(
            Command::new("conda")
                .env("PIP_REQUIRE_VIRTUALENV", "false")
                .args([
                    "run",
                    "-n",
                    "base",
                    "python",
                    "-m",
                    "pip",
                    "install",
                    "--disable-pip-version-check",
                    "--no-deps",
                    "--cache-dir",
                ])
                .arg(self.root.join("target/idl-dependencies/pip-cache"))
                .arg("--target")
                .arg(self.directory.join("python"))
                .arg("--requirement")
                .arg(self.fixture.join("requirements.txt")),
            "prepare-python",
        );
        let javascript = self.directory.join("javascript");
        fs::create_dir_all(&javascript).unwrap();
        for name in ["package.json", "package-lock.json"] {
            fs::copy(self.fixture.join(name), javascript.join(name)).unwrap();
        }
        self.execute(
            Command::new("npm")
                .args([
                    "ci",
                    "--ignore-scripts",
                    "--no-audit",
                    "--no-fund",
                    "--cache",
                ])
                .arg(self.root.join("target/idl-dependencies/npm-cache"))
                .arg("--prefix")
                .arg(javascript),
            "prepare-typescript",
        );
    }

    /// ADR-0039：同一真实 SDK fixture 分别承担原生行为与脚本输出回读。
    fn rust(&self, test: &str, stage: &str) {
        let _build = cordis_testkit::build::shared_build_lock(&self.root).unwrap();
        self.execute(
            Command::new(env!("CARGO"))
                .args(["test", "--manifest-path"])
                .arg(self.fixture.join("Cargo.toml"))
                .arg("--target-dir")
                .arg(cordis_testkit::build::shared_target_dir(&self.root))
                .args(["--", "--exact", test])
                .env("CORDIS_IDL_GENERATED", &self.generated),
            stage,
        );
    }

    /// Python 与 Node 的生成 Client 通过成熟 CBOR 库调用各自提供方。
    fn scripts(&self) {
        self.execute(
            Command::new("conda")
                .args(["run", "-n", "base", "python"])
                .arg(self.fixture.join("check_python.py"))
                .arg(&self.generated)
                .arg(self.directory.join("python")),
            "python-behavior",
        );
        let modules = self.directory.join("javascript/node_modules");
        self.execute(
            Command::new("node")
                .arg(modules.join("typescript/bin/tsc"))
                .args(["--strict", "--target", "ES2022", "--module", "commonjs"])
                .arg("--outDir")
                .arg(self.generated.join("js"))
                .arg(self.generated.join("kv.ts"))
                .arg(self.generated.join("types.ts")),
            "typescript-build",
        );
        self.execute(
            Command::new("node")
                .arg(self.fixture.join("check_typescript.cjs"))
                .arg(&self.generated)
                .arg(modules),
            "typescript-behavior",
        );
    }
}

#[test]
fn generated_bindings_preserve_types_and_service_behavior() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "idl-bindings");
    let acceptance = Acceptance::new(&case);
    acceptance.generate();
    acceptance.prepare_scripts();
    acceptance.rust(
        "behavior::generated_bindings_execute_through_real_sdk",
        "rust-behavior",
    );
    acceptance.scripts();
    acceptance.rust(
        "behavior::script_results_preserve_wit_values_in_rust",
        "rust-script-results",
    );
    eprintln!(
        "IDL 验收证据：{}",
        case.evidence_for(&acceptance.directory).display()
    );
    case.success();
}
