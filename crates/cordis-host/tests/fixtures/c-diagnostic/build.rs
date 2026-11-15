fn main() {
    let source = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("native/diagnostic.c");
    cc::Build::new().file(source).compile("native-diagnostic");
    println!("cargo:rerun-if-changed=native/diagnostic.c");
}
