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

/// Message type for the streaming channel between the daemon
/// worker thread and the GTK main thread.
#[derive(Debug)]
enum StreamMsg {
    Delta(String),
    Done(Result<String, String>),
}

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
///
/// Returns `true` if the text was actually delivered to a
/// terminal, `false` if no terminal tab was available — the chat
/// pane uses the failure signal to surface a toast instead of
/// silently no-opping.
pub type SendToTerminal = Box<dyn Fn(&str) -> bool + 'static>;

/// The chat pane widget.
///
/// Holds the entire message history in a `RefCell<Vec<ChatMessage>>`
/// so it can be resent to the daemon on every follow-up, and a
/// `pending` flag that disables the send button while a request
/// is in flight so users can't queue up parallel requests.
/// One image attachment pending on the input: base64-encoded PNG
/// bytes plus the thumbnail box so we can remove it from the UI
/// when the user clicks ✕ or when the message is sent.
struct PendingAttachment {
    base64: String,
    thumbnail: gtk4::Box,
}

pub struct ChatView {
    root: gtk4::Box,
    message_list: gtk4::ListBox,
    input_entry: gtk4::Entry,
    send_button: gtk4::Button,
    scroller: gtk4::ScrolledWindow,
    toast_overlay: adw::ToastOverlay,
    history: RefCell<Vec<ChatMessage>>,
    pending: Cell<bool>,
    send_to_terminal: RefCell<Option<SendToTerminal>>,
    /// Horizontal strip of thumbnail widgets shown above the
    /// entry for each pending image attachment.
    attachments_row: gtk4::Box,
    /// Base64 PNG data for every image currently staged on the
    /// input. Cleared when a message is sent successfully.
    attachments: RefCell<Vec<PendingAttachment>>,
}

impl ChatView {
    /// Build a new chat view. Wire the send button + entry-activate
    /// signals in the caller because they capture an `Rc<Self>`.
    #[must_use]
    #[allow(clippy::too_many_lines)]
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

        let attachments_row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .margin_start(12)
            .margin_end(12)
            .margin_top(4)
            .visible(false)
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

        let inner = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        inner.append(&scroller);
        inner.append(&attachments_row);
        inner.append(&input_row);

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&inner));

        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&toast_overlay);

        let view = Rc::new(Self {
            root,
            message_list,
            input_entry,
            send_button,
            scroller,
            toast_overlay,
            history: RefCell::new(Vec::new()),
            pending: Cell::new(false),
            send_to_terminal: RefCell::new(None),
            attachments_row,
            attachments: RefCell::new(Vec::new()),
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

        // Ctrl+Shift+V routing for image paste happens at the
        // window-action level (see `install_paste_action` in
        // window.rs) so it wins against the chat entry's
        // default paste handler. Plain Ctrl+V still performs
        // the Entry's built-in text paste.

        view
    }

    /// Widget handle for insertion into an `AdwTabView` page.
    #[must_use]
    pub fn widget(&self) -> &gtk4::Widget {
        self.root.upcast_ref()
    }

    /// A short title derived from the first user message in the
    /// conversation, or `None` if no user messages exist yet.
    /// Used by the reaper timer to set the tab page title so the
    /// sidebar shows something meaningful instead of "Chat".
    #[must_use]
    pub fn conversation_title(&self) -> Option<String> {
        let history = self.history.borrow();
        let first_user = history.iter().find(|m| m.role == Role::User)?;
        let content = first_user.content.trim();
        if content.is_empty() {
            return None;
        }
        // Take the first line, truncated to 30 chars.
        let first_line = content.lines().next().unwrap_or(content);
        if first_line.len() <= 30 {
            Some(first_line.to_string())
        } else {
            Some(format!("{}…", &first_line[..29]))
        }
    }

    /// Install the "send to focused terminal" callback. `window.rs`
    /// wires this when it creates the chat tab so the callback
    /// can reach the tabs vector.
    pub fn set_send_to_terminal(&self, handler: impl Fn(&str) -> bool + 'static) {
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

    /// Show a transient toast inside the chat tab. Used when the
    /// Run↗ handler has nothing to paste to, or when the clipboard
    /// write succeeds so the user gets visual confirmation.
    fn toast(&self, message: &str) {
        let toast = adw::Toast::builder().title(message).timeout(3).build();
        self.toast_overlay.add_toast(toast);
    }

    /// Try to read an image from the system clipboard and stage
    /// it as a pending attachment. No-op if clipboard has no
    /// image data. Exposed so the window-level paste action can
    /// route here when the chat tab is active.
    pub fn try_paste_image_from_clipboard(self: &Rc<Self>) {
        let Some(display) = gtk4::gdk::Display::default() else {
            return;
        };
        let clipboard = display.clipboard();
        let view_clone = Rc::clone(self);
        clipboard.read_texture_async(None::<&gtk4::gio::Cancellable>, move |result| {
            if let Ok(Some(texture)) = result {
                let bytes = texture.save_to_png_bytes();
                let b64 = base64_encode_png(&bytes);
                view_clone.add_attachment(b64);
            } else {
                view_clone.toast("Clipboard has no image");
            }
        });
    }

    /// Append a base64-encoded PNG attachment + a thumbnail
    /// widget above the input with an ✕ remove button.
    fn add_attachment(self: &Rc<Self>, base64: String) {
        let Ok(bytes_for_preview) = base64_decode(&base64) else {
            return;
        };
        let glib_bytes = gtk4::glib::Bytes::from_owned(bytes_for_preview);
        let Ok(texture) = gtk4::gdk::Texture::from_bytes(&glib_bytes) else {
            return;
        };

        let thumb = gtk4::Picture::for_paintable(&texture);
        thumb.set_content_fit(gtk4::ContentFit::Cover);
        thumb.set_size_request(60, 60);

        let remove_btn = gtk4::Button::builder()
            .label("✕")
            .css_classes(["mg-btn-ghost", "mg-btn-pill"])
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Start)
            .build();

        let wrapper = gtk4::Overlay::builder()
            .width_request(60)
            .height_request(60)
            .build();
        wrapper.set_child(Some(&thumb));
        wrapper.add_overlay(&remove_btn);

        let thumb_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();
        thumb_box.append(&wrapper);

        self.attachments_row.append(&thumb_box);
        self.attachments_row.set_visible(true);

        let thumb_box_clone = thumb_box.clone();
        let view_clone = Rc::clone(self);
        let base64_clone = base64.clone();
        remove_btn.connect_clicked(move |_| {
            view_clone.remove_attachment(&base64_clone, &thumb_box_clone);
        });

        self.attachments.borrow_mut().push(PendingAttachment {
            base64,
            thumbnail: thumb_box,
        });
    }

    fn remove_attachment(&self, base64: &str, thumb_box: &gtk4::Box) {
        self.attachments_row.remove(thumb_box);
        self.attachments.borrow_mut().retain(|a| a.base64 != base64);
        if self.attachments.borrow().is_empty() {
            self.attachments_row.set_visible(false);
        }
    }

    fn clear_attachments(&self) {
        let thumbs: Vec<gtk4::Box> = self
            .attachments
            .borrow()
            .iter()
            .map(|a| a.thumbnail.clone())
            .collect();
        for t in thumbs {
            self.attachments_row.remove(&t);
        }
        self.attachments.borrow_mut().clear();
        self.attachments_row.set_visible(false);
    }

    fn install_welcome(self: &Rc<Self>) {
        let row = self.build_message_row(
            Role::Assistant,
            "Hi — this is Claude running as a mugenkai sidecar.\n\n\
             • Ask me anything right here.\n\
             • Or hit Ctrl+Shift+E in any terminal pane to ship its visible output \
             over and have me explain what's going on.\n\
             • I'll wrap suggested commands in ``` code blocks — tap Run ↗ on one \
             to send it straight into your focused shell.",
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

    /// Send an arbitrary user prompt through the daemon with
    /// streaming — tokens appear in the chat as they arrive from
    /// Claude instead of waiting for the full response.
    ///
    /// Architecture: a worker `std::thread` calls
    /// `daemon_client::call_streaming` which reads JSON-RPC
    /// notifications (`chat.delta`) and forwards each text chunk
    /// via an `async_channel`. A `glib::MainContext::spawn_local`
    /// future drains the channel on the GTK main thread and
    /// appends each chunk to a growing assistant label. When the
    /// final response arrives the worker sends a `Done` sentinel
    /// so the main thread knows to finalize the message row
    /// (parse code blocks, add buttons, update history).
    #[allow(clippy::too_many_lines)]
    fn send_prompt(self: &Rc<Self>, prompt: &str) {
        if self.pending.get() {
            return;
        }

        self.append_message(Role::User, prompt);
        self.set_pending(true);

        // Drain pending image attachments into the last (user)
        // message's content as Anthropic-format blocks. Clear
        // the UI row afterwards so the next prompt starts empty.
        let attachment_blocks: Vec<serde_json::Value> = self
            .attachments
            .borrow()
            .iter()
            .map(|att| {
                serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": att.base64,
                    },
                })
            })
            .collect();
        self.clear_attachments();

        let history_len = self.history.borrow().len();
        let history_snapshot: Vec<serde_json::Value> = self
            .history
            .borrow()
            .iter()
            .enumerate()
            .map(|(idx, msg)| {
                let is_last_user =
                    idx + 1 == history_len && msg.role == Role::User && !attachment_blocks.is_empty();
                if is_last_user {
                    // Mixed content: text + image blocks.
                    let mut blocks = vec![serde_json::json!({
                        "type": "text",
                        "text": msg.content,
                    })];
                    blocks.extend(attachment_blocks.iter().cloned());
                    serde_json::json!({
                        "role": msg.role.as_str(),
                        "content": blocks,
                    })
                } else {
                    serde_json::json!({
                        "role": msg.role.as_str(),
                        "content": msg.content,
                    })
                }
            })
            .collect();

        let (tx, rx) = async_channel::unbounded::<StreamMsg>();

        // Worker thread: blocking streaming call.
        std::thread::spawn(move || {
            let params = serde_json::json!({ "messages": history_snapshot });
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
                    .ok_or_else(|| "daemon returned no content field".to_string()),
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.send_blocking(StreamMsg::Done(final_msg));
        });

        // Create a placeholder assistant row with an empty label
        // that we'll append to as deltas arrive.
        let streaming_label = gtk4::Label::builder()
            .label("")
            .halign(gtk4::Align::Start)
            .wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .selectable(true)
            .xalign(0.0)
            .build();

        let who = gtk4::Label::builder()
            .label(Role::Assistant.label())
            .halign(gtk4::Align::Start)
            .css_classes(["caption", "dim-label"])
            .build();

        let placeholder_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(4)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        placeholder_box.append(&who);
        placeholder_box.append(&streaming_label);

        let placeholder_row = gtk4::ListBoxRow::builder()
            .selectable(false)
            .activatable(false)
            .child(&placeholder_box)
            .build();
        self.message_list.append(&placeholder_row);
        self.scroll_to_bottom();

        // Main-thread consumer: drain deltas into the label, then
        // finalize on Done.
        let view_clone = Rc::clone(self);
        let mut accumulated = String::new();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(msg) = rx.recv().await {
                match msg {
                    StreamMsg::Delta(text) => {
                        accumulated.push_str(&text);
                        streaming_label.set_text(&accumulated);
                        view_clone.scroll_to_bottom();
                    }
                    StreamMsg::Done(result) => {
                        // Remove the placeholder row and replace
                        // with a properly built message row that
                        // has code-block parsing + Run↗ buttons.
                        view_clone.message_list.remove(&placeholder_row);

                        let content = match result {
                            Ok(c) => c,
                            Err(err) => format!("chat.send failed: {err}"),
                        };
                        view_clone.append_message(Role::Assistant, &content);
                        view_clone.set_pending(false);
                        break;
                    }
                }
            }
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
    /// the block contents plus two action buttons — **Copy** to
    /// stash the block on the system clipboard, and **Run ↗** to
    /// paste it straight into the focused terminal pane via the
    /// registered `send_to_terminal` callback.
    ///
    /// If no terminal callback is wired (e.g. the user closed all
    /// terminal tabs and only the chat tab remains), Run↗ surfaces
    /// a toast instead of silently no-opping so the user knows why
    /// nothing happened.
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

        let copy_button = gtk4::Button::builder()
            .label("Copy")
            .css_classes(["pill"])
            .build();
        let run_button = gtk4::Button::builder()
            .label("Run ↗")
            .css_classes(["suggested-action", "pill"])
            .build();

        // Copy: system-clipboard write via the default display,
        // then a confirmation toast.
        {
            let view_clone = Rc::clone(self);
            let code_owned = code.to_string();
            copy_button.connect_clicked(move |_| {
                if let Some(display) = gtk4::gdk::Display::default() {
                    display.clipboard().set_text(&code_owned);
                    view_clone.toast("Copied to clipboard");
                }
            });
        }

        // Run: paste-to-focused-terminal via the installed
        // callback, with a newline appended so the command
        // actually executes on landing. The callback returns
        // false when no terminal tab exists — in that case
        // we surface a toast telling the user how to get one.
        {
            let view_clone = Rc::clone(self);
            let code_owned = code.to_string();
            run_button.connect_clicked(move |_| {
                let payload = if code_owned.ends_with('\n') {
                    code_owned.clone()
                } else {
                    format!("{code_owned}\n")
                };
                let delivered = view_clone
                    .send_to_terminal
                    .borrow()
                    .as_ref()
                    .is_some_and(|handler| handler(&payload));
                if !delivered {
                    view_clone.toast("No terminal pane — open one with Ctrl+Shift+T");
                }
            });
        }

        let button_row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .halign(gtk4::Align::End)
            .build();
        button_row.append(&copy_button);
        button_row.append(&run_button);

        let box_ = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(4)
            .build();
        box_.append(&code_label);
        box_.append(&button_row);
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

/// Encode a slice of PNG bytes to a base64 string (standard
/// alphabet, with padding) — Anthropic's API expects this
/// format for image content blocks.
fn base64_encode_png(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s)
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
