#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = cordis_runtime::storage::parse_state(bytes);
});
