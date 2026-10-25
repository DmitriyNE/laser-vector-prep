#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| lvp_core::fuzz::run("gds", data));
