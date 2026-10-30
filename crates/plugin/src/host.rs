//! [`PluginHost`]: discovery, installation, permissions and lifecycle of plugins.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ferrocord_core::{Client, Config};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use crate::manifest::{MANIFEST_FILE, Permission, PluginManifest, is_valid_id};
use crate::proxy::PluginProxy;
use crate::runtime::{self, Launch, LogBuffer, LogLevel, Notifier, NotifierSlot, PluginLog, push_log};
use crate::storage::STORAGE_FILE;
use crate::PluginError;

/// How long unloading waits for a plugin thread to finish its current callback.
const UNLOAD_WAIT: Duration = Duration::from_secs(1);
const MAX_INSTALL_BYTES: u64 = 64 << 20;

/// One permission of a plugin as shown in a permission dialog.
#[derive(Debug, Clone, Serialize)]
pub struct PermissionStatus {
    /// The string from `plugin.toml`.
    pub name: String,
    /// `None` when Ferrocord does not recognise the permission.
    #[serde(skip)]
    pub permission: Option<Permission>,
    pub description: String,
    /// Granted by the user (in `config.plugins.grants`).
    pub granted: bool,
    /// Can leak data or act on the user's behalf.
    pub sensitive: bool,
}

/// A plugin found in the plugins directory.
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub manifest: PluginManifest,
    pub dir: PathBuf,
    /// Listed in `config.plugins.enabled`.
    pub enabled: bool,
    /// Currently running.
    pub loaded: bool,
    /// Manifest or load error, if any.
    pub error: Option<String>,
    /// Declared permissions with their grant state.
    pub permissions: Vec<PermissionStatus>,
}

struct Inner {
    client: Client,
    notifier: NotifierSlot,
    loaded: Mutex<HashMap<String, Arc<PluginProxy>>>,
    errors: Mutex<HashMap<String, String>>,
    logs: Mutex<HashMap<String, LogBuffer>>,
    /// Serialises load/unload operations.
    ops: Mutex<()>,
}

/// Manages plugins for a [`Client`]. Cheap to clone; clones share state.
#[derive(Clone)]
pub struct PluginHost {
    inner: Arc<Inner>,
}

impl PluginHost {
    pub fn new(client: Client) -> Self {
        Self {
            inner: Arc::new(Inner {
                client,
                notifier: Arc::new(RwLock::new(None)),
                loaded: Mutex::new(HashMap::new()),
                errors: Mutex::new(HashMap::new()),
                logs: Mutex::new(HashMap::new()),
                ops: Mutex::new(()),
            }),
        }
    }

    /// Directory scanned for plugins (`<data_dir>/plugins`).
    pub fn plugins_dir(&self) -> PathBuf {
        self.inner.client.paths().plugins_dir()
    }

    /// Installs the callback used by `ferrocord.notify(title, body)`. It is
    /// invoked on the plugin's thread, so it should only hand the data off.
    pub fn set_notifier(&self, f: impl Fn(String, String) + Send + Sync + 'static) {
        let n: Notifier = Arc::new(f);
        *self.inner.notifier.write() = Some(n);
    }

    /// Lists every plugin directory (sorted by name), including broken ones.
    pub fn discover(&self) -> Vec<PluginInfo> {
        let Ok(rd) = std::fs::read_dir(self.plugins_dir()) else { return vec![] };
        let config = self.inner.client.config();
        let mut out: Vec<PluginInfo> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && e.path().join(MANIFEST_FILE).is_file())
            .map(|e| self.info_for_dir(&e.path(), &config))
            .collect();
        out.sort_by_key(|p| (p.manifest.name.to_lowercase(), p.manifest.id.clone()));
        out
    }

    /// Information about one installed plugin.
    pub fn info(&self, id: &str) -> Option<PluginInfo> {
        let dir = self.dir_of(id).ok()?;
        dir.join(MANIFEST_FILE).is_file().then(|| self.info_for_dir(&dir, &self.inner.client.config()))
    }

    fn info_for_dir(&self, dir: &Path, config: &Config) -> PluginInfo {
        let dir_name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (manifest, mut error) = match PluginManifest::load(dir) {
            Ok(m) if m.id != dir_name => {
                let err = format!("plugin id `{}` does not match its directory name `{dir_name}`", m.id);
                (m, Some(err))
            }
            Ok(m) => (m, None),
            Err(e) => (
                PluginManifest {
                    id: dir_name.clone(),
                    name: dir_name.clone(),
                    version: String::new(),
                    author: String::new(),
                    description: String::new(),
                    entry: String::new(),
                    permissions: vec![],
                },
                Some(e.to_string()),
            ),
        };
        if error.is_none() {
            error = self.inner.errors.lock().get(&dir_name).cloned();
        }
        PluginInfo {
            permissions: permission_statuses(&manifest, config),
            enabled: config.plugins.enabled.contains(&dir_name),
            loaded: self.inner.loaded.lock().contains_key(&dir_name),
            dir: dir.to_path_buf(),
            manifest,
            error,
        }
    }

    /// Declared permissions of a plugin with their grant state.
    pub fn permissions(&self, id: &str) -> Result<Vec<PermissionStatus>, PluginError> {
        let m = PluginManifest::load(&self.dir_of(id)?)?;
        Ok(permission_statuses(&m, &self.inner.client.config()))
    }

    /// Starts every plugin listed in `config.plugins.enabled`. Returns the ones that failed.
    pub fn load_enabled(&self) -> Vec<(String, PluginError)> {
        let enabled = self.inner.client.config().plugins.enabled;
        enabled.into_iter().filter_map(|id| self.load(&id).err().map(|e| (id, e))).collect()
    }

    /// Marks a plugin enabled (persisted) and starts it. The plugin stays
    /// enabled even if loading fails, so it is retried on the next start.
    pub fn enable(&self, id: &str) -> Result<(), PluginError> {
        self.dir_of(id)?;
        self.inner.client.update_config(|c| {
            if !c.plugins.enabled.iter().any(|e| e == id) {
                c.plugins.enabled.push(id.to_string());
            }
        });
        self.load(id)
    }

    /// Stops a plugin and removes it from `config.plugins.enabled`.
    pub fn disable(&self, id: &str) {
        self.inner.client.update_config(|c| c.plugins.enabled.retain(|e| e != id));
        let _g = self.inner.ops.lock();
        self.unload_locked(id);
        self.inner.errors.lock().remove(id);
    }

    /// Restarts a running plugin (re-reading its manifest and script).
    pub fn reload(&self, id: &str) -> Result<(), PluginError> {
        self.load(id)
    }

    /// Grants a declared permission. A running plugin is restarted so the new API appears.
    pub fn grant(&self, id: &str, perm: &str) -> Result<(), PluginError> {
        let p: Permission = perm.parse()?;
        let m = PluginManifest::load(&self.dir_of(id)?)?;
        if !m.declares(p) {
            return Err(PluginError::NotDeclared { id: id.to_string(), permission: p.as_str().to_string() });
        }
        self.inner.client.update_config(|c| {
            let g = c.plugins.grants.entry(id.to_string()).or_default();
            if !g.iter().any(|x| x == p.as_str()) {
                g.push(p.as_str().to_string());
            }
        });
        self.restart_if_loaded(id)
    }

    /// Grants every permission the plugin declares (and Ferrocord recognises).
    pub fn grant_all(&self, id: &str) -> Result<(), PluginError> {
        let m = PluginManifest::load(&self.dir_of(id)?)?;
        let perms: Vec<String> = m.declared_permissions().iter().map(|p| p.as_str().to_string()).collect();
        self.inner.client.update_config(|c| {
            c.plugins.grants.insert(id.to_string(), perms);
        });
        self.restart_if_loaded(id)
    }

    /// Revokes a permission. A running plugin is restarted without it.
    pub fn revoke(&self, id: &str, perm: &str) -> Result<(), PluginError> {
        let p: Permission = perm.parse()?;
        self.inner.client.update_config(|c| {
            if let Some(g) = c.plugins.grants.get_mut(id) {
                g.retain(|x| x != p.as_str());
                if g.is_empty() {
                    c.plugins.grants.remove(id);
                }
            }
        });
        self.restart_if_loaded(id)
    }

    fn restart_if_loaded(&self, id: &str) -> Result<(), PluginError> {
        if self.is_loaded(id) { self.load(id) } else { Ok(()) }
    }

    /// Copies a plugin directory into the plugins directory, replacing an
    /// existing installation with the same id (its `storage.json` is kept).
    /// The plugin is not enabled automatically; a running instance is reloaded.
    pub fn install_from_dir(&self, src: &Path) -> Result<PluginManifest, PluginError> {
        let manifest = PluginManifest::load(src)?;
        let root = self.plugins_dir();
        std::fs::create_dir_all(&root)?;
        let dest = root.join(&manifest.id);
        let same = match (src.canonicalize(), dest.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same {
            let mut budget = MAX_INSTALL_BYTES;
            let staging = root.join(format!(".install-{}", manifest.id));
            let _ = std::fs::remove_dir_all(&staging);
            if let Err(e) = copy_tree(src, &staging, 0, &mut budget) {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
            if dest.is_dir() {
                let storage = dest.join(STORAGE_FILE);
                if storage.is_file() {
                    let _ = std::fs::copy(&storage, staging.join(STORAGE_FILE));
                }
                let _g = self.inner.ops.lock();
                self.unload_locked(&manifest.id);
                std::fs::remove_dir_all(&dest)?;
            }
            std::fs::rename(&staging, &dest)?;
        }
        self.inner.errors.lock().remove(&manifest.id);
        if self.inner.client.config().plugins.enabled.contains(&manifest.id) {
            self.load(&manifest.id)?;
        }
        Ok(manifest)
    }

    /// Disables a plugin, forgets its grants and deletes its directory (including storage).
    pub fn uninstall(&self, id: &str) -> Result<(), PluginError> {
        let dir = self.dir_of(id)?;
        self.disable(id);
        self.inner.client.update_config(|c| {
            c.plugins.grants.remove(id);
        });
        if dir.is_dir() {
            std::fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn is_loaded(&self, id: &str) -> bool {
        self.inner.loaded.lock().contains_key(id)
    }

    /// Recent output of a plugin (log lines, callback errors), oldest first.
    pub fn logs(&self, id: &str) -> Vec<PluginLog> {
        self.inner.logs.lock().get(id).map(|l| l.lock().iter().cloned().collect()).unwrap_or_default()
    }

    /// Stops every running plugin (without changing the config).
    pub fn shutdown(&self) {
        let _g = self.inner.ops.lock();
        let ids: Vec<String> = self.inner.loaded.lock().keys().cloned().collect();
        for id in ids {
            self.unload_locked(&id);
        }
    }

    fn dir_of(&self, id: &str) -> Result<PathBuf, PluginError> {
        if !is_valid_id(id) {
            return Err(PluginError::NotFound(id.to_string()));
        }
        let dir = self.plugins_dir().join(id);
        if dir.is_dir() { Ok(dir) } else { Err(PluginError::NotFound(id.to_string())) }
    }

    fn log_buffer(&self, id: &str) -> LogBuffer {
        self.inner.logs.lock().entry(id.to_string()).or_insert_with(|| Arc::new(Mutex::new(VecDeque::new()))).clone()
    }

    /// (Re)starts a plugin with its current manifest and grants.
    fn load(&self, id: &str) -> Result<(), PluginError> {
        let _g = self.inner.ops.lock();
        self.unload_locked(id);
        let result = self.start(id);
        match &result {
            Ok(()) => {
                self.inner.errors.lock().remove(id);
            }
            Err(e) => {
                self.inner.errors.lock().insert(id.to_string(), e.to_string());
                push_log(&self.log_buffer(id), id, LogLevel::Error, format!("failed to load: {e}"));
            }
        }
        result
    }

    fn start(&self, id: &str) -> Result<(), PluginError> {
        let dir = self.dir_of(id)?;
        let manifest = PluginManifest::load(&dir)?;
        if manifest.id != id {
            return Err(PluginError::Manifest(format!("plugin id `{}` does not match its directory name `{id}`", manifest.id)));
        }
        let config = self.inner.client.config();
        let perms = effective_permissions(&manifest, &config);
        let running = runtime::spawn(Launch {
            client: self.inner.client.clone(),
            manifest,
            dir,
            perms,
            notifier: self.inner.notifier.clone(),
            logs: self.log_buffer(id),
        })?;
        let proxy = Arc::new(PluginProxy::new(running));
        self.inner.client.add_extension(proxy.clone());
        self.inner.loaded.lock().insert(id.to_string(), proxy);
        Ok(())
    }

    fn unload_locked(&self, id: &str) {
        let Some(proxy) = self.inner.loaded.lock().remove(id) else { return };
        self.inner.client.remove_extension(ferrocord_core::Extension::id(&*proxy));
        proxy.stop();
        proxy.wait_exit(UNLOAD_WAIT);
        tracing::info!(plugin = proxy.plugin_id(), "plugin unloaded");
    }
}

/// Permissions a plugin actually receives: declared in its manifest *and* granted by the user.
pub fn effective_permissions(manifest: &PluginManifest, config: &Config) -> BTreeSet<Permission> {
    let granted = config.plugins.grants.get(&manifest.id);
    manifest
        .declared_permissions()
        .into_iter()
        .filter(|p| granted.is_some_and(|g| g.iter().any(|x| x == p.as_str())))
        .collect()
}

fn permission_statuses(m: &PluginManifest, config: &Config) -> Vec<PermissionStatus> {
    let granted = config.plugins.grants.get(&m.id);
    let mut seen = BTreeSet::new();
    m.permissions
        .iter()
        .filter(|name| seen.insert(name.trim().to_string()))
        .map(|name| {
            let p = name.parse::<Permission>().ok();
            PermissionStatus {
                name: name.trim().to_string(),
                permission: p,
                description: p.map(|p| p.description().to_string()).unwrap_or_else(|| "Unknown permission (ignored)".into()),
                granted: p.is_some() && granted.is_some_and(|g| g.iter().any(|x| x == name.trim())),
                sensitive: p.is_some_and(Permission::is_sensitive),
            }
        })
        .collect()
}

fn copy_tree(src: &Path, dest: &Path, depth: usize, budget: &mut u64) -> Result<(), PluginError> {
    if depth > 8 {
        return Err(PluginError::Install("directory tree is too deep".into()));
    }
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let name = entry.file_name();
        if ty.is_symlink() || (depth == 0 && name == STORAGE_FILE) {
            continue;
        }
        let to = dest.join(&name);
        if ty.is_dir() {
            copy_tree(&entry.path(), &to, depth + 1, budget)?;
        } else if ty.is_file() {
            let len = entry.metadata()?.len();
            if len > *budget {
                return Err(PluginError::Install("plugin is too large".into()));
            }
            *budget -= len;
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}
