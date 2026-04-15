//! Unix-socket client for mugenkaid, the Bun-based sidecar daemon.
//!
//! # Wire format
//!
//! Newline-delimited JSON. One request per line, one response per
//! line. The daemon implements JSON-RPC 2.0; see
//! `daemon/protocol/src/index.ts` for the full envelope spec.
//!
//! # Lifecycle
//!
//! [`DaemonClient::spawn_and_connect`] forks a `bun run` child for
//! the daemon entry point, waits up to 2s for the socket file to
//! appear, then opens a `UnixStream` and performs a ping handshake.
//! The returned [`DaemonClient`] owns both the stream and the child
//! process; dropping it closes the socket and sends `SIGTERM` to
//! the daemon so cleanup is automatic on window close.
//!
//! # Development vs production
//!
//! The daemon entry point is resolved at compile time via
//! `CARGO_MANIFEST_DIR` — so a debug build of `host-ui` finds the
//! TypeScript source at `$REPO/daemon/server/src/index.ts`. A
//! packaged release build should override this with the
//! `MUGENKAI_DAEMON_CMD` environment variable pointing at a
//! pre-bundled daemon binary.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Compile-time path to the daemon's TypeScript entry point,
/// resolved relative to this crate. Used when `MUGENKAI_DAEMON_CMD`
/// isn't set (i.e. during normal cargo-based dev builds).
const DEV_DAEMON_ENTRY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../daemon/server/src/index.ts"
);

/// A live connection to the mugenkai sidecar daemon.
///
/// Owns the child process and the unix stream. Drop automatically
/// tears both down — the `Child` field sends SIGTERM via its Drop
/// when the struct goes out of scope, and the socket closes
/// naturally when the stream is dropped.
pub struct DaemonClient {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
    // Kept alive so the daemon outlives the client. The daemon
    // installs a SIGTERM handler that cleans up its socket file,
    // so a graceful shutdown happens automatically on drop.
    _child: ManagedChild,
}

struct ManagedChild(Child);

impl Drop for ManagedChild {
    fn drop(&mut self) {
        // Send SIGTERM by killing the process; `kill` maps to
        // SIGKILL on most Unixes but the daemon's SIGINT/SIGTERM
        // handler isn't relied on for correctness — stale sockets
        // are cleaned up on next startup too.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl DaemonClient {
    /// Spawn the daemon as a sidecar child process and connect to
    /// its Unix socket. Blocks until the daemon's `ping` reply
    /// comes back, or times out after 2 seconds.
    ///
    /// # Errors
    ///
    /// - `io::Error` of kind `Other` if the daemon binary can't
    ///   be found or exits before the socket appears
    /// - `io::Error` forwarded from `UnixStream::connect` /
    ///   `read_line` / `write_all`
    pub fn spawn_and_connect() -> io::Result<Self> {
        let socket_path = daemon_socket_path();

        let mut child = spawn_daemon()?;
        let deadline = Instant::now() + Duration::from_millis(2000);
        let stream = loop {
            if Instant::now() > deadline {
                let _ = child.kill();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "daemon did not create its socket within 2s",
                ));
            }
            if socket_path.exists() {
                match UnixStream::connect(&socket_path) {
                    Ok(s) => break s,
                    Err(err) if err.kind() == io::ErrorKind::ConnectionRefused => {}
                    Err(err) => {
                        let _ = child.kill();
                        return Err(err);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(30));
        };

        let reader = BufReader::new(stream.try_clone()?);
        let mut client = Self {
            stream,
            reader,
            next_id: 1,
            _child: ManagedChild(child),
        };

        // Sanity ping — confirms the daemon is actually responding,
        // not just listening on a stale socket from a zombie
        // process.
        let pong = client.call("ping", &serde_json::Value::Null)?;
        if pong != serde_json::Value::String("pong".into()) {
            return Err(io::Error::other(format!(
                "daemon ping returned unexpected value: {pong}"
            )));
        }
        Ok(client)
    }

    /// Send a JSON-RPC request with `method` and `params`, wait
    /// for the matching response, and return the `result` payload.
    ///
    /// # Errors
    ///
    /// - I/O errors from the socket
    /// - `io::Error` of kind `Other` if the daemon returned a
    ///   JSON-RPC error envelope or malformed JSON
    pub fn call(
        &mut self,
        method: &str,
        params: &serde_json::Value,
    ) -> io::Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.clone(),
        });
        let mut line = serde_json::to_string(&request)
            .map_err(|err| io::Error::other(format!("encode request: {err}")))?;
        line.push('\n');
        self.stream.write_all(line.as_bytes())?;
        self.stream.flush()?;

        let mut response_line = String::new();
        self.reader.read_line(&mut response_line)?;
        let response: serde_json::Value = serde_json::from_str(response_line.trim_end())
            .map_err(|err| io::Error::other(format!("parse response: {err}")))?;

        if let Some(error) = response.get("error") {
            return Err(io::Error::other(format!("daemon error: {error}")));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| io::Error::other("daemon response missing result"))
    }
}

/// One-shot connect + call + disconnect against the running
/// daemon. Used from background threads that need to make a
/// daemon call without sharing the main-thread [`DaemonClient`].
///
/// The chat pane uses this from a worker `std::thread` so the
/// Anthropic roundtrip (multi-second) doesn't freeze the GTK
/// main loop. Each call opens a fresh unix socket, writes one
/// request, reads one response, and closes — the daemon handles
/// concurrent connections via `node:net`.
///
/// # Errors
///
/// - `io::ErrorKind::NotFound` if the daemon socket doesn't exist
///   (i.e. the daemon never started or already exited)
/// - Any other I/O error from the socket operations
/// - `io::Error` of kind `Other` if the daemon returned a
///   JSON-RPC error envelope
#[allow(dead_code)]
pub fn call_one_shot(
    method: &str,
    params: &serde_json::Value,
) -> io::Result<serde_json::Value> {
    let path = daemon_socket_path();
    let stream = UnixStream::connect(&path)?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params.clone(),
    });
    let mut line = serde_json::to_string(&request)
        .map_err(|err| io::Error::other(format!("encode request: {err}")))?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()?;

    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;
    let response: serde_json::Value = serde_json::from_str(response_line.trim_end())
        .map_err(|err| io::Error::other(format!("parse response: {err}")))?;

    if let Some(error) = response.get("error") {
        return Err(io::Error::other(format!("daemon error: {error}")));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| io::Error::other("daemon response missing result"))
}

/// Streaming variant of [`call_one_shot`]. Sends the request,
/// then reads lines until it sees a response (has `id`). Any
/// line without `id` is treated as a notification — if the
/// `method` field is `"chat.delta"`, the `params.text` value is
/// sent through `on_delta`.
///
/// Used by the chat pane so tokens appear as they arrive from
/// Claude instead of waiting for the full response.
pub fn call_streaming(
    method: &str,
    params: &serde_json::Value,
    on_delta: &dyn Fn(&str),
) -> io::Result<serde_json::Value> {
    let path = daemon_socket_path();
    let stream = UnixStream::connect(&path)?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params.clone(),
    });
    let mut line = serde_json::to_string(&request)
        .map_err(|err| io::Error::other(format!("encode request: {err}")))?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()?;

    // Read lines until we get a response (object with "id" field).
    // Notifications (chat.delta) arrive first with no id.
    loop {
        let mut response_line = String::new();
        let n = reader.read_line(&mut response_line)?;
        if n == 0 {
            return Err(io::Error::other("daemon closed connection before response"));
        }
        let msg: serde_json::Value = serde_json::from_str(response_line.trim_end())
            .map_err(|err| io::Error::other(format!("parse line: {err}")))?;

        // Notification (no id) — check for chat.delta.
        if msg.get("id").is_none() || msg["id"].is_null() {
            if msg.get("method").and_then(|m| m.as_str()) == Some("chat.delta")
                && let Some(text) = msg
                    .get("params")
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
            {
                on_delta(text);
            }
            continue;
        }

        // Response (has id) — this is the final message.
        if let Some(error) = msg.get("error") {
            return Err(io::Error::other(format!("daemon error: {error}")));
        }
        return msg
            .get("result")
            .cloned()
            .ok_or_else(|| io::Error::other("daemon response missing result"));
    }
}

/// Resolve the Unix socket path the daemon listens on — matches
/// the TypeScript-side `defaultSocketPath()` in
/// `daemon/protocol/src/index.ts`.
fn daemon_socket_path() -> PathBuf {
    let runtime =
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime).join("mugenkai").join("daemon.sock")
}

/// Spawn the daemon as a child process, inheriting stderr so
/// startup errors surface in the host's terminal during dev.
/// stdout is piped to /dev/null so daemon logs don't scribble
/// into the host's own stdout.
fn spawn_daemon() -> io::Result<Child> {
    let mut cmd = if let Ok(override_cmd) = std::env::var("MUGENKAI_DAEMON_CMD") {
        // Release builds or custom installs set this to a full
        // command line. Split on whitespace for simplicity — no
        // quoted arguments supported yet.
        let mut parts = override_cmd.split_whitespace();
        let program = parts.next().ok_or_else(|| {
            io::Error::other("MUGENKAI_DAEMON_CMD is set but empty")
        })?;
        let mut c = Command::new(program);
        for arg in parts {
            c.arg(arg);
        }
        c
    } else {
        let mut c = Command::new("bun");
        c.arg("run").arg(DEV_DAEMON_ENTRY);
        c
    };

    cmd.stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
}
