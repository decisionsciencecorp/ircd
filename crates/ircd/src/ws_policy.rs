//! WebSocket handshake policy (H-12) — origin allowlist + IRC subprotocol.

/// Decide whether an HTTP Origin is acceptable for IRC-over-WebSocket.
///
/// - `None` / empty: allowed only when `allow_missing_origin` (native clients).
/// - Otherwise must match an entry in `allowlist` (exact, case-sensitive for URLs).
pub fn origin_allowed(
    origin: Option<&str>,
    allowlist: &[String],
    allow_missing_origin: bool,
) -> bool {
    match origin.map(str::trim).filter(|s| !s.is_empty()) {
        None => allow_missing_origin,
        Some(o) => {
            if allowlist.is_empty() {
                // Empty allowlist = deny browser Origins (fail closed for web).
                false
            } else {
                allowlist.iter().any(|a| a == o)
            }
        }
    }
}

/// Pick the IRC WebSocket subprotocol from a client offer list.
/// Returns `Some("irc")` when the client offered `irc` (case-insensitive).
pub fn select_irc_subprotocol(offered: &[String]) -> Option<&'static str> {
    if offered
        .iter()
        .any(|p| p.split(',').any(|t| t.trim().eq_ignore_ascii_case("irc")))
    {
        Some("irc")
    } else {
        None
    }
}

/// When `require_irc_subprotocol`, the offer must include `irc`.
pub fn subprotocol_acceptable(offered: &[String], require_irc: bool) -> bool {
    if !require_irc {
        return true;
    }
    select_irc_subprotocol(offered).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_origin_policy() {
        assert!(origin_allowed(None, &[], true));
        assert!(!origin_allowed(None, &[], false));
        assert!(!origin_allowed(Some("https://evil.test"), &[], true));
    }

    #[test]
    fn allowlist_exact() {
        let list = vec!["https://app.example".into()];
        assert!(origin_allowed(Some("https://app.example"), &list, false));
        assert!(!origin_allowed(Some("https://other.example"), &list, false));
    }

    #[test]
    fn irc_subprotocol() {
        assert_eq!(
            select_irc_subprotocol(&["chat".into(), "irc".into()]),
            Some("irc")
        );
        assert!(subprotocol_acceptable(&["IRC".into()], true));
        assert!(!subprotocol_acceptable(&["chat".into()], true));
        assert!(subprotocol_acceptable(&[], false));
    }
}
