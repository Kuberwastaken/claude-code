use std::path::Path;
use std::sync::Arc;
use once_cell::sync::Lazy;
use dashmap::DashMap;

use super::shadow::ShadowSnapshot;

/// Process-global registry of `ShadowSnapshot` instances, keyed by the
/// canonical worktree path.  Sharing by worktree ensures concurrent sessions
/// on the same directory use the same shadow repo and lock.
static REGISTRY: Lazy<DashMap<String, Arc<ShadowSnapshot>>> = Lazy::new(DashMap::new);

/// Return the `ShadowSnapshot` for `working_dir`, creating it on first call.
/// Returns `None` when git is unavailable or the directory is not in a repo.
pub fn get_or_create(working_dir: &Path) -> Option<Arc<ShadowSnapshot>> {
    let key = working_dir
        .canonicalize()
        .unwrap_or_else(|_| working_dir.to_path_buf())
        .to_string_lossy()
        .into_owned();

    if let Some(existing) = REGISTRY.get(&key) {
        return Some(existing.clone());
    }

    let snap = ShadowSnapshot::for_session(working_dir)?;
    let arc = Arc::new(snap);
    REGISTRY.insert(key, arc.clone());
    Some(arc)
}

/// Drop the cached snapshot for `working_dir` (e.g. when a session ends).
pub fn remove(working_dir: &Path) {
    let key = working_dir
        .canonicalize()
        .unwrap_or_else(|_| working_dir.to_path_buf())
        .to_string_lossy()
        .into_owned();
    REGISTRY.remove(&key);
}
