pub trait Storage {
    fn load(&self, key: &str) -> Option<String>;
    fn save(&self, key: &str, data: &str) -> anyhow::Result<()>;

    /// Load a binary blob. Defaults to "unsupported" (returns `None`); backends
    /// that can hold large binary data (the native file store) override it.
    fn load_bytes(&self, _key: &str) -> Option<Vec<u8>> {
        None
    }

    /// Save a binary blob. Defaults to "unsupported"; see [`load_bytes`].
    fn save_bytes(&self, _key: &str, _data: &[u8]) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "binary storage is not supported by this backend"
        ))
    }

    /// Whether this backend can persist binary blobs. Defaults to `false`; the
    /// native file store overrides it. Lets callers skip binary-only features
    /// (the base-world cache) on backends like web localStorage instead of
    /// attempting a write that always fails and logs.
    fn supports_bytes(&self) -> bool {
        false
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct FileStorage {
    /// Directory all storage files live under. `.` for the default (single
    /// instance) layout; `instances/<name>` when an instance name is set so
    /// concurrent instances don't clobber each other's save/config/plants.
    base: std::path::PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl FileStorage {
    pub fn new(base: std::path::PathBuf) -> Self {
        Self { base }
    }

    fn validate_key(key: &str) -> anyhow::Result<()> {
        if key.is_empty()
            || key.contains("..")
            || key
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        {
            return Err(anyhow::anyhow!("invalid storage key"));
        }
        Ok(())
    }

    fn path_for(&self, key: &str) -> anyhow::Result<std::path::PathBuf> {
        Self::validate_key(key)?;
        Ok(self.base.join(format!("{key}.json")))
    }

    fn bin_path_for(&self, key: &str) -> anyhow::Result<std::path::PathBuf> {
        Self::validate_key(key)?;
        Ok(self.base.join(format!("{key}.bin")))
    }

    /// Write `data` to `path` atomically: a full write to `path.tmp` followed by
    /// a rename, so a crash or full disk mid-write can't truncate or corrupt the
    /// existing file (the rename is atomic on the same filesystem). Important for
    /// the large `plants.bin`.
    fn atomic_write(path: &std::path::Path, data: &[u8]) -> anyhow::Result<()> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        std::fs::write(&tmp, data)?;
        if let Err(err) = std::fs::rename(&tmp, path) {
            // Windows refuses to rename onto an existing file; remove it and
            // retry (a brief non-atomic window, but avoids a persistent save
            // failure). Other platforms replace atomically, so a genuine error
            // there is propagated rather than masked.
            if cfg!(windows) {
                let _ = std::fs::remove_file(path);
                std::fs::rename(&tmp, path)?;
            } else {
                let _ = std::fs::remove_file(&tmp);
                return Err(err.into());
            }
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Storage for FileStorage {
    fn load(&self, key: &str) -> Option<String> {
        let path = match self.path_for(key) {
            Ok(p) => p,
            Err(_) => return None,
        };
        match std::fs::read_to_string(&path) {
            Ok(contents) => Some(contents),
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "Warning: failed to read storage file '{}': {}",
                        path.display(),
                        err
                    );
                }
                None
            }
        }
    }

    fn save(&self, key: &str, data: &str) -> anyhow::Result<()> {
        let path = self.path_for(key)?;
        FileStorage::atomic_write(&path, data.as_bytes())
    }

    fn load_bytes(&self, key: &str) -> Option<Vec<u8>> {
        let path = self.bin_path_for(key).ok()?;
        match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "Warning: failed to read storage file '{}': {}",
                        path.display(),
                        err
                    );
                }
                None
            }
        }
    }

    fn save_bytes(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        let path = self.bin_path_for(key)?;
        FileStorage::atomic_write(&path, data)
    }

    fn supports_bytes(&self) -> bool {
        true
    }
}

#[cfg(target_arch = "wasm32")]
pub struct WebStorage;

#[cfg(target_arch = "wasm32")]
impl WebStorage {
    fn local_storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok()?
    }
}

#[cfg(target_arch = "wasm32")]
impl Storage for WebStorage {
    fn load(&self, key: &str) -> Option<String> {
        let storage = Self::local_storage()?;
        match storage.get_item(key) {
            Ok(value) => value,
            Err(err) => {
                log::warn!("localStorage get_item failed for key '{}': {:?}", key, err);
                None
            }
        }
    }

    fn save(&self, key: &str, data: &str) -> anyhow::Result<()> {
        let storage =
            Self::local_storage().ok_or_else(|| anyhow::anyhow!("localStorage not available"))?;
        storage.set_item(key, data).map_err(|e| {
            anyhow::anyhow!("localStorage setItem failed for key '{}': {:?}", key, e)
        })?;
        Ok(())
    }
}

/// Validates an instance name used to namespace on-disk state. Same character
/// rules as a storage key (alphanumeric, `_`, `-`; no `..`) so it can't escape
/// the `instances/` directory.
pub fn validate_instance_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty()
        || name.contains("..")
        || name
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    {
        return Err(anyhow::anyhow!(
            "invalid instance name '{name}' (use letters, digits, '_' or '-')"
        ));
    }
    Ok(())
}

/// Root directory for an instance's on-disk state. `.` for the unnamed default
/// (current single-instance layout); `instances/<name>` otherwise. Callers that
/// also write non-storage files (e.g. screenshots) join their own subdir onto
/// this so every instance's output stays together.
///
/// Defends in depth: an invalid name (one that could escape `instances/` via a
/// path separator or `..`) is rejected here and falls back to the default root
/// rather than building a traversing path, so no caller can write outside the
/// intended directory even if it skipped [`validate_instance_name`].
#[cfg(not(target_arch = "wasm32"))]
pub fn instance_root(instance: Option<&str>) -> std::path::PathBuf {
    match instance {
        Some(name) => match validate_instance_name(name) {
            Ok(()) => std::path::Path::new("instances").join(name),
            Err(err) => {
                // Silent under `cargo test` (the traversal test feeds invalid
                // names on purpose) so test output stays clean; warns otherwise.
                if cfg!(not(test)) {
                    eprintln!("Warning: {err}; using default storage root");
                }
                std::path::PathBuf::from(".")
            }
        },
        None => std::path::PathBuf::from("."),
    }
}

/// Creates the storage backend for the given optional instance name. On native
/// builds a named instance roots all files under `instances/<name>/`, which is
/// created if missing; the unnamed default keeps the working-directory layout.
/// Web builds are single-instance and ignore the name.
pub fn create_storage(instance: Option<&str>) -> Box<dyn Storage> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let base = instance_root(instance);
        if let Err(err) = std::fs::create_dir_all(&base) {
            eprintln!(
                "Warning: failed to create instance dir '{}': {}",
                base.display(),
                err
            );
        }
        Box::new(FileStorage::new(base))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = instance;
        Box::new(WebStorage)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::instance_root;
    use std::path::{Path, PathBuf};

    #[test]
    fn instance_root_defaults_when_unnamed() {
        assert_eq!(instance_root(None), PathBuf::from("."));
    }

    #[test]
    fn instance_root_namespaces_valid_name() {
        assert_eq!(
            instance_root(Some("alpha")),
            Path::new("instances").join("alpha")
        );
    }

    #[test]
    fn instance_root_rejects_traversal() {
        // A name that would escape `instances/` must fall back to the default
        // root, never produce a traversing path.
        assert_eq!(instance_root(Some("../../tmp")), PathBuf::from("."));
        assert_eq!(instance_root(Some("a/b")), PathBuf::from("."));
    }
}
