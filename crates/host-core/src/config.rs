//! Mugenkai user configuration.
//!
//! Read from `$XDG_CONFIG_HOME/mugenkai/config` (falling back to
//! `~/.config/mugenkai/config`) in a simple `key = value` DSL
//! modeled after ghostty's config format. Unknown keys are logged
//! and ignored; malformed values fall back to compile-time
//! defaults so a broken config never bricks the terminal.
//!
//! # Example
//!
//! ```text
//! # ~/.config/mugenkai/config
//!
//! # ─── keybindings ─────────────────────────────────────────
//! mugenkai-keybinding-quit              = ctrl+q
//! mugenkai-keybinding-split-horizontal  = ctrl+shift+d
//! mugenkai-keybinding-split-vertical    = ctrl+shift+r
//! mugenkai-keybinding-close-pane        = ctrl+shift+w
//! mugenkai-keybinding-focus-next        = ctrl+tab
//! mugenkai-keybinding-focus-prev        = ctrl+shift+tab
//! mugenkai-keybinding-new-tab           = ctrl+shift+t
//! mugenkai-keybinding-close-tab         = ctrl+shift+q
//! mugenkai-keybinding-next-tab          = ctrl+page_down
//! mugenkai-keybinding-prev-tab          = ctrl+page_up
//!
//! # ─── cursor ──────────────────────────────────────────────
//! mugenkai-cursor-style                 = block
//! mugenkai-cursor-blink                 = true
//! mugenkai-cursor-blink-interval-ms     = 530
//! mugenkai-cursor-color                 = #d3d7cf
//! mugenkai-cursor-text-color            = #000000
//!
//! # ─── panes ───────────────────────────────────────────────
//! mugenkai-pane-focus-ring              = true
//! mugenkai-pane-focus-ring-color        = #4c71f2
//! mugenkai-pane-split-ratio             = 0.5
//! ```
//!
//! # API
//!
//! - [`MugenkaiConfig::default`] — compile-time defaults
//! - [`MugenkaiConfig::load`] — load from the standard XDG path,
//!   falling back to defaults on missing file
//! - [`MugenkaiConfig::load_from_path`] — load from an explicit path
//! - [`MugenkaiConfig::parse_str`] — parse an in-memory string

use std::path::{Path, PathBuf};

/// The full user configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct MugenkaiConfig {
    pub keybindings: KeyBindings,
    pub cursor: CursorConfig,
    pub pane: PaneConfig,
    pub font: FontConfig,
    /// Active theme preset name. Swaps the `--mg-*` CSS variables
    /// at runtime when the user picks a different theme in Settings
    /// or the onboarding wizard. One of the names returned by
    /// `host_ui::css::available_themes()`.
    pub theme: String,
}

impl Default for MugenkaiConfig {
    fn default() -> Self {
        Self {
            keybindings: KeyBindings::default(),
            cursor: CursorConfig::default(),
            pane: PaneConfig::default(),
            font: FontConfig::default(),
            theme: "mugenkai-dark".into(),
        }
    }
}

/// Typography settings for the monospace terminal grid.
///
/// Applied to the selectable label that renders the pty output
/// via a pango font description. Changing `family` or `size_pt`
/// and reloading the pane forces the resize watcher to re-measure
/// the cell grid so `cols`/`rows` tracks the new glyph metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct FontConfig {
    pub family: String,
    pub size_pt: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            // `monospace` picks up whatever the user's system has
            // configured as their monospace font — typically the
            // distro's default (e.g. DejaVu Sans Mono on Ubuntu,
            // JetBrains Mono if the user installed it).
            family: "monospace".into(),
            size_pt: 11.0,
        }
    }
}

/// Keybinding accelerators in a simple "ctrl+shift+KEY" format.
///
/// The string stored here is the human-readable form from the
/// config file. Translation to GTK's `<Primary><Shift>KEY` format
/// happens at the host-ui boundary via [`to_gtk_accelerator`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBindings {
    pub quit: String,
    pub split_horizontal: String,
    pub split_vertical: String,
    pub close_pane: String,
    pub focus_next: String,
    pub focus_prev: String,
    pub new_tab: String,
    pub close_tab: String,
    pub next_tab: String,
    pub prev_tab: String,
    pub open_settings: String,
    pub copy: String,
    pub paste: String,
    pub open_chat: String,
    pub explain_in_chat: String,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            quit: "ctrl+q".into(),
            split_horizontal: "ctrl+shift+d".into(),
            split_vertical: "ctrl+shift+r".into(),
            close_pane: "ctrl+shift+w".into(),
            focus_next: "ctrl+tab".into(),
            focus_prev: "ctrl+shift+tab".into(),
            new_tab: "ctrl+shift+t".into(),
            close_tab: "ctrl+shift+q".into(),
            next_tab: "ctrl+page_down".into(),
            prev_tab: "ctrl+page_up".into(),
            open_settings: "ctrl+comma".into(),
            copy: "ctrl+shift+c".into(),
            paste: "ctrl+shift+v".into(),
            open_chat: "ctrl+shift+a".into(),
            explain_in_chat: "ctrl+shift+e".into(),
        }
    }
}

/// Cursor appearance and blink behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorConfig {
    pub style: CursorStyle,
    pub blink: bool,
    pub blink_interval_ms: u64,
    pub color: String,      // "#rrggbb"
    pub text_color: String, // "#rrggbb"
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: CursorStyle::Block,
            blink: true,
            blink_interval_ms: 530,
            color: "#d3d7cf".into(),
            text_color: "#000000".into(),
        }
    }
}

/// Cursor drawing style.
///
/// The label-based renderer in v0.1 supports `Block` (pango
/// background override) and `Underline` (pango underline
/// attribute). `Bar` (vertical line) is reserved for the v0.2
/// `DrawingArea` renderer where we can draw it cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Underline,
}

impl CursorStyle {
    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "block" => Some(Self::Block),
            "underline" => Some(Self::Underline),
            _ => None,
        }
    }
}

/// Pane appearance + layout behavior.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneConfig {
    pub focus_ring: bool,
    pub focus_ring_color: String,
    pub split_ratio: f32,
}

impl Default for PaneConfig {
    fn default() -> Self {
        Self {
            focus_ring: true,
            // Very dark navy (~ Tailwind blue-950). Stands out
            // against both light and dark terminal themes as
            // clearly "window chrome" rather than competing with
            // terminal content.
            focus_ring_color: "#1e40af".into(),
            split_ratio: 0.5,
        }
    }
}

impl MugenkaiConfig {
    /// Load config from the standard XDG path, falling back to
    /// defaults if the file does not exist.
    ///
    /// Path resolution:
    /// - `$XDG_CONFIG_HOME/mugenkai/config` if `$XDG_CONFIG_HOME` is set
    /// - `$HOME/.config/mugenkai/config` otherwise
    #[must_use]
    pub fn load() -> Self {
        let Some(path) = Self::default_path() else {
            return Self::default();
        };
        Self::load_from_path(&path)
    }

    /// Compute the default config file path without reading it.
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
            && !xdg.is_empty()
        {
            return Some(PathBuf::from(xdg).join("mugenkai").join("config"));
        }
        let home = std::env::var("HOME").ok()?;
        Some(
            PathBuf::from(home)
                .join(".config")
                .join("mugenkai")
                .join("config"),
        )
    }

    /// Serialize the config back into the same `key = value`
    /// DSL format that [`parse_str`](Self::parse_str) reads.
    ///
    /// Used by the in-terminal Settings tab to persist user
    /// edits. The output is stable — re-parsing the result
    /// yields a config equal to the original.
    #[must_use]
    pub fn to_dsl_string(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::with_capacity(1024);
        out.push_str("# mugenkai configuration\n");
        out.push_str("# Written by the in-terminal Settings tab.\n");
        out.push_str("# See `mugenkai --help` (coming) or docs/configuration.md.\n");
        out.push('\n');

        out.push_str("# ─── keybindings ─────────────────────────────────────────\n");
        let kb = &self.keybindings;
        for (key, value) in [
            ("mugenkai-keybinding-quit", &kb.quit),
            ("mugenkai-keybinding-split-horizontal", &kb.split_horizontal),
            ("mugenkai-keybinding-split-vertical", &kb.split_vertical),
            ("mugenkai-keybinding-close-pane", &kb.close_pane),
            ("mugenkai-keybinding-focus-next", &kb.focus_next),
            ("mugenkai-keybinding-focus-prev", &kb.focus_prev),
            ("mugenkai-keybinding-new-tab", &kb.new_tab),
            ("mugenkai-keybinding-close-tab", &kb.close_tab),
            ("mugenkai-keybinding-next-tab", &kb.next_tab),
            ("mugenkai-keybinding-prev-tab", &kb.prev_tab),
            ("mugenkai-keybinding-open-settings", &kb.open_settings),
            ("mugenkai-keybinding-copy", &kb.copy),
            ("mugenkai-keybinding-paste", &kb.paste),
            ("mugenkai-keybinding-open-chat", &kb.open_chat),
            ("mugenkai-keybinding-explain-in-chat", &kb.explain_in_chat),
        ] {
            let _ = writeln!(out, "{key:<48} = {value}");
        }
        out.push('\n');

        out.push_str("# ─── cursor ──────────────────────────────────────────────\n");
        let cursor_style = match self.cursor.style {
            CursorStyle::Block => "block",
            CursorStyle::Underline => "underline",
        };
        let _ = writeln!(out, "{:<48} = {}", "mugenkai-cursor-style", cursor_style);
        let _ = writeln!(
            out,
            "{:<48} = {}",
            "mugenkai-cursor-blink", self.cursor.blink
        );
        let _ = writeln!(
            out,
            "{:<48} = {}",
            "mugenkai-cursor-blink-interval-ms", self.cursor.blink_interval_ms
        );
        let _ = writeln!(
            out,
            "{:<48} = {}",
            "mugenkai-cursor-color", self.cursor.color
        );
        let _ = writeln!(
            out,
            "{:<48} = {}",
            "mugenkai-cursor-text-color", self.cursor.text_color
        );
        out.push('\n');

        out.push_str("# ─── panes ───────────────────────────────────────────────\n");
        let _ = writeln!(
            out,
            "{:<48} = {}",
            "mugenkai-pane-focus-ring", self.pane.focus_ring
        );
        let _ = writeln!(
            out,
            "{:<48} = {}",
            "mugenkai-pane-focus-ring-color", self.pane.focus_ring_color
        );
        let _ = writeln!(
            out,
            "{:<48} = {}",
            "mugenkai-pane-split-ratio", self.pane.split_ratio
        );
        out.push('\n');

        out.push_str("# ─── theme ───────────────────────────────────────────────\n");
        let _ = writeln!(out, "{:<48} = {}", "mugenkai-theme", self.theme);
        out.push('\n');

        out.push_str("# ─── font ────────────────────────────────────────────────\n");
        let _ = writeln!(out, "{:<48} = {}", "mugenkai-font-family", self.font.family);
        let _ = writeln!(
            out,
            "{:<48} = {}",
            "mugenkai-font-size-pt", self.font.size_pt
        );

        out
    }

    /// Write the config to its default XDG path, creating
    /// parent directories as needed.
    ///
    /// # Errors
    ///
    /// Returns an `std::io::Error` if the parent directory can't
    /// be created or the write fails.
    pub fn write_to_default_path(&self) -> std::io::Result<PathBuf> {
        let path = Self::default_path().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no HOME or XDG_CONFIG_HOME set; cannot locate config path",
            )
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, self.to_dsl_string())?;
        Ok(path)
    }

    /// Load config from an explicit path. Non-existent files
    /// resolve to defaults; read/parse errors log a warning to
    /// stderr and fall back to defaults.
    #[must_use]
    pub fn load_from_path(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => Self::parse_str(&contents),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                eprintln!(
                    "[mugenkai] failed to read config at {}: {err}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Parse a config file's contents as a string. Unknown keys
    /// and malformed values log warnings and fall through to
    /// defaults for their respective fields.
    #[must_use]
    pub fn parse_str(contents: &str) -> Self {
        let mut cfg = Self::default();
        for (line_no, raw_line) in contents.lines().enumerate() {
            let line_no = line_no + 1;

            // Strip comments and trim.
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }

            // Split `key = value`.
            let Some((key, value)) = line.split_once('=') else {
                eprintln!(
                    "[mugenkai] config line {line_no}: expected 'key = value', got: {raw_line}"
                );
                continue;
            };
            let key = key.trim();
            let value = value.trim();

            cfg.apply(key, value, line_no);
        }
        cfg
    }

    #[allow(clippy::too_many_lines)]
    fn apply(&mut self, key: &str, value: &str, line_no: usize) {
        match key {
            // ─── keybindings ─────────────────────────────────
            "mugenkai-keybinding-quit" => self.keybindings.quit = value.into(),
            "mugenkai-keybinding-split-horizontal" => {
                self.keybindings.split_horizontal = value.into();
            }
            "mugenkai-keybinding-split-vertical" => {
                self.keybindings.split_vertical = value.into();
            }
            "mugenkai-keybinding-close-pane" => self.keybindings.close_pane = value.into(),
            "mugenkai-keybinding-focus-next" => self.keybindings.focus_next = value.into(),
            "mugenkai-keybinding-focus-prev" => self.keybindings.focus_prev = value.into(),
            "mugenkai-keybinding-new-tab" => self.keybindings.new_tab = value.into(),
            "mugenkai-keybinding-close-tab" => self.keybindings.close_tab = value.into(),
            "mugenkai-keybinding-next-tab" => self.keybindings.next_tab = value.into(),
            "mugenkai-keybinding-prev-tab" => self.keybindings.prev_tab = value.into(),
            "mugenkai-keybinding-open-settings" => self.keybindings.open_settings = value.into(),
            "mugenkai-keybinding-copy" => self.keybindings.copy = value.into(),
            "mugenkai-keybinding-paste" => self.keybindings.paste = value.into(),
            "mugenkai-keybinding-open-chat" => self.keybindings.open_chat = value.into(),
            "mugenkai-keybinding-explain-in-chat" => {
                self.keybindings.explain_in_chat = value.into();
            }

            // ─── cursor ──────────────────────────────────────
            "mugenkai-cursor-style" => {
                if let Some(style) = CursorStyle::parse(value) {
                    self.cursor.style = style;
                } else {
                    warn_invalid(line_no, key, value);
                }
            }
            "mugenkai-cursor-blink" => {
                if let Some(b) = parse_bool(value) {
                    self.cursor.blink = b;
                } else {
                    warn_invalid(line_no, key, value);
                }
            }
            "mugenkai-cursor-blink-interval-ms" => {
                if let Ok(n) = value.parse::<u64>() {
                    self.cursor.blink_interval_ms = n;
                } else {
                    warn_invalid(line_no, key, value);
                }
            }
            "mugenkai-cursor-color" => {
                if is_hex_color(value) {
                    self.cursor.color = value.into();
                } else {
                    warn_invalid(line_no, key, value);
                }
            }
            "mugenkai-cursor-text-color" => {
                if is_hex_color(value) {
                    self.cursor.text_color = value.into();
                } else {
                    warn_invalid(line_no, key, value);
                }
            }

            // ─── panes ───────────────────────────────────────
            "mugenkai-pane-focus-ring" => {
                if let Some(b) = parse_bool(value) {
                    self.pane.focus_ring = b;
                } else {
                    warn_invalid(line_no, key, value);
                }
            }
            "mugenkai-pane-focus-ring-color" => {
                if is_hex_color(value) {
                    self.pane.focus_ring_color = value.into();
                } else {
                    warn_invalid(line_no, key, value);
                }
            }
            "mugenkai-pane-split-ratio" => {
                if let Ok(r) = value.parse::<f32>()
                    && (0.05..=0.95).contains(&r)
                {
                    self.pane.split_ratio = r;
                } else {
                    warn_invalid(line_no, key, value);
                }
            }

            // ─── theme ───────────────────────────────────────
            "mugenkai-theme" => self.theme = value.into(),

            // ─── font ────────────────────────────────────────
            "mugenkai-font-family" => {
                if value.is_empty() {
                    warn_invalid(line_no, key, value);
                } else {
                    self.font.family = value.into();
                }
            }
            "mugenkai-font-size-pt" => {
                if let Ok(v) = value.parse::<f32>()
                    && (4.0..=96.0).contains(&v)
                {
                    self.font.size_pt = v;
                } else {
                    warn_invalid(line_no, key, value);
                }
            }

            _ => {
                eprintln!("[mugenkai] config line {line_no}: unknown key '{key}'");
            }
        }
    }
}

/// Translate a human-readable accelerator like `ctrl+shift+d`
/// into GTK's `<Primary><Shift>d` format.
///
/// Returns `None` if the string is empty or contains no key
/// after the modifiers. Unknown modifier tokens are passed
/// through as-is (wrapped in angle brackets with the first
/// letter capitalized) so experimental modifiers can still be
/// spelled by power users.
#[must_use]
pub fn to_gtk_accelerator(accel: &str) -> Option<String> {
    let trimmed = accel.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('+').map(str::trim).collect();
    if parts.is_empty() {
        return None;
    }
    let key = parts.last()?;
    if key.is_empty() {
        return None;
    }
    let mut out = String::new();
    for modifier in &parts[..parts.len() - 1] {
        let m_lower = modifier.to_ascii_lowercase();
        let gtk_name = match m_lower.as_str() {
            "ctrl" | "control" | "primary" => "Primary",
            "shift" => "Shift",
            "alt" | "meta" => "Alt",
            "super" | "cmd" | "win" => "Super",
            other => {
                // Unknown modifier: pass through with leading cap.
                let mut chars = other.chars();
                let first = chars.next()?.to_ascii_uppercase();
                let mut capped = String::new();
                capped.push(first);
                capped.extend(chars);
                return Some(format!("<{capped}>{}", normalize_key(key)));
            }
        };
        out.push('<');
        out.push_str(gtk_name);
        out.push('>');
    }
    out.push_str(&normalize_key(key));
    Some(out)
}

/// Normalize a user-facing key name into GTK's canonical form.
///
/// Single-letter keys lowercase (`D` → `d`). Named keys get
/// their GTK capitalization (`tab` → `Tab`, `page_down` → `Page_Down`).
fn normalize_key(key: &str) -> String {
    if key.len() == 1 {
        return key.to_ascii_lowercase();
    }
    match key.to_ascii_lowercase().as_str() {
        "tab" => "Tab".into(),
        "return" | "enter" => "Return".into(),
        "escape" | "esc" => "Escape".into(),
        "space" => "space".into(),
        "backspace" => "BackSpace".into(),
        "delete" => "Delete".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "page_up" | "pageup" | "pgup" => "Page_Up".into(),
        "page_down" | "pagedown" | "pgdn" | "pgdown" => "Page_Down".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "comma" => "comma".into(),
        "period" | "dot" => "period".into(),
        "semicolon" => "semicolon".into(),
        "apostrophe" | "quote" => "apostrophe".into(),
        "slash" => "slash".into(),
        "backslash" => "backslash".into(),
        "minus" => "minus".into(),
        "equal" | "equals" => "equal".into(),
        "grave" | "backtick" => "grave".into(),
        other => other.to_string(),
    }
}

/// Treat a whole line as a comment if its first non-whitespace
/// character is `#`. Anything else returns unchanged, which keeps
/// hex color values like `#ff8800` intact on the right-hand side
/// of a `key = value` assignment.
///
/// Trailing comments (after a value on the same line) are not
/// supported in v0.1 — the `#` in hex colors makes them
/// ambiguous. Put comments on their own line.
fn strip_comment(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        ""
    } else {
        line
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" | "on" => Some(true),
        "false" | "no" | "0" | "off" => Some(false),
        _ => None,
    }
}

fn is_hex_color(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn warn_invalid(line_no: usize, key: &str, value: &str) {
    eprintln!("[mugenkai] config line {line_no}: invalid value for {key}: {value:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_hardcoded_previous_behavior() {
        let cfg = MugenkaiConfig::default();
        assert_eq!(cfg.keybindings.quit, "ctrl+q");
        assert_eq!(cfg.keybindings.split_horizontal, "ctrl+shift+d");
        assert_eq!(cfg.keybindings.split_vertical, "ctrl+shift+r");
        assert_eq!(cfg.keybindings.close_pane, "ctrl+shift+w");
        assert_eq!(cfg.keybindings.focus_next, "ctrl+tab");
        assert_eq!(cfg.keybindings.focus_prev, "ctrl+shift+tab");
        assert_eq!(cfg.cursor.style, CursorStyle::Block);
        assert!(cfg.cursor.blink);
        assert_eq!(cfg.cursor.blink_interval_ms, 530);
        assert_eq!(cfg.cursor.color, "#d3d7cf");
        assert!(cfg.pane.focus_ring);
        assert_eq!(cfg.pane.focus_ring_color, "#1e40af");
    }

    #[test]
    fn empty_input_returns_defaults() {
        let cfg = MugenkaiConfig::parse_str("");
        assert_eq!(cfg, MugenkaiConfig::default());
    }

    #[test]
    fn whitespace_and_comments_are_ignored() {
        let input = "\n  \n# a comment\n   # another\n";
        assert_eq!(MugenkaiConfig::parse_str(input), MugenkaiConfig::default());
    }

    #[test]
    fn parses_keybinding_overrides() {
        let input = "\
            mugenkai-keybinding-quit = ctrl+escape\n\
            mugenkai-keybinding-split-horizontal = ctrl+shift+return\n\
        ";
        let cfg = MugenkaiConfig::parse_str(input);
        assert_eq!(cfg.keybindings.quit, "ctrl+escape");
        assert_eq!(cfg.keybindings.split_horizontal, "ctrl+shift+return");
    }

    #[test]
    fn parses_cursor_style_block_and_underline() {
        let cfg = MugenkaiConfig::parse_str("mugenkai-cursor-style = block");
        assert_eq!(cfg.cursor.style, CursorStyle::Block);
        let cfg = MugenkaiConfig::parse_str("mugenkai-cursor-style = UNDERLINE");
        assert_eq!(cfg.cursor.style, CursorStyle::Underline);
    }

    #[test]
    fn parses_boolean_variants() {
        for (input, expected) in [
            ("mugenkai-cursor-blink = true", true),
            ("mugenkai-cursor-blink = FALSE", false),
            ("mugenkai-cursor-blink = yes", true),
            ("mugenkai-cursor-blink = no", false),
            ("mugenkai-cursor-blink = 1", true),
            ("mugenkai-cursor-blink = 0", false),
            ("mugenkai-cursor-blink = on", true),
            ("mugenkai-cursor-blink = off", false),
        ] {
            assert_eq!(
                MugenkaiConfig::parse_str(input).cursor.blink,
                expected,
                "input {input}"
            );
        }
    }

    #[test]
    fn parses_blink_interval() {
        let cfg = MugenkaiConfig::parse_str("mugenkai-cursor-blink-interval-ms = 200");
        assert_eq!(cfg.cursor.blink_interval_ms, 200);
    }

    #[test]
    fn rejects_malformed_interval_and_keeps_default() {
        let cfg = MugenkaiConfig::parse_str("mugenkai-cursor-blink-interval-ms = fast");
        assert_eq!(cfg.cursor.blink_interval_ms, 530);
    }

    #[test]
    fn parses_hex_colors() {
        let cfg = MugenkaiConfig::parse_str("mugenkai-cursor-color = #ff8800");
        assert_eq!(cfg.cursor.color, "#ff8800");
    }

    #[test]
    fn rejects_non_hex_colors() {
        let cfg = MugenkaiConfig::parse_str("mugenkai-cursor-color = red");
        assert_eq!(cfg.cursor.color, "#d3d7cf"); // default retained
    }

    #[test]
    fn parses_split_ratio_within_bounds() {
        let cfg = MugenkaiConfig::parse_str("mugenkai-pane-split-ratio = 0.3");
        assert!((cfg.pane.split_ratio - 0.3).abs() < 1e-6);
    }

    #[test]
    fn rejects_out_of_range_split_ratio() {
        let cfg = MugenkaiConfig::parse_str("mugenkai-pane-split-ratio = 1.5");
        assert!((cfg.pane.split_ratio - 0.5).abs() < 1e-6);
    }

    #[test]
    fn unknown_keys_are_ignored_without_affecting_parse() {
        let cfg = MugenkaiConfig::parse_str(
            "\
            mugenkai-unknown-thing = hello\n\
            mugenkai-cursor-blink = false\n\
        ",
        );
        assert!(!cfg.cursor.blink);
    }

    #[test]
    fn trailing_comment_is_not_stripped_to_avoid_hex_ambiguity() {
        // Trailing comments conflict with hex color values
        // (`# ff8800` looks like a comment, `#ff8800` does not).
        // For v0.1 we simply don't support trailing comments —
        // put them on their own line. A line that ends with a
        // stray `# ...` turns the whole value into garbage and
        // falls through to the default.
        let cfg = MugenkaiConfig::parse_str("mugenkai-cursor-blink = false  # turn off blinking");
        // Parsed value is "false  # turn off blinking" which
        // isn't a valid bool; defaults to blink=true.
        assert!(cfg.cursor.blink);
    }

    #[test]
    fn to_gtk_accelerator_primary_q() {
        assert_eq!(to_gtk_accelerator("ctrl+q").unwrap(), "<Primary>q");
    }

    #[test]
    fn to_gtk_accelerator_primary_shift_d() {
        assert_eq!(
            to_gtk_accelerator("ctrl+shift+d").unwrap(),
            "<Primary><Shift>d"
        );
    }

    #[test]
    fn to_gtk_accelerator_primary_shift_tab() {
        assert_eq!(
            to_gtk_accelerator("ctrl+shift+tab").unwrap(),
            "<Primary><Shift>Tab"
        );
    }

    #[test]
    fn to_gtk_accelerator_page_down() {
        assert_eq!(
            to_gtk_accelerator("ctrl+page_down").unwrap(),
            "<Primary>Page_Down"
        );
    }

    #[test]
    fn to_gtk_accelerator_alt_return() {
        assert_eq!(to_gtk_accelerator("alt+enter").unwrap(), "<Alt>Return");
    }

    #[test]
    fn to_gtk_accelerator_empty_returns_none() {
        assert!(to_gtk_accelerator("").is_none());
        assert!(to_gtk_accelerator("  ").is_none());
    }

    #[test]
    fn to_gtk_accelerator_single_key_no_mods() {
        assert_eq!(to_gtk_accelerator("f11").unwrap(), "f11");
    }

    #[test]
    fn dsl_round_trip_preserves_defaults() {
        let cfg = MugenkaiConfig::default();
        let text = cfg.to_dsl_string();
        let reparsed = MugenkaiConfig::parse_str(&text);
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn dsl_round_trip_preserves_custom_values() {
        let mut cfg = MugenkaiConfig::default();
        cfg.keybindings.quit = "ctrl+escape".into();
        cfg.keybindings.split_horizontal = "alt+shift+right".into();
        cfg.cursor.style = CursorStyle::Underline;
        cfg.cursor.blink = false;
        cfg.cursor.blink_interval_ms = 250;
        cfg.cursor.color = "#ff8800".into();
        cfg.pane.focus_ring = false;
        cfg.pane.focus_ring_color = "#00ff00".into();
        cfg.pane.split_ratio = 0.3;

        let text = cfg.to_dsl_string();
        let reparsed = MugenkaiConfig::parse_str(&text);
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn parse_full_config_file() {
        let input = "\
            # mugenkai config\n\
            \n\
            mugenkai-keybinding-quit              = ctrl+q\n\
            mugenkai-keybinding-split-horizontal  = alt+shift+right\n\
            mugenkai-keybinding-close-pane        = ctrl+shift+x\n\
            \n\
            mugenkai-cursor-style                 = underline\n\
            mugenkai-cursor-blink                 = false\n\
            mugenkai-cursor-blink-interval-ms     = 250\n\
            mugenkai-cursor-color                 = #ff8800\n\
            \n\
            mugenkai-pane-focus-ring              = true\n\
            mugenkai-pane-focus-ring-color        = #00ff00\n\
            mugenkai-pane-split-ratio             = 0.7\n\
        ";
        let cfg = MugenkaiConfig::parse_str(input);
        assert_eq!(cfg.keybindings.quit, "ctrl+q");
        assert_eq!(cfg.keybindings.split_horizontal, "alt+shift+right");
        assert_eq!(cfg.keybindings.close_pane, "ctrl+shift+x");
        assert_eq!(cfg.cursor.style, CursorStyle::Underline);
        assert!(!cfg.cursor.blink);
        assert_eq!(cfg.cursor.blink_interval_ms, 250);
        assert_eq!(cfg.cursor.color, "#ff8800");
        assert!(cfg.pane.focus_ring);
        assert_eq!(cfg.pane.focus_ring_color, "#00ff00");
        assert!((cfg.pane.split_ratio - 0.7).abs() < 1e-6);
    }
}
