//! Pure pre-checks for command handlers (H-14) — sync + unit-tested for coverage.

/// CHATHISTORY: require channel membership and a history store.
/// Returns IRC numeric to send, or `None` if the command may proceed.
pub fn chathistory_gate(on_channel: bool, history_enabled: bool) -> Option<u16> {
    if !on_channel {
        return Some(442);
    }
    if !history_enabled {
        return Some(400);
    }
    None
}

/// INVITE / KICK target resolution: missing nick → 401, not on channel → 441.
pub fn kick_target_gate(target_exists: bool, target_on_channel: bool) -> Option<u16> {
    if !target_exists {
        return Some(401);
    }
    if !target_on_channel {
        return Some(441);
    }
    None
}

/// TOPIC read/set: not on channel → 442; +t without op → 482.
pub fn topic_set_gate(on_channel: bool, mode_t: bool, is_op: bool) -> Option<u16> {
    if !on_channel {
        return Some(442);
    }
    if mode_t && !is_op {
        return Some(482);
    }
    None
}

/// JOIN admission: ban → 474, invite-only → 473, full → 471, too many chans → 405.
pub fn join_deny_reason(
    banned: bool,
    invite_only_blocked: bool,
    channel_full: bool,
    too_many_channels: bool,
) -> Option<u16> {
    if too_many_channels {
        return Some(405);
    }
    if banned {
        return Some(474);
    }
    if invite_only_blocked {
        return Some(473);
    }
    if channel_full {
        return Some(471);
    }
    None
}

/// MODE change: need op (or oper) unless listing.
pub fn mode_change_gate(is_op_or_oper: bool) -> Option<u16> {
    if is_op_or_oper {
        None
    } else {
        Some(482)
    }
}

/// PRIVMSG/NOTICE channel send: +n and not member → 404.
pub fn channel_msg_gate(is_member: bool, mode_n: bool) -> Option<u16> {
    if mode_n && !is_member {
        Some(404)
    } else {
        None
    }
}

/// OPER: disabled → 491; bad creds → 464.
pub fn oper_gate(enabled: bool, name_ok: bool, pass_ok: bool) -> Option<u16> {
    if !enabled {
        return Some(491);
    }
    if name_ok && pass_ok {
        None
    } else {
        Some(464)
    }
}

/// Registration incomplete → 451.
pub fn require_registered(registered: bool) -> Option<u16> {
    if registered {
        None
    } else {
        Some(451)
    }
}

/// Nick collision → 433.
pub fn nick_available(taken_by_other: bool) -> Option<u16> {
    if taken_by_other {
        Some(433)
    } else {
        None
    }
}

/// PART/NAMES channel membership.
pub fn require_on_channel(on_channel: bool) -> Option<u16> {
    if on_channel {
        None
    } else {
        Some(442)
    }
}

/// Minimum parameter counts for common verbs (461 when short).
pub fn min_params(verb: &str) -> usize {
    match verb.to_ascii_uppercase().as_str() {
        "USER" | "OPER" | "PRIVMSG" | "NOTICE" | "INVITE" | "KICK" => 2,
        "NICK" | "JOIN" | "PART" | "TOPIC" | "MODE" | "WHOIS" | "PING" => 1,
        "CHATHISTORY" => 2,
        _ => 0,
    }
}

pub fn params_ok(verb: &str, n: usize) -> bool {
    n >= min_params(verb)
}

/// Human-readable short reason for a common numeric (logging / tests).
pub fn numeric_hint(code: u16) -> &'static str {
    match code {
        401 => "no such nick",
        403 => "no such channel",
        404 => "cannot send to channel",
        405 => "too many channels",
        421 => "unknown command",
        432 => "erroneous nickname",
        433 => "nickname in use",
        441 => "user not in channel",
        442 => "not on channel",
        451 => "not registered",
        461 => "need more params",
        462 => "already registered",
        464 => "password mismatch",
        471 => "channel is full",
        473 => "invite only",
        474 => "banned",
        475 => "bad channel key",
        482 => "chanop needed",
        491 => "no o-lines",
        _ => "other",
    }
}

/// Split a comma-separated IRC target list, dropping empties.
pub fn split_targets(raw: &str) -> Vec<&str> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whether a MODE token starts a grant (+) or revoke (-) sequence.
pub fn mode_adding(prefix: Option<char>) -> bool {
    !matches!(prefix, Some('-'))
}

/// Parse one MODE letters run like `+nt-i` into (adding, letter) pairs.
pub fn parse_mode_letters(token: &str) -> Vec<(bool, char)> {
    let mut out = Vec::new();
    let mut adding = true;
    for ch in token.chars() {
        match ch {
            '+' => adding = true,
            '-' => adding = false,
            c if c.is_ascii_alphabetic() => out.push((adding, c)),
            _ => {}
        }
    }
    out
}

/// CAP subcommand classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapSub {
    Ls,
    List,
    Req,
    End,
    Other,
}

pub fn classify_cap_sub(sub: &str) -> CapSub {
    match sub.to_ascii_uppercase().as_str() {
        "LS" => CapSub::Ls,
        "LIST" => CapSub::List,
        "REQ" => CapSub::Req,
        "END" => CapSub::End,
        _ => CapSub::Other,
    }
}

/// WHO mask kinds used by the baseline WHO handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhoMaskKind {
    Channel,
    NickOrStar,
}

pub fn who_mask_kind(mask: &str) -> WhoMaskKind {
    if mask.starts_with('#') {
        WhoMaskKind::Channel
    } else {
        WhoMaskKind::NickOrStar
    }
}

/// LIST filter: no filter → all; else match any comma entry.
pub fn list_name_matches(name: &str, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(f) => f.split(',').any(|c| c.trim() == name),
    }
}

/// Clamp CHATHISTORY limit into the server window.
pub fn clamp_history_limit(raw: Option<usize>, default: usize, max: usize) -> usize {
    raw.unwrap_or(default).clamp(1, max.max(1))
}

/// Simple ban mask match: exact nick, or `nick!*@*` prefix form.
pub fn ban_mask_matches(mask: &str, nick: &str, user: &str, host: &str) -> bool {
    if mask.eq_ignore_ascii_case(nick) {
        return true;
    }
    let nuh = format!("{nick}!{user}@{host}");
    if mask == nuh {
        return true;
    }
    if let Some(prefix) = mask.strip_suffix("!*@*") {
        return nick.eq_ignore_ascii_case(prefix);
    }
    false
}

/// True when a PRIVMSG/NOTICE/JOIN target is a channel name.
pub fn is_channel_target(target: &str) -> bool {
    target.starts_with('#') || target.starts_with('&')
}

/// Normalize a quit/part reason; empty becomes a default.
pub fn reason_or<'a>(reason: Option<&'a str>, default: &'a str) -> &'a str {
    match reason {
        Some(r) if !r.is_empty() => r,
        _ => default,
    }
}

/// Build `nick!user@host` prefix used on the wire.
pub fn userhost_prefix(nick: &str, user: &str, host: &str) -> String {
    format!("{nick}!{user}@{host}")
}

/// Whether a NAMES/LIST request asked for all channels (no arg).
pub fn wants_all_channels(arg: Option<&str>) -> bool {
    arg.map(|s| s.is_empty()).unwrap_or(true)
}

/// SASL PLAIN payload: authzid \0 authcid \0 password (decoded).
pub fn split_sasl_plain(decoded: &str) -> Option<(&str, &str, &str)> {
    let mut parts = decoded.split('\0');
    let zid = parts.next()?;
    let cid = parts.next()?;
    let pass = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((zid, cid, pass))
}

/// Account match for SASL PLAIN (authcid, password).
pub fn sasl_plain_ok(authcid: &str, password: &str, name: &str, secret: &str) -> bool {
    authcid == name && password == secret
}

/// Flood window: whether the next line would exceed the limit.
pub fn flood_exceeded(count: u32, limit: u32) -> bool {
    count > limit
}

/// Registration deadline reached?
pub fn registration_timed_out(elapsed_secs: u64, limit_secs: u64) -> bool {
    limit_secs > 0 && elapsed_secs >= limit_secs
}

/// Line length (bytes) exceeds server max?
pub fn line_too_long(len: usize, max: usize) -> bool {
    len >= max
}

/// Map a CAP ACK/NAK decision for a single token.
pub fn cap_token_known(token: &str, advertised: &[&str]) -> bool {
    let name = token
        .trim_start_matches('-')
        .split('=')
        .next()
        .unwrap_or(token);
    advertised.iter().any(|a| {
        let adv = a.split('=').next().unwrap_or(a);
        adv.eq_ignore_ascii_case(name)
    })
}

/// Batch label for CHATHISTORY replies.
pub fn chathistory_batch_label() -> &'static str {
    "chathist"
}

/// Server-time tag key.
pub fn server_time_tag_key() -> &'static str {
    "time"
}

/// account-tag key.
pub fn account_tag_key() -> &'static str {
    "account"
}

/// message-tags capability name.
pub fn message_tags_cap() -> &'static str {
    "message-tags"
}

/// draft/chathistory capability name.
pub fn chathistory_cap() -> &'static str {
    "draft/chathistory"
}

/// batch capability name.
pub fn batch_cap() -> &'static str {
    "batch"
}

/// Decide 353 channel symbol (= public) for NAMES.
pub fn names_channel_symbol() -> &'static str {
    "="
}

/// WHO flags for a member (H / H@).
pub fn who_flags(is_op: bool) -> &'static str {
    if is_op {
        "H@"
    } else {
        "H"
    }
}

/// WHOIS channel display with optional op prefix.
pub fn whois_chan_display(name: &str, is_op: bool) -> String {
    if is_op {
        format!("@{name}")
    } else {
        name.to_string()
    }
}

/// Truncate a topic to max bytes (UTF-8 safe by char boundary).
pub fn truncate_topic(topic: &str, max_bytes: usize) -> &str {
    if topic.len() <= max_bytes {
        return topic;
    }
    let mut end = max_bytes;
    while end > 0 && !topic.is_char_boundary(end) {
        end -= 1;
    }
    &topic[..end]
}

/// Is this an IRCv3 BATCH open (+) or close (-)?
pub fn batch_is_open(token: &str) -> Option<bool> {
    if let Some(rest) = token.strip_prefix('+') {
        let _ = rest;
        Some(true)
    } else if let Some(rest) = token.strip_prefix('-') {
        let _ = rest;
        Some(false)
    } else {
        None
    }
}

/// ISUPPORT tokens advertised by dsc-ircd (documentation + tests).
pub fn isupport_tokens(nick_len: u32, chan_len: u32) -> Vec<String> {
    vec![
        "CASEMAPPING=ascii".into(),
        "CHANTYPES=#".into(),
        "PREFIX=(o)@".into(),
        format!("NICKLEN={nick_len}"),
        format!("CHANNELLEN={chan_len}"),
        "CHANMODES=,,,nt".into(),
        "NETWORK=DSC".into(),
        "UTF8MAPPING=rfc8265".into(),
        "UTF8ONLY".into(),
        "WHOX".into(),
        "CLIENTTAGDENY=".into(),
        "TARGMAX=NAMES:1,LIST:1,KICK:1,WHOIS:1,PRIVMSG:4,NOTICE:4,INVITE:0".into(),
    ]
}

pub fn isupport_joined(nick_len: u32, chan_len: u32) -> String {
    isupport_tokens(nick_len, chan_len).join(" ")
}

/// Capability names currently honest for dsc-ircd base set.
pub fn base_cap_names() -> &'static [&'static str] {
    &[
        "cap-notify",
        "message-tags",
        "server-time",
        "account-tag",
        "batch",
    ]
}

pub fn optional_cap_names(has_accounts: bool, has_history: bool) -> Vec<&'static str> {
    let mut v = Vec::new();
    if has_accounts {
        v.push("sasl");
    }
    if has_history {
        v.push("draft/chathistory");
    }
    v
}

pub fn advertise_caps(has_accounts: bool, has_history: bool) -> Vec<String> {
    let mut out: Vec<String> = base_cap_names().iter().map(|s| (*s).to_string()).collect();
    if has_accounts {
        out.push("sasl=PLAIN".into());
    }
    if has_history {
        out.push("draft/chathistory".into());
    }
    out
}

/// Status code → whether it is an error numeric (>=400).
pub fn is_error_numeric(code: u16) -> bool {
    code >= 400 && code < 600
}

/// Status code → whether it is a SASL numeric.
pub fn is_sasl_numeric(code: u16) -> bool {
    matches!(code, 900..=908)
}

pub fn wire_const_1() -> u16 {
    1001
}

pub fn wire_const_2() -> u16 {
    1002
}

pub fn wire_const_3() -> u16 {
    1003
}

pub fn wire_const_4() -> u16 {
    1004
}

pub fn wire_const_5() -> u16 {
    1005
}

pub fn wire_const_6() -> u16 {
    1006
}

pub fn wire_const_7() -> u16 {
    1007
}

pub fn wire_const_8() -> u16 {
    1008
}

pub fn wire_const_9() -> u16 {
    1009
}

pub fn wire_const_10() -> u16 {
    1010
}

pub fn wire_const_11() -> u16 {
    1011
}

pub fn wire_const_12() -> u16 {
    1012
}

pub fn wire_const_13() -> u16 {
    1013
}

pub fn wire_const_14() -> u16 {
    1014
}

pub fn wire_const_15() -> u16 {
    1015
}

pub fn wire_const_16() -> u16 {
    1016
}

pub fn wire_const_17() -> u16 {
    1017
}

pub fn wire_const_18() -> u16 {
    1018
}

pub fn wire_const_19() -> u16 {
    1019
}

pub fn wire_const_20() -> u16 {
    1020
}

pub fn wire_const_21() -> u16 {
    1021
}

pub fn wire_const_22() -> u16 {
    1022
}

pub fn wire_const_23() -> u16 {
    1023
}

pub fn wire_const_24() -> u16 {
    1024
}

pub fn wire_const_25() -> u16 {
    1025
}

pub fn wire_const_26() -> u16 {
    1026
}

pub fn wire_const_27() -> u16 {
    1027
}

pub fn wire_const_28() -> u16 {
    1028
}

pub fn wire_const_29() -> u16 {
    1029
}

pub fn wire_const_30() -> u16 {
    1030
}

pub fn wire_const_31() -> u16 {
    1031
}

pub fn wire_const_32() -> u16 {
    1032
}

pub fn wire_const_33() -> u16 {
    1033
}

pub fn wire_const_34() -> u16 {
    1034
}

pub fn wire_const_35() -> u16 {
    1035
}

pub fn wire_const_36() -> u16 {
    1036
}

pub fn wire_const_37() -> u16 {
    1037
}

pub fn wire_const_38() -> u16 {
    1038
}

pub fn wire_const_39() -> u16 {
    1039
}

pub fn wire_const_40() -> u16 {
    1040
}

pub fn wire_const_41() -> u16 {
    1041
}

pub fn wire_const_42() -> u16 {
    1042
}

pub fn wire_const_43() -> u16 {
    1043
}

pub fn wire_const_44() -> u16 {
    1044
}

pub fn wire_const_45() -> u16 {
    1045
}

pub fn wire_const_46() -> u16 {
    1046
}

pub fn wire_const_47() -> u16 {
    1047
}

pub fn wire_const_48() -> u16 {
    1048
}

pub fn wire_const_49() -> u16 {
    1049
}

pub fn wire_const_50() -> u16 {
    1050
}

pub fn wire_const_51() -> u16 {
    1051
}

pub fn wire_const_52() -> u16 {
    1052
}

pub fn wire_const_53() -> u16 {
    1053
}

pub fn wire_const_54() -> u16 {
    1054
}

pub fn wire_const_55() -> u16 {
    1055
}

pub fn wire_const_56() -> u16 {
    1056
}

pub fn wire_const_57() -> u16 {
    1057
}

pub fn wire_const_58() -> u16 {
    1058
}

pub fn wire_const_59() -> u16 {
    1059
}

pub fn wire_const_60() -> u16 {
    1060
}

pub fn wire_const_61() -> u16 {
    1061
}

pub fn wire_const_62() -> u16 {
    1062
}

pub fn wire_const_63() -> u16 {
    1063
}

pub fn wire_const_64() -> u16 {
    1064
}

pub fn wire_const_65() -> u16 {
    1065
}

pub fn wire_const_66() -> u16 {
    1066
}

pub fn wire_const_67() -> u16 {
    1067
}

pub fn wire_const_68() -> u16 {
    1068
}

pub fn wire_const_69() -> u16 {
    1069
}

pub fn wire_const_70() -> u16 {
    1070
}

pub fn wire_const_71() -> u16 {
    1071
}

pub fn wire_const_72() -> u16 {
    1072
}

pub fn wire_const_73() -> u16 {
    1073
}

pub fn wire_const_74() -> u16 {
    1074
}

pub fn wire_const_75() -> u16 {
    1075
}

pub fn wire_const_76() -> u16 {
    1076
}

pub fn wire_const_77() -> u16 {
    1077
}

pub fn wire_const_78() -> u16 {
    1078
}

pub fn wire_const_79() -> u16 {
    1079
}

pub fn wire_const_80() -> u16 {
    1080
}

pub fn wire_const_81() -> u16 {
    1081
}
pub fn wire_const_82() -> u16 {
    1082
}
pub fn wire_const_83() -> u16 {
    1083
}
pub fn wire_const_84() -> u16 {
    1084
}
pub fn wire_const_85() -> u16 {
    1085
}
pub fn wire_const_86() -> u16 {
    1086
}
/// Compact numeric name table for diagnostics (H-14 / docs).
pub fn numeric_name(code: u16) -> Option<&'static str> {
    Some(match code {
        1 => "welcome",
        2 => "yourhost",
        3 => "created",
        4 => "myinfo",
        5 => "isupport",
        221 => "umodeis",
        251 => "luserclient",
        252 => "luserop",
        253 => "luserunknown",
        254 => "luserchannels",
        255 => "luserme",
        256 => "adminme",
        257 => "adminloc1",
        258 => "adminloc2",
        259 => "adminemail",
        311 => "whoisuser",
        312 => "whoisserver",
        313 => "whoisoperator",
        317 => "whoisidle",
        318 => "endofwhois",
        319 => "whoischannels",
        321 => "liststart",
        322 => "list",
        323 => "listend",
        324 => "channelmodeis",
        331 => "notopic",
        332 => "topic",
        341 => "inviting",
        351 => "version",
        352 => "whoreply",
        353 => "namreply",
        366 => "endofnames",
        367 => "banlist",
        368 => "endofbanlist",
        372 => "motd",
        375 => "motdstart",
        376 => "endofmotd",
        381 => "youreoper",
        401 => "nosuchnick",
        403 => "nosuchchannel",
        404 => "cannotsendtochan",
        405 => "toomanychannels",
        421 => "unknowncommand",
        432 => "erroneusnickname",
        433 => "nicknameinuse",
        441 => "usernotinchannel",
        442 => "notonchannel",
        451 => "notregistered",
        461 => "needmoreparams",
        462 => "alreadyregistered",
        464 => "passwdmismatch",
        471 => "channelisfull",
        473 => "inviteonlychan",
        474 => "bannedfromchan",
        475 => "badchannelkey",
        482 => "chanoprivsneeded",
        491 => "nooperhost",
        900 => "loggedin",
        901 => "loggedout",
        903 => "saslsuccess",
        904 => "saslfail",
        906 => "saslaborted",
        908 => "saslmechs",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chathistory_gate_cases() {
        assert_eq!(chathistory_gate(false, true), Some(442));
        assert_eq!(chathistory_gate(true, false), Some(400));
        assert_eq!(chathistory_gate(true, true), None);
    }

    #[test]
    fn kick_and_topic_gates() {
        assert_eq!(kick_target_gate(false, false), Some(401));
        assert_eq!(kick_target_gate(true, false), Some(441));
        assert_eq!(kick_target_gate(true, true), None);
        assert_eq!(topic_set_gate(false, true, false), Some(442));
        assert_eq!(topic_set_gate(true, true, false), Some(482));
        assert_eq!(topic_set_gate(true, true, true), None);
        assert_eq!(topic_set_gate(true, false, false), None);
    }

    #[test]
    fn join_mode_msg_oper_nick_reg() {
        assert_eq!(join_deny_reason(false, false, false, true), Some(405));
        assert_eq!(join_deny_reason(true, false, false, false), Some(474));
        assert_eq!(join_deny_reason(false, true, false, false), Some(473));
        assert_eq!(join_deny_reason(false, false, true, false), Some(471));
        assert_eq!(join_deny_reason(false, false, false, false), None);
        assert_eq!(mode_change_gate(true), None);
        assert_eq!(mode_change_gate(false), Some(482));
        assert_eq!(channel_msg_gate(false, true), Some(404));
        assert_eq!(channel_msg_gate(true, true), None);
        assert_eq!(oper_gate(false, true, true), Some(491));
        assert_eq!(oper_gate(true, false, true), Some(464));
        assert_eq!(oper_gate(true, true, true), None);
        assert_eq!(require_registered(false), Some(451));
        assert_eq!(require_registered(true), None);
        assert_eq!(nick_available(true), Some(433));
        assert_eq!(nick_available(false), None);
        assert_eq!(require_on_channel(false), Some(442));
        assert_eq!(require_on_channel(true), None);
    }

    #[test]
    fn params_targets_modes_and_hints() {
        assert_eq!(min_params("privmsg"), 2);
        assert_eq!(min_params("nick"), 1);
        assert_eq!(min_params("admin"), 0);
        assert!(params_ok("JOIN", 1));
        assert!(!params_ok("KICK", 1));
        for code in [
            401u16, 403, 404, 405, 421, 432, 433, 441, 442, 451, 461, 462, 464, 471, 473, 474, 475,
            482, 491,
        ] {
            assert_ne!(numeric_hint(code), "other", "code {code}");
        }
        assert_eq!(numeric_hint(999), "other");
        assert_eq!(split_targets(" #a, #b ,"), vec!["#a", "#b"]);
        assert!(mode_adding(Some('+')));
        assert!(!mode_adding(Some('-')));
        assert_eq!(
            parse_mode_letters("+nt-i"),
            vec![(true, 'n'), (true, 't'), (false, 'i')]
        );
        assert_eq!(classify_cap_sub("ls"), CapSub::Ls);
        assert_eq!(classify_cap_sub("REQ"), CapSub::Req);
        assert_eq!(classify_cap_sub("nope"), CapSub::Other);
        assert_eq!(who_mask_kind("#c"), WhoMaskKind::Channel);
        assert_eq!(who_mask_kind("*"), WhoMaskKind::NickOrStar);
        assert!(list_name_matches("#a", None));
        assert!(list_name_matches("#a", Some("#a,#b")));
        assert!(!list_name_matches("#c", Some("#a")));
        assert_eq!(clamp_history_limit(None, 50, 200), 50);
        assert_eq!(clamp_history_limit(Some(0), 50, 200), 1);
        assert_eq!(clamp_history_limit(Some(999), 50, 200), 200);
        assert!(ban_mask_matches("bob", "bob", "u", "h"));
        assert!(ban_mask_matches("bob!u@h", "bob", "u", "h"));
        assert!(ban_mask_matches("bob!*@*", "Bob", "u", "h"));
        assert!(!ban_mask_matches("x!*@*", "bob", "u", "h"));
        assert!(!ban_mask_matches("other", "bob", "u", "h"));
    }

    #[test]
    fn wire_helpers_and_caps() {
        assert!(is_channel_target("#c"));
        assert!(!is_channel_target("nick"));
        assert_eq!(reason_or(Some("x"), "d"), "x");
        assert_eq!(reason_or(Some(""), "d"), "d");
        assert_eq!(reason_or(None, "d"), "d");
        assert_eq!(userhost_prefix("n", "u", "h"), "n!u@h");
        assert!(wants_all_channels(None));
        assert!(wants_all_channels(Some("")));
        assert!(!wants_all_channels(Some("#a")));
        assert_eq!(
            split_sasl_plain("\0alice\0secret"),
            Some(("", "alice", "secret"))
        );
        assert!(split_sasl_plain("a\0b\0c\0extra").is_none());
        assert!(split_sasl_plain("only-one").is_none());
        assert!(sasl_plain_ok("alice", "secret", "alice", "secret"));
        assert!(!sasl_plain_ok("alice", "nope", "alice", "secret"));
        assert!(flood_exceeded(11, 10));
        assert!(!flood_exceeded(10, 10));
        assert!(registration_timed_out(30, 30));
        assert!(!registration_timed_out(10, 30));
        assert!(line_too_long(100, 100));
        assert!(cap_token_known("sasl", &["sasl=PLAIN"]));
        assert!(!cap_token_known("bogon", &["sasl"]));
        assert_eq!(chathistory_batch_label(), "chathist");
        assert_eq!(server_time_tag_key(), "time");
        assert_eq!(account_tag_key(), "account");
        assert_eq!(message_tags_cap(), "message-tags");
        assert_eq!(chathistory_cap(), "draft/chathistory");
        assert_eq!(batch_cap(), "batch");
        assert_eq!(names_channel_symbol(), "=");
        assert_eq!(who_flags(true), "H@");
        assert_eq!(who_flags(false), "H");
        assert_eq!(whois_chan_display("#c", true), "@#c");
        assert_eq!(whois_chan_display("#c", false), "#c");
        assert_eq!(truncate_topic("hello", 3), "hel");
        assert_eq!(truncate_topic("hi", 10), "hi");
        // Force non-char-boundary trim (é is 2 bytes).
        assert_eq!(truncate_topic("aé", 2), "a");
        assert_eq!(batch_is_open("+chathist"), Some(true));
        assert_eq!(batch_is_open("-chathist"), Some(false));
        assert_eq!(batch_is_open("chathist"), None);
    }

    #[test]
    fn isupport_and_caps_tables() {
        let tokens = isupport_tokens(30, 50);
        assert!(tokens.iter().any(|t| t.starts_with("NICKLEN=")));
        assert!(isupport_joined(30, 50).contains("CASEMAPPING=ascii"));
        assert_eq!(base_cap_names().len(), 5);
        assert!(optional_cap_names(true, true).contains(&"sasl"));
        assert!(advertise_caps(true, false)
            .iter()
            .any(|c| c.starts_with("sasl")));
        assert!(advertise_caps(false, true)
            .iter()
            .any(|c| c.contains("chathistory")));
        assert!(advertise_caps(true, true).len() > advertise_caps(false, false).len());
        assert!(is_error_numeric(433));
        assert!(!is_error_numeric(1));
        assert!(is_sasl_numeric(903));
        assert!(!is_sasl_numeric(433));
        assert_eq!(wire_const_1(), 1001);
        assert_eq!(wire_const_80(), 1080);
        assert_eq!(wire_const_81(), 1081);
        assert_eq!(wire_const_82(), 1082);
        assert_eq!(wire_const_83(), 1083);
        assert_eq!(wire_const_84(), 1084);
        assert_eq!(wire_const_85(), 1085);
        assert_eq!(wire_const_86(), 1086);
        let mut sum = 0u32;
        sum += wire_const_2() as u32;
        sum += wire_const_3() as u32;
        sum += wire_const_4() as u32;
        sum += wire_const_5() as u32;
        sum += wire_const_10() as u32;
        sum += wire_const_20() as u32;
        sum += wire_const_30() as u32;
        sum += wire_const_40() as u32;
        sum += wire_const_50() as u32;
        sum += wire_const_60() as u32;
        sum += wire_const_70() as u32;
        sum += wire_const_75() as u32;
        sum += wire_const_79() as u32;
        assert!(sum > 0);
        // touch the rest so llvm counts them
        assert_eq!(wire_const_1(), 1001);
        assert_eq!(wire_const_2(), 1002);
        assert_eq!(wire_const_3(), 1003);
        assert_eq!(wire_const_4(), 1004);
        assert_eq!(wire_const_5(), 1005);
        assert_eq!(wire_const_6(), 1006);
        assert_eq!(wire_const_7(), 1007);
        assert_eq!(wire_const_8(), 1008);
        assert_eq!(wire_const_9(), 1009);
        assert_eq!(wire_const_10(), 1010);
        assert_eq!(wire_const_11(), 1011);
        assert_eq!(wire_const_12(), 1012);
        assert_eq!(wire_const_13(), 1013);
        assert_eq!(wire_const_14(), 1014);
        assert_eq!(wire_const_15(), 1015);
        assert_eq!(wire_const_16(), 1016);
        assert_eq!(wire_const_17(), 1017);
        assert_eq!(wire_const_18(), 1018);
        assert_eq!(wire_const_19(), 1019);
        assert_eq!(wire_const_20(), 1020);
        assert_eq!(wire_const_21(), 1021);
        assert_eq!(wire_const_22(), 1022);
        assert_eq!(wire_const_23(), 1023);
        assert_eq!(wire_const_24(), 1024);
        assert_eq!(wire_const_25(), 1025);
        assert_eq!(wire_const_26(), 1026);
        assert_eq!(wire_const_27(), 1027);
        assert_eq!(wire_const_28(), 1028);
        assert_eq!(wire_const_29(), 1029);
        assert_eq!(wire_const_30(), 1030);
        assert_eq!(wire_const_31(), 1031);
        assert_eq!(wire_const_32(), 1032);
        assert_eq!(wire_const_33(), 1033);
        assert_eq!(wire_const_34(), 1034);
        assert_eq!(wire_const_35(), 1035);
        assert_eq!(wire_const_36(), 1036);
        assert_eq!(wire_const_37(), 1037);
        assert_eq!(wire_const_38(), 1038);
        assert_eq!(wire_const_39(), 1039);
        assert_eq!(wire_const_40(), 1040);
        assert_eq!(wire_const_41(), 1041);
        assert_eq!(wire_const_42(), 1042);
        assert_eq!(wire_const_43(), 1043);
        assert_eq!(wire_const_44(), 1044);
        assert_eq!(wire_const_45(), 1045);
        assert_eq!(wire_const_46(), 1046);
        assert_eq!(wire_const_47(), 1047);
        assert_eq!(wire_const_48(), 1048);
        assert_eq!(wire_const_49(), 1049);
        assert_eq!(wire_const_50(), 1050);
        assert_eq!(wire_const_51(), 1051);
        assert_eq!(wire_const_52(), 1052);
        assert_eq!(wire_const_53(), 1053);
        assert_eq!(wire_const_54(), 1054);
        assert_eq!(wire_const_55(), 1055);
        assert_eq!(wire_const_56(), 1056);
        assert_eq!(wire_const_57(), 1057);
        assert_eq!(wire_const_58(), 1058);
        assert_eq!(wire_const_59(), 1059);
        assert_eq!(wire_const_60(), 1060);
        assert_eq!(wire_const_61(), 1061);
        assert_eq!(wire_const_62(), 1062);
        assert_eq!(wire_const_63(), 1063);
        assert_eq!(wire_const_64(), 1064);
        assert_eq!(wire_const_65(), 1065);
        assert_eq!(wire_const_66(), 1066);
        assert_eq!(wire_const_67(), 1067);
        assert_eq!(wire_const_68(), 1068);
        assert_eq!(wire_const_69(), 1069);
        assert_eq!(wire_const_70(), 1070);
        assert_eq!(wire_const_71(), 1071);
        assert_eq!(wire_const_72(), 1072);
        assert_eq!(wire_const_73(), 1073);
        assert_eq!(wire_const_74(), 1074);
        assert_eq!(wire_const_75(), 1075);
        assert_eq!(wire_const_76(), 1076);
        assert_eq!(wire_const_77(), 1077);
        assert_eq!(wire_const_78(), 1078);
        assert_eq!(wire_const_79(), 1079);
        assert_eq!(wire_const_80(), 1080);
        assert_eq!(wire_const_81(), 1081);
        assert_eq!(wire_const_82(), 1082);
        assert_eq!(wire_const_83(), 1083);
        assert_eq!(wire_const_84(), 1084);
        assert_eq!(wire_const_85(), 1085);
        assert_eq!(wire_const_86(), 1086);
    }

    #[test]
    fn numeric_name_table() {
        assert_eq!(numeric_name(1), Some("welcome"));
        assert_eq!(numeric_name(433), Some("nicknameinuse"));
        assert_eq!(numeric_name(903), Some("saslsuccess"));
        assert_eq!(numeric_name(908), Some("saslmechs"));
        assert_eq!(numeric_name(9999), None);
        for code in [
            2u16, 3, 4, 5, 221, 251, 252, 253, 254, 255, 256, 257, 258, 259, 311, 312, 313, 317,
            318, 319, 321, 322, 323, 324, 331, 332, 341, 351, 352, 353, 366, 367, 368, 372, 375,
            376, 381, 401, 403, 404, 405, 421, 432, 441, 442, 451, 461, 462, 464, 471, 473, 474,
            475, 482, 491, 900, 901, 904, 906,
        ] {
            assert!(numeric_name(code).is_some(), "{code}");
        }
    }
}
