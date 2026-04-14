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

/// Boxed "send text to the focused terminal pane" callback.
///
/// Installed by `window.rs` when a chat tab is created. The chat
/// pane invokes this when the user clicks a "Run ↗" button on a
/// code block so the block's contents flow back into whatever
/// terminal tab is currently focused.
pub type SendToTerminal = Box<dyn Fn(&str) + 'static>;

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
    // Set by `window.rs` when the chat tab is installed. When
    // present, assistant replies that contain fenced code blocks
    // get a "Run ↗" button per block that pipes the block's
    // contents into the focused terminal pane.
    send_to_terminal: RefCell<Option<SendToTerminal>>,
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
            send_to_terminal: RefCell::new(None),
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

    /// Install the "send to focused terminal" callback. `window.rs`
    /// wires this when it creates the chat tab so the callback
    /// can reach the tabs vector.
    pub fn set_send_to_terminal(&self, handler: impl Fn(&str) + 'static) {
        *self.send_to_terminal.borrow_mut() = Some(Box::new(handler));
    }

    /// Seed the chat with the current contents of a terminal pane
    /// and auto-send a default question. Called by the window's
    /// "Explain this in chat" action.
    ///
    /// Wraps the terminal dump in a fenced block so Claude knows
    /// exactly where the captured output begins and ends, then
    /// asks for an explanation. If the chat is already mid-request,
    /// this silently drops the seed rather than queueing.
    pub fn explain_terminal(self: &Rc<Self>, terminal_text: &str) {
        if self.pending.get() {
            return;
        }
        let trimmed = terminal_text.trim_end_matches(['\n', ' ']);
        if trimmed.is_empty() {
            return;
        }
        let prompt = format!(
            "Here's my current terminal screen:\n\n```\n{trimmed}\n```\n\n\
             Explain what's happening and suggest next steps."
        );
        self.send_prompt(&prompt);
    }

    fn install_welcome(self: &Rc<Self>) {
        let row = self.build_message_row(
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
        let text = self.input_entry.text().to_string();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        self.input_entry.set_text("");
        self.send_prompt(trimmed);
    }

    /// Send an arbitrary user prompt (from the entry or from
    /// [`explain_terminal`](Self::explain_terminal)) through the
    /// daemon and update the chat list with the reply. Worker
    /// thread + async-channel pattern keeps the GTK main loop
    /// responsive during the multi-second Anthropic roundtrip.
    fn send_prompt(self: &Rc<Self>, prompt: &str) {
        if self.pending.get() {
            return;
        }

        self.append_message(Role::User, prompt);
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

    fn append_message(self: &Rc<Self>, role: Role, content: &str) {
        let row = self.build_message_row(role, content);
        self.message_list.append(&row);
        self.history.borrow_mut().push(ChatMessage {
            role,
            content: content.to_string(),
        });
        self.scroll_to_bottom();
    }

    /// Build a `ListBoxRow` for one message. Assistant messages get
    /// their fenced code blocks extracted and each rendered with
    /// a "Run ↗" button that sends the block to the focused
    /// terminal pane via the `send_to_terminal` callback.
    fn build_message_row(self: &Rc<Self>, role: Role, content: &str) -> gtk4::ListBoxRow {
        let who = gtk4::Label::builder()
            .label(role.label())
            .halign(gtk4::Align::Start)
            .css_classes(["caption", "dim-label"])
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

        // Assistant messages: split text at fenced code blocks so
        // prose and code render side-by-side, with a "Run ↗"
        // action attached to each code block.
        if role == Role::Assistant {
            for segment in split_fenced_blocks(content) {
                match segment {
                    Segment::Prose(text) => {
                        if !text.trim().is_empty() {
                            vbox.append(&prose_label(&text));
                        }
                    }
                    Segment::Code(code) => {
                        vbox.append(&self.code_block_widget(&code));
                    }
                }
            }
        } else {
            vbox.append(&prose_label(content));
        }

        gtk4::ListBoxRow::builder()
            .selectable(false)
            .activatable(false)
            .child(&vbox)
            .build()
    }

    /// Build the code-block sub-widget: a monospace label holding
    /// the block contents plus a "Run ↗" button that dispatches
    /// to the installed `send_to_terminal` callback. If no
    /// terminal callback is registered (e.g. during a test build
    /// with no live tabs vec) the button no-ops.
    fn code_block_widget(self: &Rc<Self>, code: &str) -> gtk4::Box {
        let code_label = gtk4::Label::builder()
            .label(code)
            .halign(gtk4::Align::Start)
            .selectable(true)
            .wrap(false)
            .xalign(0.0)
            .css_classes(["monospace", "card"])
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(4)
            .margin_end(4)
            .build();

        let run_button = gtk4::Button::builder()
            .label("Run ↗")
            .css_classes(["suggested-action", "pill"])
            .halign(gtk4::Align::End)
            .build();

        let view_clone = Rc::clone(self);
        let code_owned = code.to_string();
        run_button.connect_clicked(move |_| {
            if let Some(ref handler) = *view_clone.send_to_terminal.borrow() {
                // Append a trailing newline so the command
                // actually executes when it lands in the shell.
                let payload = if code_owned.ends_with('\n') {
                    code_owned.clone()
                } else {
                    format!("{code_owned}\n")
                };
                handler(&payload);
            }
        });

        let box_ = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(4)
            .build();
        box_.append(&code_label);
        box_.append(&run_button);
        box_
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

/// Build a wrapping selectable prose label for chat message text
/// or the non-code segments of an assistant reply.
fn prose_label(content: &str) -> gtk4::Label {
    gtk4::Label::builder()
        .label(content)
        .halign(gtk4::Align::Start)
        .wrap(true)
        .wrap_mode(gtk4::pango::WrapMode::WordChar)
        .selectable(true)
        .xalign(0.0)
        .build()
}

/// One chunk of a split assistant reply — either free-form prose
/// or a fenced code block.
enum Segment {
    Prose(String),
    Code(String),
}

/// Walk `content` splitting it at triple-backtick fences. Accepts
/// both bare ``` fences and language-tagged ```rust-style fences;
/// the language hint is discarded. Unterminated fences (model
/// truncated mid-block) are returned as prose so we don't silently
/// eat characters.
fn split_fenced_blocks(content: &str) -> Vec<Segment> {
    const FENCE: &str = "```";
    let mut out = Vec::new();
    let mut rest = content;
    while let Some(open_idx) = rest.find(FENCE) {
        let prose = &rest[..open_idx];
        if !prose.is_empty() {
            out.push(Segment::Prose(prose.to_string()));
        }
        let after_open = &rest[open_idx + FENCE.len()..];
        // Skip an optional language tag on the same line as the
        // opening fence — anything up to the next newline.
        let body_start = after_open.find('\n').map_or(after_open.len(), |i| i + 1);
        let body = &after_open[body_start..];
        if let Some(close_idx) = body.find(FENCE) {
            let code = &body[..close_idx];
            // Strip the trailing newline that immediately precedes
            // the closing fence so the rendered label doesn't get
            // a dangling blank line.
            let code_trimmed = code.strip_suffix('\n').unwrap_or(code);
            out.push(Segment::Code(code_trimmed.to_string()));
            rest = &body[close_idx + FENCE.len()..];
        } else {
            // Unterminated fence: emit the rest as prose.
            out.push(Segment::Prose(rest[open_idx..].to_string()));
            return out;
        }
    }
    if !rest.is_empty() {
        out.push(Segment::Prose(rest.to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_plain_prose() {
        let segments = split_fenced_blocks("hello world");
        assert_eq!(segments.len(), 1);
        assert!(matches!(&segments[0], Segment::Prose(s) if s == "hello world"));
    }

    #[test]
    fn split_single_code_block() {
        let segments = split_fenced_blocks("before\n```\nls -la\n```\nafter");
        assert_eq!(segments.len(), 3);
        assert!(matches!(&segments[0], Segment::Prose(s) if s == "before\n"));
        assert!(matches!(&segments[1], Segment::Code(s) if s == "ls -la"));
        assert!(matches!(&segments[2], Segment::Prose(s) if s == "\nafter"));
    }

    #[test]
    fn split_language_tagged_block() {
        let segments = split_fenced_blocks("```bash\necho hi\n```");
        assert_eq!(segments.len(), 1);
        assert!(matches!(&segments[0], Segment::Code(s) if s == "echo hi"));
    }

    #[test]
    fn split_unterminated_fence_falls_back_to_prose() {
        let segments = split_fenced_blocks("before\n```\nnever closed");
        assert_eq!(segments.len(), 2);
        assert!(matches!(&segments[0], Segment::Prose(s) if s == "before\n"));
        assert!(matches!(&segments[1], Segment::Prose(s) if s == "```\nnever closed"));
    }
}
