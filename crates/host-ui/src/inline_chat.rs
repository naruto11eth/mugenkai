//! Inline chat widget embedded at the bottom of each terminal pane.
//!
//! Unlike the full Chat tab which maintains a persistent
//! conversation, the inline chat is lightweight — it shows the
//! last exchange (user question + Claude's answer) and disappears
//! when dismissed. Think of it as "quick ask" without leaving the
//! terminal.
//!
//! # Layout
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │  terminal output (PaneWidget)       │
//! │                                     │
//! ├─────────────────────────────────────┤
//! │  ┌ inline chat ───────────────────┐ │
//! │  │ [response area — streaming]    │ │
//! │  │ [input entry] [Send]           │ │
//! │  └────────────────────────────────┘ │
//! └─────────────────────────────────────┘
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk4::glib;

use crate::daemon_client;

/// Per-pane inline chat widget.
///
/// Constructed once per `PaneWidget` and toggled visible/hidden
/// via [`toggle`](Self::toggle). Holds its own mini conversation
/// history (just the visible exchange, not the full history).
type TerminalTextFn = Box<dyn Fn() -> String>;
type SendToTermFn = Box<dyn Fn(&str)>;

#[allow(dead_code)]
pub struct InlineChat {
    root: gtk4::Box,
    response_label: gtk4::Label,
    input_entry: gtk4::Entry,
    send_button: gtk4::Button,
    pending: Cell<bool>,
    get_terminal_text: RefCell<Option<TerminalTextFn>>,
    send_to_terminal: RefCell<Option<SendToTermFn>>,
}

/// Message type for streaming channel.
#[derive(Debug)]
enum StreamMsg {
    Delta(String),
    Done(Result<String, String>),
}

impl InlineChat {
    #[must_use]
    pub fn new() -> Rc<Self> {
        let response_label = gtk4::Label::builder()
            .label("")
            .halign(gtk4::Align::Start)
            .wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .selectable(true)
            .xalign(0.0)
            .visible(false)
            .margin_start(8)
            .margin_end(8)
            .margin_top(6)
            .margin_bottom(4)
            .build();

        let input_entry = gtk4::Entry::builder()
            .placeholder_text("Ask Claude about this terminal…")
            .hexpand(true)
            .build();
        input_entry.add_css_class("mg-chat-input");

        let send_button = gtk4::Button::builder()
            .label("Ask")
            .css_classes(["mg-btn-primary", "mg-btn-pill"])
            .build();

        let input_row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(6)
            .build();
        input_row.append(&input_entry);
        input_row.append(&send_button);

        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(4)
            .css_classes(["mg-card"])
            .margin_start(6)
            .margin_end(6)
            .margin_bottom(4)
            .visible(false)
            .build();

        let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
        root.append(&sep);
        root.append(&response_label);
        root.append(&input_row);

        let chat = Rc::new(Self {
            root,
            response_label,
            input_entry,
            send_button,
            pending: Cell::new(false),
            get_terminal_text: RefCell::new(None),
            send_to_terminal: RefCell::new(None),
        });

        {
            let c = Rc::clone(&chat);
            chat.send_button.connect_clicked(move |_| c.submit());
        }
        {
            let c = Rc::clone(&chat);
            chat.input_entry.connect_activate(move |_| c.submit());
        }

        chat
    }

    pub fn widget(&self) -> &gtk4::Widget {
        self.root.upcast_ref()
    }

    /// Show or hide the inline chat. When shown, focus moves to
    /// the input entry so the user can start typing immediately.
    pub fn toggle(&self) {
        let visible = !self.root.is_visible();
        self.root.set_visible(visible);
        if visible {
            self.input_entry.grab_focus();
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn is_visible(&self) -> bool {
        self.root.is_visible()
    }

    pub fn set_get_terminal_text(&self, f: impl Fn() -> String + 'static) {
        *self.get_terminal_text.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_send_to_terminal(&self, f: impl Fn(&str) + 'static) {
        *self.send_to_terminal.borrow_mut() = Some(Box::new(f));
    }

    fn submit(self: &Rc<Self>) {
        if self.pending.get() {
            return;
        }
        let text = self.input_entry.text().to_string();
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        self.input_entry.set_text("");
        self.pending.set(true);
        self.send_button.set_sensitive(false);
        self.send_button.set_label("…");
        self.response_label.set_visible(true);
        self.response_label.set_text("Thinking…");

        // Grab terminal context if available.
        let terminal_text = self
            .get_terminal_text
            .borrow()
            .as_ref()
            .map(|f| f())
            .unwrap_or_default();

        let prompt = if terminal_text.trim().is_empty() {
            trimmed
        } else {
            format!(
                "Terminal output:\n```\n{}\n```\n\nQuestion: {}",
                terminal_text.trim_end(),
                trimmed
            )
        };

        let messages = vec![serde_json::json!({
            "role": "user",
            "content": prompt,
        })];

        let (tx, rx) = async_channel::unbounded::<StreamMsg>();

        std::thread::spawn(move || {
            let params = serde_json::json!({ "messages": messages });
            let tx_delta = tx.clone();
            let result = daemon_client::call_streaming(
                "chat.send",
                &params,
                &|text: &str| {
                    let _ = tx_delta.send_blocking(StreamMsg::Delta(text.to_string()));
                },
            );
            let final_msg = match result {
                Ok(value) => value
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string)
                    .ok_or_else(|| "no content".to_string()),
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.send_blocking(StreamMsg::Done(final_msg));
        });

        let chat = Rc::clone(self);
        let mut accumulated = String::new();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(msg) = rx.recv().await {
                match msg {
                    StreamMsg::Delta(text) => {
                        accumulated.push_str(&text);
                        chat.response_label.set_text(&accumulated);
                    }
                    StreamMsg::Done(result) => {
                        match result {
                            Ok(content) => chat.response_label.set_text(&content),
                            Err(err) => {
                                chat.response_label
                                    .set_text(&format!("Error: {err}"));
                            }
                        }
                        chat.pending.set(false);
                        chat.send_button.set_sensitive(true);
                        chat.send_button.set_label("Ask");
                        break;
                    }
                }
            }
        });
    }
}
