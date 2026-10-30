//! The [`Extension`] registered with the client on behalf of a plugin thread.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use ferrocord_core::model::Snowflake;
use ferrocord_core::{Client, ClientEvent, Extension, Flow, LocalCommand, OutgoingMessage};
use parking_lot::Mutex;

use crate::manifest::Permission;
use crate::runtime::{HookResult, OUTGOING_TIMEOUT, Running, Shared, ToPlugin};

pub(crate) struct PluginProxy {
    shared: Arc<Shared>,
    tx: SyncSender<ToPlugin>,
    exited: Mutex<Option<Receiver<()>>>,
}

impl PluginProxy {
    pub fn new(r: Running) -> Self {
        Self { shared: r.shared, tx: r.tx, exited: Mutex::new(Some(r.exited)) }
    }

    pub fn plugin_id(&self) -> &str {
        &self.shared.id
    }

    /// Asks the thread to stop (idempotent).
    pub fn stop(&self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        let _ = self.tx.try_send(ToPlugin::Shutdown);
    }

    /// Blocks until the plugin thread has exited or `timeout` elapses.
    pub fn wait_exit(&self, timeout: Duration) {
        if let Some(rx) = self.exited.lock().take() {
            let _ = rx.recv_timeout(timeout);
        }
    }

    fn post(&self, msg: ToPlugin) -> bool {
        if self.shared.stopping.load(Ordering::Relaxed) {
            return false;
        }
        match self.tx.try_send(msg) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                tracing::warn!(plugin = %self.shared.id, "plugin queue is full; dropping message");
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    fn on_plugin_thread(&self) -> bool {
        self.shared.thread.get() == Some(&std::thread::current().id())
    }
}

impl Extension for PluginProxy {
    /// `plugin:<id>`, so plugin ids can never collide with built-in extensions.
    #[allow(clippy::misnamed_getters)]
    fn id(&self) -> &str {
        &self.shared.ext_id
    }

    fn on_event(&self, _client: &Client, event: &ClientEvent) {
        let name = event.name();
        if !self.shared.wants_event(name) {
            return;
        }
        let mut json = event.to_json();
        let data = json.get_mut("data").map(serde_json::Value::take).unwrap_or_default();
        self.post(ToPlugin::Event { name: name.to_string(), data });
    }

    fn on_outgoing_message(&self, _client: &Client, msg: &mut OutgoingMessage) -> Flow {
        if !self.shared.has(Permission::MessagesModify) || !self.shared.send_hook.load(Ordering::SeqCst) || self.on_plugin_thread() {
            return Flow::Continue;
        }
        let (reply, rx) = sync_channel(1);
        if !self.post(ToPlugin::Outgoing { msg: msg.clone(), sent: Instant::now(), reply }) {
            return Flow::Continue;
        }
        match rx.recv_timeout(OUTGOING_TIMEOUT) {
            Ok(HookResult::Cancel) => Flow::Cancel,
            Ok(HookResult::Continue(Some(content))) => {
                msg.content = content;
                Flow::Continue
            }
            Ok(HookResult::Continue(None)) => Flow::Continue,
            Err(_) => {
                tracing::warn!(plugin = %self.shared.id, "outgoing message hook timed out");
                Flow::Continue
            }
        }
    }

    fn commands(&self) -> Vec<LocalCommand> {
        if !self.shared.has(Permission::Commands) {
            return vec![];
        }
        self.shared.commands.read().clone()
    }

    fn run_command(&self, _client: &Client, channel: Snowflake, name: &str, args: &str) -> bool {
        if !self.shared.has(Permission::Commands) || !self.shared.commands.read().iter().any(|c| c.name == name) {
            return false;
        }
        self.post(ToPlugin::Command { name: name.to_string(), channel, args: args.trim().to_string() });
        true
    }

    fn shutdown(&self) {
        self.stop();
    }
}
