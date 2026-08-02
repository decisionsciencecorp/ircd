//! IRCv3 message-tag helpers (parse strip, outbound adapt, server-time).

use std::collections::HashSet;

/// Split a wire line into optional tag payload and the IRC message body.
///
/// `@tags-only` (no space) yields `(Some(tags), "")` rather than treating `@` as
/// a message body prefix.
///
/// ```
/// use ircd_core::tags::split_tags;
/// let (tags, body) = split_tags("@msgid=1 :n PRIVMSG #c :hi");
/// assert_eq!(tags, Some("msgid=1"));
/// assert!(body.starts_with(':'));
/// ```
pub fn split_tags(line: &str) -> (Option<&str>, &str) {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(rest) = line.strip_prefix('@') {
        if let Some((tags, body)) = rest.split_once(' ') {
            return (Some(tags), body);
        }
        return (Some(rest), "");
    }
    (None, line)
}

/// Ensure a line ends with CRLF.
pub fn ensure_crlf(line: &str) -> String {
    let line = line.trim_end_matches(['\r', '\n']);
    format!("{line}\r\n")
}

/// Prepend `key=value` onto an IRC line (creating or extending the tag block).
pub fn prepend_tag(line: &str, key: &str, value: &str) -> String {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(rest) = line.strip_prefix('@') {
        format!("@{key}={value};{rest}\r\n")
    } else {
        format!("@{key}={value} {line}\r\n")
    }
}

fn has_cap(caps: &HashSet<String>, name: &str) -> bool {
    caps.iter()
        .any(|c| c.split('=').next().unwrap_or(c).eq_ignore_ascii_case(name))
}

/// Adapt a (possibly tagged) bus line to the client's negotiated caps.
///
/// ```
/// use std::collections::HashSet;
/// use ircd_core::tags::adapt_bus_line;
/// let mut caps = HashSet::new();
/// let out = adapt_bus_line("@msgid=1 :a!b@c PRIVMSG #x :hi\r\n", &caps);
/// assert_eq!(out, ":a!b@c PRIVMSG #x :hi\r\n");
/// ```
pub fn adapt_bus_line(line: &str, caps: &HashSet<String>) -> String {
    let (tags, rest) = split_tags(line);
    let Some(tags) = tags else {
        if has_cap(caps, "server-time") {
            return tag_server_time(line);
        }
        return ensure_crlf(line);
    };
    let want_msg = has_cap(caps, "message-tags");
    let want_time = has_cap(caps, "server-time");
    let want_account = has_cap(caps, "account-tag");
    if !want_msg && !want_time && !want_account {
        return ensure_crlf(rest);
    }
    let mut keep = Vec::new();
    for part in tags.split(';') {
        if part.is_empty() {
            continue;
        }
        if part.starts_with("msgid=") && want_msg {
            keep.push(part);
        } else if part.starts_with("time=") && (want_time || want_msg) {
            keep.push(part);
        } else if part.starts_with("account=") && (want_account || want_msg) {
            keep.push(part);
        } else if want_msg
            && !part.starts_with("msgid=")
            && !part.starts_with("time=")
            && !part.starts_with("account=")
        {
            keep.push(part);
        }
    }
    if keep.is_empty() {
        ensure_crlf(rest)
    } else {
        format!("@{} {}\r\n", keep.join(";"), rest)
    }
}

/// Stamp `time=` on a line. Does not duplicate when `time=` is already present.
pub fn tag_server_time(line: &str) -> String {
    let stamp = unix_ms_to_rfc3339(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    );
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(rest) = line.strip_prefix('@') {
        let tag_part = rest.split_once(' ').map(|(t, _)| t).unwrap_or(rest);
        if tag_part.split(';').any(|p| p.starts_with("time=")) {
            return ensure_crlf(line);
        }
        prepend_time_stamp(rest, &stamp)
    } else {
        format!("@time={stamp} {line}\r\n")
    }
}

/// `@time=<stamp>;<existing-tag-body>`
pub fn prepend_time_stamp(rest_after_at: &str, stamp: &str) -> String {
    format!("@time={stamp};{rest_after_at}\r\n")
}

/// Convert unix milliseconds to an RFC3339 UTC stamp with millisecond precision.
///
/// ```
/// use ircd_core::tags::unix_ms_to_rfc3339;
/// let s = unix_ms_to_rfc3339(0);
/// assert_eq!(s, "1970-01-01T00:00:00.000Z");
/// ```
pub fn unix_ms_to_rfc3339(ts_ms: i64) -> String {
    let secs = (ts_ms / 1000) as u64;
    let millis = (ts_ms.rem_euclid(1000)) as u32;
    let days = secs / 86400;
    let tod = secs % 86400;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}.{millis:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_tags_only_no_space() {
        let (tags, body) = split_tags("@msgid=x");
        assert_eq!(tags, Some("msgid=x"));
        assert_eq!(body, "");
    }

    #[test]
    fn adapt_strips_when_no_caps() {
        let caps = HashSet::new();
        let out = adapt_bus_line(
            "@msgid=dsc1;time=2026-01-01T00:00:00.000Z :a!b@c PRIVMSG #x :hi\r\n",
            &caps,
        );
        assert_eq!(out, ":a!b@c PRIVMSG #x :hi\r\n");
    }

    #[test]
    fn adapt_keeps_msgid_and_account() {
        let mut caps = HashSet::new();
        caps.insert("message-tags".into());
        caps.insert("account-tag".into());
        let out = adapt_bus_line(
            "@msgid=dsc1;account=alice;time=2026-01-01T00:00:00.000Z :a!b@c PRIVMSG #x :hi\r\n",
            &caps,
        );
        assert!(out.contains("msgid=dsc1"));
        assert!(out.contains("account=alice"));
        assert!(out.contains("time="));
    }

    #[test]
    fn adapt_server_time_only() {
        let mut caps = HashSet::new();
        caps.insert("server-time".into());
        let out = adapt_bus_line(":a!b@c PRIVMSG #x :hi\r\n", &caps);
        assert!(out.starts_with("@time="));
        assert!(out.contains("PRIVMSG #x"));
    }

    #[test]
    fn prepend_extends_existing() {
        let out = prepend_tag("@msgid=1 :x PRIVMSG #c :h", "account", "a");
        assert!(out.starts_with("@account=a;msgid=1 "));
    }

    #[test]
    fn tag_server_time_no_duplicate() {
        let line = "@time=2026-01-01T00:00:00.000Z :x PRIVMSG #c :hi";
        let out = tag_server_time(line);
        assert_eq!(out.matches("time=").count(), 1);
    }

    #[test]
    fn ensure_crlf_idempotent() {
        assert_eq!(ensure_crlf("PING"), "PING\r\n");
        assert_eq!(ensure_crlf("PING\r\n"), "PING\r\n");
    }

    #[test]
    fn adapt_keeps_only_time_with_server_time_cap() {
        let mut caps = HashSet::new();
        caps.insert("server-time".into());
        let out = adapt_bus_line(
            "@msgid=1;time=2026-01-01T00:00:00.000Z;account=a :x PRIVMSG #c :h\r\n",
            &caps,
        );
        assert!(out.contains("time="));
        assert!(!out.contains("msgid="));
        assert!(!out.contains("account="));
    }

    #[test]
    fn prepend_on_untagged() {
        let out = prepend_tag(":x PRIVMSG #c :h", "account", "a");
        assert_eq!(out, "@account=a :x PRIVMSG #c :h\r\n");
    }

    #[test]
    fn tag_server_time_on_untagged() {
        let out = tag_server_time("PRIVMSG #c :hi");
        assert!(out.starts_with("@time="));
        assert!(out.contains("PRIVMSG #c :hi"));
    }

    #[test]
    fn adapt_keeps_unknown_tags_with_message_tags() {
        let mut caps = HashSet::new();
        caps.insert("message-tags".into());
        let out = adapt_bus_line("@foo=bar;msgid=1 :x PRIVMSG #c :h\r\n", &caps);
        assert!(out.contains("foo=bar"));
        assert!(out.contains("msgid=1"));
    }

    #[test]
    fn adapt_empty_tag_parts() {
        let mut caps = HashSet::new();
        caps.insert("message-tags".into());
        let out = adapt_bus_line("@; :x PRIVMSG #c :h\r\n", &caps);
        assert_eq!(out, ":x PRIVMSG #c :h\r\n");
    }

    #[test]
    fn unix_ms_negative_millis_component() {
        // rem_euclid keeps millisecond field non-negative
        let s = unix_ms_to_rfc3339(-1);
        assert!(s.ends_with("Z"));
    }

    #[test]
    fn tag_server_time_extends_existing_tag_block() {
        let out = tag_server_time("@msgid=1 :x PRIVMSG #c :hi");
        assert!(out.starts_with("@time="));
        assert!(out.contains("msgid=1"));
        assert!(out.contains("PRIVMSG"));
    }

    #[test]
    fn adapt_drops_msgid_when_only_server_time() {
        let mut caps = HashSet::new();
        caps.insert("server-time".into());
        let out = adapt_bus_line("@msgid=1 :x PRIVMSG #c :h\r\n", &caps);
        assert_eq!(out, ":x PRIVMSG #c :h\r\n");
    }

    #[test]
    fn prepend_time_stamp_helper() {
        let out = prepend_time_stamp("msgid=1 :x PRIVMSG #c :hi", "2026-01-01T00:00:00.000Z");
        assert_eq!(
            out,
            "@time=2026-01-01T00:00:00.000Z;msgid=1 :x PRIVMSG #c :hi\r\n"
        );
    }
}
