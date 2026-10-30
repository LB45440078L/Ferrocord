//! Per-plugin persistent key-value storage (`<plugin dir>/storage.json`).

use std::path::PathBuf;

use serde_json::{Map, Value};

/// File name of the storage file inside a plugin directory.
pub const STORAGE_FILE: &str = "storage.json";

/// Upper bound on the serialized size of a plugin's storage.
pub const MAX_STORAGE_BYTES: usize = 1 << 20;

pub(crate) struct Storage {
    path: PathBuf,
    data: Map<String, Value>,
}

impl Storage {
    /// Opens the store; a missing or corrupt file yields an empty store.
    pub fn open(path: PathBuf) -> Self {
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Map<String, Value>>(&bytes).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "plugin storage is corrupt; starting empty");
                Map::new()
            }),
            Err(_) => Map::new(),
        };
        Self { path, data }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    pub fn keys(&self) -> Vec<String> {
        self.data.keys().cloned().collect()
    }

    /// Sets (or with `None`, removes) a key and writes the file.
    pub fn set(&mut self, key: &str, value: Option<Value>) -> Result<(), String> {
        let old = match value {
            Some(v) => self.data.insert(key.to_string(), v),
            None => self.data.remove(key),
        };
        let encoded = match serde_json::to_vec_pretty(&self.data) {
            Ok(e) => e,
            Err(e) => {
                self.restore(key, old);
                return Err(e.to_string());
            }
        };
        if encoded.len() > MAX_STORAGE_BYTES {
            self.restore(key, old);
            return Err(format!("storage limit of {} KiB exceeded", MAX_STORAGE_BYTES / 1024));
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &encoded).and_then(|_| std::fs::rename(&tmp, &self.path)).map_err(|e| e.to_string())
    }

    fn restore(&mut self, key: &str, old: Option<Value>) {
        match old {
            Some(v) => self.data.insert(key.to_string(), v),
            None => self.data.remove(key),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STORAGE_FILE);
        let mut s = Storage::open(path.clone());
        s.set("a", Some(Value::from(1))).unwrap();
        s.set("b", Some(Value::from("x"))).unwrap();
        s.set("b", None).unwrap();
        let s2 = Storage::open(path);
        assert_eq!(s2.get("a"), Some(&Value::from(1)));
        assert!(s2.get("b").is_none());

        let mut s = s2;
        let huge = "x".repeat(MAX_STORAGE_BYTES + 1);
        assert!(s.set("huge", Some(Value::from(huge))).is_err());
        assert!(s.get("huge").is_none());
    }
}
