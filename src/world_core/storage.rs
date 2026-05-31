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
}

#[cfg(not(target_arch = "wasm32"))]
pub struct FileStorage;

#[cfg(not(target_arch = "wasm32"))]
impl FileStorage {
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

    fn path_for(key: &str) -> anyhow::Result<String> {
        Self::validate_key(key)?;
        Ok(format!("{key}.json"))
    }

    fn bin_path_for(key: &str) -> anyhow::Result<String> {
        Self::validate_key(key)?;
        Ok(format!("{key}.bin"))
    }

    /// Write `data` to `path` atomically: a full write to `path.tmp` followed by
    /// a rename, so a crash or full disk mid-write can't truncate or corrupt the
    /// existing file (the rename is atomic on the same filesystem). Important for
    /// the large `plants.bin`.
    fn atomic_write(path: &str, data: &[u8]) -> anyhow::Result<()> {
        let tmp = format!("{path}.tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Storage for FileStorage {
    fn load(&self, key: &str) -> Option<String> {
        let path = match FileStorage::path_for(key) {
            Ok(p) => p,
            Err(_) => return None,
        };
        match std::fs::read_to_string(&path) {
            Ok(contents) => Some(contents),
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("Warning: failed to read storage file '{}': {}", path, err);
                }
                None
            }
        }
    }

    fn save(&self, key: &str, data: &str) -> anyhow::Result<()> {
        let path = FileStorage::path_for(key)?;
        FileStorage::atomic_write(&path, data.as_bytes())
    }

    fn load_bytes(&self, key: &str) -> Option<Vec<u8>> {
        let path = FileStorage::bin_path_for(key).ok()?;
        match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("Warning: failed to read storage file '{}': {}", path, err);
                }
                None
            }
        }
    }

    fn save_bytes(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        let path = FileStorage::bin_path_for(key)?;
        FileStorage::atomic_write(&path, data)
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

pub fn create_storage() -> Box<dyn Storage> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Box::new(FileStorage)
    }
    #[cfg(target_arch = "wasm32")]
    {
        Box::new(WebStorage)
    }
}
