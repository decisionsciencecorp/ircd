//! Registration / welcome numerics (H-14).

use anyhow::Result;
use ircd_core::numeric;
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::VERSION;

/// Max CHATHISTORY limit advertised in 005 when history is enabled (matches `latest` clamp).
pub const CHATHISTORY_ISUPPORT_MAX: usize = 200;

pub(crate) async fn try_register<W>(
    writer: &mut W,
    server_name: &str,
    motd: &str,
    registered: &mut bool,
    nick: Option<&str>,
    user: Option<&str>,
    _realname: Option<&str>,
    cap_negotiating: bool,
    nick_len: usize,
    chan_len: usize,
    // When Some(max), advertise CHATHISTORY=<max> + MSGREFTYPES (F3).
    chathistory_max: Option<usize>,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    if *registered {
        return Ok(());
    }
    if cap_negotiating {
        return Ok(());
    }
    let (Some(nick), Some(_user)) = (nick, user) else {
        return Ok(());
    };
    *registered = true;
    writer
        .write_all(
            numeric(
                server_name,
                1,
                nick,
                &[&format!("Welcome to the DSC Internet Relay Network {nick}")],
            )
            .as_bytes(),
        )
        .await?;
    writer
        .write_all(
            numeric(
                server_name,
                2,
                nick,
                &[&format!(
                    "Your host is {server_name}, running version dsc-ircd-{VERSION}"
                )],
            )
            .as_bytes(),
        )
        .await?;
    writer
        .write_all(
            numeric(
                server_name,
                3,
                nick,
                &["This server was created for the Mark × Cody IRC rebuild"],
            )
            .as_bytes(),
        )
        .await?;
    writer
        .write_all(
            numeric(
                server_name,
                4,
                nick,
                &[server_name, &format!("dsc-ircd-{VERSION}"), "i", "nt"],
            )
            .as_bytes(),
        )
        .await?;
    let mut tokens = crate::cmd_precheck::isupport_tokens(nick_len as u32, chan_len as u32);
    if let Some(max) = chathistory_max {
        tokens.extend(crate::cmd_precheck::isupport_history_tokens(max));
    }
    let isupport = format!("{} :are supported by this server", tokens.join(" "));
    writer
        .write_all(numeric(server_name, 5, nick, &[isupport.as_str()]).as_bytes())
        .await?;
    let motd_start = format!("- {server_name} Message of the day -");
    writer
        .write_all(numeric(server_name, 375, nick, &[motd_start.as_str()]).as_bytes())
        .await?;
    for line in motd.lines() {
        let body = format!("- {line}");
        writer
            .write_all(numeric(server_name, 372, nick, &[body.as_str()]).as_bytes())
            .await?;
    }
    writer
        .write_all(numeric(server_name, 376, nick, &["End of /MOTD command"]).as_bytes())
        .await?;
    info!(%nick, "client registered");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[tokio::test]
    async fn welcome_numerics_with_history_tokens() {
        let (c, s) = tokio::io::duplex(8192);
        let mut w = s;
        let mut registered = false;
        try_register(
            &mut w,
            "srv",
            "line1\nline2",
            &mut registered,
            Some("nick"),
            Some("user"),
            Some("Real"),
            false,
            16,
            50,
            Some(200),
        )
        .await
        .unwrap();
        drop(w);
        assert!(registered);
        let mut body = String::new();
        let mut r = BufReader::new(c);
        let mut line = String::new();
        while r.read_line(&mut line).await.unwrap() > 0 {
            body.push_str(&line);
            line.clear();
        }
        assert!(body.contains("001"));
        assert!(body.contains("005"));
        assert!(body.contains("CHATHISTORY"));
        assert!(body.contains("376"));
    }
}
