//! Warp-compatible YAML theme schema.
//!
//! Warp ships every theme as a small YAML file like:
//!
//! ```yaml
//! name: 'Tokyo Night'
//! background: '#1a1b26'
//! foreground: '#a9b1d6'
//! accent: '#7aa2f7'
//! details: 'darker'
//! terminal_colors:
//!   normal:
//!     black: '#32344a'
//!     red: '#f7768e'
//!     # ...
//!   bright:
//!     black: '#444b6a'
//!     # ...
//! ```
//!
//! There is a public, MIT-licensed library of hundreds of these
//! at <https://github.com/warpdotdev/themes>. Mugenkai parses the
//! same schema so users can drop those files into
//! `~/.warp/themes/` (or `~/.config/mugenkai/themes/`) and get
//! the full library for free.
//!
//! ## License posture
//!
//! The YAML schema itself is not copyrightable. The reference
//! implementation in `warpdotdev/warp` is AGPL-3.0; this module
//! is a clean-room reimplementation written from the schema +
//! example files in the MIT-licensed `warpdotdev/themes` repo.
//! No code is copied from the AGPL source.
//!
//! ## What we deliberately do NOT support
//!
//! - `background_image` — Warp themes can specify an image overlay.
//!   Mugenkai's CSS pipeline doesn't have an obvious place to put
//!   this; we silently drop the field on parse.
//! - Base16-style `accent` palettes — supported as a single accent
//!   color only.
//! - Nested theme references / inheritance — Warp doesn't do this
//!   either, but the field exists in some forks.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Whether the theme prefers a darker or lighter UI chrome treatment.
///
/// Warp uses this to decide whether sidebars/menus render as
/// "darker than the terminal background" or "lighter." We map it
/// onto the CSS `--mg-bg-secondary` / `--mg-bg-tertiary` vars
/// when generating the mugenkai stylesheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeDetails {
    Darker,
    Lighter,
}

impl Default for ThemeDetails {
    fn default() -> Self {
        Self::Darker
    }
}

/// One row of ANSI 0-7 colors. Used for both the `normal` and
/// `bright` blocks under `terminal_colors`.
#[derive(Debug, Clone, Deserialize)]
pub struct AnsiColors {
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
}

/// `terminal_colors:` block — both ANSI palettes.
#[derive(Debug, Clone, Deserialize)]
pub struct TerminalColors {
    pub normal: AnsiColors,
    pub bright: AnsiColors,
}

/// A parsed Warp-compatible theme file.
///
/// We deliberately keep the parsed shape close to the YAML so
/// mismatches surface as deserialization errors with field
/// context, instead of getting flattened into an opaque
/// "invalid theme" message.
#[derive(Debug, Clone, Deserialize)]
pub struct WarpTheme {
    /// Display name shown in the theme picker. Falls back to the
    /// filename stem at load time if missing.
    #[serde(default)]
    pub name: Option<String>,

    /// Primary terminal background.
    pub background: String,

    /// Default text color on the terminal background.
    pub foreground: String,

    /// Accent / highlight color — used for the focus ring,
    /// suggested-action button background, and the path color
    /// on the prompt.
    pub accent: String,

    /// Whether sidebars / menus should look darker or lighter
    /// than `background`. Defaults to `darker` (matches most of
    /// the existing community themes).
    #[serde(default)]
    pub details: ThemeDetails,

    /// 16-color ANSI palette used to render shell escape codes.
    pub terminal_colors: TerminalColors,
}

/// Errors returned by the [`WarpTheme`] loader. Wraps both the I/O
/// errors from reading theme files and the structured serde
/// parse errors so callers can distinguish "couldn't open the
/// file" from "the file isn't valid YAML."
#[derive(Debug, thiserror::Error)]
pub enum WarpThemeError {
    #[error("could not read theme file {path:?}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse theme file {path:?}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
}

impl WarpTheme {
    /// Parse a YAML string into a [`WarpTheme`]. Used by tests +
    /// directly when the caller already has the bytes in memory.
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml_ng::Error> {
        serde_yaml_ng::from_str(yaml)
    }

    /// Read + parse a theme file off disk. The filename's stem
    /// (e.g. `Tokyo Night.yaml` → `Tokyo Night`) is used as a
    /// fallback name when the file omits the `name:` field, so
    /// the picker never shows an unnamed entry.
    pub fn from_path(path: &Path) -> Result<Self, WarpThemeError> {
        let raw = fs::read_to_string(path).map_err(|source| WarpThemeError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut theme: Self = serde_yaml_ng::from_str(&raw).map_err(|source| {
            WarpThemeError::Parse {
                path: path.to_path_buf(),
                source,
            }
        })?;
        if theme.name.is_none()
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            theme.name = Some(stem.to_string());
        }
        Ok(theme)
    }

    /// Display name, with a sensible fallback if neither the
    /// YAML nor the filename gave us one.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or("Unnamed Warp Theme")
    }

    /// Stable identifier used by the config + picker. We
    /// lowercase + replace whitespace with `-` so `"Tokyo Night"`
    /// becomes `"tokyo-night"` — matches how the existing built-in
    /// themes are keyed (`"mugenkai-dark"`, `"solarized-dark"`).
    #[must_use]
    pub fn slug(&self) -> String {
        let name = self.display_name();
        name.to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_whitespace() { '-' } else { c })
            .collect()
    }
}

/// Recursively-bounded list of well-known theme directories,
/// in priority order. Earlier directories shadow later ones if
/// two themes share a slug — this lets a user override a
/// system-installed theme by dropping a same-named YAML in
/// their `~/.config/mugenkai/themes/`.
#[must_use]
pub fn default_theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(config) = dirs::config_dir() {
        dirs.push(config.join("mugenkai").join("themes"));
    }
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".warp").join("themes"));
    }
    dirs
}

/// Scan the given directories for `*.yaml`/`*.yml` files and
/// parse each as a Warp theme. Files that fail to parse are
/// reported to `on_error` (e.g. for logging) but do NOT abort
/// the whole load — one bad theme should not break the picker.
///
/// Returns the parsed themes in directory-priority order;
/// callers that want to dedupe by slug should walk the result
/// and `insert` into a `HashMap` so earlier entries win.
pub fn load_warp_themes_from_dirs<F>(dirs: &[PathBuf], mut on_error: F) -> Vec<WarpTheme>
where
    F: FnMut(WarpThemeError),
{
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            // Missing directory is fine — the user may not have
            // ~/.warp/themes/ at all. Permission errors fall here
            // too; a logging callback can warn if desired but
            // failure-to-list is not a hard error.
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_yaml = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"));
            if !is_yaml {
                continue;
            }
            match WarpTheme::from_path(&path) {
                Ok(theme) => out.push(theme),
                Err(err) => on_error(err),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical Warp YAML — matches the exact shape Warp ships
    /// in `warpdotdev/themes` so the parser is pinned to real
    /// community themes, not a synthetic fixture we control.
    const TOKYO_NIGHT: &str = "
name: 'Tokyo Night'
background: '#1a1b26'
foreground: '#a9b1d6'
accent: '#7aa2f7'
details: darker
terminal_colors:
  normal:
    black:   '#32344a'
    red:     '#f7768e'
    green:   '#9ece6a'
    yellow:  '#e0af68'
    blue:    '#7aa2f7'
    magenta: '#ad8ee6'
    cyan:    '#449dab'
    white:   '#787c99'
  bright:
    black:   '#444b6a'
    red:     '#ff7a93'
    green:   '#b9f27c'
    yellow:  '#ff9e64'
    blue:    '#7da6ff'
    magenta: '#bb9af7'
    cyan:    '#0db9d7'
    white:   '#acb0d0'
";

    #[test]
    fn parses_canonical_warp_yaml() {
        let theme = WarpTheme::from_yaml(TOKYO_NIGHT).expect("parse");
        assert_eq!(theme.display_name(), "Tokyo Night");
        assert_eq!(theme.background, "#1a1b26");
        assert_eq!(theme.accent, "#7aa2f7");
        assert_eq!(theme.details, ThemeDetails::Darker);
        assert_eq!(theme.terminal_colors.normal.green, "#9ece6a");
        assert_eq!(theme.terminal_colors.bright.magenta, "#bb9af7");
    }

    #[test]
    fn slug_lowercases_and_dashes_whitespace() {
        let theme = WarpTheme::from_yaml(TOKYO_NIGHT).expect("parse");
        assert_eq!(theme.slug(), "tokyo-night");
    }

    #[test]
    fn defaults_details_to_darker_when_missing() {
        let yaml = "
name: 'No Details'
background: '#000000'
foreground: '#ffffff'
accent: '#ff0000'
terminal_colors:
  normal: { black: '#0', red: '#0', green: '#0', yellow: '#0', blue: '#0', magenta: '#0', cyan: '#0', white: '#0' }
  bright: { black: '#0', red: '#0', green: '#0', yellow: '#0', blue: '#0', magenta: '#0', cyan: '#0', white: '#0' }
";
        let theme = WarpTheme::from_yaml(yaml).expect("parse");
        assert_eq!(theme.details, ThemeDetails::Darker);
    }

    #[test]
    fn rejects_missing_required_field() {
        // No `accent:` — must surface as a parse error rather
        // than silently default. Catches typos like `acent:`.
        let yaml = "
name: 'Bad'
background: '#000000'
foreground: '#ffffff'
terminal_colors:
  normal: { black: '#0', red: '#0', green: '#0', yellow: '#0', blue: '#0', magenta: '#0', cyan: '#0', white: '#0' }
  bright: { black: '#0', red: '#0', green: '#0', yellow: '#0', blue: '#0', magenta: '#0', cyan: '#0', white: '#0' }
";
        let err = WarpTheme::from_yaml(yaml).expect_err("missing accent must fail");
        let msg = err.to_string();
        assert!(msg.contains("accent"), "error did not mention `accent`: {msg}");
    }

    #[test]
    fn from_path_uses_filename_stem_when_name_missing() {
        let dir = tempdir();
        let path = dir.join("My Cool Theme.yaml");
        std::fs::write(
            &path,
            "
background: '#000000'
foreground: '#ffffff'
accent: '#ff0000'
terminal_colors:
  normal: { black: '#0', red: '#0', green: '#0', yellow: '#0', blue: '#0', magenta: '#0', cyan: '#0', white: '#0' }
  bright: { black: '#0', red: '#0', green: '#0', yellow: '#0', blue: '#0', magenta: '#0', cyan: '#0', white: '#0' }
",
        )
        .expect("write fixture");
        let theme = WarpTheme::from_path(&path).expect("load");
        assert_eq!(theme.display_name(), "My Cool Theme");
        assert_eq!(theme.slug(), "my-cool-theme");
    }

    #[test]
    fn load_warp_themes_from_dirs_skips_non_yaml_and_continues_on_error() {
        let dir = tempdir();
        // Valid theme.
        std::fs::write(dir.join("Good.yaml"), TOKYO_NIGHT).expect("write good");
        // Wrong extension — should be ignored, not parsed.
        std::fs::write(dir.join("notes.txt"), "hello").expect("write txt");
        // Invalid YAML — should surface via the on_error callback
        // but must not abort the whole load.
        std::fs::write(dir.join("Broken.yaml"), "this: is: not: yaml").expect("write broken");

        let mut errors: Vec<WarpThemeError> = Vec::new();
        let themes = load_warp_themes_from_dirs(std::slice::from_ref(&dir), |e| errors.push(e));

        assert_eq!(themes.len(), 1, "exactly one valid theme expected");
        assert_eq!(themes[0].slug(), "tokyo-night");
        assert_eq!(errors.len(), 1, "broken file should produce one error");
        assert!(matches!(errors[0], WarpThemeError::Parse { .. }));
    }

    /// Tiny helper for I/O smoke tests above.
    ///
    /// `std::env::temp_dir` plus a per-test subdirectory keeps
    /// dev-deps lean and avoids pulling in `tempfile`. Cleanup
    /// runs at process exit; for unit tests that's fine.
    fn tempdir() -> PathBuf {
        let id = format!(
            "mugenkai-warp-theme-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let dir = std::env::temp_dir().join(id);
        std::fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }
}
