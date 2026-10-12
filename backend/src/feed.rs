use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gritshield::http::SseStream;
use serde_json::Value;
use tokio::sync::Mutex;

/// Maximum number of events retained in the in-memory ring buffer.
pub const HISTORY_CAPACITY: usize = 2000;

/// A newly-registered SSE stream is given this long to attach a subscriber
/// before it becomes a prune candidate.
const STREAM_ATTACH_GRACE: Duration = Duration::from_secs(5);

struct RegisteredStream {
    sse: SseStream,
    since: Instant,
}

struct FeedInner {
    history: VecDeque<(String, Value)>,
    streams: Vec<RegisteredStream>,
    received_total: u64,
    attached_total: u64,
}

/// Shared pub/sub feed for incident/alert/timeline/run events. Every event is
/// a named SSE frame plus a JSON payload:
///
/// ```text
/// event: incident
/// data: {"id":7,"status":"open", ...}
/// ```
#[derive(Clone)]
pub struct Feed {
    inner: Arc<Mutex<FeedInner>>,
}

impl Feed {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FeedInner {
                history: VecDeque::with_capacity(HISTORY_CAPACITY),
                streams: Vec::new(),
                received_total: 0,
                attached_total: 0,
            })),
        }
    }

    /// Most recent events first, newest at index 0.
    pub async fn history(&self, limit: usize) -> Vec<(String, Value)> {
        let guard = self.inner.lock().await;
        guard.history.iter().rev().take(limit).cloned().collect()
    }

    /// Attach a new consumer stream. The stream must also be handed to
    /// `Response::sse` by the caller; this copy is what `publish` broadcasts on.
    pub async fn attach(&self, stream: SseStream) {
        let mut guard = self.inner.lock().await;
        guard.streams.push(RegisteredStream {
            sse: stream,
            since: Instant::now(),
        });
        guard.attached_total += 1;
    }

    /// Push an event into the ring buffer and fan it out to every live SSE
    /// consumer. Returns the number of consumers that received the frame.
    pub async fn publish(&self, event: &str, data: Value) -> usize {
        let mut guard = self.inner.lock().await;
        guard.received_total += 1;

        if guard.history.len() >= HISTORY_CAPACITY {
            guard.history.pop_front();
        }
        guard.history.push_back((event.to_string(), data.clone()));

        guard.streams.retain(|s| {
            s.since.elapsed() < STREAM_ATTACH_GRACE || s.sse.listeners() > 0
        });

        let payload = data.to_string();
        let mut live = 0usize;
        for registered in &guard.streams {
            if let Ok(n) = registered.sse.send(event, &payload) {
                live += n;
            }
        }
        live
    }

    pub async fn stats(&self) -> (usize, usize, u64, u64) {
        let guard = self.inner.lock().await;
        (
            guard.history.len(),
            guard.streams.len(),
            guard.received_total,
            guard.attached_total,
        )
    }
}

impl Default for Feed {
    fn default() -> Self {
        Self::new()
    }
}