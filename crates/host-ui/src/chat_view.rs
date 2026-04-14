//! In-terminal chat pane for conversing with Claude via the
//! sidecar daemon.
//!
//! A `ChatView` opens as a regular `AdwTabPage` alongside terminal
//! tabs and the Settings tab. The user types a message in the
//! bottom entry, hits Enter (or the Send button), and the view
//! dispatches a `chat.send` request to the daemon. The daemon
//! forwards to Anthropic's API using `ANTHROPIC_API_KEY` from its
//! environment.
//!
//! # Threading
//!
//! The daemon call is synchronous but slow (multi-second Anthropic
//! roundtrip). To keep the GTK main loop responsive, the request
//! runs on a worker `std::thread` and posts its result back via an
//! `async-channel` consumed by `glib::MainContext::spawn_local`.
//! All widget mutation happens on the main thread.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk4::glib;

use crate::daemon_client;

/// A single turn of conversation.
#[derive(Debug, Clone)]
struct ChatMessage {
    role: Role,
    content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::User => "you",
            Self::Assistant => "claude",
        }
    }
}

/// The chat pane widget.
///
/// Holds the entire message history in a `RefCell<Vec<ChatMessage>>`
/// so it can be resent to the daemon on every follow-up, and a
/// `pending` flag that disables the send button while a request
/// is in flight so users can't queue up parallel requests.
pub struct ChatView {
    root: gtk4::Box,
    message_list: gtk4::ListBox,
    input_entry: gtk4::Entry,
    send_button: gtk4::Button,
    scroller: gtk4::ScrolledWindow,
    history: RefCell<Vec<ChatMessage>>,
    pending: Cell<bool>,
}

impl ChatView {
    /// Build a new chat view. Wire the send button + entry-activate
    /// signals in the caller because they capture an `Rc<Self>`.
    #[must_use]
    pub fn new() -> Rc<Self> {
        let message_list = gtk4::ListBox::builder()
            .selection_mode(gtk4::SelectionMode::None)
            .show_separators(true)
            .css_classes(["boxed-list"])
            .build();

        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .child(&message_list)
            .build();

        let input_entry = gtk4::Entry::builder()
            .placeholder_text("Message Claude…")
            .hexpand(true)
            .build();
        let send_button = gtk4::Button::builder()
            .label("Send")
            .css_classes(["suggested-action"])
            .build();

        let input_row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .margin_top(6)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        input_row.append(&input_entry);
        input_row.append(&send_button);

        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&scroller);
        root.append(&input_row);

        let view = Rc::new(Self {
            root,
            message_list,
            input_entry,
            send_button,
            scroller,
            history: RefCell::new(Vec::new()),
            pending: Cell::new(false),
        });

        view.install_welcome();

        // Wire the send paths — both the button click and pressing
        // Enter in the entry trigger the same handler.
        {
            let view_clone = Rc::clone(&view);
            view.send_button.connect_clicked(move |_| {
                view_clone.submit();
            });
        }
        {
            let view_clone = Rc::clone(&view);
            view.input_entry.connect_activate(move |_| {
                view_clone.submit();
            });
        }

        view
    }

    /// Widget handle for insertion into an `AdwTabView` page.
    #[must_use]
    pub fn widget(&self) -> &gtk4::Widget {
        self.root.upcast_ref()
    }

    fn install_welcome(&self) {
        let row = build_message_row(
            Role::Assistant,
            "Hi — this is Claude running as a mugenkai sidecar. Ask me about a command, \
             a stack trace, or anything the shell is showing you.",
        );
        self.message_list.append(&row);
    }

    /// Handle a send press: grab the entry's text, push the user
    /// message, clear the entry, and kick off a worker thread to
    /// call the daemon.
    fn submit(self: &Rc<Self>) {
        if self.pending.get() {
            return;
        }
        let text = self.input_entry.text().to_string();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }

        self.append_message(Role::User, trimmed);
        self.input_entry.set_text("");
        self.set_pending(true);

        // Snapshot history for the worker. It gets sent verbatim
        // to the daemon so Claude has the full conversation.
        let history_snapshot: Vec<serde_json::Value> = self
            .history
            .borrow()
            .iter()
            .map(|msg| {
                serde_json::json!({
                    "role": msg.role.as_str(),
                    "content": msg.content,
                })
            })
            .collect();

        let (tx, rx) = async_channel::bounded::<Result<String, String>>(1);

        // Worker thread: blocking daemon call. Keeps the GTK main
        // loop responsive.
        std::thread::spawn(move || {
            let params = serde_json::json!({ "messages": history_snapshot });
            let result = match daemon_client::call_one_shot("chat.send", &params) {
                Ok(value) => value
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string)
                    .ok_or_else(|| "daemon returned no content field".to_string()),
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.send_blocking(result);
        });

        // Main-thread waiter: consume the async-channel result and
        // update the UI. `spawn_local` runs on the GTK main context
        // so all widget mutation stays on the main thread.
        let view_clone = Rc::clone(self);
        glib::MainContext::default().spawn_local(async move {
            match rx.recv().await {
                Ok(Ok(content)) => {
                    view_clone.append_message(Role::Assistant, &content);
                }
                Ok(Err(err)) => {
                    view_clone.append_message(
                        Role::Assistant,
                        &format!("⚠️  chat.send failed: {err}"),
                    );
                }
                Err(err) => {
                    eprintln!("[mugenkai] chat worker channel dropped: {err}");
                }
            }
            view_clone.set_pending(false);
        });
    }

    fn append_message(&self, role: Role, content: &str) {
        let row = build_message_row(role, content);
        self.message_list.append(&row);
        self.history.borrow_mut().push(ChatMessage {
            role,
            content: content.to_string(),
        });
        self.scroll_to_bottom();
    }

    fn set_pending(&self, pending: bool) {
        self.pending.set(pending);
        self.send_button.set_sensitive(!pending);
        self.input_entry.set_sensitive(!pending);
        self.send_button
            .set_label(if pending { "Sending…" } else { "Send" });
    }

    fn scroll_to_bottom(&self) {
        // Queue a deferred scroll — if we do it synchronously the
        // ListBox hasn't yet laid out the new row, so the adj upper
        // bound is still the old one. An idle callback fires after
        // layout settles.
        let adj = self.scroller.vadjustment();
        glib::idle_add_local_once(move || {
            adj.set_value(adj.upper());
        });
    }
}

fn build_message_row(role: Role, content: &str) -> gtk4::ListBoxRow {
    let who = gtk4::Label::builder()
        .label(role.label())
        .halign(gtk4::Align::Start)
        .css_classes(["caption", "dim-label"])
        .build();

    let body = gtk4::Label::builder()
        .label(content)
        .halign(gtk4::Align::Start)
        .wrap(true)
        .wrap_mode(gtk4::pango::WrapMode::WordChar)
        .selectable(true)
        .xalign(0.0)
        .build();

    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(4)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();
    vbox.append(&who);
    vbox.append(&body);

    gtk4::ListBoxRow::builder()
        .selectable(false)
        .activatable(false)
        .child(&vbox)
        .build()
}
