//! Glue between the UI and the plugin host / local API server, plus small
//! channels used to hand background results back to the UI thread.

use std::path::PathBuf;
use std::sync::mpsc;

use ferrocord_core::Client;
use ferrocord_core::model::{Message, Snowflake};

pub struct PluginRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub enabled: bool,
    pub error: Option<String>,
    pub declared: Vec<String>,
    pub granted: Vec<String>,
}

pub struct Integrations {
    client: Client,
    pub nav_tx: mpsc::Sender<Snowflake>,
    pub nav_rx: mpsc::Receiver<Snowflake>,
    pub pins_tx: mpsc::Sender<(Snowflake, Result<Vec<Message>, String>)>,
    pub pins_rx: mpsc::Receiver<(Snowflake, Result<Vec<Message>, String>)>,
    pub join_tx: mpsc::Sender<String>,
    pub join_rx: mpsc::Receiver<String>,
}

impl Integrations {
    pub fn start(client: &Client) -> Self {
        let (nav_tx, nav_rx) = mpsc::channel();
        let (pins_tx, pins_rx) = mpsc::channel();
        let (join_tx, join_rx) = mpsc::channel();
        Self { client: client.clone(), nav_tx, nav_rx, pins_tx, pins_rx, join_tx, join_rx }
    }

    pub fn stop(&mut self) {}

    pub fn plugins_dir(&self) -> PathBuf {
        self.client.paths().plugins_dir()
    }

    pub fn rescan(&mut self) {}

    pub fn plugin_list(&self) -> Vec<PluginRow> {
        vec![]
    }

    pub fn set_plugin_enabled(&mut self, _id: &str, _on: bool) {}

    pub fn reload_plugin(&mut self, _id: &str) {}

    pub fn grant(&mut self, _id: &str, _perm: &str, _on: bool) {}

    pub fn apply_api_config(&mut self) {}

    pub fn api_status(&self) -> Option<(String, String)> {
        None
    }

    pub fn api_error(&self) -> Option<String> {
        None
    }

    pub fn regenerate_api_token(&mut self) {}
}
