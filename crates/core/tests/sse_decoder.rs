//! The SSE wire decoder's own contract: framing, the sentinel, the
//! chunk shape, and split-read behavior (a chunk boundary mid-frame
//! must not lose or corrupt an event).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unwrap_in_result, clippy::panic_in_result_fn)]

use gravity_core::llm::sse::{SseDecoder, SseEvent};

fn deltas(events: &[SseEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            SseEvent::Delta(d) => Some(d.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn frames_decode_and_terminate() {
    let mut d = SseDecoder::new();
    d.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n");
    d.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n");
    d.feed(b"data: [DONE]\n\n");

    let mut events = Vec::new();
    while let Some(e) = d.decode().unwrap() {
        events.push(e);
    }
    assert_eq!(deltas(&events), vec!["hel", "lo"]);
    assert!(matches!(events.last(), Some(SseEvent::Done)));
}

#[test]
fn a_chunk_boundary_mid_frame_is_not_a_frame() {
    // The frame is split across two feeds at an arbitrary byte — the
    // decoder must not emit a partial event.
    let frame = b"data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n";
    let mut d = SseDecoder::new();
    d.feed(&frame[..20]);
    assert!(d.decode().unwrap().is_none(), "no complete frame yet");
    d.feed(&frame[20..]);
    let ev = d.decode().unwrap().unwrap();
    assert!(matches!(ev, SseEvent::Delta(s) if s == "hel"));
}

#[test]
fn empty_deltas_are_skipped() {
    // A chunk with `content: ""` (some providers emit them) produces no
    // event.
    let mut d = SseDecoder::new();
    d.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}\n\n");
    d.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n");
    let mut events = Vec::new();
    while let Some(e) = d.decode().unwrap() {
        events.push(e);
    }
    assert_eq!(deltas(&events), vec!["x"]);
}

#[test]
fn malformed_payload_is_an_error_not_a_panic() {
    let mut d = SseDecoder::new();
    d.feed(b"data: not-json\n\n");
    assert!(d.decode().is_err());
}

#[test]
fn tool_calls_ride_the_chunk() {
    let mut d = SseDecoder::new();
    d.feed(br#"data: {"choices":[{"delta":{"tool_calls":[{"id":"c1","function":{"name":"read_file","arguments":"{\"path\":\"/x\"}"}}]}}]}"#);
    d.feed(b"\n\ndata: [DONE]\n\n");
    let mut events = Vec::new();
    while let Some(e) = d.decode().unwrap() {
        events.push(e);
    }
    assert_eq!(events.len(), 2);
    match &events[0] {
        SseEvent::ToolCalls(calls) => {
            assert_eq!(calls[0]["id"], "c1");
            assert_eq!(calls[0]["function"]["name"], "read_file");
        }
        other => panic!("expected tool calls, got {other:?}"),
    }
    assert!(matches!(events[1], SseEvent::Done));
}

#[test]
fn a_chunk_without_tool_calls_yields_no_tool_event() {
    let mut d = SseDecoder::new();
    d.feed(br#"data: {"choices":[{"delta":{"content":"hi"}}]}"#);
    d.feed(b"\n\n");
    let mut events = Vec::new();
    while let Some(e) = d.decode().unwrap() {
        events.push(e);
    }
    assert!(matches!(events[0], SseEvent::Delta(_)));
}
