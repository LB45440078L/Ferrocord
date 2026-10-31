//! The extension interface: how plugins, the local API server and built-in
//! features hook into the client without the core knowing about them.

use ferrocord_model::Snowflake;
use serde::{Deserialize, Serialize};

use crate::{Client, ClientEvent};

/// Result of an outgoing-message hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// Drop the message (e.g. a local command handled it).
    Cancel,
}

/// A client-side text command such as `/shrug`, provided by an extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalCommand {
    pub name: String,
    pub description: String,
    /// Extension that owns the command.
    pub owner: String,
}

/// Context passed to outgoing-message hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub content: String,
    pub reply_to: Option<Snowflake>,
}

/// Implemented by anything that wants to observe or influence the client.
///
/// All hooks run on the client's event task; they must be quick and must not
/// block. Spawn work on the Tokio runtime for anything slow.
pub trait Extension: Send + Sync {
    /// Stable unique id (e.g. `"builtin.commands"` or a plugin id).
    fn id(&self) -> &str;

    /// Every client event after it has been applied to the store.
    fn on_event(&self, _client: &Client, _event: &ClientEvent) {}

    /// Called before a message is sent. May rewrite `msg.content` or cancel.
    fn on_outgoing_message(&self, _client: &Client, _msg: &mut OutgoingMessage) -> Flow {
        Flow::Continue
    }

    /// Local commands contributed by this extension.
    fn commands(&self) -> Vec<LocalCommand> {
        vec![]
    }

    /// Runs a local command. Return `true` if handled.
    fn run_command(&self, _client: &Client, _channel: Snowflake, _name: &str, _args: &str) -> bool {
        false
    }

    /// Called when the extension is removed or the client shuts down.
    fn shutdown(&self) {}
}

/// Built-in text commands that the official client also offers.
pub struct BuiltinCommands;

impl BuiltinCommands {
    const SIMPLE: &'static [(&'static str, &'static str, &'static str)] = &[
        ("shrug", "Appends ¯\\_(ツ)_/¯ to your message", "¯\\\\_(ツ)\\_/¯"),
        ("tableflip", "Appends (╯°□°)╯︵ ┻━┻ to your message", "(╯°□°)╯︵ ┻━┻"),
        ("unflip", "Appends ┬─┬ノ( º _ ºノ) to your message", "┬─┬ノ( º _ ºノ)"),
        ("lenny", "Appends ( ͡° ͜ʖ ͡°) to your message", "( ͡° ͜ʖ ͡°)"),
    ];
}

impl Extension for BuiltinCommands {
    fn id(&self) -> &str {
        "builtin.commands"
    }

    fn commands(&self) -> Vec<LocalCommand> {
        let mut v: Vec<LocalCommand> =
            Self::SIMPLE.iter().map(|(n, d, _)| LocalCommand { name: (*n).into(), description: (*d).into(), owner: self.id().into() }).collect();
        v.push(LocalCommand { name: "me".into(), description: "Displays text with emphasis".into(), owner: self.id().into() });
        v.push(LocalCommand { name: "spoiler".into(), description: "Marks your message as a spoiler".into(), owner: self.id().into() });
        v.push(LocalCommand { name: "nick".into(), description: "Change your nickname on this server".into(), owner: self.id().into() });
        v
    }

    fn on_outgoing_message(&self, client: &Client, msg: &mut OutgoingMessage) -> Flow {
        let Some(rest) = msg.content.strip_prefix('/') else { return Flow::Continue };
        let (cmd, args) = rest.split_once(' ').unwrap_or((rest, ""));
        let args = args.trim();
        if let Some((_, _, suffix)) = Self::SIMPLE.iter().find(|(n, _, _)| *n == cmd) {
            msg.content = if args.is_empty() { (*suffix).to_string() } else { format!("{args} {suffix}") };
            return Flow::Continue;
        }
        match cmd {
            "me" if !args.is_empty() => msg.content = format!("_{args}_"),
            "spoiler" if !args.is_empty() => msg.content = format!("||{args}||"),
            "nick" => {
                if let Some(g) = msg.guild_id {
                    client.set_nickname(g, if args.is_empty() { None } else { Some(args.to_string()) });
                }
                return Flow::Cancel;
            }
            _ => {}
        }
        Flow::Continue
    }
}
