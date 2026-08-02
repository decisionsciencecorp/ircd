//! Shared types and small IRC line helpers for the DSC IRCd.
//!
//! Protocol work stays intentionally thin at bootstrap — grow toward Unreal-class
//! features without importing Unreal source.

#![forbid(unsafe_code)]

use std::fmt;

/// IRC message line without trailing CR/LF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLine {
    pub prefix: Option<String>,
    pub command: String,
    pub params: Vec<String>,
}

impl RawLine {
    pub fn parse(input: &str) -> Option<Self> {
        let line = input.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return None;
        }

        let mut rest = line;
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
        if let Some(p) = &self.prefix {
            write!(f, ":{p} ")?;
        }
        write!(f, "{}", self.command)?;
        let n = self.params.len();
        for (i, p) in self.params.iter().enumerate() {
            if i + 1 == n && (p.contains(' ') || p.is_empty()) {
                write!(f, " :{p}")?;
            } else {
                write!(f, " {p}")?;
            }
        }
        Ok(())
    }
}

/// Format a numeric reply: `:server ### nick ...`
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
    }

    #[test]
    fn parse_with_prefix() {
        let line = RawLine::parse(":nick!u@h JOIN #test").unwrap();
        assert_eq!(line.prefix.as_deref(), Some("nick!u@h"));
        assert!(line.command_eq("JOIN"));
    }
}
