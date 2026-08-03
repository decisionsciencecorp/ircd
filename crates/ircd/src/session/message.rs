//! Message-lane helpers (PRIVMSG/NOTICE/TAGMSG client-tag relay) (C7 / C14).

use std::collections::HashSet;

use ircd_core::tags::{client_only_tag_payload, escape_client_tag_block, prepend_tag_block};
use ircd_core::RawLine;

use super::cap::has_cap;

pub(super) fn relay_client_tags(line: String, msg: &RawLine, enabled_caps: &HashSet<String>) -> String {
    if !has_cap(enabled_caps, "message-tags") {
        return line;
    }
    match client_only_tag_payload(msg.tags.as_deref()) {
        Some(block) => prepend_tag_block(&line, &escape_client_tag_block(&block)),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ircd_core::RawLine;

    #[test]
    fn relay_passthrough_without_message_tags() {
        let caps = HashSet::new();
        let msg = RawLine::parse("@+foo=bar PRIVMSG #c :hi").unwrap();
        let out = relay_client_tags("LINE\r\n".into(), &msg, &caps);
        assert_eq!(out, "LINE\r\n");
    }

    #[test]
    fn relay_none_when_no_client_tags() {
        let mut caps = HashSet::new();
        caps.insert("message-tags".into());
        let msg = RawLine::parse("PRIVMSG #c :hi").unwrap();
        let out = relay_client_tags(":a PRIVMSG #c :hi\r\n".into(), &msg, &caps);
        assert_eq!(out, ":a PRIVMSG #c :hi\r\n");
    }

    #[test]
    fn relay_prepends_client_plus_tag() {
        let mut caps = HashSet::new();
        caps.insert("message-tags".into());
        let msg = RawLine::parse("@+foo=bar PRIVMSG #c :hi").unwrap();
        let out = relay_client_tags(":a!u@h PRIVMSG #c :hi\r\n".into(), &msg, &caps);
        assert!(out.contains("+foo=bar"), "{out}");
        assert!(out.contains("PRIVMSG"), "{out}");
    }
}
