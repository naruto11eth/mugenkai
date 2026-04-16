//! Generate a mugenkai-compatible CSS stylesheet from a parsed
//! [`host_core::WarpTheme`].
//!
//! The Warp YAML schema is intentionally minimal: a primary
//! background, a foreground, one accent color, and the 16-color
//! ANSI palette. Mugenkai's design system has a richer token set
//! (`--mg-bg-secondary`, `--mg-bg-tertiary`, hover states, four
//! text levels, accent-on-hover, focus ring color, etc.), so we
//! *derive* the missing tokens from the three Warp inputs using
//! HSL-ish lighten/darken in RGB space. The result is a
//! stylesheet identical in shape to the hardcoded `THEME_*`
//! constants in [`crate::css`], just generated at runtime.
//!
//! # License posture
//!
//! Clean-room. Implemented from the published YAML examples in
//! the MIT-licensed `warpdotdev/themes` repo, not from the
//! AGPL-3.0 source of `warpdotdev/warp`. None of the derivation
//! logic below is borrowed from Warp's renderer (which uses
//! `pathfinder_color::ColorU` and a wgpu-only path).

use host_core::{ThemeDetails, WarpTheme};

/// Build the full `:root { --mg-* }` stylesheet for a parsed
/// Warp theme. Output is byte-equivalent in shape to the
/// existing hardcoded built-in themes — same variable names,
/// same units, same trailing semicolons — so the GTK CSS
/// provider can swap one for the other at runtime without any
/// downstream code change.
#[must_use]
#[allow(dead_code, reason = "wired in by registry plumbing commit")]
pub fn generate_warp_theme_css(theme: &WarpTheme) -> String {
    let bg = parse_hex(&theme.background).unwrap_or([0, 0, 0]);
    let fg = parse_hex(&theme.foreground).unwrap_or([230, 232, 238]);
    let accent = parse_hex(&theme.accent).unwrap_or([74, 255, 0]);

    // Derive secondary/tertiary backgrounds. `details: darker`
    // (the common case for dark themes) means card surfaces
    // should be slightly lighter than the terminal so they
    // stand out; `details: lighter` (light themes) inverts that.
    let (bg_secondary, bg_tertiary, bg_hover) = match theme.details {
        ThemeDetails::Darker => (lighten(bg, 0.04), lighten(bg, 0.08), lighten(bg, 0.12)),
        ThemeDetails::Lighter => (darken(bg, 0.04), darken(bg, 0.08), darken(bg, 0.12)),
    };

    // Text hierarchy — fade toward the background as importance
    // drops. Foreground stays primary; secondary, tertiary,
    // quaternary mix increasing amounts of background into fg.
    let text_secondary = mix(fg, bg, 0.30);
    let text_tertiary = mix(fg, bg, 0.55);
    let text_quaternary = mix(fg, bg, 0.75);

    // Pick a readable on-accent text color — black if the accent
    // is bright, white if it's dark. Same heuristic as web
    // contrast checkers (luminance threshold ~0.55).
    let text_on_accent = if luminance(accent) > 0.55 {
        [0, 0, 0]
    } else {
        [255, 255, 255]
    };
    let accent_hover = lighten(accent, 0.10);

    // Borders sit between bg and bg_tertiary in luminance.
    let border_primary = mix(bg, fg, 0.12);
    let border_subtle = mix(bg, fg, 0.06);

    // ANSI status colors come from the user's palette so e.g.
    // a Solarized theme's reds stay Solarized.
    let status_success = theme.terminal_colors.normal.green.clone();
    let status_warning = theme.terminal_colors.normal.yellow.clone();
    let status_error = theme.terminal_colors.normal.red.clone();

    format!(
        ":root {{
    --mg-bg-primary:    {bg_primary};
    --mg-bg-secondary:  {bg_secondary};
    --mg-bg-tertiary:   {bg_tertiary};
    --mg-bg-hover:      {bg_hover};

    --mg-text-primary:    {text_primary};
    --mg-text-secondary:  {text_secondary};
    --mg-text-tertiary:   {text_tertiary};
    --mg-text-quaternary: {text_quaternary};
    --mg-text-on-accent:  {text_on_accent};

    --mg-accent:        {accent};
    --mg-accent-hover:  {accent_hover};

    --mg-border-primary: {border_primary};
    --mg-border-subtle:  {border_subtle};

    --mg-status-success: {status_success};
    --mg-status-warning: {status_warning};
    --mg-status-error:   {status_error};

    --mg-radius-sm:   4px;
    --mg-radius-md:   6px;
    --mg-radius-lg:   8px;
    --mg-radius-xl:   12px;
    --mg-radius-full: 9999px;

    --mg-space-1: 4px;
    --mg-space-2: 8px;
    --mg-space-3: 12px;
    --mg-space-4: 16px;
    --mg-space-6: 24px;
    --mg-space-8: 32px;
}}",
        bg_primary = format_hex(bg),
        bg_secondary = format_hex(bg_secondary),
        bg_tertiary = format_hex(bg_tertiary),
        bg_hover = format_hex(bg_hover),
        text_primary = format_hex(fg),
        text_secondary = format_hex(text_secondary),
        text_tertiary = format_hex(text_tertiary),
        text_quaternary = format_hex(text_quaternary),
        text_on_accent = format_hex(text_on_accent),
        accent = format_hex(accent),
        accent_hover = format_hex(accent_hover),
        border_primary = format_hex(border_primary),
        border_subtle = format_hex(border_subtle),
    )
}

/// Four-color preview for a Warp theme matching the order used
/// by mugenkai's existing `theme_swatches`: accent, primary bg,
/// foreground, tertiary bg.
#[must_use]
#[allow(dead_code, reason = "wired in by registry plumbing commit")]
pub fn warp_theme_swatches(theme: &WarpTheme) -> [String; 4] {
    let bg = parse_hex(&theme.background).unwrap_or([0, 0, 0]);
    let bg_tertiary = match theme.details {
        ThemeDetails::Darker => lighten(bg, 0.08),
        ThemeDetails::Lighter => darken(bg, 0.08),
    };
    [
        theme.accent.clone(),
        theme.background.clone(),
        theme.foreground.clone(),
        format_hex(bg_tertiary),
    ]
}

/// Color used by the focus ring around the active terminal pane.
#[must_use]
#[allow(dead_code, reason = "wired in by registry plumbing commit")]
pub fn warp_theme_focus_color(theme: &WarpTheme) -> String {
    theme.accent.clone()
}

// ─── color math ──────────────────────────────────────────────
//
// Tiny enough to inline. We deliberately avoid pulling in a
// color crate (`palette`, `csscolorparser`) — those add MB of
// dependencies for what is ~30 lines of arithmetic.

/// Parse `#rrggbb` or `#rgb` into RGB bytes. Returns `None` if
/// the input doesn't look like a hex color so callers can fall
/// back to a sensible default rather than panic on a typoed
/// theme file.
fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let hex = s.strip_prefix('#')?;
    match hex.len() {
        3 => {
            // `#rgb` shorthand — each nibble doubled.
            let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
            Some([r * 17, g * 17, b * 17])
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some([r, g, b])
        }
        _ => None,
    }
}

/// `[r, g, b]` → `"#rrggbb"`.
fn format_hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// Move every channel `factor` of the way toward white.
/// `factor` is clamped to `[0, 1]`.
fn lighten(rgb: [u8; 3], factor: f32) -> [u8; 3] {
    let f = factor.clamp(0.0, 1.0);
    [
        lerp(rgb[0], 255, f),
        lerp(rgb[1], 255, f),
        lerp(rgb[2], 255, f),
    ]
}

/// Move every channel `factor` of the way toward black.
fn darken(rgb: [u8; 3], factor: f32) -> [u8; 3] {
    let f = factor.clamp(0.0, 1.0);
    [
        lerp(rgb[0], 0, f),
        lerp(rgb[1], 0, f),
        lerp(rgb[2], 0, f),
    ]
}

/// Linearly mix two colors. `t=0` returns `a`, `t=1` returns `b`.
fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let f = t.clamp(0.0, 1.0);
    [lerp(a[0], b[0], f), lerp(a[1], b[1], f), lerp(a[2], b[2], f)]
}

/// Single-channel linear interpolation with rounding.
fn lerp(a: u8, b: u8, t: f32) -> u8 {
    let a_f = f32::from(a);
    let b_f = f32::from(b);
    let v = (a_f + (b_f - a_f) * t).round().clamp(0.0, 255.0);
    // `clamp(0, 255)` + `round()` guarantees the value fits a u8;
    // the cast is therefore lossless. We assert the lower bound
    // explicitly so future refactors that touch the clamp can't
    // silently introduce signed-overflow behaviour.
    debug_assert!((0.0..=255.0).contains(&v));
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "value is clamped to [0, 255] above; cast is lossless"
    )]
    let out = v as u8;
    out
}

/// Perceived luminance via the standard sRGB weights. Used to
/// pick black vs white for on-accent text.
fn luminance(rgb: [u8; 3]) -> f32 {
    let r = f32::from(rgb[0]) / 255.0;
    let g = f32::from(rgb[1]) / 255.0;
    let b = f32::from(rgb[2]) / 255.0;
    0.299 * r + 0.587 * g + 0.114 * b
}

#[cfg(test)]
mod tests {
    use super::*;
    use host_core::{AnsiColors, TerminalColors, WarpTheme};

    fn fixture(bg: &str, fg: &str, accent: &str, details: ThemeDetails) -> WarpTheme {
        WarpTheme {
            name: Some("Test".into()),
            background: bg.into(),
            foreground: fg.into(),
            accent: accent.into(),
            details,
            terminal_colors: TerminalColors {
                normal: AnsiColors {
                    black: "#000".into(),
                    red: "#ff0000".into(),
                    green: "#00ff00".into(),
                    yellow: "#ffff00".into(),
                    blue: "#0000ff".into(),
                    magenta: "#ff00ff".into(),
                    cyan: "#00ffff".into(),
                    white: "#ffffff".into(),
                },
                bright: AnsiColors {
                    black: "#000".into(),
                    red: "#ff0000".into(),
                    green: "#00ff00".into(),
                    yellow: "#ffff00".into(),
                    blue: "#0000ff".into(),
                    magenta: "#ff00ff".into(),
                    cyan: "#00ffff".into(),
                    white: "#ffffff".into(),
                },
            },
        }
    }

    #[test]
    fn parse_hex_handles_six_digit() {
        assert_eq!(parse_hex("#1a1b26"), Some([0x1a, 0x1b, 0x26]));
    }

    #[test]
    fn parse_hex_handles_three_digit_shorthand() {
        // `#abc` should be treated like `#aabbcc`.
        assert_eq!(parse_hex("#abc"), Some([0xaa, 0xbb, 0xcc]));
    }

    #[test]
    fn parse_hex_rejects_bad_input() {
        assert_eq!(parse_hex("not a color"), None);
        assert_eq!(parse_hex("#zzz"), None);
        assert_eq!(parse_hex("#12345"), None);
    }

    #[test]
    fn format_hex_lowercases_and_zero_pads() {
        assert_eq!(format_hex([0xab, 0x01, 0xff]), "#ab01ff");
        assert_eq!(format_hex([0, 0, 0]), "#000000");
    }

    #[test]
    fn lighten_at_one_returns_white() {
        assert_eq!(lighten([0x10, 0x20, 0x30], 1.0), [255, 255, 255]);
    }

    #[test]
    fn darken_at_one_returns_black() {
        assert_eq!(darken([0xfe, 0xfd, 0xfc], 1.0), [0, 0, 0]);
    }

    #[test]
    fn lighten_at_zero_is_identity() {
        assert_eq!(lighten([0x40, 0x80, 0xc0], 0.0), [0x40, 0x80, 0xc0]);
    }

    #[test]
    fn luminance_white_is_one() {
        assert!((luminance([255, 255, 255]) - 1.0).abs() < 0.001);
    }

    #[test]
    fn luminance_black_is_zero() {
        assert!(luminance([0, 0, 0]) < 0.001);
    }

    #[test]
    fn dark_theme_picks_white_on_accent_for_dim_accent() {
        // Dim purple accent → on-accent text should be white.
        let theme = fixture("#000000", "#ffffff", "#3b3b75", ThemeDetails::Darker);
        let css = generate_warp_theme_css(&theme);
        assert!(
            css.contains("--mg-text-on-accent:  #ffffff"),
            "expected white text on dim accent, got:\n{css}"
        );
    }

    #[test]
    fn dark_theme_picks_black_on_accent_for_bright_accent() {
        let theme = fixture("#000000", "#ffffff", "#ffff00", ThemeDetails::Darker);
        let css = generate_warp_theme_css(&theme);
        assert!(
            css.contains("--mg-text-on-accent:  #000000"),
            "expected black text on bright accent, got:\n{css}"
        );
    }

    #[test]
    fn css_uses_input_background_verbatim() {
        let theme = fixture("#1a1b26", "#a9b1d6", "#7aa2f7", ThemeDetails::Darker);
        let css = generate_warp_theme_css(&theme);
        assert!(
            css.contains("--mg-bg-primary:    #1a1b26"),
            "expected input bg verbatim, got:\n{css}"
        );
    }

    #[test]
    fn details_darker_lightens_secondary_bg() {
        // For a dark theme (details: darker), secondary should
        // be brighter than primary so cards stand out.
        let theme = fixture("#000000", "#ffffff", "#ff0000", ThemeDetails::Darker);
        let css = generate_warp_theme_css(&theme);
        // Secondary background = lighten(#000000, 0.04) ≈ #0a0a0a.
        assert!(
            css.contains("--mg-bg-secondary:  #0a0a0a"),
            "expected lightened secondary bg, got:\n{css}"
        );
    }

    #[test]
    fn details_lighter_darkens_secondary_bg() {
        // For a light theme (details: lighter), secondary should
        // be slightly darker than primary.
        let theme = fixture("#ffffff", "#000000", "#ff0000", ThemeDetails::Lighter);
        let css = generate_warp_theme_css(&theme);
        // Secondary background = darken(#ffffff, 0.04) ≈ #f5f5f5.
        assert!(
            css.contains("--mg-bg-secondary:  #f5f5f5"),
            "expected darkened secondary bg, got:\n{css}"
        );
    }

    #[test]
    fn swatches_match_picker_contract() {
        let theme = fixture("#1a1b26", "#a9b1d6", "#7aa2f7", ThemeDetails::Darker);
        let s = warp_theme_swatches(&theme);
        assert_eq!(s[0], "#7aa2f7", "slot 0 = accent");
        assert_eq!(s[1], "#1a1b26", "slot 1 = primary bg");
        assert_eq!(s[2], "#a9b1d6", "slot 2 = foreground");
        // slot 3 = derived tertiary bg
        assert!(s[3].starts_with('#') && s[3].len() == 7);
    }
}
