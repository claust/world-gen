use std::path::Path;
use std::sync::mpsc::{self, Receiver};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub struct ModelReload {
    pub name: String,
    pub bytes: Vec<u8>,
}

pub struct AssetWatcher {
    rx: Receiver<ModelReload>,
    _watcher: RecommendedWatcher,
}

impl AssetWatcher {
    /// Start watching `assets/models/` for `.glb` file changes.
    /// Returns `None` if the directory doesn't exist or the watcher fails to start.
    pub fn start() -> Option<Self> {
        let watch_dir = Path::new("assets/models");
        if !watch_dir.is_dir() {
            log::info!("asset watcher: {watch_dir:?} not found, skipping");
            return None;
        }

        let (tx, rx) = mpsc::channel();

        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                let event = match res {
                    Ok(e) => e,
                    Err(e) => {
                        log::warn!("asset watcher error: {e}");
                        return;
                    }
                };

                let dominated = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_));
                if !dominated {
                    return;
                }

                for path in &event.paths {
                    if path.extension().and_then(|e| e.to_str()) != Some("glb") {
                        continue;
                    }
                    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    let name = stem.to_string();

                    match std::fs::read(path) {
                        Ok(bytes) => {
                            log::info!("asset watcher: detected change in {}", path.display());
                            let _ = tx.send(ModelReload { name, bytes });
                        }
                        Err(e) => {
                            log::warn!("asset watcher: failed to read {}: {e}", path.display());
                        }
                    }
                }
            })
            .ok()?;

        if watcher
            .watch(watch_dir, RecursiveMode::NonRecursive)
            .is_err()
        {
            log::warn!("asset watcher: failed to watch {watch_dir:?}");
            return None;
        }

        log::info!("asset watcher started on {watch_dir:?}");
        Some(Self {
            rx,
            _watcher: watcher,
        })
    }

    /// Drain all pending reloads (non-blocking). Same pattern as `DebugApiHandle::drain_commands()`.
    pub fn drain_reloads(&self) -> Vec<ModelReload> {
        let mut reloads = Vec::new();
        while let Ok(reload) = self.rx.try_recv() {
            reloads.push(reload);
        }
        reloads
    }
}
