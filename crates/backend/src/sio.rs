//! The Socket.IO server's outbound side (`sio.emit(...)` in the Python).
//!
//! Emits go to each connected socket in turn, which is synchronous in
//! socketioxide, so any thread (the UART reader, a blocking task) can emit
//! without an event loop and the order of events is the order of calls. The
//! Python's `to=` and `skip_sid=` are a target and an exclusion.

use std::sync::OnceLock;

use serde_json::Value;
use socketioxide::SocketIo;

#[derive(Default)]
pub struct Sio {
    io: OnceLock<SocketIo>,
    /// Every emit, connected clients or not: the Python suite's
    /// `RecordingSocket`.
    #[cfg(test)]
    recorded: parking_lot::Mutex<Vec<(String, Value)>>,
}

impl Sio {
    /// The events emitted so far, oldest first, and forget them.
    #[cfg(test)]
    pub fn take_recorded(&self) -> Vec<(String, Value)> {
        std::mem::take(&mut *self.recorded.lock())
    }

    /// The payloads of `event` emitted so far, oldest first.
    #[cfg(test)]
    pub fn recorded(&self, event: &str) -> Vec<Value> {
        self.recorded
            .lock()
            .iter()
            .filter(|(name, _)| name == event)
            .map(|(_, data)| data.clone())
            .collect()
    }

    pub fn attach(&self, io: SocketIo) {
        let _ = self.io.set(io);
    }

    /// Whether the server is running (emits before that go nowhere).
    pub fn attached(&self) -> bool {
        self.io.get().is_some()
    }

    /// `sio.emit(event, data)` to every client.
    pub fn emit(&self, event: &str, data: &Value) {
        self.emit_where(event, data, |_| true);
    }

    /// `sio.emit(event, data, to=sid)`.
    pub fn emit_to(&self, sid: &str, event: &str, data: &Value) {
        self.emit_where(event, data, |id| id == sid);
    }

    /// `sio.emit(event, data, skip_sid=sid)`.
    pub fn emit_skip(&self, skip_sid: &str, event: &str, data: &Value) {
        self.emit_where(event, data, |id| id != skip_sid);
    }

    fn emit_where(&self, event: &str, data: &Value, select: impl Fn(&str) -> bool) {
        #[cfg(test)]
        self.recorded.lock().push((event.to_string(), data.clone()));
        let Some(io) = self.io.get() else { return };
        let Some(ns) = io.of("/") else { return };
        for socket in ns.sockets() {
            if select(&socket.id.to_string())
                && let Err(error) = socket.emit(event, data)
            {
                tracing::debug!("emit {event} to {} failed: {error}", socket.id);
            }
        }
    }

    pub fn client_count(&self) -> usize {
        self.io
            .get()
            .and_then(|io| io.of("/"))
            .map(|ns| ns.sockets().len())
            .unwrap_or(0)
    }
}
