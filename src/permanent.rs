use crate::{
    bundle,
    game::{self, GameError, Handle},
    settings::atomic_write,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
    System::Threading::CreateMutexW,
};

pub struct InstalledMod {
    pub game_dir: PathBuf,
    pub limit: f32,
    patched_hash: String,
    previous_hash: String,
}

fn io(error: impl ToString) -> GameError {
    GameError::detail("error.permanent_io", error)
}
fn hash(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn manifest_path() -> Result<PathBuf, GameError> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(p).join("SubspaceToolkit/permanent.json"))
        .ok_or_else(|| io("LOCALAPPDATA unavailable"))
}
fn lock() -> Result<Handle, GameError> {
    let name: Vec<u16> = "Local\\SubspaceToolkit-Permanent\0"
        .encode_utf16()
        .collect();
    let raw = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let handle = Handle::new(raw)?;
    if exists {
        return Err("error.permanent_busy".into());
    }
    Ok(handle)
}
fn read_manifest(path: &Path) -> Result<Option<InstalledMod>, GameError> {
    let data = match fs::read(path) {
        Ok(data) if data.len() <= 16384 => data,
        Ok(_) => return Err("error.permanent_manifest".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&data).map_err(|_| GameError::from("error.permanent_manifest"))?;
    let parse = || -> Option<InstalledMod> {
        if value["schema"].as_u64()? != 1 {
            return None;
        }
        let game_dir = PathBuf::from(value["game_dir"].as_str()?);
        let limit = value["limit"].as_f64()?;
        let patched_hash = value["patched_hash"].as_str()?.to_owned();
        let previous_hash = value["previous_hash"].as_str()?.to_owned();
        let valid_hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        if !game_dir.is_absolute()
            || !(300.0..=1000.0).contains(&limit)
            || !valid_hash(&patched_hash)
            || !valid_hash(&previous_hash)
        {
            return None;
        }
        Some(InstalledMod {
            game_dir,
            limit: limit as f32,
            patched_hash,
            previous_hash,
        })
    };
    parse()
        .map(Some)
        .ok_or_else(|| "error.permanent_manifest".into())
}
pub fn installed() -> Result<Option<InstalledMod>, GameError> {
    checked_installation(&manifest_path()?)
}
fn checked_installation(path: &Path) -> Result<Option<InstalledMod>, GameError> {
    let installed = read_manifest(path)?;
    if let Some(state) = &installed {
        let current = fs::read(state.game_dir.join(bundle::BUNDLE_RELATIVE)).map_err(io)?;
        if hash(&current) != state.patched_hash {
            return Err("error.permanent_incomplete".into());
        }
    }
    Ok(installed)
}
fn backup_path(target: &Path) -> PathBuf {
    target.with_extension("bundle.subspace-original")
}
fn allowed(current: &str, installed: Option<&InstalledMod>) -> bool {
    current == bundle::ORIGINAL_HASH
        || installed.is_some_and(|m| current == m.patched_hash || current == m.previous_hash)
}
fn remove_if_present(path: &Path) -> Result<(), GameError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(e)),
    }
}

pub fn install(game_dir: &Path, limit: f32) -> Result<(), GameError> {
    let _lock = lock()?;
    game::ensure_game_closed()?;
    game::verify_installation(game_dir)?;
    let game_dir = fs::canonicalize(game_dir).map_err(io)?;
    let manifest = manifest_path()?;
    let existing = read_manifest(&manifest)?;
    if existing.as_ref().is_some_and(|m| m.game_dir != game_dir) {
        return Err("error.permanent_conflict".into());
    }
    let target = game_dir.join(bundle::BUNDLE_RELATIVE);
    let backup = backup_path(&target);
    let current = fs::read(&target).map_err(io)?;
    let current_hash = hash(&current);
    if !allowed(&current_hash, existing.as_ref()) {
        return Err("error.permanent_conflict".into());
    }
    let original = match fs::read(&backup) {
        Ok(bytes) => bytes,
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && current_hash == bundle::ORIGINAL_HASH =>
        {
            current
        }
        Err(_) => return Err("error.permanent_backup".into()),
    };
    let patched = bundle::patch(&original, limit)
        .map_err(|e| GameError::detail("error.permanent_patch", e))?;
    if !backup.exists() {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
            .map_err(io)?;
        file.write_all(&original)
            .and_then(|_| file.sync_all())
            .map_err(io)?;
    }
    if hash(&fs::read(&backup).map_err(io)?) != bundle::ORIGINAL_HASH {
        return Err("error.permanent_backup".into());
    }
    let patched_hash = hash(&patched);
    // Journal before replacement: reset can recover either side of an interrupted install.
    let journal = serde_json::to_vec_pretty(&serde_json::json!({
        "schema": 1, "game_dir": game_dir, "limit": limit,
        "patched_hash": patched_hash, "previous_hash": current_hash
    }))
    .map_err(io)?;
    game::ensure_game_closed()?;
    if hash(&fs::read(&target).map_err(io)?) != current_hash {
        return Err("error.permanent_conflict".into());
    }
    atomic_write(&manifest, &journal).map_err(io)?;
    atomic_write(&target, &patched).map_err(io)?;
    if hash(&fs::read(&target).map_err(io)?) != patched_hash {
        return Err("error.permanent_conflict".into());
    }
    Ok(())
}

pub fn reset() -> Result<(), GameError> {
    let _lock = lock()?;
    let manifest = manifest_path()?;
    let Some(installed) = read_manifest(&manifest)? else {
        return Ok(());
    };
    game::ensure_game_closed()?;
    restore(&installed, &manifest)
}
fn restore(installed: &InstalledMod, manifest: &Path) -> Result<(), GameError> {
    let target = installed.game_dir.join(bundle::BUNDLE_RELATIVE);
    let backup = backup_path(&target);
    let current_hash = hash(&fs::read(&target).map_err(io)?);
    if !allowed(&current_hash, Some(installed)) {
        return Err("error.permanent_conflict".into());
    }
    if current_hash != bundle::ORIGINAL_HASH {
        let original = fs::read(&backup).map_err(|_| GameError::from("error.permanent_backup"))?;
        if hash(&original) != bundle::ORIGINAL_HASH {
            return Err("error.permanent_backup".into());
        }
        atomic_write(&target, &original).map_err(io)?;
        if hash(&fs::read(&target).map_err(io)?) != bundle::ORIGINAL_HASH {
            return Err("error.permanent_conflict".into());
        }
    }
    // Never delete an unrelated file occupying our backup name.
    if backup.exists() && hash(&fs::read(&backup).map_err(io)?) != bundle::ORIGINAL_HASH {
        return Err("error.permanent_backup".into());
    }
    remove_if_present(&backup)?;
    remove_if_present(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_rejects_invalid_paths_limits_and_hashes() {
        let path =
            std::env::temp_dir().join(format!("subspace-manifest-{}.json", std::process::id()));
        for value in [
            serde_json::json!({}),
            serde_json::json!({"schema":1,"game_dir":"relative","limit":1000,"patched_hash":"bad","previous_hash":"bad"}),
        ] {
            fs::write(&path, value.to_string()).unwrap();
            assert_eq!(
                read_manifest(&path).err().unwrap().key,
                "error.permanent_manifest"
            );
        }
        fs::remove_file(path).unwrap();
        assert!(allowed(bundle::ORIGINAL_HASH, None));
        assert!(!allowed("foreign", None));
    }
    #[test]
    fn pending_journal_does_not_report_success() {
        let dir = std::env::temp_dir().join(format!("subspace-pending-{}", std::process::id()));
        let target = dir.join(bundle::BUNDLE_RELATIVE);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let manifest = dir.join("permanent.json");
        let patched = b"new bundle";
        fs::write(
            &manifest,
            serde_json::json!({
                "schema":1, "game_dir":dir, "limit":1000,
                "patched_hash":hash(patched), "previous_hash":hash(b"old bundle")
            })
            .to_string(),
        )
        .unwrap();
        fs::write(&target, b"old bundle").unwrap();
        assert_eq!(
            checked_installation(&manifest).err().unwrap().key,
            "error.permanent_incomplete"
        );
        assert!(read_manifest(&manifest).unwrap().is_some());
        fs::write(&target, patched).unwrap();
        assert_eq!(
            checked_installation(&manifest).unwrap().unwrap().limit,
            1000.0
        );
        let resolved = fs::canonicalize(&dir).unwrap();
        assert!(resolved.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(resolved).unwrap();
    }
    #[test]
    #[ignore = "reads supported local game bundle; writes isolated test directory only"]
    fn backup_restore_and_conflict_preserve_bytes() {
        let original = fs::read(
            PathBuf::from(std::env::var_os("OUTPOSTS_GAME_DIR").expect("set OUTPOSTS_GAME_DIR"))
                .join(bundle::BUNDLE_RELATIVE),
        )
        .unwrap();
        assert_eq!(hash(&original), bundle::ORIGINAL_HASH);
        let dir = std::env::temp_dir().join(format!("subspace-restore-{}", std::process::id()));
        let target = dir.join(bundle::BUNDLE_RELATIVE);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let backup = backup_path(&target);
        let manifest = dir.join("permanent.json");
        let patched = bundle::patch(&original, 1000.0).unwrap();
        let state = InstalledMod {
            game_dir: dir.clone(),
            limit: 1000.0,
            patched_hash: hash(&patched),
            previous_hash: bundle::ORIGINAL_HASH.into(),
        };
        fs::write(&backup, &original).unwrap();
        fs::write(&manifest, b"journal").unwrap();
        fs::write(&target, b"foreign update").unwrap();
        assert_eq!(
            restore(&state, &manifest).unwrap_err().key,
            "error.permanent_conflict"
        );
        assert_eq!(fs::read(&target).unwrap(), b"foreign update");
        assert!(backup.exists());
        fs::write(&target, patched).unwrap();
        restore(&state, &manifest).unwrap();
        assert_eq!(fs::read(&target).unwrap(), original);
        assert!(!backup.exists());
        assert!(!manifest.exists());
        restore(&state, &manifest).unwrap();
        let resolved = fs::canonicalize(&dir).unwrap();
        assert!(resolved.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(resolved).unwrap();
    }
}
