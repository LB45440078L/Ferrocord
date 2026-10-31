//! Integration-style tests that run real plugins against a real (logged-out) [`Client`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferrocord_core::model::{Event, Snowflake};
use ferrocord_core::{Client, ClientEvent, Extension, Flow, OutgoingMessage, Paths};
use parking_lot::Mutex;
use serde_json::json;

use crate::{LogLevel, PluginError, PluginHost};

struct Env {
    _rt: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
    client: Client,
    host: PluginHost,
}

fn env() -> Env {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let client = Client::new(rt.handle().clone(), paths);
    let host = PluginHost::new(client.clone());
    Env { _rt: rt, _dir: dir, client, host }
}

/// Writes a plugin into the plugins dir and grants `granted`.
fn write_plugin(e: &Env, id: &str, declared: &[&str], granted: &[&str], script: &str) {
    let dir = e.host.plugins_dir().join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let perms: Vec<String> = declared.iter().map(|p| format!("\"{p}\"")).collect();
    std::fs::write(dir.join("plugin.toml"), format!("id = \"{id}\"\nname = \"{id}\"\npermissions = [{}]\n", perms.join(", "))).unwrap();
    std::fs::write(dir.join("main.lua"), script).unwrap();
    for g in granted {
        e.host.grant(id, g).unwrap();
    }
}

fn wait_log(e: &Env, id: &str, needle: &str) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if e.host.logs(id).iter().any(|l| l.message.contains(needle)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn ext(e: &Env, id: &str) -> Arc<dyn Extension> {
    let ext_id = format!("plugin:{id}");
    e.client.extensions().into_iter().find(|x| x.id() == ext_id).expect("plugin extension registered")
}

fn dispatch(e: &Env, name: &str, data: serde_json::Value) {
    let (event, err) = Event::parse(name, &data);
    assert!(err.is_none(), "{err:?}");
    e.client.emit(ClientEvent::Dispatch { name: name.into(), event: Arc::new(event), raw: Arc::new(data) });
}

#[test]
fn sandbox_hides_dangerous_globals() {
    let e = env();
    write_plugin(
        &e,
        "sandbox",
        &[],
        &[],
        r#"
        local hidden = {}
        for _, name in ipairs({"io", "require", "loadstring", "load", "dofile", "debug", "getfenv", "setfenv"}) do
            if _G[name] ~= nil then table.insert(hidden, name) end
        end
        ferrocord.log("exposed: [" .. table.concat(hidden, ",") .. "]")
        ferrocord.log("os.execute: " .. tostring(os.execute) .. ", os.exit: " .. tostring(os.exit))
        local ok = pcall(function() string.upper = nil end)
        ferrocord.log("can modify string lib: " .. tostring(ok))
        local ok2 = pcall(function() ferrocord.log = nil end)
        local ok3 = pcall(function() ferrocord.json.encode = nil end)
        local ok4 = pcall(function() table.insert(ferrocord.plugin.permissions, "http") end)
        ferrocord.log("can modify api: " .. tostring(ok2 or ok3 or ok4))
        my_global = 1
        ferrocord.log("own globals: " .. tostring(my_global))
        "#,
    );
    e.host.enable("sandbox").unwrap();
    assert!(wait_log(&e, "sandbox", "exposed: []"));
    assert!(wait_log(&e, "sandbox", "os.execute: nil, os.exit: nil"));
    assert!(wait_log(&e, "sandbox", "can modify string lib: false"));
    assert!(wait_log(&e, "sandbox", "can modify api: false"));
    assert!(wait_log(&e, "sandbox", "own globals: 1"));
}

#[test]
fn functions_absent_without_grant() {
    let e = env();
    write_plugin(
        &e,
        "gated",
        &["commands", "notify", "messages.send"],
        &["commands"],
        r#"
        ferrocord.log("command=" .. type(ferrocord.command) .. " notify=" .. type(ferrocord.notify)
            .. " send=" .. type(ferrocord.send_message) .. " state=" .. type(ferrocord.state)
            .. " http=" .. type(ferrocord.http) .. " on=" .. type(ferrocord.on))
        ferrocord.log("has commands: " .. tostring(ferrocord.has_permission("commands")) .. ", has notify: " .. tostring(ferrocord.has_permission("notify")))
        if ferrocord.command then ferrocord.command("gated", "test", function() end) end
        "#,
    );
    e.host.enable("gated").unwrap();
    assert!(wait_log(&e, "gated", "command=function notify=nil send=nil state=nil http=nil on=nil"));
    assert!(wait_log(&e, "gated", "has commands: true, has notify: false"));
    assert!(e.client.local_commands().iter().any(|c| c.name == "gated" && c.owner == "plugin:gated"));

    // Granting is persisted and takes effect on the (automatic) restart.
    e.host.grant("gated", "notify").unwrap();
    assert!(wait_log(&e, "gated", "notify=function"));
    assert!(e.client.config().plugins.grants["gated"].contains(&"notify".to_string()));
    assert!(matches!(e.host.grant("gated", "http"), Err(PluginError::NotDeclared { .. })));
    assert!(matches!(e.host.grant("gated", "nope"), Err(PluginError::UnknownPermission(_))));

    let perms = e.host.permissions("gated").unwrap();
    assert_eq!(perms.iter().filter(|p| p.granted).count(), 2);

    e.host.revoke("gated", "commands").unwrap();
    assert!(wait_log(&e, "gated", "command=nil"));
}

#[test]
fn outgoing_hook_rewrites_and_cancels() {
    let e = env();
    write_plugin(
        &e,
        "rewrite",
        &["messages.modify"],
        &["messages.modify"],
        r#"
        ferrocord.on_send(function(content, info)
            if content == "cancel me" then return false end
            if info.channel_id ~= "42" then return nil end
            return string.upper(content)
        end)
        "#,
    );
    e.host.enable("rewrite").unwrap();
    let x = ext(&e, "rewrite");
    let mut msg = OutgoingMessage { channel_id: Snowflake(42), guild_id: None, content: "hello".into(), reply_to: None };
    assert_eq!(x.on_outgoing_message(&e.client, &mut msg), Flow::Continue);
    assert_eq!(msg.content, "HELLO");

    let mut other = OutgoingMessage { channel_id: Snowflake(7), guild_id: None, content: "hello".into(), reply_to: None };
    assert_eq!(x.on_outgoing_message(&e.client, &mut other), Flow::Continue);
    assert_eq!(other.content, "hello");

    let mut cancel = OutgoingMessage { channel_id: Snowflake(42), guild_id: None, content: "cancel me".into(), reply_to: None };
    assert_eq!(x.on_outgoing_message(&e.client, &mut cancel), Flow::Cancel);
}

#[test]
fn slow_outgoing_hook_times_out() {
    let e = env();
    write_plugin(&e, "slow", &["messages.modify"], &["messages.modify"], "ferrocord.on_send(function() while true do end end)");
    e.host.enable("slow").unwrap();
    let x = ext(&e, "slow");
    let mut msg = OutgoingMessage { channel_id: Snowflake(1), guild_id: None, content: "hi".into(), reply_to: None };
    let start = Instant::now();
    assert_eq!(x.on_outgoing_message(&e.client, &mut msg), Flow::Continue);
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(msg.content, "hi");
    assert!(wait_log(&e, "slow", "time limit"));
}

#[test]
fn infinite_loops_are_aborted() {
    let e = env();
    write_plugin(
        &e,
        "spin",
        &["commands"],
        &["commands"],
        r#"
        ferrocord.command("spin", "loops forever", function()
            while true do pcall(function() while true do end end) end
        end)
        ferrocord.command("ok", "", function(channel, args) ferrocord.log("still alive in " .. channel .. " with " .. args) end)
        "#,
    );
    e.host.enable("spin").unwrap();
    let x = ext(&e, "spin");
    assert!(x.run_command(&e.client, Snowflake(5), "spin", ""));
    assert!(x.run_command(&e.client, Snowflake(5), "ok", "abc"));
    assert!(!x.run_command(&e.client, Snowflake(5), "missing", ""));
    assert!(wait_log(&e, "spin", "time limit"));
    assert!(wait_log(&e, "spin", "still alive in 5 with abc"));

    // A top-level infinite loop fails the load instead of hanging the client.
    write_plugin(&e, "spin-load", &[], &[], "while true do end");
    let err = e.host.enable("spin-load").unwrap_err();
    assert!(err.to_string().contains("time limit"), "{err}");
    let info = e.host.info("spin-load").unwrap();
    assert!(info.enabled && !info.loaded && info.error.is_some());
}

#[test]
fn script_errors_do_not_kill_the_plugin() {
    let e = env();
    write_plugin(
        &e,
        "errors",
        &["commands"],
        &["commands"],
        r#"
        ferrocord.command("boom", "", function() error("kaboom") end)
        ferrocord.command("ping", "", function() ferrocord.log("pong") end)
        "#,
    );
    e.host.enable("errors").unwrap();
    let x = ext(&e, "errors");
    x.run_command(&e.client, Snowflake(1), "boom", "");
    x.run_command(&e.client, Snowflake(1), "ping", "");
    assert!(wait_log(&e, "errors", "kaboom"));
    assert!(wait_log(&e, "errors", "pong"));
    assert!(e.host.logs("errors").iter().any(|l| l.level == LogLevel::Error));

    write_plugin(&e, "syntax", &[], &[], "this is not lua");
    assert!(matches!(e.host.enable("syntax"), Err(PluginError::Script(_))));
}

#[test]
fn timers_and_events() {
    let e = env();
    write_plugin(
        &e,
        "events",
        &["events.messages"],
        &["events.messages"],
        r#"
        local n = 0
        local id
        id = ferrocord.every(50, function()
            n += 1
            if n == 3 then ferrocord.cancel(id); ferrocord.log("interval done") end
        end)
        ferrocord.after(10, function() ferrocord.log("timeout fired") end)
        ferrocord.on("MESSAGE_CREATE", function(m, name)
            ferrocord.log(name .. " " .. m.content .. " from " .. m.author.username .. " id " .. m.id)
        end)
        local ok, err = pcall(ferrocord.on, "PRESENCE_UPDATE", function() end)
        ferrocord.log("presence allowed: " .. tostring(ok))
        "#,
    );
    e.host.enable("events").unwrap();
    assert!(wait_log(&e, "events", "timeout fired"));
    assert!(wait_log(&e, "events", "interval done"));
    assert!(wait_log(&e, "events", "presence allowed: false"));
    dispatch(
        &e,
        "MESSAGE_CREATE",
        json!({"id": "1234567890123456789", "channel_id": "2", "content": "hi there", "author": {"id": "3", "username": "ferris"},
               "timestamp": "2026-01-01T00:00:00Z", "mentions": []}),
    );
    assert!(wait_log(&e, "events", "MESSAGE_CREATE hi there from ferris id 1234567890123456789"));
}

#[test]
fn storage_persists_across_reloads() {
    let e = env();
    write_plugin(
        &e,
        "store",
        &["storage"],
        &["storage"],
        r#"
        local runs = (ferrocord.storage.get("runs") or 0) + 1
        ferrocord.storage.set("runs", runs)
        ferrocord.storage.set("nested", { list = {1, 2, 3}, name = "x" })
        local nested = ferrocord.storage.get("nested")
        ferrocord.log("runs=" .. runs .. " list=" .. #nested.list .. " keys=" .. #ferrocord.storage.keys())
        "#,
    );
    e.host.enable("store").unwrap();
    assert!(wait_log(&e, "store", "runs=1 list=3 keys=2"));
    e.host.reload("store").unwrap();
    assert!(wait_log(&e, "store", "runs=2"));
    assert!(e.host.plugins_dir().join("store").join(crate::STORAGE_FILE).is_file());
}

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/examples")
}

#[test]
fn example_plugins_install_and_run() {
    let e = env();
    let notes: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let n = notes.clone();
    e.host.set_notifier(move |t, b| n.lock().push((t, b)));

    for id in ["hello", "keyword-alerts", "message-logger"] {
        let m = e.host.install_from_dir(&examples_dir().join(id)).unwrap();
        assert_eq!(m.id, id);
        e.host.grant_all(id).unwrap();
        e.host.enable(id).unwrap();
    }
    let found = e.host.discover();
    assert_eq!(found.len(), 3);
    assert!(found.iter().all(|p| p.enabled && p.loaded && p.error.is_none() && p.permissions.iter().all(|s| s.granted)));
    let names: Vec<String> = e.client.local_commands().into_iter().map(|c| c.name).collect();
    for c in ["hello", "alerts", "deleted"] {
        assert!(names.contains(&c.to_string()), "{names:?}");
    }

    // keyword-alerts: add a keyword, then a matching message triggers a notification.
    ext(&e, "keyword-alerts").run_command(&e.client, Snowflake(2), "alerts", "add Rust");
    assert!(wait_log(&e, "keyword-alerts", "Watching for \"rust\""));
    dispatch(
        &e,
        "MESSAGE_CREATE",
        json!({"id": "10", "channel_id": "2", "content": "I love RUST", "author": {"id": "3", "username": "ferris"},
               "timestamp": "2026-01-01T00:00:00Z", "mentions": []}),
    );
    let start = Instant::now();
    while !notes.lock().iter().any(|(_, b)| b == "ferris: I love RUST") && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(notes.lock().iter().any(|(t, b)| t.contains("\"rust\"") && b == "ferris: I love RUST"), "{:?}", notes.lock());

    // message-logger: a deleted message is remembered and shown by /deleted.
    dispatch(&e, "MESSAGE_DELETE", json!({"id": "10", "channel_id": "2"}));
    std::thread::sleep(Duration::from_millis(100));
    ext(&e, "message-logger").run_command(&e.client, Snowflake(2), "deleted", "");
    assert!(wait_log(&e, "message-logger", "ferris in <#2>: I love RUST"));

    // Reinstalling keeps storage; uninstalling removes everything.
    e.host.install_from_dir(&examples_dir().join("keyword-alerts")).unwrap();
    ext(&e, "keyword-alerts").run_command(&e.client, Snowflake(2), "alerts", "list");
    assert!(wait_log(&e, "keyword-alerts", "Watching: rust"));
    e.host.uninstall("keyword-alerts").unwrap();
    assert!(!e.host.is_loaded("keyword-alerts"));
    assert!(e.host.info("keyword-alerts").is_none());
    assert!(!e.client.config().plugins.enabled.contains(&"keyword-alerts".to_string()));

    e.host.disable("hello");
    assert!(!e.client.local_commands().iter().any(|c| c.name == "hello"));
    e.host.shutdown();
    assert!(!e.host.is_loaded("message-logger"));
}
