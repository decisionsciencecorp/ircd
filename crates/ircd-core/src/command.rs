//! Typed IRC commands — classify a [`RawLine`] into a validated [`Command`] (H-14).

use crate::RawLine;

/// Known dsc-ircd verbs plus an escape hatch for unknowns (421).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Cap {
        params: Vec<String>,
    },
    Authenticate {
        params: Vec<String>,
    },
    Nick {
        nick: Option<String>,
    },
    User {
        params: Vec<String>,
    },
    Ping {
        token: Option<String>,
    },
    Quit {
        reason: Option<String>,
    },
    Admin,
    Oper {
        name: Option<String>,
        pass: Option<String>,
    },
    Join {
        channels: Option<String>,
        keys: Option<String>,
    },
    Part {
        channels: Option<String>,
        reason: Option<String>,
    },
    Privmsg {
        target: Option<String>,
        text: Option<String>,
    },
    Notice {
        target: Option<String>,
        text: Option<String>,
    },
    /// IRCv3 TAGMSG — requires `message-tags` (C7).
    Tagmsg {
        target: Option<String>,
        text: Option<String>,
    },
    Topic {
        channel: Option<String>,
        topic: Option<String>,
    },
    Kick {
        channel: Option<String>,
        nick: Option<String>,
        reason: Option<String>,
    },
    Mode {
        target: Option<String>,
        args: Vec<String>,
    },
    Invite {
        nick: Option<String>,
        channel: Option<String>,
    },
    Names {
        channel: Option<String>,
    },
    List {
        channel: Option<String>,
    },
    Who {
        mask: Option<String>,
    },
    Whois {
        nick: Option<String>,
    },
    Motd,
    Version,
    Lusers,
    /// Set or clear away status (Modern IRC + IRCv3 `away-notify`).
    Away {
        /// `None` = clear away; `Some` = set (message may be empty).
        message: Option<String>,
    },
    /// Connection password (pre-register when `server.password` is set).
    Pass {
        password: Option<String>,
    },
    Userhost {
        nicks: Vec<String>,
    },
    Ison {
        nicks: Vec<String>,
    },
    Time,
    Info,
    Kill {
        nick: Option<String>,
        reason: Option<String>,
    },
    Wallops {
        text: Option<String>,
    },
    Chathistory {
        params: Vec<String>,
    },
    /// Unrecognized verb — surface as 421 with this name.
    Unknown {
        verb: String,
    },
}

impl Command {
    /// Classify `line` by verb (ASCII case-insensitive). Params are not fully
    /// validated here — handlers still enforce arity / shape for numerics.
    pub fn from_raw(line: &RawLine) -> Self {
        let p = &line.params;
        let at = |i: usize| p.get(i).cloned();
        match line.command.to_ascii_uppercase().as_str() {
            "CAP" => Self::Cap { params: p.clone() },
            "AUTHENTICATE" => Self::Authenticate { params: p.clone() },
            "NICK" => Self::Nick { nick: at(0) },
            "USER" => Self::User { params: p.clone() },
            "PING" => Self::Ping { token: at(0) },
            "QUIT" => Self::Quit { reason: at(0) },
            "ADMIN" => Self::Admin,
            "OPER" => Self::Oper {
                name: at(0),
                pass: at(1),
            },
            "JOIN" => Self::Join {
                channels: at(0),
                keys: at(1),
            },
            "PART" => Self::Part {
                channels: at(0),
                reason: at(1),
            },
            "PRIVMSG" => Self::Privmsg {
                target: at(0),
                text: at(1),
            },
            "NOTICE" => Self::Notice {
                target: at(0),
                text: at(1),
            },
            "TAGMSG" => Self::Tagmsg {
                target: at(0),
                text: at(1),
            },
            "TOPIC" => Self::Topic {
                channel: at(0),
                topic: at(1),
            },
            "KICK" => Self::Kick {
                channel: at(0),
                nick: at(1),
                reason: at(2),
            },
            "MODE" => {
                let target = at(0);
                let args = if p.len() > 1 {
                    p[1..].to_vec()
                } else {
                    Vec::new()
                };
                Self::Mode { target, args }
            }
            "INVITE" => Self::Invite {
                nick: at(0),
                channel: at(1),
            },
            "NAMES" => Self::Names { channel: at(0) },
            "LIST" => Self::List { channel: at(0) },
            "WHO" => Self::Who { mask: at(0) },
            "WHOIS" => Self::Whois { nick: at(0) },
            "MOTD" => Self::Motd,
            "VERSION" => Self::Version,
            "LUSERS" => Self::Lusers,
            "AWAY" => Self::Away { message: at(0) },
            "PASS" => Self::Pass { password: at(0) },
            "USERHOST" => Self::Userhost { nicks: p.clone() },
            "ISON" => Self::Ison { nicks: p.clone() },
            "TIME" => Self::Time,
            "INFO" => Self::Info,
            "KILL" => Self::Kill {
                nick: at(0),
                reason: at(1),
            },
            "WALLOPS" => Self::Wallops { text: at(0) },
            "CHATHISTORY" => Self::Chathistory { params: p.clone() },
            _ => Self::Unknown {
                verb: line.command.clone(),
            },
        }
    }

    /// Wire verb name for 421 / logging.
    pub fn verb(&self) -> &str {
        match self {
            Self::Cap { .. } => "CAP",
            Self::Authenticate { .. } => "AUTHENTICATE",
            Self::Nick { .. } => "NICK",
            Self::User { .. } => "USER",
            Self::Ping { .. } => "PING",
            Self::Quit { .. } => "QUIT",
            Self::Admin => "ADMIN",
            Self::Oper { .. } => "OPER",
            Self::Join { .. } => "JOIN",
            Self::Part { .. } => "PART",
            Self::Privmsg { .. } => "PRIVMSG",
            Self::Notice { .. } => "NOTICE",
            Self::Tagmsg { .. } => "TAGMSG",
            Self::Topic { .. } => "TOPIC",
            Self::Kick { .. } => "KICK",
            Self::Mode { .. } => "MODE",
            Self::Invite { .. } => "INVITE",
            Self::Names { .. } => "NAMES",
            Self::List { .. } => "LIST",
            Self::Who { .. } => "WHO",
            Self::Whois { .. } => "WHOIS",
            Self::Motd => "MOTD",
            Self::Version => "VERSION",
            Self::Lusers => "LUSERS",
            Self::Away { .. } => "AWAY",
            Self::Pass { .. } => "PASS",
            Self::Userhost { .. } => "USERHOST",
            Self::Ison { .. } => "ISON",
            Self::Time => "TIME",
            Self::Info => "INFO",
            Self::Kill { .. } => "KILL",
            Self::Wallops { .. } => "WALLOPS",
            Self::Chathistory { .. } => "CHATHISTORY",
            Self::Unknown { verb } => verb.as_str(),
        }
    }

    /// Whether this command may run before NICK/USER registration completes.
    pub fn allowed_pre_register(&self) -> bool {
        matches!(
            self,
            Self::Cap { .. }
                | Self::Authenticate { .. }
                | Self::Nick { .. }
                | Self::User { .. }
                | Self::Pass { .. }
                | Self::Ping { .. }
                | Self::Quit { .. }
        )
    }

    /// Coarse handler lane for logging / metrics (H-14 structure).
    pub fn lane(&self) -> &'static str {
        match self {
            Self::Cap { .. } | Self::Authenticate { .. } => "cap",
            Self::Nick { .. } | Self::User { .. } | Self::Pass { .. } => "registration",
            Self::Ping { .. }
            | Self::Quit { .. }
            | Self::Admin
            | Self::Motd
            | Self::Version
            | Self::Lusers
            | Self::Away { .. }
            | Self::Time
            | Self::Info => "session",
            Self::Oper { .. } | Self::Kill { .. } | Self::Wallops { .. } => "oper",
            Self::Join { .. }
            | Self::Part { .. }
            | Self::Topic { .. }
            | Self::Kick { .. }
            | Self::Mode { .. }
            | Self::Invite { .. }
            | Self::Names { .. }
            | Self::List { .. } => "channel",
            Self::Privmsg { .. } | Self::Notice { .. } | Self::Tagmsg { .. } => "message",
            Self::Who { .. } | Self::Whois { .. } | Self::Userhost { .. } | Self::Ison { .. } => {
                "query"
            }
            Self::Chathistory { .. } => "history",
            Self::Unknown { .. } => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_privmsg() {
        let line = RawLine::parse("PRIVMSG #c :hi there").unwrap();
        match Command::from_raw(&line) {
            Command::Privmsg {
                target: Some(t),
                text: Some(x),
            } => {
                assert_eq!(t, "#c");
                assert_eq!(x, "hi there");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn classifies_mode_args() {
        let line = RawLine::parse("MODE #c +b nick!*@*").unwrap();
        match Command::from_raw(&line) {
            Command::Mode {
                target: Some(t),
                args,
            } => {
                assert_eq!(t, "#c");
                assert_eq!(args, vec!["+b", "nick!*@*"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_preserves_verb() {
        let line = RawLine::parse("FROBNICATE a b").unwrap();
        match Command::from_raw(&line) {
            Command::Unknown { verb } => assert_eq!(verb, "FROBNICATE"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn case_insensitive_verb() {
        let line = RawLine::parse("join #lab").unwrap();
        assert!(matches!(
            Command::from_raw(&line),
            Command::Join {
                channels: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn verb_covers_every_variant() {
        let cases: &[(&str, &str)] = &[
            ("CAP LS", "CAP"),
            ("authenticate +", "AUTHENTICATE"),
            ("NICK n", "NICK"),
            ("USER u 0 * :r", "USER"),
            ("PING t", "PING"),
            ("QUIT", "QUIT"),
            ("ADMIN", "ADMIN"),
            ("OPER a b", "OPER"),
            ("JOIN #x key", "JOIN"),
            ("PART #x :r", "PART"),
            ("PRIVMSG #x :m", "PRIVMSG"),
            ("NOTICE n :m", "NOTICE"),
            ("TAGMSG #x :", "TAGMSG"),
            ("TOPIC #x :t", "TOPIC"),
            ("KICK #x n :r", "KICK"),
            ("MODE #x", "MODE"),
            ("INVITE n #x", "INVITE"),
            ("NAMES", "NAMES"),
            ("LIST", "LIST"),
            ("WHO", "WHO"),
            ("WHOIS n", "WHOIS"),
            ("MOTD", "MOTD"),
            ("VERSION", "VERSION"),
            ("LUSERS", "LUSERS"),
            ("AWAY :gone", "AWAY"),
            ("AWAY", "AWAY"),
            ("PASS secret", "PASS"),
            ("USERHOST a b", "USERHOST"),
            ("ISON a b", "ISON"),
            ("TIME", "TIME"),
            ("INFO", "INFO"),
            ("KILL n :r", "KILL"),
            ("WALLOPS :hi", "WALLOPS"),
            ("CHATHISTORY LATEST #x * 5", "CHATHISTORY"),
            ("ZZZ", "ZZZ"),
        ];
        for (raw, want) in cases {
            let cmd = Command::from_raw(&RawLine::parse(raw).unwrap());
            assert_eq!(cmd.verb(), *want, "{raw}");
            let _ = cmd.lane();
            let _ = cmd.allowed_pre_register();
        }
        assert!(Command::from_raw(&RawLine::parse("CAP LS").unwrap()).allowed_pre_register());
        assert!(!Command::from_raw(&RawLine::parse("JOIN #x").unwrap()).allowed_pre_register());
        assert_eq!(
            Command::from_raw(&RawLine::parse("OPER a b").unwrap()).lane(),
            "oper"
        );
        assert_eq!(
            Command::from_raw(&RawLine::parse("PRIVMSG #c :x").unwrap()).lane(),
            "message"
        );
        assert_eq!(
            Command::from_raw(&RawLine::parse("CHATHISTORY LATEST #c * 1").unwrap()).lane(),
            "history"
        );
        assert_eq!(
            Command::from_raw(&RawLine::parse("WHOIS n").unwrap()).lane(),
            "query"
        );
        assert_eq!(
            Command::from_raw(&RawLine::parse("NOSUCH").unwrap()).lane(),
            "unknown"
        );
    }
}
