#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(configuration) = cordis_runtime::config::parse(bytes) {
        let _ = configuration.effective(&cordis_runtime::config::Overlay::default());
    }
    let _ = cordis_runtime::config::parse_overlay(bytes);
});
