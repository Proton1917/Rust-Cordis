//! ADR-0053 配套发布验收：真实 C 依赖的诊断位置参与最终动态库。

use cordis_sdk::prelude::*;

// SAFETY: C 函数由本 crate 的 build.rs 编译，返回静态零结尾字符串。
unsafe extern "C" {
    fn native_build_location() -> *const std::ffi::c_char;
}

struct Diagnostic;
impl Component for Diagnostic {
    const NAME: &'static str = "native-diagnostic";
    type Config = EmptyConfig;

    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        // SAFETY: C 诊断字符串为进程寿命内有效的静态数据。
        let location = unsafe { std::ffi::CStr::from_ptr(native_build_location()) };
        context.log(2, location.to_str().map_err(|error| Error::new(1, error.to_string()))?)
    }
}

cordis_sdk::export_plugin!(name = "native-diagnostic", version = "0.1.0", components = [Diagnostic]);
