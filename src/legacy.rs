use crate::game::{self, GameError, Handle};
use std::{
    fs::{self, File},
    io::{self, Seek},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
    Storage::FileSystem::{
        FILE_SHARE_READ, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    },
    System::Threading::CreateMutexW,
};

pub struct InstalledMod {
    game_dir: PathBuf,
    patched_hash: String,
    previous_hash: String,
}
fn io(error: impl ToString) -> GameError {
    GameError::detail("error.permanent_io", error)
}
fn open_read(path: &Path) -> Result<File, GameError> {
    // Streaming must not race a writer changing the validated source mid-copy.
    File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(io)
}
fn hash_file(path: &Path) -> Result<String, GameError> {
    hash(&mut open_read(path)?).map_err(io)
}
pub(super) fn manifest_path() -> Result<PathBuf, GameError> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(p).join("SubspaceToolkit/permanent.json"))
        .ok_or_else(|| io("LOCALAPPDATA unavailable"))
}
pub(super) fn lock() -> Result<Handle, GameError> {
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
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&data).map_err(|_| GameError::from("error.permanent_manifest"))?;
    let parse = || -> Option<InstalledMod> {
        if value["schema"].as_u64()? != 2 {
            return None;
        }
        let game_dir = PathBuf::from(value["game_dir"].as_str()?);
        let limit = value["limit"].as_f64()?;
        let movement_speed = value
            .get("movement_speed")
            .map(|v| v.as_f64())
            .unwrap_or(Some(1.0))?;
        let patched_hash = value["patched_hash"].as_str()?.to_owned();
        let previous_hash = value["previous_hash"].as_str()?.to_owned();
        let valid_hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        if !game_dir.is_absolute()
            || !(300.0..=1000.0).contains(&limit)
            || !(0.25..=3.0).contains(&movement_speed)
            || !valid_hash(&patched_hash)
            || !valid_hash(&previous_hash)
        {
            return None;
        }
        Some(InstalledMod {
            game_dir,
            patched_hash,
            previous_hash,
        })
    };
    parse()
        .map(Some)
        .ok_or_else(|| "error.permanent_manifest".into())
}
fn backup_path(target: &Path) -> PathBuf {
    target.with_extension("bundle.subspace-original")
}
fn allowed(current: &str, installed: Option<&InstalledMod>) -> bool {
    current == ORIGINAL_HASH
        || installed.is_some_and(|m| current == m.patched_hash || current == m.previous_hash)
}
fn remove_if_present(path: &Path) -> Result<(), GameError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(e)),
    }
}
fn replace_file<T>(
    target: &Path,
    write: impl FnOnce(&mut File) -> Result<T, GameError>,
) -> Result<T, GameError> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io)?
        .as_nanos();
    let temp = target.with_extension(format!("subspace-{}-{stamp}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(io)?;
    let result = write(&mut file).and_then(|value| {
        file.sync_all().map_err(io)?;
        Ok(value)
    });
    drop(file);
    let result = result.and_then(|value| {
        use std::os::windows::ffi::OsStrExt;
        let wide = |p: &Path| {
            p.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        if unsafe {
            MoveFileExW(
                wide(&temp).as_ptr(),
                wide(target).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io(io::Error::last_os_error()));
        }
        Ok(value)
    });
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
pub fn reset() -> Result<(), GameError> {
    let manifest = manifest_path()?;
    if let Some(installed) = read_manifest(&manifest)? {
        game::ensure_game_closed()?;
        restore(&installed, &manifest)?;
    }
    Ok(())
}

fn restore(installed: &InstalledMod, manifest: &Path) -> Result<(), GameError> {
    let target = installed.game_dir.join(BUNDLE_RELATIVE);
    let backup = backup_path(&target);
    let current_hash = hash_file(&target)?;
    if !allowed(&current_hash, Some(installed)) {
        return Err("error.permanent_conflict".into());
    }
    if current_hash != ORIGINAL_HASH {
        if hash_file(&backup).map_err(|_| GameError::from("error.permanent_backup"))?
            != ORIGINAL_HASH
        {
            return Err("error.permanent_backup".into());
        }
        replace_file(&target, |output| {
            io::copy(&mut open_read(&backup)?, output).map_err(io)?;
            output.rewind().map_err(io)?;
            if hash(output).map_err(io)? != ORIGINAL_HASH {
                return Err("error.permanent_backup".into());
            }
            if hash_file(&target)? != current_hash {
                return Err("error.permanent_conflict".into());
            }
            game::ensure_game_closed()
        })?;
        if hash_file(&target)? != ORIGINAL_HASH {
            return Err("error.permanent_conflict".into());
        }
    }
    // Never delete an unrelated file occupying our backup name.
    if backup.exists() && hash_file(&backup)? != ORIGINAL_HASH {
        return Err("error.permanent_backup".into());
    }
    remove_if_present(&backup)?;
    remove_if_present(manifest)
}

const BUNDLE_RELATIVE: &str = "StarTrekOutpostsUnknown_Data/StreamingAssets/aa/StandaloneWindows64/defaultlocalgroup_assets_all_992970c1c41bfb219a238d41ea4fd212.bundle";
const ORIGINAL_HASH: &str = "701c17aab143d83f15d63ed77a3bb2b122e8c2a765c9cfe95bdd24dc218cb108";
fn hash(input: &mut impl std::io::Read) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
