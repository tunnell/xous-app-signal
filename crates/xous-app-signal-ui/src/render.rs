//! Render surfaces.
//!
//! Stage 9c only ships `TextSurface` — a stdout flush of the screen
//! lines plus a status bar and hint footer. A future
//! `cfg(target_os = "xous")` `GamSurface` will translate the same
//! `Vec<String>`-shaped screen output into Xous GAM `TextView`
//! draws.

use std::io::Write;

/// Approximate display width in characters. The Precursor LCD is
/// 336 px wide; at our 7-px-per-char font that's ~48 chars. We
/// round up to 50 so the box-drawing characters in mockups line up.
pub const WIDTH: usize = 50;

/// Approximate display height in lines (status + content + footer).
/// 22 = 24 / 24 + 492 / 24 + 20 / 24, rounded.
#[allow(dead_code)]
pub const HEIGHT: usize = 22;

/// Render one frame to stdout, framed with the status bar and hint
/// footer described in UI.md §3. Lines too long for `WIDTH` are
/// truncated; lines shorter are padded out so the box-rendering
/// stays aligned.
///
/// `status_chips` are the inverted chips (`[WiFi]`, `[TLS]`, `[OFF]`).
/// `hint` is the per-screen footer text.
pub fn render_frame(
    out: &mut impl Write,
    status_chips: &str,
    body_lines: &[String],
    hint: &str,
) -> std::io::Result<()> {
    writeln!(out, "{}", "┌".to_string() + &"─".repeat(WIDTH) + "┐")?;
    writeln!(out, "│{}│", pad(&format!(" xas   {status_chips}"), WIDTH))?;
    writeln!(out, "{}", "├".to_string() + &"─".repeat(WIDTH) + "┤")?;

    // Body — pad / truncate each line to fit. Cap at 18 lines so the
    // total frame fits in HEIGHT; longer body output is silently
    // truncated rather than scrolled (Stage 9c screens are all short).
    let body_max = 18;
    for line in body_lines.iter().take(body_max) {
        writeln!(out, "│{}│", pad(line, WIDTH))?;
    }
    for _ in body_lines.len().min(body_max)..body_max {
        writeln!(out, "│{}│", " ".repeat(WIDTH))?;
    }

    writeln!(out, "{}", "├".to_string() + &"─".repeat(WIDTH) + "┤")?;
    writeln!(out, "│{}│", pad(&format!(" {hint}"), WIDTH))?;
    writeln!(out, "{}", "└".to_string() + &"─".repeat(WIDTH) + "┘")?;
    out.flush()?;
    Ok(())
}

/// Truncate or right-pad `line` to exactly `width` chars. Counts
/// chars (not bytes) so multi-byte UTF-8 like `█`/`✓` still aligns —
/// at the cost of mixed-width glyphs (full-width CJK) being
/// mis-counted, which we don't render in Stage 9c.
fn pad(line: &str, width: usize) -> String {
    let count = line.chars().count();
    if count >= width {
        line.chars().take(width).collect()
    } else {
        let mut out = String::with_capacity(width);
        out.push_str(line);
        for _ in 0..(width - count) {
            out.push(' ');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_truncates_long_lines() {
        assert_eq!(pad("0123456789abc", 5), "01234");
    }

    #[test]
    fn pad_extends_short_lines() {
        assert_eq!(pad("hi", 5), "hi   ");
    }

    #[test]
    fn pad_handles_multibyte() {
        // `✓` is a single char, 3 bytes UTF-8. Count must use chars
        // so the resulting string is `✓    ` (5 chars total).
        let out = pad("✓", 5);
        assert_eq!(out.chars().count(), 5);
    }

    #[test]
    fn render_frame_writes_a_box() {
        let mut buf = Vec::new();
        render_frame(&mut buf, "[OFF]", &[String::from("hello")], "hint").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("xas"));
        assert!(s.contains("hello"));
        assert!(s.contains("hint"));
        assert!(s.contains("┌"));
        assert!(s.contains("└"));
    }
}
