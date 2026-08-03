//! Source-level guard: session must not hold `shared.lock()` across `.await` (H-05).

/// Scan session sources for `let … = shared.lock().await` blocks that contain `.await`
/// before the binding scope closes. Returns offending line numbers (1-based).
pub fn find_lock_across_await(src: &str) -> Vec<usize> {
    let lines: Vec<&str> = src.lines().collect();
    let mut bad = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let is_bind = line.contains("shared.lock().await")
            && (line.contains("let g ")
                || line.contains("let mut g ")
                || line.contains("let mut g=")
                || line.contains("let g="));
        if !is_bind {
            i += 1;
            continue;
        }
        // Track brace depth from this line
        let mut depth = 0i32;
        let mut started = false;
        for (j, l) in lines.iter().enumerate().skip(i) {
            for ch in l.chars() {
                if ch == '{' {
                    depth += 1;
                    started = true;
                } else if ch == '}' {
                    depth -= 1;
                }
            }
            if l.contains("drop(g)") {
                break;
            }
            if j > i && l.contains(".await") {
                // Ignore comments
                let trimmed = l.trim_start();
                if !trimmed.starts_with("//") {
                    bad.push(i + 1);
                    break;
                }
            }
            if started && depth <= 0 {
                break;
            }
            // Also end on bare statement without braces: single-line let g = ...;
            if !started && j > i {
                break;
            }
        }
        i += 1;
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_rs_has_no_lock_across_await() {
        for (name, src) in [
            ("session/mod.rs", include_str!("session/mod.rs")),
            ("session/cap.rs", include_str!("session/cap.rs")),
            ("session/register.rs", include_str!("session/register.rs")),
        ] {
            let bad = find_lock_across_await(src);
            assert!(
                bad.is_empty(),
                "shared.lock() held across .await at {name} lines {bad:?}"
            );
        }
    }

    #[test]
    fn detector_catches_bad_pattern() {
        let bad_src = r#"
            {
                let mut g = shared.lock().await;
                writer.write_all(b"x").await?;
            }
        "#;
        assert!(!find_lock_across_await(bad_src).is_empty());
    }

    #[test]
    fn detector_allows_drop_then_await() {
        let ok_src = r#"
            {
                let on_chan = {
                    let g = shared.lock().await;
                    g.foo
                };
                writer.write_all(b"x").await?;
            }
        "#;
        assert!(find_lock_across_await(ok_src).is_empty());
    }
}
