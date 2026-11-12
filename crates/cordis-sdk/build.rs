use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let compiler = std::env::var_os("RUSTC").expect("Cargo 提供 RUSTC");
    let output = Command::new(compiler)
        .arg("--version")
        .output()
        .expect("读取 Rust 编译器版本");
    assert!(output.status.success(), "Rust 编译器版本命令失败");
    let version = String::from_utf8(output.stdout).expect("Rust 编译器版本为 UTF-8");
    println!("cargo:rustc-env=CORDIS_RUSTC={}", version.trim());
    println!(
        "cargo:rustc-env=CORDIS_TARGET={}",
        std::env::var("TARGET").expect("Cargo 提供 TARGET")
    );
}
