//! Convert libghostty-vt's HTML formatter output into pango markup.
//!
//! libghostty-vt's `GHOSTTY_FORMATTER_FORMAT_HTML` produces a flat
//! string of the shape:
//!
//! ```text
//! <div style="font-family: monospace; white-space: pre;">
//! plain text <div style="display: inline;color: var(--vt-palette-1);">red</div> more
//! </div>
//! ```
//!
//! No nesting, one inline `<div>` per style run, HTML entities
//! already escaped. Pango markup uses `<span foreground="#rrggbb">`
//! with a different attribute vocabulary, so we translate.
//!
//! Style attributes we currently understand:
//! - `color: var(--vt-palette-N)` → `foreground="#rrggbb"`
//! - `color: rgb(R, G, B)` → `foreground="#rrggbb"`
//! - `background-color: ...` → `background="#rrggbb"`
//! - `font-weight: bold` → `weight="bold"`
//! - `font-style: italic` → `style="italic"`
//! - `text-decoration-line: underline` → `underline="single"`
//!
//! Anything unrecognized is silently dropped (Pango would reject
//! malformed markup, so we err on the side of stripping).

use std::fmt::Write;

const OUTER_OPEN: &str = "<div style=\"font-family: monospace; white-space: pre;\">";
const OUTER_CLOSE: &str = "</div>";
const INNER_PREFIX: &str = "<div style=\"display: inline;";

/// Transform ghostty's HTML formatter output into a pango-markup
/// string suitable for [`gtk4::Label::set_markup`].
///
/// The input is expected to be the entire HTML output as produced
/// by libghostty-vt; any deviation from the documented shape falls
/// through to a plain-text fallback (everything as-is inside a
/// `<span>` with monospace hint).
pub fn html_to_pango_markup(html: &str) -> String {
    // Strip the outer `<div style="font-family: monospace; ...">`
    // wrapper. If ghostty ever changes the wrapper shape, fall
    // back to pass-through.
    let inner = html
        .strip_prefix(OUTER_OPEN)
        .and_then(|s| s.strip_suffix(OUTER_CLOSE))
        .unwrap_or(html);

    let mut out = String::with_capacity(inner.len() + 16);
    let mut remaining = inner;

    while let Some(start) = remaining.find(INNER_PREFIX) {
        // Emit any plain text before the inline div.
        out.push_str(&remaining[..start]);
        remaining = &remaining[start + INNER_PREFIX.len()..];

        // Find the end of the opening tag (`">`).
        let Some(tag_end) = remaining.find("\">") else {
            out.push_str(remaining);
            return out;
        };
        let style_attr = &remaining[..tag_end];
        remaining = &remaining[tag_end + 2..];

        // Emit the translated opening span.
        let span_open = style_to_span_open(style_attr);
        out.push_str(&span_open);

        // Emit the run content up to the matching `</div>`.
        if let Some(close_idx) = remaining.find("</div>") {
            out.push_str(&remaining[..close_idx]);
            out.push_str("</span>");
            remaining = &remaining[close_idx + "</div>".len()..];
        } else {
            // Malformed input: no closing tag. Emit what we have
            // and bail.
            out.push_str(remaining);
            out.push_str("</span>");
            return out;
        }
    }

    // Trailing plain text.
    out.push_str(remaining);
    out
}

/// Translate a semicolon-separated CSS style attribute (the part
/// after `display: inline;` and before `"`) into an opening pango
/// `<span ...>` tag.
fn style_to_span_open(style_attr: &str) -> String {
    let mut span = String::from("<span");
    for raw_prop in style_attr.split(';') {
        let prop = raw_prop.trim();
        if prop.is_empty() {
            continue;
        }

        if let Some(value) = prop.strip_prefix("color: ")
            && let Some(color) = parse_css_color(value.trim())
        {
            let _ = write!(span, " foreground=\"{color}\"");
        } else if let Some(value) = prop.strip_prefix("background-color: ")
            && let Some(color) = parse_css_color(value.trim())
        {
            let _ = write!(span, " background=\"{color}\"");
        } else if prop == "font-weight: bold" {
            span.push_str(" weight=\"bold\"");
        } else if prop == "font-style: italic" {
            span.push_str(" style=\"italic\"");
        } else if prop == "text-decoration-line: underline" {
            span.push_str(" underline=\"single\"");
        }
        // text-decoration-style and other informational props are
        // intentionally ignored.
    }
    span.push('>');
    span
}

/// Parse a CSS color expression into a `#rrggbb` string.
///
/// Supports:
/// - `var(--vt-palette-N)` for N in 0..=255
/// - `rgb(R, G, B)` where each component is 0..=255
/// - `#rrggbb` literal (pass-through)
fn parse_css_color(css: &str) -> Option<String> {
    if let Some(rest) = css.strip_prefix("var(--vt-palette-") {
        let n: usize = rest.strip_suffix(')')?.parse().ok()?;
        return Some(palette_color(n));
    }
    if let Some(rest) = css.strip_prefix("rgb(") {
        let rest = rest.strip_suffix(')')?;
        let parts: Vec<&str> = rest.split(',').map(str::trim).collect();
        if parts.len() != 3 {
            return None;
        }
        let r: u8 = parts[0].parse().ok()?;
        let g: u8 = parts[1].parse().ok()?;
        let b: u8 = parts[2].parse().ok()?;
        return Some(format!("#{r:02x}{g:02x}{b:02x}"));
    }
    if css.starts_with('#') && css.len() == 7 {
        // Pass-through hex literal, already in pango's shape.
        return Some(css.to_string());
    }
    None
}

/// Map a 256-color palette index to a `#rrggbb` string.
///
/// Uses the Tango-derived ANSI 16 palette for indices 0..=15, the
/// standard xterm 6×6×6 cube for 16..=231, and the 24-step
/// grayscale ramp for 232..=255.
fn palette_color(n: usize) -> String {
    match n {
        // ANSI 16 (Tango-ish, readable on both light and dark).
        0 => "#000000".into(),  // black
        1 => "#cc0000".into(),  // red
        2 => "#4e9a06".into(),  // green
        3 => "#c4a000".into(),  // yellow
        4 => "#3465a4".into(),  // blue
        5 => "#75507b".into(),  // magenta
        6 => "#06989a".into(),  // cyan
        7 => "#d3d7cf".into(),  // light grey / white
        8 => "#555753".into(),  // dark grey
        9 => "#ef2929".into(),  // bright red
        10 => "#8ae234".into(), // bright green
        11 => "#fce94f".into(), // bright yellow
        12 => "#729fcf".into(), // bright blue
        13 => "#ad7fa8".into(), // bright magenta
        14 => "#34e2e2".into(), // bright cyan
        15 => "#eeeeec".into(), // white
        16..=231 => {
            // 6×6×6 cube: scaled = {0, 95, 135, 175, 215, 255}
            let idx = n - 16;
            let r = (idx / 36) % 6;
            let g = (idx / 6) % 6;
            let b = idx % 6;
            // Known range: x ∈ 0..6 so x * 40 + 55 ≤ 255, safe u8 cast.
            let scale = |x: usize| -> u8 {
                if x == 0 {
                    0
                } else {
                    u8::try_from(x * 40 + 55).unwrap_or(u8::MAX)
                }
            };
            format!("#{:02x}{:02x}{:02x}", scale(r), scale(g), scale(b))
        }
        232..=255 => {
            // Known range: n ∈ 232..=255, so v ∈ 8..=238 ≤ 255.
            let v = 8 + (n - 232) * 10;
            let v_u8 = u8::try_from(v).unwrap_or(u8::MAX);
            format!("#{v_u8:02x}{v_u8:02x}{v_u8:02x}")
        }
        _ => "#ffffff".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_outer_wrapper_and_passes_plain_text() {
        let html = r#"<div style="font-family: monospace; white-space: pre;">hello world</div>"#;
        assert_eq!(html_to_pango_markup(html), "hello world");
    }

    #[test]
    fn translates_palette_foreground_color() {
        let html = r#"<div style="font-family: monospace; white-space: pre;">before <div style="display: inline;color: var(--vt-palette-1);">red</div> after</div>"#;
        let out = html_to_pango_markup(html);
        assert_eq!(out, "before <span foreground=\"#cc0000\">red</span> after");
    }

    #[test]
    fn translates_rgb_color() {
        let html = r#"<div style="font-family: monospace; white-space: pre;"><div style="display: inline;color: rgb(100, 200, 50);">rgb</div></div>"#;
        let out = html_to_pango_markup(html);
        assert_eq!(out, "<span foreground=\"#64c832\">rgb</span>");
    }

    #[test]
    fn translates_bold_italic_underline_combo() {
        let html = r#"<div style="font-family: monospace; white-space: pre;"><div style="display: inline;color: var(--vt-palette-1);background-color: var(--vt-palette-4);font-weight: bold;">bold red on blue</div></div>"#;
        let out = html_to_pango_markup(html);
        assert_eq!(
            out,
            "<span foreground=\"#cc0000\" background=\"#3465a4\" weight=\"bold\">bold red on blue</span>"
        );
    }

    #[test]
    fn underline_decoration_maps_to_pango_underline() {
        let html = r#"<div style="font-family: monospace; white-space: pre;"><div style="display: inline;text-decoration-line: underline;text-decoration-style: solid;">x</div></div>"#;
        let out = html_to_pango_markup(html);
        assert_eq!(out, "<span underline=\"single\">x</span>");
    }

    #[test]
    fn italic_span() {
        let html = r#"<div style="font-family: monospace; white-space: pre;"><div style="display: inline;font-style: italic;">i</div></div>"#;
        let out = html_to_pango_markup(html);
        assert_eq!(out, "<span style=\"italic\">i</span>");
    }

    #[test]
    fn preserves_html_entities_in_text() {
        let html =
            r#"<div style="font-family: monospace; white-space: pre;">A&lt;B&amp;C&gt;D</div>"#;
        assert_eq!(html_to_pango_markup(html), "A&lt;B&amp;C&gt;D");
    }

    #[test]
    fn palette_cube_index_208_is_xterm_orange() {
        // xterm 256 color index 208 = (5, 2, 0) in the 6×6×6 cube
        // → r=255, g=135, b=0.
        assert_eq!(palette_color(208), "#ff8700");
    }

    #[test]
    fn palette_grayscale_ramp_end() {
        assert_eq!(palette_color(232), "#080808");
        // 232+23 = 255 → v = 8 + 23*10 = 238.
        assert_eq!(palette_color(255), "#eeeeee");
    }

    #[test]
    fn handles_multiple_sequential_runs() {
        let html = r#"<div style="font-family: monospace; white-space: pre;">one <div style="display: inline;color: var(--vt-palette-1);">red</div> two <div style="display: inline;color: var(--vt-palette-2);">green</div> three</div>"#;
        let out = html_to_pango_markup(html);
        assert_eq!(
            out,
            "one <span foreground=\"#cc0000\">red</span> two <span foreground=\"#4e9a06\">green</span> three"
        );
    }

    #[test]
    fn unknown_css_property_is_silently_dropped() {
        let html = r#"<div style="font-family: monospace; white-space: pre;"><div style="display: inline;quantum-spin: up;color: var(--vt-palette-2);">q</div></div>"#;
        let out = html_to_pango_markup(html);
        assert_eq!(out, "<span foreground=\"#4e9a06\">q</span>");
    }

    #[test]
    fn malformed_input_falls_back_to_passthrough() {
        let html = "not ghostty output at all";
        assert_eq!(html_to_pango_markup(html), "not ghostty output at all");
    }
}
