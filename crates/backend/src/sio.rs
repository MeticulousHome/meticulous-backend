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
    /// Every emit, attached or not: handler tests read what clients would
    /// have received (the Python suite's `RecordingSocketIO`).
    #[cfg(test)]
    recorded: parking_lot::Mutex<Vec<Emitted>>,
}

/// One recorded emit and its `to=` / `skip_sid=`.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub struct Emitted {
    pub event: String,
    pub data: Value,
    pub to: Option<String>,
    pub skip_sid: Option<String>,
}

impl Sio {
    /// The events emitted so far, oldest first, and forget them.
    #[cfg(test)]
    pub fn take_recorded(&self) -> Vec<(String, Value)> {
        std::mem::take(&mut *self.recorded.lock())
            .into_iter()
            .map(|e| (e.event, e.data))
            .collect()
    }

    /// The payloads of `event` emitted so far, oldest first.
    #[cfg(test)]
    pub fn recorded(&self, event: &str) -> Vec<Value> {
        self.recorded
            .lock()
            .iter()
            .filter(|e| e.event == event)
            .map(|e| e.data.clone())
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
        #[cfg(test)]
        self.record(event, data, None, None);
        self.emit_where(event, data, |_| true);
    }

    /// `sio.emit(event, data, to=sid)`.
    pub fn emit_to(&self, sid: &str, event: &str, data: &Value) {
        #[cfg(test)]
        self.record(event, data, Some(sid), None);
        self.emit_where(event, data, |id| id == sid);
    }

    /// `sio.emit(event, data, skip_sid=sid)`.
    pub fn emit_skip(&self, skip_sid: &str, event: &str, data: &Value) {
        #[cfg(test)]
        self.record(event, data, None, Some(skip_sid));
        self.emit_where(event, data, |id| id != skip_sid);
    }

    #[cfg(test)]
    fn record(&self, event: &str, data: &Value, to: Option<&str>, skip_sid: Option<&str>) {
        crate::socketio_types::assert_conforms(event, data);
        self.recorded.lock().push(Emitted {
            event: event.to_string(),
            data: data.clone(),
            to: to.map(str::to_string),
            skip_sid: skip_sid.map(str::to_string),
        });
    }

    /// The emits so far, oldest first.
    #[cfg(test)]
    pub fn emitted(&self) -> Vec<Emitted> {
        self.recorded.lock().clone()
    }

    #[cfg(test)]
    pub fn clear_emitted(&self) {
        self.recorded.lock().clear();
    }

    fn emit_where(&self, event: &str, data: &Value, select: impl Fn(&str) -> bool) {
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
