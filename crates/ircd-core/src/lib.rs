//! Shared types and IRC line helpers for the DSC IRCd.
//!
//! Parsing, casemapping, and the typed [`Command`] enum live here. Session
//! behavior lives in the `ircd` crate. This is a clean-room implementation:
//! UnrealIRCd and Ergo are references for behavior, not source.
//!
//! # Examples
//!
//! Parse a client PRIVMSG:
//!
//! ```
//! use ircd_core::RawLine;
//! let line = RawLine::parse("PRIVMSG #lab :hello world").unwrap();
//! assert!(line.command_eq("PRIVMSG"));
//! assert_eq!(line.params, vec!["#lab", "hello world"]);
//! ```
//!
//! Tagged lines strip `@tags` before the command:
//!
//! ```
//! use ircd_core::RawLine;
//! let line = RawLine::parse("@msgid=dsc1 :nick!u@h PRIVMSG #c :hi").unwrap();
//! assert!(line.command_eq("PRIVMSG"));
//! assert_eq!(line.tags.as_deref(), Some("msgid=dsc1"));
//! ```

#![forbid(unsafe_code)]

pub mod casemap;
pub mod command;
pub mod tags;

use std::fmt;

use tags::split_tags;

pub use casemap::{ascii_casefold, ChannelKey, ChannelName, Nick, NickKey};
pub use command::Command;

/// True if `s` contains ASCII control characters (including NUL/CR/LF) that must
/// never appear in nick, user, channel, or topic fields on the wire.
pub fn field_has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// Channel name shape used by dsc-ircd: leading `#`, length, no controls/spaces/commas.
pub fn valid_channel_name(s: &str, max_len: usize) -> bool {
    !s.is_empty()
        && s.len() <= max_len
        && s.starts_with('#')
        && !field_has_control(s)
        && !s.contains(' ')
        && !s.contains(',')
}

/// IRC message line without trailing CR/LF.
///
/// Optional IRCv3 client/server tags are captured in [`RawLine::tags`] and are
/// not part of [`RawLine::command`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLine {
    /// Raw tag string (`k=v;k2=v2`) without the leading `@`, when present.
    pub tags: Option<String>,
    pub prefix: Option<String>,
    pub command: String,
    pub params: Vec<String>,
}

impl RawLine {
    /// Parse one IRC line. Never panics; returns `None` for empty/malformed input
    /// that cannot yield a command.
    ///
    /// ```
    /// use ircd_core::RawLine;
    /// assert!(RawLine::parse("").is_none());
    /// assert!(RawLine::parse("PING :token").unwrap().command_eq("PING"));
    /// ```
    pub fn parse(input: &str) -> Option<Self> {
        let (tag_str, body) = split_tags(input);
        if body.is_empty() {
            return None;
        }

        let mut rest = body;
        let prefix = if let Some(stripped) = rest.strip_prefix(':') {
            let (p, after) = stripped.split_once(' ')?;
            rest = after;
            Some(p.to_string())
        } else {
            None
        };

        let mut params = Vec::new();
        let command;
        if let Some((cmd, after)) = rest.split_once(' ') {
            command = cmd.to_string();
            rest = after;
        } else {
            return Some(Self {
                tags: tag_str.map(str::to_string),
                prefix,
                command: rest.to_string(),
                params,
            });
        }

        while !rest.is_empty() {
            if let Some(trailing) = rest.strip_prefix(':') {
                params.push(trailing.to_string());
                break;
            }
            if let Some((p, after)) = rest.split_once(' ') {
                params.push(p.to_string());
                rest = after.trim_start();
            } else {
                params.push(rest.to_string());
                break;
            }
        }

        Some(Self {
            tags: tag_str.map(str::to_string),
            prefix,
            command,
            params,
        })
    }

    pub fn command_eq(&self, other: &str) -> bool {
        self.command.eq_ignore_ascii_case(other)
    }
}

impl fmt::Display for RawLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(tags) = &self.tags {
            write!(f, "@{tags} ")?;
        }
        if let Some(p) = &self.prefix {
            write!(f, ":{p} ")?;
        }
        write!(f, "{}", self.command)?;
        let n = self.params.len();
        for (i, p) in self.params.iter().enumerate() {
            if i + 1 == n && (p.contains(' ') || p.is_empty() || p.starts_with(':')) {
                write!(f, " :{p}")?;
            } else {
                write!(f, " {p}")?;
            }
        }
        Ok(())
    }
}

/// Format a numeric reply: `:server ### nick ...`
///
/// ```
/// use ircd_core::numeric;
/// let line = numeric("irc.test", 1, "alice", &["Welcome"]);
/// assert!(line.starts_with(":irc.test 001 alice :Welcome"));
/// assert!(line.ends_with("\r\n"));
/// ```
pub fn numeric(server: &str, code: u16, nick: &str, params: &[&str]) -> String {
    let mut out = format!(":{server} {code:03} {nick}");
    for (i, p) in params.iter().enumerate() {
        if i + 1 == params.len() {
            out.push_str(" :");
            out.push_str(p);
        } else {
            out.push(' ');
            out.push_str(p);
        }
    }
    out.push_str("\r\n");
    out
}

/// Server NOTICE to `*`.
///
/// ```
/// use ircd_core::server_notice;
/// assert_eq!(
///     server_notice("irc.test", "hi"),
///     ":irc.test NOTICE * :hi\r\n"
/// );
/// ```
pub fn server_notice(server: &str, text: &str) -> String {
    format!(":{server} NOTICE * :{text}\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_privmsg() {
        let line = RawLine::parse("PRIVMSG #test :hello world").unwrap();
        assert!(line.command_eq("PRIVMSG"));
        assert_eq!(line.params, vec!["#test", "hello world"]);
        assert!(line.tags.is_none());
    }

    #[test]
    fn parse_with_prefix() {
        let line = RawLine::parse(":nick!u@h JOIN #test").unwrap();
        assert_eq!(line.prefix.as_deref(), Some("nick!u@h"));
        assert!(line.command_eq("JOIN"));
    }

    #[test]
    fn parse_strips_message_tags() {
        let line = RawLine::parse("@account=alice;msgid=dsc1 :n!u@h PRIVMSG #c :hi").unwrap();
        assert!(line.command_eq("PRIVMSG"));
        assert_eq!(line.tags.as_deref(), Some("account=alice;msgid=dsc1"));
        assert_eq!(line.params, vec!["#c", "hi"]);
    }

    #[test]
    fn display_roundtrip_simple() {
        let raw = "PRIVMSG #c :hello there";
        let line = RawLine::parse(raw).unwrap();
        let again = RawLine::parse(&line.to_string()).unwrap();
        assert_eq!(line, again);
    }

    #[test]
    fn empty_trailing() {
        let line = RawLine::parse("PRIVMSG #c :").unwrap();
        assert_eq!(line.params, vec!["#c", ""]);
    }

    #[test]
    fn display_with_tags_and_prefix() {
        let line = RawLine {
            tags: Some("msgid=1".into()),
            prefix: Some("n!u@h".into()),
            command: "PRIVMSG".into(),
            params: vec!["#c".into(), "hi there".into()],
        };
        assert_eq!(line.to_string(), "@msgid=1 :n!u@h PRIVMSG #c :hi there");
    }

    #[test]
    fn numeric_and_notice_helpers() {
        let n = numeric("s", 1, "n", &["a", "b c"]);
        assert_eq!(n, ":s 001 n a :b c\r\n");
        assert_eq!(server_notice("s", "x"), ":s NOTICE * :x\r\n");
    }
}

#[cfg(test)]
mod field_validation_tests {
    use super::{field_has_control, valid_channel_name};

    #[test]
    fn field_has_control_rejects_nul_and_crlf() {
        assert!(field_has_control("a\0b"));
        assert!(field_has_control("a\rb"));
        assert!(field_has_control("a\nb"));
        assert!(!field_has_control("alice"));
    }

    #[test]
    fn valid_channel_name_rejects_controls_and_spaces() {
        assert!(valid_channel_name("#lab", 64));
        assert!(!valid_channel_name("lab", 64));
        assert!(!valid_channel_name("#bad name", 64));
        assert!(!valid_channel_name("#a\nb", 64));
    }
}
