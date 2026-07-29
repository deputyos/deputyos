//! Tiny ASCII QR code renderer.
//!
//! `deputywizard print-qr` prints a QR for the wizard launch URL to stdout
//! (and writes the URL itself to `/run/deputyos/wizard.url`). A first-boot
//! systemd unit pipes that into `/dev/tty1` so the head-attached operator
//! can scan it from their phone — no second screen required.
//!
//! We use [`qrcodegen`] (no_std, pure-Rust) and render with two rows of
//! pixels per terminal row using the unicode upper/lower half-blocks
//! (`▀ ▄ █  `). That keeps the QR roughly square at typical terminal cell
//! aspect ratios. Quiet zone of 4 modules per QR spec.

use qrcodegen::{QrCode, QrCodeEcc};

const QUIET: i32 = 4;

/// Render `text` as an ASCII QR. Returns the rendered string (newline-
/// terminated lines) on success. Errors out only if the data is too big for
/// even the highest QR version, which we never hit in practice for
/// `deputyos.local` URLs.
pub fn render_url(text: &str) -> Result<String, String> {
    let qr =
        QrCode::encode_text(text, QrCodeEcc::Medium).map_err(|e| format!("encoding QR: {e}"))?;
    Ok(render_qr(&qr))
}

fn render_qr(qr: &QrCode) -> String {
    let size = qr.size();
    let mut out = String::new();
    let mut y = -QUIET;
    while y < size + QUIET {
        for x in (-QUIET)..(size + QUIET) {
            let top = qr.get_module(x, y);
            let bot = if y + 1 < size + QUIET {
                qr.get_module(x, y + 1)
            } else {
                false
            };
            // We want LIGHT background and DARK foreground in the terminal.
            // Use upper-half-block for top-only, lower-half-block for
            // bottom-only, full-block for both, space for neither.
            let ch = match (top, bot) {
                (false, false) => ' ',
                (true, false) => '▀',
                (false, true) => '▄',
                (true, true) => '█',
            };
            out.push(ch);
        }
        out.push('\n');
        y += 2;
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn renders_non_empty_qr_for_url() {
        let s = render_url("https://example.com/test").unwrap();
        // Some non-space pixels appear.
        assert!(s.chars().any(|c| c == '█' || c == '▀' || c == '▄'));
        // It's a multi-line block.
        assert!(s.lines().count() > 5);
    }

    #[test]
    fn each_line_is_uniform_width() {
        let s = render_url("https://example.com").unwrap();
        let widths: std::collections::HashSet<usize> =
            s.lines().map(|l| l.chars().count()).collect();
        // All rows are the same width — module count + 2*quiet zones.
        assert_eq!(widths.len(), 1, "lines: {widths:?}");
    }
}
