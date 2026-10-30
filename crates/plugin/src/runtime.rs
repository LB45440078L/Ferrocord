//! The per-plugin thread: owns the Luau VM, its timer queue and every Lua
//! callback. The host talks to it exclusively through a bounded channel.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, OnceLock};
use std::thread::ThreadId;
use std::time::{Duration, Instant, SystemTime};

use ferrocord_core::model::{AllowedMentions, CreateMessage, MessageReference, Snowflake};
use ferrocord_core::store::{GuildListEntry, Store};
use ferrocord_core::{Client, LocalCommand, OutgoingMessage};
use mlua::chunk::ChunkMode;
use mlua::{Function, Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib, Table, Value as LuaValue, VmState};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde_json::{Value, json};

use crate::manifest::{Permission, PluginManifest};
use crate::storage::{STORAGE_FILE, Storage};
use crate::PluginError;

/// Time a single Lua callback (event handler, command, timer) may run.
pub const CALLBACK_BUDGET: Duration = Duration::from_millis(250);
/// Time the top-level entry script may run while loading.
pub const LOAD_BUDGET: Duration = Duration::from_secs(2);
/// How long the client waits for a plugin's outgoing-message hooks.
pub const OUTGOING_TIMEOUT: Duration = Duration::from_millis(150);
/// Heap limit of a plugin's Luau VM.
pub const MEMORY_LIMIT: usize = 64 << 20;
/// Messages queued for a plugin before new events are dropped.
const QUEUE_CAPACITY: usize = 1024;
const MAX_TIMERS: usize = 256;
const MIN_INTERVAL: Duration = Duration::from_millis(50);
const MAX_PENDING: usize = 256;
/// `ferrocord.send_message`/`reply` rate limit: this many messages per window.
const SEND_LIMIT: usize = 5;
const SEND_WINDOW: Duration = Duration::from_secs(5);
const MAX_HTTP_BODY: usize = 4 << 20;
const MAX_SCRIPT_BYTES: u64 = 4 << 20;
const LOG_CAPACITY: usize = 200;

/// Severity of a [`PluginLog`] line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Error,
}

/// A line of plugin output (`ferrocord.log`, `print`, callback errors).
#[derive(Debug, Clone, Serialize)]
pub struct PluginLog {
    pub level: LogLevel,
    pub message: String,
    pub time: SystemTime,
}

pub(crate) type LogBuffer = Arc<Mutex<VecDeque<PluginLog>>>;

/// Callback the UI installs to display plugin notifications: `(title, body)`.
pub type Notifier = Arc<dyn Fn(String, String) + Send + Sync>;
pub(crate) type NotifierSlot = Arc<RwLock<Option<Notifier>>>;

pub(crate) fn push_log(logs: &LogBuffer, id: &str, level: LogLevel, message: String) {
    match level {
        LogLevel::Info => tracing::info!(plugin = id, "{message}"),
        LogLevel::Error => tracing::warn!(plugin = id, "{message}"),
    }
    let mut l = logs.lock();
    if l.len() >= LOG_CAPACITY {
        l.pop_front();
    }
    l.push_back(PluginLog { level, message, time: SystemTime::now() });
}

/// Messages from the host to a plugin thread.
pub(crate) enum ToPlugin {
    Event { name: String, data: Value },
    Outgoing { msg: OutgoingMessage, sent: Instant, reply: SyncSender<HookResult> },
    Command { name: String, channel: Snowflake, args: String },
    Completion { id: u64, result: Result<Value, String> },
    Shutdown,
}

/// Outcome of the plugin's outgoing-message hooks.
pub(crate) enum HookResult {
    Continue(Option<String>),
    Cancel,
}

/// State visible to both the plugin thread and its [`Extension`](ferrocord_core::Extension) proxy.
pub(crate) struct Shared {
    pub id: String,
    pub ext_id: String,
    pub perms: BTreeSet<Permission>,
    pub events: RwLock<HashSet<String>>,
    pub commands: RwLock<Vec<LocalCommand>>,
    pub send_hook: AtomicBool,
    pub thread: OnceLock<ThreadId>,
    pub stopping: AtomicBool,
}

impl Shared {
    pub fn has(&self, p: Permission) -> bool {
        self.perms.contains(&p)
    }

    /// Whether the plugin may receive an event with this name.
    pub fn may_receive(&self, name: &str) -> bool {
        self.has(Permission::EventsAll) || (self.has(Permission::EventsMessages) && is_message_event(name))
    }

    /// Whether the event should be forwarded (subscribed and permitted).
    pub fn wants_event(&self, name: &str) -> bool {
        let subs = self.events.read();
        !subs.is_empty() && (subs.contains(name) || subs.contains("*")) && self.may_receive(name)
    }
}

fn is_message_event(name: &str) -> bool {
    name.starts_with("MESSAGE_")
}

/// Everything needed to start a plugin thread.
pub(crate) struct Launch {
    pub client: Client,
    pub manifest: PluginManifest,
    pub dir: PathBuf,
    pub perms: BTreeSet<Permission>,
    pub notifier: NotifierSlot,
    pub logs: LogBuffer,
}

/// A started plugin thread.
pub(crate) struct Running {
    pub shared: Arc<Shared>,
    pub tx: SyncSender<ToPlugin>,
    /// Disconnects when the thread exits.
    pub exited: Receiver<()>,
}

/// Spawns the plugin thread and waits until its entry script has run.
pub(crate) fn spawn(launch: Launch) -> Result<Running, PluginError> {
    let id = launch.manifest.id.clone();
    let shared = Arc::new(Shared {
        id: id.clone(),
        ext_id: format!("plugin:{id}"),
        perms: launch.perms.clone(),
        events: RwLock::new(HashSet::new()),
        commands: RwLock::new(vec![]),
        send_hook: AtomicBool::new(false),
        thread: OnceLock::new(),
        stopping: AtomicBool::new(false),
    });
    let (tx, rx) = sync_channel::<ToPlugin>(QUEUE_CAPACITY);
    let (ready_tx, ready_rx) = sync_channel::<Result<(), String>>(1);
    let (exit_tx, exit_rx) = sync_channel::<()>(1);
    let thread = {
        let shared = shared.clone();
        let tx = tx.clone();
        std::thread::Builder::new().name(format!("plugin:{id}")).stack_size(8 << 20).spawn(move || {
            let _exit = exit_tx;
            thread_main(launch, shared, tx, rx, ready_tx);
        })?
    };
    let _ = shared.thread.set(thread.thread().id());
    match ready_rx.recv_timeout(LOAD_BUDGET + Duration::from_secs(3)) {
        Ok(Ok(())) => Ok(Running { shared, tx, exited: exit_rx }),
        Ok(Err(e)) => Err(PluginError::Script(e)),
        Err(RecvTimeoutError::Disconnected) => Err(PluginError::Script("plugin thread exited during load".into())),
        Err(RecvTimeoutError::Timeout) => {
            shared.stopping.store(true, Ordering::SeqCst);
            let _ = tx.try_send(ToPlugin::Shutdown);
            Err(PluginError::LoadTimeout)
        }
    }
}

fn thread_main(launch: Launch, shared: Arc<Shared>, tx: SyncSender<ToPlugin>, rx: Receiver<ToPlugin>, ready: SyncSender<Result<(), String>>) {
    let logs = launch.logs.clone();
    let id = shared.id.clone();
    let rt = match Runtime::new(launch, shared, tx) {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready.send(Err(format!("failed to initialise Luau: {e}")));
            return;
        }
    };
    if let Err(e) = rt.load_entry() {
        let _ = ready.send(Err(e));
        rt.teardown();
        return;
    }
    let _ = ready.send(Ok(()));
    push_log(&logs, &id, LogLevel::Info, "loaded".into());
    rt.event_loop(&rx);
    rt.teardown();
    tracing::debug!(plugin = %id, "plugin thread stopped");
}

struct Timer {
    due: Instant,
    interval: Option<Duration>,
    func: Function,
}

/// Plugin-thread-local state shared by all Lua API closures.
struct Ctx {
    client: Client,
    shared: Arc<Shared>,
    manifest: PluginManifest,
    dir: PathBuf,
    tx: SyncSender<ToPlugin>,
    notifier: NotifierSlot,
    logs: LogBuffer,
    deadline: Rc<Cell<Option<Instant>>>,
    handlers: RefCell<HashMap<String, Vec<Function>>>,
    send_hooks: RefCell<Vec<Function>>,
    commands: RefCell<HashMap<String, Function>>,
    timers: RefCell<HashMap<u64, Timer>>,
    pending: RefCell<HashMap<u64, Function>>,
    next_id: Cell<u64>,
    storage: RefCell<Option<Storage>>,
    sends: RefCell<VecDeque<Instant>>,
}

impl Ctx {
    fn log(&self, level: LogLevel, message: String) {
        push_log(&self.logs, &self.shared.id, level, message);
    }

    fn next_id(&self) -> u64 {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        id
    }

    /// Runs a Lua function under a time budget; errors are logged, never propagated.
    fn call<R: mlua::FromLuaMulti>(&self, what: &str, f: &Function, args: impl mlua::IntoLuaMulti, budget: Duration) -> Option<R> {
        let prev = self.deadline.get();
        let limit = Instant::now() + budget;
        self.deadline.set(Some(prev.map_or(limit, |p| p.min(limit))));
        let r = f.call::<R>(args);
        self.deadline.set(prev);
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.log(LogLevel::Error, format!("error in {what}: {e}"));
                None
            }
        }
    }

    fn register_pending(&self, f: Function) -> mlua::Result<u64> {
        let mut p = self.pending.borrow_mut();
        if p.len() >= MAX_PENDING {
            return Err(mlua::Error::runtime("too many pending requests"));
        }
        let id = self.next_id();
        p.insert(id, f);
        Ok(id)
    }

    fn add_timer(&self, ms: f64, interval: bool, func: Function) -> mlua::Result<u64> {
        if !ms.is_finite() || ms < 0.0 {
            return Err(mlua::Error::runtime("delay must be a non-negative number of milliseconds"));
        }
        let mut d = Duration::from_millis(ms.min(1e12) as u64);
        if interval {
            d = d.max(MIN_INTERVAL);
        }
        let mut timers = self.timers.borrow_mut();
        if timers.len() >= MAX_TIMERS {
            return Err(mlua::Error::runtime(format!("too many timers (max {MAX_TIMERS})")));
        }
        let id = self.next_id();
        timers.insert(id, Timer { due: Instant::now() + d, interval: interval.then_some(d), func });
        Ok(id)
    }

    fn next_due(&self) -> Option<Instant> {
        self.timers.borrow().values().map(|t| t.due).min()
    }

    fn check_send_rate(&self) -> mlua::Result<()> {
        let mut s = self.sends.borrow_mut();
        let now = Instant::now();
        while s.front().is_some_and(|t| now.duration_since(*t) > SEND_WINDOW) {
            s.pop_front();
        }
        if s.len() >= SEND_LIMIT {
            return Err(mlua::Error::runtime(format!(
                "rate limited: at most {SEND_LIMIT} messages per {}s",
                SEND_WINDOW.as_secs()
            )));
        }
        s.push_back(now);
        Ok(())
    }

    /// Spawns an async job on the client runtime whose result is delivered back
    /// to `cb` (if any) on the plugin thread.
    fn spawn_job<F>(&self, cb: Option<Function>, what: &'static str, fut: F) -> mlua::Result<()>
    where
        F: std::future::Future<Output = Result<Value, String>> + Send + 'static,
    {
        let pid = match cb {
            Some(f) => Some(self.register_pending(f)?),
            None => None,
        };
        let tx = self.tx.clone();
        let logs = self.logs.clone();
        let plugin = self.shared.id.clone();
        self.client.runtime().spawn(async move {
            let result = fut.await;
            match pid {
                Some(id) => {
                    let _ = tx.try_send(ToPlugin::Completion { id, result });
                }
                None => {
                    if let Err(e) = result {
                        push_log(&logs, &plugin, LogLevel::Error, format!("{what} failed: {e}"));
                    }
                }
            }
        });
        Ok(())
    }
}

struct Runtime {
    lua: Lua,
    ctx: Rc<Ctx>,
}

impl Runtime {
    fn new(launch: Launch, shared: Arc<Shared>, tx: SyncSender<ToPlugin>) -> mlua::Result<Self> {
        let libs = StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::BIT | StdLib::MATH | StdLib::BUFFER | StdLib::OS;
        let lua = Lua::new_with(libs, LuaOptions::default())?;
        lua.set_memory_limit(MEMORY_LIMIT)?;
        let storage = shared.has(Permission::Storage).then(|| Storage::open(launch.dir.join(STORAGE_FILE)));
        let ctx = Rc::new(Ctx {
            client: launch.client,
            shared,
            manifest: launch.manifest,
            dir: launch.dir,
            tx,
            notifier: launch.notifier,
            logs: launch.logs,
            deadline: Rc::new(Cell::new(None)),
            handlers: RefCell::new(HashMap::new()),
            send_hooks: RefCell::new(vec![]),
            commands: RefCell::new(HashMap::new()),
            timers: RefCell::new(HashMap::new()),
            pending: RefCell::new(HashMap::new()),
            next_id: Cell::new(0),
            storage: RefCell::new(storage),
            sends: RefCell::new(VecDeque::new()),
        });

        let globals = lua.globals();
        for name in ["require", "loadstring", "load", "dofile", "loadfile", "getfenv", "setfenv", "debug"] {
            globals.raw_set(name, LuaValue::Nil)?;
        }
        let c = ctx.clone();
        globals.raw_set(
            "print",
            lua.create_function(move |_, args: MultiValue| {
                c.log(LogLevel::Info, join_values(&args));
                Ok(())
            })?,
        )?;
        globals.raw_set("ferrocord", build_api(&lua, &ctx)?)?;
        lua.sandbox(true)?;

        let deadline = ctx.deadline.clone();
        lua.set_interrupt(move |_| match deadline.get() {
            Some(d) if Instant::now() > d => Err(mlua::Error::runtime("script exceeded its time limit and was aborted")),
            _ => Ok(VmState::Continue),
        });
        Ok(Self { lua, ctx })
    }

    fn load_entry(&self) -> Result<(), String> {
        let path = self.ctx.dir.join(&self.ctx.manifest.entry);
        let meta = std::fs::metadata(&path).map_err(|e| format!("cannot read {}: {e}", self.ctx.manifest.entry))?;
        if meta.len() > MAX_SCRIPT_BYTES {
            return Err(format!("{} is too large", self.ctx.manifest.entry));
        }
        let src = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", self.ctx.manifest.entry))?;
        self.ctx.deadline.set(Some(Instant::now() + LOAD_BUDGET));
        let r = self
            .lua
            .load(src.as_str())
            .set_name(format!("={}/{}", self.ctx.manifest.id, self.ctx.manifest.entry))
            .set_mode(ChunkMode::Text)
            .exec();
        self.ctx.deadline.set(None);
        r.map_err(|e| e.to_string())
    }

    fn event_loop(&self, rx: &Receiver<ToPlugin>) {
        loop {
            if self.ctx.shared.stopping.load(Ordering::SeqCst) {
                break;
            }
            self.run_due_timers();
            let msg = match self.ctx.next_due() {
                Some(due) => match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(m) => m,
                    Err(_) => break,
                },
            };
            match msg {
                ToPlugin::Shutdown => break,
                m => self.handle(m),
            }
        }
    }

    fn run_due_timers(&self) {
        let now = Instant::now();
        let mut due: Vec<(Instant, u64)> =
            self.ctx.timers.borrow().iter().filter(|(_, t)| t.due <= now).map(|(id, t)| (t.due, *id)).collect();
        due.sort();
        for (_, id) in due {
            let func = {
                let mut timers = self.ctx.timers.borrow_mut();
                let Some(t) = timers.get_mut(&id) else { continue };
                let f = t.func.clone();
                match t.interval {
                    Some(i) => t.due = Instant::now() + i,
                    None => {
                        timers.remove(&id);
                    }
                }
                f
            };
            self.ctx.call::<()>(&format!("timer {id}"), &func, (), CALLBACK_BUDGET);
        }
    }

    fn handle(&self, msg: ToPlugin) {
        let ctx = &self.ctx;
        match msg {
            ToPlugin::Event { name, data } => {
                let funcs: Vec<Function> = {
                    let h = ctx.handlers.borrow();
                    h.get(&name).into_iter().chain(h.get("*")).flatten().cloned().collect()
                };
                if funcs.is_empty() {
                    return;
                }
                let v = match to_lua(&self.lua, &data) {
                    Ok(v) => v,
                    Err(e) => return ctx.log(LogLevel::Error, format!("could not convert {name}: {e}")),
                };
                for f in funcs {
                    ctx.call::<()>(&format!("handler for {name}"), &f, (v.clone(), name.as_str()), CALLBACK_BUDGET);
                }
            }
            ToPlugin::Outgoing { msg, sent, reply } => {
                let remaining = OUTGOING_TIMEOUT.saturating_sub(sent.elapsed() + Duration::from_millis(10));
                if remaining.is_zero() {
                    return;
                }
                let hooks = ctx.send_hooks.borrow().clone();
                let info = (|| -> mlua::Result<Table> {
                    let t = self.lua.create_table()?;
                    t.set("channel_id", msg.channel_id.to_string())?;
                    t.set("guild_id", msg.guild_id.map(|g| g.to_string()))?;
                    t.set("reply_to", msg.reply_to.map(|g| g.to_string()))?;
                    Ok(t)
                })();
                let Ok(info) = info else { return };
                let deadline = sent + OUTGOING_TIMEOUT.saturating_sub(Duration::from_millis(10));
                let mut content = msg.content;
                let mut changed = false;
                for f in hooks {
                    let budget = deadline.saturating_duration_since(Instant::now());
                    if budget.is_zero() {
                        break;
                    }
                    match ctx.call::<LuaValue>("on_send hook", &f, (content.as_str(), info.clone()), budget) {
                        Some(LuaValue::String(s)) => {
                            content = s.to_string_lossy();
                            changed = true;
                        }
                        Some(LuaValue::Boolean(false)) => {
                            let _ = reply.try_send(HookResult::Cancel);
                            return;
                        }
                        _ => {}
                    }
                }
                let _ = reply.try_send(HookResult::Continue(changed.then_some(content)));
            }
            ToPlugin::Command { name, channel, args } => {
                let f = ctx.commands.borrow().get(&name).cloned();
                if let Some(f) = f {
                    ctx.call::<()>(&format!("command /{name}"), &f, (channel.to_string(), args), CALLBACK_BUDGET);
                }
            }
            ToPlugin::Completion { id, result } => {
                let Some(f) = ctx.pending.borrow_mut().remove(&id) else { return };
                match result {
                    Ok(v) => match to_lua(&self.lua, &v) {
                        Ok(v) => {
                            ctx.call::<()>("callback", &f, (v, LuaValue::Nil), CALLBACK_BUDGET);
                        }
                        Err(e) => ctx.log(LogLevel::Error, format!("could not convert result: {e}")),
                    },
                    Err(e) => {
                        ctx.call::<()>("callback", &f, (LuaValue::Nil, e), CALLBACK_BUDGET);
                    }
                }
            }
            ToPlugin::Shutdown => {}
        }
    }

    /// Drops every Lua reference before the VM itself.
    fn teardown(self) {
        let ctx = &self.ctx;
        ctx.shared.stopping.store(true, Ordering::SeqCst);
        ctx.shared.events.write().clear();
        ctx.shared.commands.write().clear();
        ctx.shared.send_hook.store(false, Ordering::SeqCst);
        ctx.handlers.borrow_mut().clear();
        ctx.send_hooks.borrow_mut().clear();
        ctx.commands.borrow_mut().clear();
        ctx.timers.borrow_mut().clear();
        ctx.pending.borrow_mut().clear();
        self.lua.remove_interrupt();
        drop(self.lua);
    }
}

// ---------------------------------------------------------------- conversions

fn to_lua(lua: &Lua, v: &Value) -> mlua::Result<LuaValue> {
    let opts = mlua::serde::ser::Options::new().serialize_none_to_null(false).serialize_unit_to_null(false).set_array_metatable(false);
    lua.to_value_with(v, opts)
}

fn ser_to_lua(lua: &Lua, v: &impl Serialize) -> mlua::Result<LuaValue> {
    to_lua(lua, &serde_json::to_value(v).map_err(mlua::Error::external)?)
}

fn join_values(args: &MultiValue) -> String {
    args.iter().map(|v| v.to_string().unwrap_or_else(|_| format!("{v:?}"))).collect::<Vec<_>>().join("\t")
}

fn snowflake(v: &LuaValue, what: &str) -> mlua::Result<Snowflake> {
    let bad = || mlua::Error::runtime(format!("{what} must be a snowflake id string"));
    match v {
        LuaValue::String(s) => s.to_str().map_err(|_| bad())?.trim().parse::<u64>().map(Snowflake).map_err(|_| bad()),
        LuaValue::Integer(i) if *i > 0 => Ok(Snowflake(*i as u64)),
        LuaValue::Number(n) if n.fract() == 0.0 && *n > 0.0 && *n < 9.0e15 => Ok(Snowflake(*n as u64)),
        _ => Err(bad()),
    }
}

fn opt_snowflake(v: &LuaValue, what: &str) -> mlua::Result<Option<Snowflake>> {
    match v {
        LuaValue::Nil => Ok(None),
        v => snowflake(v, what).map(Some),
    }
}

// ---------------------------------------------------------------- Lua API

/// Builds the `ferrocord` global. Functions guarded by a permission are only
/// present when the plugin both declares and was granted it.
fn build_api(lua: &Lua, ctx: &Rc<Ctx>) -> mlua::Result<Table> {
    let api = lua.create_table()?;
    let shared = ctx.shared.clone();
    let m = &ctx.manifest;

    let info = lua.create_table()?;
    info.set("id", m.id.as_str())?;
    info.set("name", m.name.as_str())?;
    info.set("version", m.version.as_str())?;
    info.set("author", m.author.as_str())?;
    info.set("description", m.description.as_str())?;
    info.set("permissions", lua.create_sequence_from(shared.perms.iter().map(|p| p.as_str()))?)?;
    api.set("plugin", info)?;

    let c = ctx.clone();
    api.set(
        "log",
        lua.create_function(move |_, args: MultiValue| {
            c.log(LogLevel::Info, join_values(&args));
            Ok(())
        })?,
    )?;

    let s = shared.clone();
    api.set(
        "has_permission",
        lua.create_function(move |_, name: String| Ok(name.parse::<Permission>().is_ok_and(|p| s.has(p))))?,
    )?;

    let c = ctx.clone();
    api.set("after", lua.create_function(move |_, (ms, f): (f64, Function)| c.add_timer(ms, false, f))?)?;
    let c = ctx.clone();
    api.set("every", lua.create_function(move |_, (ms, f): (f64, Function)| c.add_timer(ms, true, f))?)?;
    let c = ctx.clone();
    api.set("cancel", lua.create_function(move |_, id: u64| Ok(c.timers.borrow_mut().remove(&id).is_some()))?)?;

    let json_t = lua.create_table()?;
    json_t.set(
        "encode",
        lua.create_function(|lua, v: LuaValue| {
            let v: Value = lua.from_value(v)?;
            serde_json::to_string(&v).map_err(mlua::Error::external)
        })?,
    )?;
    json_t.set(
        "decode",
        lua.create_function(|lua, s: mlua::LuaString| {
            let v: Value = serde_json::from_slice(&s.as_bytes()).map_err(mlua::Error::external)?;
            to_lua(lua, &v)
        })?,
    )?;
    api.set("json", json_t)?;

    if shared.has(Permission::EventsMessages) || shared.has(Permission::EventsAll) {
        let c = ctx.clone();
        api.set(
            "on",
            lua.create_function(move |_, (name, f): (String, Function)| {
                let name = name.trim().to_ascii_uppercase();
                if name != "*" && !c.shared.may_receive(&name) {
                    return Err(mlua::Error::runtime(format!(
                        "not permitted to receive {name} (requires the events.all permission)"
                    )));
                }
                c.handlers.borrow_mut().entry(name.clone()).or_default().push(f);
                c.shared.events.write().insert(name);
                Ok(())
            })?,
        )?;
    }

    if shared.has(Permission::MessagesModify) {
        let c = ctx.clone();
        api.set(
            "on_send",
            lua.create_function(move |_, f: Function| {
                c.send_hooks.borrow_mut().push(f);
                c.shared.send_hook.store(true, Ordering::SeqCst);
                Ok(())
            })?,
        )?;
    }

    if shared.has(Permission::Commands) {
        let c = ctx.clone();
        api.set(
            "command",
            lua.create_function(move |_, (name, description, f): (String, String, Function)| {
                let name = name.trim().trim_start_matches('/').to_ascii_lowercase();
                let valid = !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
                if !valid {
                    return Err(mlua::Error::runtime("command names must be 1-32 characters of a-z, 0-9, - or _"));
                }
                c.commands.borrow_mut().insert(name.clone(), f);
                let mut cmds = c.shared.commands.write();
                cmds.retain(|x| x.name != name);
                cmds.push(LocalCommand { name, description, owner: c.shared.ext_id.clone() });
                Ok(())
            })?,
        )?;
    }

    if shared.has(Permission::MessagesSend) {
        let c = ctx.clone();
        api.set(
            "send_message",
            lua.create_function(move |_, (channel, content, cb): (LuaValue, String, Option<Function>)| {
                let channel = snowflake(&channel, "channel_id")?;
                send(&c, channel, content, None, cb)
            })?,
        )?;
        let c = ctx.clone();
        api.set(
            "reply",
            lua.create_function(move |_, (channel, message, content, cb): (LuaValue, LuaValue, String, Option<Function>)| {
                let channel = snowflake(&channel, "channel_id")?;
                let message = snowflake(&message, "message_id")?;
                send(&c, channel, content, Some(message), cb)
            })?,
        )?;
    }

    if shared.has(Permission::StateRead) {
        api.set("state", build_state(lua, ctx)?)?;
    }

    if shared.has(Permission::Storage) {
        api.set("storage", build_storage(lua, ctx)?)?;
    }

    if shared.has(Permission::Notify) {
        let c = ctx.clone();
        api.set(
            "notify",
            lua.create_function(move |_, (title, body): (String, Option<String>)| {
                let title: String = title.chars().take(256).collect();
                let body: String = body.unwrap_or_default().chars().take(1024).collect();
                let notifier = c.notifier.read().clone();
                match notifier {
                    Some(n) => n(title, body),
                    None => c.log(LogLevel::Info, format!("notification: {title}: {body}")),
                }
                Ok(())
            })?,
        )?;
    }

    if shared.has(Permission::Http) {
        api.set("http", build_http(lua, ctx)?)?;
    }

    freeze(&api)?;
    Ok(api)
}

/// Makes a table and every table nested in it read-only (Luau's sandbox only
/// freezes the top level of each global).
fn freeze(t: &Table) -> mlua::Result<()> {
    for pair in t.pairs::<LuaValue, LuaValue>() {
        if let (_, LuaValue::Table(inner)) = pair? {
            freeze(&inner)?;
        }
    }
    t.set_readonly(true);
    Ok(())
}

fn send(ctx: &Rc<Ctx>, channel: Snowflake, content: String, reply_to: Option<Snowflake>, cb: Option<Function>) -> mlua::Result<()> {
    if content.trim().is_empty() {
        return Err(mlua::Error::runtime("message content is empty"));
    }
    ctx.check_send_rate()?;
    let guild_id = ctx.client.store().channels.get(&channel).and_then(|c| c.guild_id);
    let req = CreateMessage {
        content,
        nonce: Some(Snowflake::nonce_now().to_string()),
        message_reference: reply_to.map(|id| MessageReference {
            kind: Some(0),
            message_id: Some(id),
            channel_id: Some(channel),
            guild_id,
            fail_if_not_exists: Some(false),
        }),
        allowed_mentions: reply_to.map(|_| AllowedMentions::all(true)),
        ..Default::default()
    };
    let client = ctx.client.clone();
    ctx.spawn_job(cb, "send_message", async move {
        match client.http().send_message(channel, &req, vec![]).await {
            Ok(m) => Ok(serde_json::to_value(&m).unwrap_or(Value::Null)),
            Err(e) => Err(ferrocord_core::error_text(&e)),
        }
    })
}

fn user_json(store: &Store, id: Snowflake, guild: Option<Snowflake>) -> Option<Value> {
    let u = store.user(id)?;
    Some(json!({
        "id": u.id,
        "username": u.username,
        "global_name": u.global_name,
        "display_name": store.display_name(guild, u.id),
        "tag": u.tag(),
        "bot": u.bot,
        "avatar_url": u.avatar_url(128),
    }))
}

fn channel_json(store: &Store, id: Snowflake) -> Option<Value> {
    let c = store.channels.get(&id)?;
    Some(json!({
        "id": c.id,
        "type": u8::from(c.kind),
        "name": c.name,
        "display_name": c.display_name(),
        "guild_id": c.guild_id,
        "parent_id": c.parent_id,
        "topic": c.topic,
        "position": c.position,
        "nsfw": c.nsfw,
        "last_message_id": c.last_message_id,
        "recipient_ids": c.recipient_ids(),
        "unread": store.is_unread(c.id),
        "mentions": store.mention_count(c.id),
    }))
}

fn guild_json(store: &Store, id: Snowflake) -> Option<Value> {
    let g = store.guilds.get(&id)?;
    let (unread, mentions) = store.guild_unread(id);
    Some(json!({
        "id": g.id,
        "name": g.name(),
        "icon_url": g.icon_url(128),
        "owner_id": g.props.owner_id,
        "description": g.props.description,
        "member_count": g.member_count,
        "unavailable": g.unavailable,
        "unread": unread,
        "mentions": mentions,
    }))
}

fn build_state(lua: &Lua, ctx: &Rc<Ctx>) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let c = ctx.clone();
    t.set(
        "me",
        lua.create_function(move |lua, ()| {
            let s = c.client.store();
            match s.me.as_ref().and_then(|m| user_json(&s, m.user.id, None)) {
                Some(v) => to_lua(lua, &v),
                None => Ok(LuaValue::Nil),
            }
        })?,
    )?;

    let c = ctx.clone();
    t.set(
        "guilds",
        lua.create_function(move |lua, ()| {
            let s = c.client.store();
            let ids: Vec<Snowflake> = s
                .guild_list()
                .into_iter()
                .flat_map(|e| match e {
                    GuildListEntry::Guild(g) => vec![g],
                    GuildListEntry::Folder { guilds, .. } => guilds,
                })
                .collect();
            let v: Vec<Value> = ids.into_iter().filter_map(|g| guild_json(&s, g)).collect();
            to_lua(lua, &Value::Array(v))
        })?,
    )?;

    let c = ctx.clone();
    t.set(
        "guild",
        lua.create_function(move |lua, id: LuaValue| {
            let id = snowflake(&id, "guild_id")?;
            let s = c.client.store();
            to_lua(lua, &guild_json(&s, id).unwrap_or(Value::Null))
        })?,
    )?;

    let c = ctx.clone();
    t.set(
        "channels",
        lua.create_function(move |lua, id: LuaValue| {
            let id = snowflake(&id, "guild_id")?;
            let s = c.client.store();
            let v: Vec<Value> = s
                .guild_channel_tree(id)
                .into_iter()
                .flat_map(|(cat, kids)| cat.into_iter().chain(kids))
                .filter_map(|ch| channel_json(&s, ch.id))
                .collect();
            to_lua(lua, &Value::Array(v))
        })?,
    )?;

    let c = ctx.clone();
    t.set(
        "channel",
        lua.create_function(move |lua, id: LuaValue| {
            let id = snowflake(&id, "channel_id")?;
            let s = c.client.store();
            to_lua(lua, &channel_json(&s, id).unwrap_or(Value::Null))
        })?,
    )?;

    let c = ctx.clone();
    t.set(
        "dms",
        lua.create_function(move |lua, ()| {
            let s = c.client.store();
            let v: Vec<Value> = s.sorted_private_channels().into_iter().filter_map(|ch| channel_json(&s, ch.id)).collect();
            to_lua(lua, &Value::Array(v))
        })?,
    )?;

    let c = ctx.clone();
    t.set(
        "user",
        lua.create_function(move |lua, (id, guild): (LuaValue, LuaValue)| {
            let id = snowflake(&id, "user_id")?;
            let guild = opt_snowflake(&guild, "guild_id")?;
            let s = c.client.store();
            to_lua(lua, &user_json(&s, id, guild).unwrap_or(Value::Null))
        })?,
    )?;

    let c = ctx.clone();
    t.set(
        "messages",
        lua.create_function(move |lua, (id, limit): (LuaValue, Option<f64>)| {
            let id = snowflake(&id, "channel_id")?;
            let limit = limit.unwrap_or(50.0).clamp(1.0, 100.0) as usize;
            let msgs: Vec<Value> = {
                let s = c.client.store();
                match s.messages.get(&id) {
                    Some(cm) => {
                        let start = cm.messages.len().saturating_sub(limit);
                        cm.messages[start..].iter().filter_map(|m| serde_json::to_value(m).ok()).collect()
                    }
                    None => vec![],
                }
            };
            ser_to_lua(lua, &msgs)
        })?,
    )?;

    Ok(t)
}

fn build_storage(lua: &Lua, ctx: &Rc<Ctx>) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    let c = ctx.clone();
    t.set(
        "get",
        lua.create_function(move |lua, key: String| {
            let st = c.storage.borrow();
            match st.as_ref().and_then(|s| s.get(&key)) {
                Some(v) => to_lua(lua, v),
                None => Ok(LuaValue::Nil),
            }
        })?,
    )?;
    let c = ctx.clone();
    t.set(
        "set",
        lua.create_function(move |lua, (key, value): (String, LuaValue)| {
            if key.is_empty() || key.len() > 256 {
                return Err(mlua::Error::runtime("storage keys must be 1-256 bytes"));
            }
            let value = match value {
                LuaValue::Nil => None,
                v => Some(lua.from_value::<Value>(v)?),
            };
            let mut st = c.storage.borrow_mut();
            let Some(s) = st.as_mut() else { return Ok(()) };
            s.set(&key, value).map_err(mlua::Error::runtime)
        })?,
    )?;
    let c = ctx.clone();
    t.set(
        "keys",
        lua.create_function(move |lua, ()| {
            let keys = c.storage.borrow().as_ref().map(|s| s.keys()).unwrap_or_default();
            lua.create_sequence_from(keys)
        })?,
    )?;
    Ok(t)
}

/// A request description built from Lua arguments.
struct HttpRequest {
    method: reqwest::Method,
    url: reqwest::Url,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    content_type: Option<&'static str>,
    timeout: Duration,
}

fn http_body(lua: &Lua, v: LuaValue) -> mlua::Result<(Option<Vec<u8>>, Option<&'static str>)> {
    match v {
        LuaValue::Nil => Ok((None, None)),
        LuaValue::String(s) => Ok((Some(s.as_bytes().to_vec()), None)),
        LuaValue::Table(_) => {
            let v: Value = lua.from_value(v)?;
            Ok((Some(serde_json::to_vec(&v).map_err(mlua::Error::external)?), Some("application/json")))
        }
        _ => Err(mlua::Error::runtime("body must be a string or a table")),
    }
}

fn parse_url(url: &str) -> mlua::Result<reqwest::Url> {
    let u = reqwest::Url::parse(url).map_err(|e| mlua::Error::runtime(format!("invalid url: {e}")))?;
    if u.scheme() != "https" {
        return Err(mlua::Error::runtime("only https:// URLs are allowed"));
    }
    Ok(u)
}

fn build_http(lua: &Lua, ctx: &Rc<Ctx>) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let c = ctx.clone();
    t.set(
        "get",
        lua.create_function(move |_, (url, cb): (String, Function)| {
            let req = HttpRequest { method: reqwest::Method::GET, url: parse_url(&url)?, headers: vec![], body: None, content_type: None, timeout: Duration::from_secs(30) };
            http_spawn(&c, req, cb)
        })?,
    )?;

    let c = ctx.clone();
    t.set(
        "post",
        lua.create_function(move |lua, (url, body, cb): (String, LuaValue, Function)| {
            let (body, content_type) = http_body(lua, body)?;
            let req = HttpRequest { method: reqwest::Method::POST, url: parse_url(&url)?, headers: vec![], body, content_type, timeout: Duration::from_secs(30) };
            http_spawn(&c, req, cb)
        })?,
    )?;

    let c = ctx.clone();
    t.set(
        "request",
        lua.create_function(move |lua, (opts, cb): (Table, Function)| {
            let method: Option<String> = opts.get("method")?;
            let method = reqwest::Method::from_bytes(method.as_deref().unwrap_or("GET").to_ascii_uppercase().as_bytes())
                .map_err(|_| mlua::Error::runtime("invalid method"))?;
            let url: String = opts.get("url")?;
            let mut headers = vec![];
            if let Some(h) = opts.get::<Option<Table>>("headers")? {
                for pair in h.pairs::<String, String>() {
                    headers.push(pair?);
                }
            }
            let (body, content_type) = http_body(lua, opts.get("body")?)?;
            let timeout_ms: Option<f64> = opts.get("timeout_ms")?;
            let timeout = Duration::from_millis(timeout_ms.unwrap_or(30_000.0).clamp(100.0, 60_000.0) as u64);
            let req = HttpRequest { method, url: parse_url(&url)?, headers, body, content_type, timeout };
            http_spawn(&c, req, cb)
        })?,
    )?;

    Ok(t)
}

fn http_spawn(ctx: &Rc<Ctx>, req: HttpRequest, cb: Function) -> mlua::Result<()> {
    let client = ctx.client.http().raw_client().clone();
    ctx.spawn_job(Some(cb), "http request", async move {
        let mut rb = client.request(req.method, req.url).timeout(req.timeout);
        if let Some(ct) = req.content_type {
            rb = rb.header("content-type", ct);
        }
        for (k, v) in req.headers {
            rb = rb.header(k, v);
        }
        if let Some(b) = req.body {
            rb = rb.body(b);
        }
        let mut resp = rb.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let headers: serde_json::Map<String, Value> = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_string(), Value::String(v.to_str().ok()?.to_string()))))
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_HTTP_BODY {
                return Err(format!("response larger than {} MiB", MAX_HTTP_BODY >> 20));
            }
        }
        Ok(json!({
            "status": status,
            "ok": (200..300).contains(&status),
            "headers": headers,
            "body": String::from_utf8_lossy(&body),
        }))
    })
}
