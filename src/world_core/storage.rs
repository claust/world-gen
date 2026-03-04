pub trait Storage {
    fn load(&self, key: &str) -> Option<String>;
    fn save(&self, key: &str, data: &str) -> anyhow::Result<()>;
}

#[cfg(not(target_arch = "wasm32"))]
pub struct FileStorage;

#[cfg(not(target_arch = "wasm32"))]
impl Storage for FileStorage {
    fn load(&self, key: &str) -> Option<String> {
        let path = format!("{key}.json");
        std::fs::read_to_string(&path).ok()
    }

    fn save(&self, key: &str, data: &str) -> anyhow::Result<()> {
        let path = format!("{key}.json");
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
        Self::local_storage()?.get_item(key).ok()?
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
