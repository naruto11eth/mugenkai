//! Integration tests for the libghostty-vt effects-callback bridge.
//!
//! The point of these tests is to prove:
//! 1. A trampoline-dispatched bell event actually reaches the Rust
//!    consumer via the event channel.
//! 2. The title_changed callback fires on OSC 2.
//! 3. The write_pty callback fires on a query that requires a
//!    response (CSI 6 n cursor-position report).
//! 4. Callbacks are synchronous with vt_write, so draining the
//!    channel immediately after vt_write returns is sufficient.
//!
//! If any of these regress, mugenkai's notification rings and
//! terminal title updates break silently, so the coverage pays for
//! itself.

use std::time::Duration;

use host_ghostty::{EffectsHandle, Terminal, TerminalEvent};

fn drain(rx: &std::sync::mpsc::Receiver<TerminalEvent>) -> Vec<TerminalEvent> {
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

#[test]
fn bell_callback_fires_on_bel_byte() {
    let mut term = Terminal::new(80, 24, 0).expect("terminal");
    let mut handle = EffectsHandle::install(&mut term).expect("install effects");
    let rx = handle.take_receiver().expect("take receiver");

    term.vt_write(b"\x07");

    let events = drain(&rx);
    assert_eq!(
        events,
        vec![TerminalEvent::Bell],
        "expected a single Bell event, got {events:?}"
    );

    // Cleanup: clear callbacks before drop so libghostty-vt no
    // longer holds a pointer into our EffectsHandle allocation.
    handle.clear(&mut term).expect("clear effects");
}

#[test]
fn title_changed_callback_fires_on_osc_2() {
    let mut term = Terminal::new(80, 24, 0).expect("terminal");
    let mut handle = EffectsHandle::install(&mut term).expect("install effects");
    let rx = handle.take_receiver().expect("take receiver");

    term.vt_write(b"\x1b]2;Hello World\x1b\\");

    let events = drain(&rx);
    assert!(
        events.contains(&TerminalEvent::TitleChanged),
        "expected TitleChanged in {events:?}"
    );

    handle.clear(&mut term).expect("clear effects");
}

#[test]
fn multiple_bells_produce_multiple_events() {
    let mut term = Terminal::new(80, 24, 0).expect("terminal");
    let mut handle = EffectsHandle::install(&mut term).expect("install effects");
    let rx = handle.take_receiver().expect("take receiver");

    term.vt_write(b"\x07\x07\x07");

    let events = drain(&rx);
    let bell_count = events
        .iter()
        .filter(|e| matches!(e, TerminalEvent::Bell))
        .count();
    assert_eq!(bell_count, 3, "expected 3 bells in {events:?}");

    handle.clear(&mut term).expect("clear effects");
}

#[test]
fn write_pty_callback_fires_on_device_status_report() {
    let mut term = Terminal::new(80, 24, 0).expect("terminal");
    let mut handle = EffectsHandle::install(&mut term).expect("install effects");
    let rx = handle.take_receiver().expect("take receiver");

    // CSI 5 n = Device Status Report (query). The terminal should
    // respond with CSI 0 n (ready) via the write_pty callback.
    term.vt_write(b"\x1b[5n");

    let events = drain(&rx);
    let has_write_pty = events
        .iter()
        .any(|e| matches!(e, TerminalEvent::WritePty(_)));
    assert!(
        has_write_pty,
        "expected WritePty event in {events:?} — did libghostty-vt stop responding to DSR?"
    );

    handle.clear(&mut term).expect("clear effects");
}

#[test]
fn clear_stops_callbacks_from_firing() {
    let mut term = Terminal::new(80, 24, 0).expect("terminal");
    let mut handle = EffectsHandle::install(&mut term).expect("install effects");
    let rx = handle.take_receiver().expect("take receiver");

    term.vt_write(b"\x07");
    assert_eq!(drain(&rx).len(), 1);

    handle.clear(&mut term).expect("clear effects");

    term.vt_write(b"\x07");
    assert_eq!(
        drain(&rx).len(),
        0,
        "expected no events after clear() but got some"
    );
}

#[test]
fn effects_on_independent_terminals_do_not_cross_contaminate() {
    let mut a = Terminal::new(80, 24, 0).expect("terminal a");
    let mut b = Terminal::new(80, 24, 0).expect("terminal b");

    let mut handle_a = EffectsHandle::install(&mut a).expect("install a");
    let mut handle_b = EffectsHandle::install(&mut b).expect("install b");
    let rx_a = handle_a.take_receiver().expect("rx a");
    let rx_b = handle_b.take_receiver().expect("rx b");

    a.vt_write(b"\x07");
    b.vt_write(b"\x07\x07");

    let events_a = drain(&rx_a);
    let events_b = drain(&rx_b);
    assert_eq!(
        events_a.len(),
        1,
        "terminal A saw wrong count: {events_a:?}"
    );
    assert_eq!(
        events_b.len(),
        2,
        "terminal B saw wrong count: {events_b:?}"
    );

    handle_a.clear(&mut a).expect("clear a");
    handle_b.clear(&mut b).expect("clear b");
}

#[test]
fn receiver_can_only_be_taken_once() {
    let mut term = Terminal::new(80, 24, 0).expect("terminal");
    let mut handle = EffectsHandle::install(&mut term).expect("install effects");
    assert!(handle.take_receiver().is_some());
    assert!(handle.take_receiver().is_none());
    handle.clear(&mut term).expect("clear effects");
}

#[test]
fn dead_receiver_does_not_panic_on_event_delivery() {
    let mut term = Terminal::new(80, 24, 0).expect("terminal");
    let mut handle = EffectsHandle::install(&mut term).expect("install effects");
    let rx = handle.take_receiver().expect("receiver");
    drop(rx);
    // The bell trampoline should silently absorb the failed send.
    term.vt_write(b"\x07");
    // Do a small sleep to make sure any deferred handling doesn't
    // turn into a panic on a later callback.
    std::thread::sleep(Duration::from_millis(10));
    handle.clear(&mut term).expect("clear effects");
}
