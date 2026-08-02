#![no_main]
use std::collections::HashSet;

use libfuzzer_sys::fuzz_target;
use ircd_core::tags::adapt_bus_line;

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    let mut caps = HashSet::new();
    if data.first().copied().unwrap_or(0) & 1 != 0 {
        caps.insert("message-tags".into());
    }
    if data.first().copied().unwrap_or(0) & 2 != 0 {
        caps.insert("server-time".into());
    }
    if data.first().copied().unwrap_or(0) & 4 != 0 {
        caps.insert("account-tag".into());
    }
    let _ = adapt_bus_line(&s, &caps);
});
