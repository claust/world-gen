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

    fn supports_bytes(&self) -> bool {
        true
    }
}

/// Source for a prebuilt `world_base.bin` snapshot. A New Game tries this before
/// generating the base world locally, so a first-time start can download the
/// expensive-to-generate flora instead of computing it.
///
/// Baked in at compile time from the `WORLD_BASE_URL` env var. When that var is
/// unset (or empty) — every local/dev build and PR CI build — this is `""`, which
/// disables the download and the game always generates locally. The release
/// workflow sets it to the pinned per-tag GitHub release asset URL
/// (`https://github.com/<owner>/<repo>/releases/download/<tag>/world_base.bin`),
/// so only shipped binaries fetch.
///
/// Treated as a generic direct-download HTTPS URL — any host that serves the raw
/// bytes (a GitHub release asset, an S3 object, Google Drive `uc?export=download`,
/// etc.) works.
///
/// Note: a downloaded snapshot is only *used* if it passes the same validation
/// as the local cache (`PlantWorld::from_base_snapshot`): its embedded seed and
/// generation key must match the current `config.world.seed` and herbarium/config.
/// If they don't, the download is silently rejected and the game generates
/// locally — so a hosted file must be built from the matching seed/config, and a
/// stale-URL or wrong-platform mismatch degrades safely to local generation.
#[cfg(not(target_arch = "wasm32"))]
pub const BASE_WORLD_URL: &str = match option_env!("WORLD_BASE_URL") {
    Some(url) => url,
    None => "",
};

/// Upper bound on a downloaded base-world body, to avoid an unbounded allocation
/// from a misbehaving or hostile server. A real snapshot of the default world is
/// ~190 MiB (one byte pair per plant across ~14M plants), so this leaves headroom
/// while still capping a runaway response.
#[cfg(not(target_arch = "wasm32"))]
const BASE_WORLD_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Try to download a prebuilt `world_base.bin` from [`BASE_WORLD_URL`]. Returns
/// the raw bytes on a `200` response, or `None` on any failure — unset URL,
/// connect/read timeout, network error, non-200 status, or an oversized body —
/// so callers fall back to local generation. Validation of the bytes themselves
/// is left to the caller (`PlantWorld::from_base_snapshot`).
///
/// Blocking: intended to run on the world-gen worker thread, not the event loop.
#[cfg(not(target_arch = "wasm32"))]
pub fn fetch_base_world() -> Option<Vec<u8>> {
    use std::io::Read;
    use std::time::Duration;

    if BASE_WORLD_URL.is_empty() {
        return None;
    }

    // Short timeouts so a dead or slow host can't stall the loading screen for
    // long before we fall back to generating locally. `redirects` is explicit
    // (it already defaults to 5) so a redirecting host — e.g. a GitHub release
    // asset URL 302s to objects.githubusercontent.com — is followed to the real
    // bytes rather than handing us a redirect page.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .redirects(5)
        .build();

    let resp = match agent.get(BASE_WORLD_URL).call() {
        Ok(resp) => resp,
        Err(err) => {
            log::warn!("base world: download from {BASE_WORLD_URL} failed: {err}");
            return None;
        }
    };

    // `.call()` only errors on 4xx/5xx; a 3xx that outran the redirect limit, or
    // a 2xx that isn't `200` (204/206/…), comes back as `Ok` with a body that
    // isn't the snapshot. Require exactly `200` so anything else falls back to
    // local generation instead of feeding junk to `from_base_snapshot`.
    if resp.status() != 200 {
        log::warn!(
            "base world: download from {BASE_WORLD_URL} returned status {}; ignoring",
            resp.status()
        );
        return None;
    }

    let mut bytes = Vec::new();
    if let Err(err) = resp
        .into_reader()
        .take(BASE_WORLD_MAX_BYTES)
        .read_to_end(&mut bytes)
    {
        log::warn!("base world: reading download body from {BASE_WORLD_URL} failed: {err}");
        return None;
    }

    if bytes.len() as u64 >= BASE_WORLD_MAX_BYTES {
        log::warn!(
            "base world: download from {BASE_WORLD_URL} exceeded {BASE_WORLD_MAX_BYTES} bytes; ignoring"
        );
        return None;
    }

    log::info!(
        "base world: downloaded {} bytes from {BASE_WORLD_URL}",
        bytes.len()
    );
    Some(bytes)
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
