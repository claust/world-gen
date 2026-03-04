pub trait Storage {
    fn load(&self, key: &str) -> Option<String>;
    fn save(&self, key: &str, data: &str) -> anyhow::Result<()>;
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
}

#[cfg(not(target_arch = "wasm32"))]
impl Storage for FileStorage {
    fn load(&self, key: &str) -> Option<String> {
        let path = match FileStorage::path_for(key) {
            Ok(p) => p,
            Err(_) => return None,
        };
        std::fs::read_to_string(&path).ok()
    }

    fn save(&self, key: &str, data: &str) -> anyhow::Result<()> {
        let path = FileStorage::path_for(key)?;
        std::fs::write(&path, data)?;
        Ok(())
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
        storage
            .set_item(key, data)
            .map_err(|_| anyhow::anyhow!("localStorage setItem failed"))?;
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
