//! ASCII casemapping keys and validated nick/channel identifiers (H-10 / A8).

use std::fmt;

/// Fold a string with CASEMAPPING=ascii (Unicode-aware lowercase on ASCII letters only).
pub fn ascii_casefold(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// Casemapped nick key for maps / collision checks.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NickKey(String);

impl NickKey {
    pub fn from_raw(raw: &str) -> Self {
        Self(ascii_casefold(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NickKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Casemapped channel key (`#` preserved, rest ascii-folded).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChannelKey(String);

impl ChannelKey {
    pub fn from_raw(raw: &str) -> Self {
        Self(ascii_casefold(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated display nick (length + charset; no controls).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nick(String);

impl Nick {
    pub fn parse(raw: &str, max_len: usize) -> Result<Self, &'static str> {
        if raw.is_empty() {
            return Err("empty");
        }
        if raw.len() > max_len {
            return Err("too long");
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err("charset");
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn key(&self) -> NickKey {
        NickKey::from_raw(&self.0)
    }
}

/// Validated channel name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelName(String);

impl ChannelName {
    pub fn parse(raw: &str, max_len: usize) -> Result<Self, &'static str> {
        if !crate::valid_channel_name(raw, max_len) {
            return Err("invalid");
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn key(&self) -> ChannelKey {
        ChannelKey::from_raw(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alice_alice_collide() {
        assert_eq!(NickKey::from_raw("Alice"), NickKey::from_raw("alice"));
        assert_eq!(
            Nick::parse("Alice", 30).unwrap().key(),
            Nick::parse("alice", 30).unwrap().key()
        );
    }

    #[test]
    fn nick_rejects_bad() {
        assert!(Nick::parse("", 30).is_err());
        assert!(Nick::parse("a b", 30).is_err());
        assert!(Nick::parse("bad\n", 30).is_err());
    }

    #[test]
    fn channel_key_folds() {
        assert_eq!(ChannelKey::from_raw("#Lab"), ChannelKey::from_raw("#lab"));
    }

    #[test]
    fn channel_name_parse() {
        assert!(ChannelName::parse("#ok", 50).is_ok());
        assert!(ChannelName::parse("bad", 50).is_err());
    }

    #[test]
    fn display_and_keys() {
        let nk = NickKey::from_raw("Alice");
        assert_eq!(nk.as_str(), "alice");
        assert_eq!(format!("{nk}"), "alice");
        let ck = ChannelKey::from_raw("#Lab");
        assert_eq!(ck.as_str(), "#lab");
        let n = Nick::parse("bob_1", 30).unwrap();
        assert_eq!(n.as_str(), "bob_1");
        assert_eq!(n.key().as_str(), "bob_1");
        assert!(Nick::parse("x", 0).is_err()); // too long / empty path
        assert!(Nick::parse("bad!", 30).is_err());
        let ch = ChannelName::parse("#ok", 50).unwrap();
        assert_eq!(ch.as_str(), "#ok");
        assert_eq!(ch.key().as_str(), "#ok");
        assert!(ChannelName::parse("nohash", 50).is_err());
    }
}
