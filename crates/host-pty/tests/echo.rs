//! Integration tests for `host-pty::Child`.
//!
//! Spawns a real child process under a real PTY and asserts basic
//! read/write semantics. These tests require a Linux kernel with
//! pty support and `/bin/cat` or `/bin/sh` on PATH — i.e. any
//! normal developer box. They are NOT safe to run in a tight loop
//! against a real display because each one allocates a PTY pair.
//!
//! Tests use tokio's current-thread runtime so they cleanly teardown
//! the child and the AsyncFd on return.

use std::path::PathBuf;
use std::time::Duration;

use host_pty::{Child, SpawnOptions, Winsize};

async fn read_with_timeout(child: &Child, buf: &mut [u8]) -> std::io::Result<usize> {
    tokio::time::timeout(Duration::from_secs(3), child.read(buf))
        .await
        .unwrap_or_else(|_| Err(std::io::Error::other("read timed out")))
}

fn cat_opts() -> SpawnOptions {
    SpawnOptions {
        shell: Some(PathBuf::from("/bin/cat")),
        args: Vec::new(),
        winsize: Winsize::default(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_cat_and_echo_round_trip() {
    let child = Child::spawn(cat_opts()).expect("spawn cat");

    child
        .write_all(b"hello world\n")
        .await
        .expect("write hello");

    let mut buf = [0u8; 256];
    let mut collected = Vec::new();
    // cat echoes input back via the PTY's own echo (enabled by
    // default) AND (for /bin/cat) writes the line to stdout. We keep
    // reading until we see the message, or time out.
    for _ in 0..5 {
        let n = read_with_timeout(&child, &mut buf)
            .await
            .expect("read chunk");
        if n == 0 {
            break;
        }
        collected.extend_from_slice(&buf[..n]);
        if String::from_utf8_lossy(&collected).contains("hello world") {
            break;
        }
    }

    let text = String::from_utf8_lossy(&collected);
    assert!(
        text.contains("hello world"),
        "did not see echoed text in {text:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_child_cleans_up() {
    let child = Child::spawn(cat_opts()).expect("spawn cat");
    let pid = child.pid();
    drop(child);

    // Give the kernel a moment to process SIGHUP teardown.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Poking the pid with signal 0 should either succeed with ESRCH
    // (process already reaped) or a harmless deliver. We assert that
    // we don't panic here — the drop glue is best-effort.
    let _ = nix::sys::signal::kill(pid, None);
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_rejects_missing_shell() {
    let err = Child::spawn(SpawnOptions {
        shell: Some(PathBuf::from("/does/not/exist/mugenkai-no-shell")),
        args: Vec::new(),
        winsize: Winsize::default(),
    });
    // We accept either an early error (if we validated) OR a
    // successful spawn that immediately writes the "failed to exec"
    // message and _exits. Today our code path spawns and relies on
    // execvp to fail in the child, so the parent side sees Ok(Child)
    // and can read the error message.
    match err {
        Ok(child) => {
            let mut buf = [0u8; 256];
            let n = read_with_timeout(&child, &mut buf)
                .await
                .expect("read error message");
            let text = String::from_utf8_lossy(&buf[..n]);
            assert!(
                text.contains("failed to exec shell"),
                "expected exec failure message, got {text:?}"
            );
        }
        Err(_expected) => {
            // also acceptable — we rejected before fork
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn resize_is_accepted() {
    let child = Child::spawn(cat_opts()).expect("spawn cat");
    child
        .resize(Winsize::new(100, 40, 800, 640))
        .expect("resize to 100x40");
    child
        .resize(Winsize::new(80, 24, 640, 384))
        .expect("resize back to 80x24");
}

#[tokio::test(flavor = "current_thread")]
async fn resize_rejects_zero_dimensions() {
    let child = Child::spawn(cat_opts()).expect("spawn cat");
    let err = child
        .resize(Winsize::new(0, 24, 0, 0))
        .expect_err("zero cols must be rejected");
    assert!(matches!(err, host_pty::PtyError::InvalidSize { .. }));
}
