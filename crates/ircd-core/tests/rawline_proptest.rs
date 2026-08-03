//! Property tests for RawLine parse/display — edge cases hand tests miss.

use ircd_core::RawLine;
use proptest::prelude::*;

fn ascii_token() -> impl Strategy<Value = String> {
    "[A-Za-z][A-Za-z0-9_-]{0,11}".prop_map(|s| s)
}

fn trailing_text() -> impl Strategy<Value = String> {
    // Printable ASCII without CR/LF; may include spaces.
    "[ -~]{0,40}".prop_filter("no lone colon-start middle issues", |s| !s.contains('\r'))
}

fn tag_atom() -> impl Strategy<Value = String> {
    (
        "[A-Za-z][A-Za-z0-9_-]{0,8}",
        prop::option::of("[A-Za-z0-9_-]{1,8}"),
    )
        .prop_map(|(k, v)| match v {
            Some(val) => format!("{k}={val}"),
            None => k,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Valid constructed lines round-trip through Display → parse.
    #[test]
    fn roundtrip_constructed(
        cmd in ascii_token(),
        prefix in prop::option::of(ascii_token()),
        middles in prop::collection::vec(ascii_token(), 0..4),
        trailing in trailing_text(),
        tags in prop::option::of(
            prop::collection::vec(tag_atom(), 1..4).prop_map(|v| v.join(";"))
        ),
    ) {
        let mut params = middles;
        params.push(trailing);
        let line = RawLine {
            tags,
            prefix,
            command: cmd,
            params,
        };
        let rendered = line.to_string();
        let parsed = RawLine::parse(&rendered).expect("parse rendered");
        prop_assert_eq!(line, parsed);
    }

    /// Arbitrary UTF-8 must never panic.
    #[test]
    fn parse_never_panics(s in "\\PC*") {
        let _ = RawLine::parse(&s);
    }

    /// Binary-ish input via Vec<u8> as lossy string never panics.
    #[test]
    fn parse_bytes_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
        let s = String::from_utf8_lossy(&bytes);
        let _ = RawLine::parse(&s);
    }
}
