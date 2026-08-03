//! Buffered session line I/O (C9) — split out of `session/mod` (C14).

use std::time::{Duration, Instant};

use tokio::io::AsyncBufReadExt;

/// Result of one cancel-safe buffered line read attempt (C9).
pub(super) enum LineOutcome {
    Complete,
    Oversized,
    Eof,
}

/// `select!` arm tag — logic runs *outside* the macro so llvm/tarpaulin sees it (C9).
pub(super) enum IoEvent {
    Outbox(Option<std::sync::Arc<str>>),
    Read(std::io::Result<LineOutcome>),
    Deadline,
}


pub(super) async fn read_line_outcome<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line_buf: &mut Vec<u8>,
    max_line: usize,
) -> std::io::Result<LineOutcome> {
    loop {
        if line_buf.len() >= max_line {
            return Ok(LineOutcome::Oversized);
        }
        let buf = reader.fill_buf().await?;
        if buf.is_empty() {
            return Ok(LineOutcome::Eof);
        }
        let room = max_line - line_buf.len();
        let chunk_len = buf.len().min(room);
        if let Some(pos) = buf[..chunk_len].iter().position(|&b| b == b'\n') {
            let take = pos + 1;
            line_buf.extend_from_slice(&buf[..take]);
            reader.consume(take);
            return Ok(LineOutcome::Complete);
        }
        line_buf.extend_from_slice(&buf[..chunk_len]);
        reader.consume(chunk_len);
        if line_buf.len() >= max_line {
            return Ok(LineOutcome::Oversized);
        }
        // Partial line in buffer; loop for more (cancel-safe across select!).
    }
}

pub(super) async fn sleep_until_deadline(deadline_at: Option<Instant>) {
    match deadline_at {
        Some(at) => {
            let now = Instant::now();
            if at > now {
                tokio::time::sleep(at.saturating_duration_since(now)).await;
            }
        }
        None => std::future::pending::<()>().await,
    }
}

pub(super) fn session_deadline_at(
    registered: bool,
    session_start: Instant,
    last_activity: Instant,
    reg_deadline: Option<Duration>,
    idle_deadline: Option<Duration>,
) -> Option<Instant> {
    if !registered {
        reg_deadline.map(|d| session_start + d)
    } else {
        idle_deadline.map(|d| last_activity + d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn oversized_when_buffer_already_at_max() {
        let mut reader = Cursor::new(b"more\n");
        let mut buf = vec![0u8; 8];
        let out = read_line_outcome(&mut reader, &mut buf, 8).await.unwrap();
        assert!(matches!(out, LineOutcome::Oversized));
    }

    #[test]
    fn deadline_none_branches() {
        let t = Instant::now();
        assert!(session_deadline_at(false, t, t, None, Some(Duration::from_secs(1))).is_none());
        assert!(session_deadline_at(true, t, t, Some(Duration::from_secs(1)), None).is_none());
    }
}

