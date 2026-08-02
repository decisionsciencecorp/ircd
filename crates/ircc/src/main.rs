//! Minimal interactive/smoke IRC client for testing dsc-ircd.
//!
//! Examples:
//!   cargo run -p ircc -- --host 127.0.0.1 --port 6667 --nick otto
//!   cargo run -p ircc -- --tls --host 127.0.0.1 --port 6697 --nick otto --join '#test' --msg 'hi' --quit

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tracing::{info, warn};

struct Args {
    host: String,
    port: u16,
    nick: String,
    user: String,
    join: Option<String>,
    msg: Option<String>,
    quit_after: bool,
    tls: bool,
    /// Negotiate IRCv3 CAP before NICK/USER.
    cap: bool,
}

fn parse_args() -> Result<Args> {
    let mut host = "127.0.0.1".to_string();
    let mut port = 6667u16;
    let mut nick = "ircc".to_string();
    let mut user = "ircc".to_string();
    let mut join = None;
    let mut msg = None;
    let mut quit_after = false;
    let mut tls = false;
    let mut cap = false;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--host" => host = it.next().context("--host needs value")?,
            "--port" => port = it.next().context("--port needs value")?.parse()?,
            "--nick" => nick = it.next().context("--nick needs value")?,
            "--user" => user = it.next().context("--user needs value")?,
            "--join" => join = Some(it.next().context("--join needs value")?),
            "--msg" => msg = Some(it.next().context("--msg needs value")?),
            "--quit" => quit_after = true,
            "--tls" => tls = true,
            "--cap" => cap = true,
            "-h" | "--help" => {
                println!(
                    "ircc — dsc-ircd smoke CLI\n\n\
                     --host HOST --port PORT --nick NICK [--user USER] [--tls] [--cap]\n\
                     [--join #chan] [--msg text] [--quit]\n\n\
                     --tls accepts any server cert (lab only).\n\
                     --cap sends CAP LS / REQ / END before register.\n\
                     Without --quit, reads stdin lines as raw IRC or bare channel chat if JOINed."
                );
                std::process::exit(0);
            }
            other => bail!("unknown arg {other}"),
        }
    }
    if msg.is_some() && join.is_none() {
        bail!("--msg requires --join");
    }
    if tls && port == 6667 {
        // common default for IRC+TLS
        port = 6697;
    }
    Ok(Args {
        host,
        port,
        nick,
        user,
        join,
        msg,
        quit_after,
        tls,
        cap,
    })
}

async fn send_line<W: AsyncWriteExt + Unpin>(writer: &mut W, line: &str) -> Result<()> {
    writer.write_all(line.as_bytes()).await?;
    print!(">> {}", line.trim_end_matches(['\r', '\n']));
    println!();
    Ok(())
}

#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ircc=info".into()),
        )
        .init();

    let args = parse_args()?;
    let addr = format!("{}:{}", args.host, args.port);
    info!(
        "connecting to {addr} as {} ({})",
        args.nick,
        if args.tls { "tls" } else { "plaintext" }
    );

    let tcp = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("connect {addr}"))?;

    if args.tls {
        let config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let server_name = ServerName::try_from(args.host.clone())
            .map_err(|_| anyhow::anyhow!("invalid TLS server name {}", args.host))?;
        let tls = connector
            .connect(server_name, tcp)
            .await
            .context("tls handshake")?;
        let (reader, writer) = tokio::io::split(tls);
        run_session(reader, writer, args).await
    } else {
        let (reader, writer) = tcp.into_split();
        run_session(reader, writer, args).await
    }
}

async fn run_session<R, W>(reader: R, mut writer: W, args: Args) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut reader = BufReader::new(reader);

    if args.cap {
        send_line(&mut writer, "CAP LS 302\r\n").await?;
        // Read until CAP LS reply (bounded)
        let mut buf = String::new();
        for _ in 0..20 {
            buf.clear();
            let n = reader.read_line(&mut buf).await?;
            if n == 0 {
                break;
            }
            let display = buf.trim_end_matches(['\r', '\n']);
            println!("<< {display}");
            if let Some(msg) = ircd_core::RawLine::parse(display) {
                if msg.command_eq("CAP")
                    && msg
                        .params
                        .get(1)
                        .map(|s| s.eq_ignore_ascii_case("LS"))
                        .unwrap_or(false)
                {
                    break;
                }
            }
        }
        send_line(
            &mut writer,
            "CAP REQ :multi-prefix server-time message-tags away-notify\r\n",
        )
        .await?;
        for _ in 0..10 {
            buf.clear();
            let n = reader.read_line(&mut buf).await?;
            if n == 0 {
                break;
            }
            let display = buf.trim_end_matches(['\r', '\n']);
            println!("<< {display}");
            if let Some(msg) = ircd_core::RawLine::parse(display) {
                if msg.command_eq("CAP")
                    && msg.params.get(1).map(|s| {
                        s.eq_ignore_ascii_case("ACK") || s.eq_ignore_ascii_case("NAK")
                    }).unwrap_or(false)
                {
                    break;
                }
            }
        }
        send_line(&mut writer, "CAP END\r\n").await?;
    }

    send_line(&mut writer, &format!("NICK {}\r\n", args.nick)).await?;
    send_line(
        &mut writer,
        &format!("USER {} 0 * :ircc smoke\r\n", args.user),
    )
    .await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let interactive = !args.quit_after;
    if interactive {
        let tx_stdin = tx.clone();
        tokio::spawn(async move {
            let stdin = BufReader::new(tokio::io::stdin());
            let mut lines = stdin.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx_stdin.send(line).is_err() {
                    break;
                }
            }
        });
    }

    let join_chan = args.join.clone();
    let mut line_buf = String::new();
    let mut registered = false;
    let mut joined = args.join.is_none();

    loop {
        tokio::select! {
            read = reader.read_line(&mut line_buf) => {
                let n = read?;
                if n == 0 {
                    warn!("server closed connection");
                    break;
                }
                let raw = std::mem::take(&mut line_buf);
                let display = raw.trim_end_matches(['\r','\n']);
                println!("<< {display}");

                if let Some(msg) = ircd_core::RawLine::parse(display) {
                    if msg.command_eq("PING") {
                        let token = msg.params.first().map(String::as_str).unwrap_or("ircd");
                        send_line(&mut writer, &format!("PONG :{token}\r\n")).await?;
                    }
                    if msg.command == "001" && !registered {
                        registered = true;
                        if let Some(chan) = &args.join {
                            send_line(&mut writer, &format!("JOIN {chan}\r\n")).await?;
                        } else if args.quit_after {
                            send_line(&mut writer, "QUIT :ircc smoke done\r\n").await?;
                            break;
                        }
                    }
                    if registered
                        && !joined
                        && (msg.command_eq("JOIN")
                            || msg.command == "353"
                            || msg.command == "366")
                    {
                        joined = true;
                        if let (Some(chan), Some(text)) = (&args.join, &args.msg) {
                            send_line(
                                &mut writer,
                                &format!("PRIVMSG {chan} :{text}\r\n"),
                            )
                            .await?;
                        }
                        if args.quit_after {
                            send_line(&mut writer, "QUIT :ircc smoke done\r\n").await?;
                            break;
                        }
                    }
                }
            }
            line = rx.recv(), if interactive => {
                let Some(line) = line else { break };
                let out = if line.starts_with('/') {
                    let rest = line.trim_start_matches('/');
                    if let Some(chan) = rest.strip_prefix("join ").or_else(|| rest.strip_prefix("JOIN ")) {
                        format!("JOIN {chan}\r\n")
                    } else if let Some(rest) = rest.strip_prefix("msg ").or_else(|| rest.strip_prefix("MSG ")) {
                        if let Some((target, text)) = rest.split_once(' ') {
                            format!("PRIVMSG {target} :{text}\r\n")
                        } else {
                            format!("{rest}\r\n")
                        }
                    } else if rest.eq_ignore_ascii_case("quit") {
                        "QUIT :ircc\r\n".to_string()
                    } else {
                        format!("{rest}\r\n")
                    }
                } else if let Some(chan) = &join_chan {
                    format!("PRIVMSG {chan} :{line}\r\n")
                } else {
                    format!("{line}\r\n")
                };
                send_line(&mut writer, &out).await?;
                if out.starts_with("QUIT") {
                    break;
                }
            }
        }
    }

    Ok(())
}
