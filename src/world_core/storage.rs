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
/// ~31 MiB (Brotli-compressed columnar plant data across ~14M plants), so this
/// leaves ample headroom while still capping a runaway response.
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

    // Read one byte past the cap so an oversized body can be distinguished from a
    // legitimate one of exactly the cap: `take(MAX)` would silently truncate to
    // MAX, making the two indistinguishable. A body of exactly MAX is accepted
    // (the cap is inclusive); anything larger is rejected.
    let mut bytes = Vec::new();
    if let Err(err) = resp
        .into_reader()
        .take(BASE_WORLD_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
    {
        log::warn!("base world: reading download body from {BASE_WORLD_URL} failed: {err}");
        return None;
    }

    if bytes.len() as u64 > BASE_WORLD_MAX_BYTES {
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

/// Try to download the prebuilt `world_base.bin` snapshot in the browser. Unlike
/// the native [`fetch_base_world`], the URL is a fixed same-origin relative path:
/// the release workflow deploys the snapshot next to the wasm/JS bundle, so it
/// resolves on any host the app is served from. A same-origin fetch also avoids
/// CORS entirely — GitHub's release-asset hosts don't send
/// `Access-Control-Allow-Origin`, so the cross-origin release URL the native
/// build uses is unreadable from a page — and lets the browser HTTP cache serve
/// repeat New Games from disk (web `localStorage` can't hold a ~31 MiB blob).
///
/// Returns the raw bytes on a `200`, or `None` on any failure (missing file,
/// network error, non-ok status) so the caller falls back to local generation.
/// Validation of the bytes is left to `PlantWorld::from_base_snapshot`.
///
/// Async: must be awaited on the browser event loop (there is no worker thread).
#[cfg(target_arch = "wasm32")]
pub async fn fetch_base_world_async() -> Option<Vec<u8>> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window()?;
    let request = match web_sys::Request::new_with_str("world_base.bin") {
        Ok(req) => req,
        Err(err) => {
            log::warn!("base world: building fetch request failed: {err:?}");
            return None;
        }
    };

    let resp_value = match JsFuture::from(window.fetch_with_request(&request)).await {
        Ok(value) => value,
        Err(err) => {
            log::warn!("base world: fetch failed: {err:?}");
            return None;
        }
    };
    let resp: web_sys::Response = match resp_value.dyn_into() {
        Ok(resp) => resp,
        Err(_) => return None,
    };
    // Require exactly 200 (not just any 2xx) so a 204/206/etc. with a body that
    // isn't the snapshot falls back to local generation instead of being fed to
    // `from_base_snapshot` — matching the native downloader.
    if resp.status() != 200 {
        log::warn!(
            "base world: fetch returned status {}; ignoring",
            resp.status()
        );
        return None;
    }

    let buffer = match resp.array_buffer() {
        Ok(promise) => match JsFuture::from(promise).await {
            Ok(buffer) => buffer,
            Err(err) => {
                log::warn!("base world: reading response body failed: {err:?}");
                return None;
            }
        },
        Err(err) => {
            log::warn!("base world: array_buffer() failed: {err:?}");
            return None;
        }
    };

    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    log::info!("base world: downloaded {} bytes", bytes.len());
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
