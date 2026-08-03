//! WebSocket framing policy helpers (normalize IRC lines from WS text).

/// Split a WebSocket text/binary payload into IRC lines, accepting `\r\n`, `\n`,
/// or lone `\r` as line endings. Empty segments are skipped (C13 / Q-07).
pub fn normalize_ws_irc_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if start < i {
                    out.push(&text[start..i]);
                }
                i += 1;
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                start = i;
            }
            b'\n' => {
                if start < i {
                    out.push(&text[start..i]);
                }
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        let tail = &text[start..];
        if !tail.is_empty() {
            out.push(tail);
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lf_crlf_and_lone_cr() {
        assert_eq!(
            normalize_ws_irc_lines("PING :a\nPING :b\r\nPING :c\rPING :d"),
            vec!["PING :a", "PING :b", "PING :c", "PING :d"]
        );
    }

    #[test]
    fn normalize_skips_empty() {
        assert_eq!(normalize_ws_irc_lines("\n\r\n\r"), Vec::<&str>::new());
        assert_eq!(normalize_ws_irc_lines("A\n\nB"), vec!["A", "B"]);
    }

    #[test]
    fn normalize_preserves_long_line_body_for_cap_check() {
        // Cap enforcement stays in ws.rs (8192 / 64 KiB); normalizer must not truncate.
        let long = "X".repeat(9000);
        let payload = format!("{long}\r\nPING :ok\n");
        let out = normalize_ws_irc_lines(&payload);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 9000);
        assert_eq!(out[1], "PING :ok");
    }
}
