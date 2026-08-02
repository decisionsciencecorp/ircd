#![no_main]
use libfuzzer_sys::fuzz_target;
use ircd_core::RawLine;

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    let _ = RawLine::parse(&s);
});
