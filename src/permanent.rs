use crate::{
    game::{self, GameError},
    legacy,
    settings::{Features, Settings, atomic_create, atomic_write},
};
use sha2::{Digest, Sha256};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_SHARE_READ};

#[derive(Debug)]
pub struct InstalledMod {
    pub game_dir: PathBuf,
    pub limit: f32,
    pub movement_speed: f32,
    pub features: Features,
}
fn io(error: impl ToString) -> GameError {
    GameError::detail("error.permanent_io", error)
}

pub fn game_directory(dir: &Path) -> Result<PathBuf, GameError> {
    let dir = fs::canonicalize(dir).map_err(io)?;
    if !dir.join("StarTrekOutpostsUnknown.exe").is_file() {
        return Err("error.invalid_game_path".into());
    }
    Ok(dir)
}
fn config_path(dir: &Path) -> Result<PathBuf, GameError> {
    let data = dir.join("UserData");
    if data.try_exists().map_err(io)? && fs::canonicalize(&data).map_err(io)?.parent() != Some(dir)
    {
        return Err("error.invalid_game_path".into());
    }
    let path = data.join("SubspaceToolkit.json");
    if path.try_exists().map_err(io)?
        && fs::symlink_metadata(&path)
            .map_err(io)?
            .file_type()
            .is_symlink()
    {
        return Err("error.permanent_manifest".into());
    }
    Ok(path)
}
fn read_config(dir: &Path) -> Result<Option<InstalledMod>, GameError> {
    let path = config_path(dir)?;
    let input = match File::open(path) {
        Ok(input) => input,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(error)),
    };
    let mut bytes = Vec::new();
    input.take(16385).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() > 16384 {
        return Err("error.permanent_manifest".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| GameError::from("error.permanent_manifest"))?;
    let parse = || -> Option<InstalledMod> {
        if value.as_object()?.keys().any(|key| {
            ![
                "schema",
                "zoom_limit",
                "movement_speed",
                "resource_highlight",
                "repeat_build",
            ]
            .contains(&key.as_str())
        }) || value["schema"].as_u64()? != 1
        {
            return None;
        }
        let limit = value["zoom_limit"].as_f64()?;
        let movement_speed = value["movement_speed"].as_f64()?;
        if !(300.0..=1000.0).contains(&limit) || !(0.25..=3.0).contains(&movement_speed) {
            return None;
        }
        Some(InstalledMod {
            game_dir: dir.to_owned(),
            limit: limit as f32,
            movement_speed: movement_speed as f32,
            features: Features::parse(&value).ok()?,
        })
    };
    parse()
        .map(Some)
        .ok_or_else(|| "error.permanent_manifest".into())
}
fn no_legacy_patch() -> Result<(), GameError> {
    if legacy::manifest_path()?.try_exists().map_err(io)? {
        return Err("error.legacy_reset_required".into());
    }
    Ok(())
}
pub fn installed(dir: Option<&Path>) -> Result<Option<InstalledMod>, GameError> {
    no_legacy_patch()?;
    let Some(dir) = dir else { return Ok(None) };
    let dir = game_directory(dir)?;
    let config = read_config(&dir)?;
    Ok(if mod_files_present(&dir)? {
        config
    } else {
        None
    })
}
fn mod_files_present(dir: &Path) -> Result<bool, GameError> {
    for marker in [
        "version.dll",
        "MelonLoader/net6/MelonLoader.dll",
        "MelonLoader/net6/Il2CppInterop.Runtime.dll",
        "Mods/SubspaceToolkit.dll",
    ] {
        if !confined(dir, marker)?.is_file() {
            return Ok(false);
        }
    }
    Ok(true)
}
fn save_config(
    dir: &Path,
    limit: f32,
    movement_speed: f32,
    features: Features,
) -> Result<(), GameError> {
    validate_camera(limit, movement_speed)?;
    // Unknown contents belong to someone else; never overwrite them.
    read_config(dir)?;
    let bytes = serde_json::to_vec_pretty(
        &serde_json::json!({"schema":1,"zoom_limit":limit,"movement_speed":movement_speed,"resource_highlight":features.resource_highlight,"repeat_build":features.repeat_build}),
    )
    .map_err(io)?;
    atomic_write(&config_path(dir)?, &bytes).map_err(io)
}
fn validate_camera(limit: f32, movement_speed: f32) -> Result<(), GameError> {
    if !limit.is_finite() || !(300.0..=1000.0).contains(&limit) {
        return Err("error.invalid_limit".into());
    }
    if !movement_speed.is_finite() || !(0.25..=3.0).contains(&movement_speed) {
        return Err("error.invalid_speed".into());
    }
    Ok(())
}
pub fn install(
    dir: &Path,
    limit: f32,
    movement_speed: f32,
    features: Features,
    progress: impl Fn(usize),
) -> Result<(), GameError> {
    let _lock = legacy::lock()?;
    no_legacy_patch()?;
    let dir = game_directory(dir)?;
    validate_camera(limit, movement_speed)?;
    read_config(&dir)?;
    let payload = std::env::current_exe()
        .map_err(io)?
        .parent()
        .ok_or_else(|| io("Missing application directory"))?
        .join("payload");
    if payload.is_dir() {
        install_payload(
            &payload,
            &dir,
            game::ensure_game_closed().is_ok(),
            &progress,
        )?;
    } else if !mod_files_present(&dir)? {
        // Development builds can use a manually installed loader.
        return Err("error.mod_not_installed".into());
    }
    save_config(&dir, limit, movement_speed, features)?;
    progress(100);
    Ok(())
}

fn confined(root: &Path, relative: &str) -> Result<PathBuf, GameError> {
    let mut path = root.to_owned();
    for part in relative.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with('.')
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return Err("error.invalid_game_path".into());
        }
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
                return Err("error.invalid_game_path".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e)),
        }
    }
    Ok(path)
}

fn digest(path: &Path) -> Result<String, GameError> {
    let mut file = File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(io)?;
    if file.metadata().map_err(io)?.len() > 256 * 1024 * 1024 {
        return Err("error.permanent_manifest".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(io)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn loader_config_compatible(path: &Path) -> Result<bool, GameError> {
    let data = fs::read_to_string(path).map_err(io)?;
    Ok([
        "hide_console = true",
        "force_quit = false",
        "capture_player_logs = false",
    ]
    .iter()
    .all(|setting| data.lines().any(|line| line.trim() == *setting)))
}

fn install_payload(
    payload: &Path,
    dir: &Path,
    closed: bool,
    progress: &impl Fn(usize),
) -> Result<(), GameError> {
    let manifest_path = confined(payload, "manifest.json")?;
    if fs::metadata(&manifest_path).map_err(io)?.len() > 2 * 1024 * 1024 {
        return Err("error.permanent_manifest".into());
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(manifest_path).map_err(io)?).map_err(io)?;
    let entries = manifest["files"]
        .as_array()
        .filter(|entries| !entries.is_empty() && entries.len() <= 2000)
        .ok_or("error.permanent_manifest")?;
    if manifest["schema"].as_u64() != Some(1) {
        return Err("error.permanent_manifest".into());
    }
    let mut files = Vec::new();
    let mut names = std::collections::HashSet::new();
    for entry in entries {
        let name = entry["path"].as_str().ok_or("error.permanent_manifest")?;
        let hash = entry["sha256"].as_str().ok_or("error.permanent_manifest")?;
        if !(name == "version.dll"
            || name == "Mods/SubspaceToolkit.dll"
            || name == "UserData/Loader.cfg"
            || name.starts_with("MelonLoader/"))
            || hash.len() != 64
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || !names.insert(name.to_ascii_lowercase())
        {
            return Err("error.permanent_manifest".into());
        }
        files.push((name, hash, confined(payload, name)?, confined(dir, name)?));
    }
    for required in [
        "version.dll",
        "melonloader/net6/melonloader.dll",
        "melonloader/net6/il2cppinterop.runtime.dll",
        "mods/subspacetoolkit.dll",
        "userdata/loader.cfg",
    ] {
        if !names.contains(required) {
            return Err("error.permanent_manifest".into());
        }
    }
    // Never activate a partially copied loader. Existing files are never overwritten.
    files.sort_by_key(|(name, _, _, _)| *name == "version.dll");
    let mut missing = Vec::new();
    for (index, (name, expected, source, target)) in files.iter().enumerate() {
        // Existing installations can save tiny settings files while the game runs.
        if !closed
            && name.starts_with("MelonLoader/")
            && ![
                "MelonLoader/net6/MelonLoader.dll",
                "MelonLoader/net6/Il2CppInterop.Runtime.dll",
            ]
            .contains(name)
        {
            continue;
        }
        if digest(source)? != expected.to_ascii_lowercase() {
            return Err(GameError::detail("error.payload_invalid", name));
        }
        if target.try_exists().map_err(io)? {
            let matches = if *name == "UserData/Loader.cfg" {
                loader_config_compatible(target)?
            } else {
                digest(target)? == expected.to_ascii_lowercase()
            };
            if !matches {
                return Err(GameError::detail("error.payload_conflict", name));
            }
        } else {
            if !closed {
                return Err("error.game_must_close".into());
            }
            missing.push((name, expected, source, target));
        }
        progress(index * 50 / files.len());
    }
    for (index, (name, expected, source, target)) in missing.iter().enumerate() {
        fs::create_dir_all(target.parent().ok_or("error.invalid_game_path")?).map_err(io)?;
        confined(dir, name)?;
        let input = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(source)
            .map_err(io)?;
        let mut data = Vec::new();
        input
            .take(256 * 1024 * 1024 + 1)
            .read_to_end(&mut data)
            .map_err(io)?;
        if data.len() > 256 * 1024 * 1024
            || Sha256::digest(&data)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                != expected.to_ascii_lowercase()
        {
            return Err("error.payload_invalid".into());
        }
        atomic_create(target, &data).map_err(io)?;
        progress(50 + (index + 1) * 49 / missing.len());
    }
    Ok(())
}
fn remove_config(dir: &Path) -> Result<(), GameError> {
    if read_config(dir)?.is_some() {
        fs::remove_file(config_path(dir)?).map_err(io)?;
    }
    Ok(())
}
pub fn reset(dir: Option<&Path>) -> Result<(), GameError> {
    let _lock = legacy::lock()?;
    // Recovery-only path for installations made by older toolkit builds.
    legacy::reset()?;
    reset_settings(dir, &crate::settings::settings_path().map_err(io)?)
}
fn reset_settings(dir: Option<&Path>, preferences: &Path) -> Result<(), GameError> {
    if let Some(dir) = dir {
        remove_config(&game_directory(dir)?)?;
    }
    Settings::default()
        .save_to(preferences)
        .map_err(|e| GameError::detail("reset_settings_failed", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires built release payload; installs only into a temporary fixture"]
    fn release_payload_installs_in_temporary_folder_and_can_save_while_running() {
        let payload = PathBuf::from(
            std::env::var_os("SUBSPACE_TEST_PAYLOAD").expect("set SUBSPACE_TEST_PAYLOAD"),
        );
        let dir = std::env::temp_dir().join(format!(
            "subspace-payload-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let dir = fs::canonicalize(dir).unwrap();
        install_payload(&payload, &dir, true, &|_| {}).unwrap();
        assert_eq!(
            digest(&dir.join("version.dll")).unwrap(),
            digest(&payload.join("version.dll")).unwrap()
        );
        install_payload(&payload, &dir, false, &|_| {}).unwrap();
        save_config(
            &dir,
            1000.0,
            3.0,
            Features {
                resource_highlight: true,
                repeat_build: true,
            },
        )
        .unwrap();
        let installed = read_config(&dir).unwrap().unwrap();
        assert_eq!((installed.limit, installed.movement_speed), (1000.0, 3.0));
        assert!(installed.features.resource_highlight && installed.features.repeat_build);
        remove_config(&dir).unwrap();
        assert!(dir.join("Mods/SubspaceToolkit.dll").exists());
        assert!(read_config(&dir).unwrap().is_none());
        assert!(dir.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn complete_reset_preserves_preferences_on_mod_failure_and_can_retry_partial_success() {
        let dir = std::env::temp_dir().join(format!(
            "subspace-reset-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("UserData")).unwrap();
        fs::write(dir.join("StarTrekOutpostsUnknown.exe"), []).unwrap();
        let dir = fs::canonicalize(dir).unwrap();
        let preferences = dir.join("preferences.json");
        let settings = Settings {
            zoom_limit: Some(700.0),
            ..Default::default()
        };
        settings.save_to(&preferences).unwrap();
        let before = fs::read(&preferences).unwrap();
        fs::write(config_path(&dir).unwrap(), b"foreign config").unwrap();
        assert!(reset_settings(Some(&dir), &preferences).is_err());
        assert_eq!(fs::read(&preferences).unwrap(), before);
        fs::remove_file(config_path(&dir).unwrap()).unwrap();
        save_config(&dir, 700.0, 2.0, Features::default()).unwrap();
        let locked = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&preferences)
            .unwrap();
        assert_eq!(
            reset_settings(Some(&dir), &preferences).unwrap_err().key,
            "reset_settings_failed"
        );
        assert!(read_config(&dir).unwrap().is_none());
        assert_eq!(fs::read(&preferences).unwrap(), before);
        drop(locked);
        reset_settings(Some(&dir), &preferences).unwrap();
        assert!(!preferences.exists());
        reset_settings(Some(&dir), &preferences).unwrap();
        assert!(dir.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn loader_install_validates_before_activation_and_never_overwrites_foreign_files() {
        let root = std::env::temp_dir().join(format!(
            "subspace-loader-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let payload = root.join("payload");
        let game = root.join("game");
        fs::create_dir_all(&game).unwrap();
        let game = fs::canonicalize(game).unwrap();
        let names = [
            "version.dll",
            "MelonLoader/net6/MelonLoader.dll",
            "MelonLoader/net6/Il2CppInterop.Runtime.dll",
            "Mods/SubspaceToolkit.dll",
            "UserData/Loader.cfg",
        ];
        let mut entries = Vec::new();
        for name in names {
            let path = payload.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                if name.ends_with(".cfg") {
                    "hide_console = true\nforce_quit = false\ncapture_player_logs = false\n"
                } else {
                    name
                },
            )
            .unwrap();
            entries.push(serde_json::json!({"path":name,"sha256":digest(&path).unwrap()}));
        }
        let manifest = serde_json::json!({"schema":1,"files":entries});
        fs::write(payload.join("manifest.json"), manifest.to_string()).unwrap();
        assert!(install_payload(&payload, &game, false, &|_| {}).is_err());
        assert!(!game.join("version.dll").exists());
        fs::create_dir_all(game.join("Mods")).unwrap();
        fs::write(game.join("Mods/SubspaceToolkit.dll"), b"someone else's mod").unwrap();
        assert!(install_payload(&payload, &game, true, &|_| {}).is_err());
        assert!(!game.join("version.dll").exists());
        assert_eq!(
            fs::read(game.join("Mods/SubspaceToolkit.dll")).unwrap(),
            b"someone else's mod"
        );
        fs::remove_file(game.join("Mods/SubspaceToolkit.dll")).unwrap();
        let original = fs::read(payload.join("version.dll")).unwrap();
        fs::write(payload.join("version.dll"), b"corrupt download").unwrap();
        assert!(install_payload(&payload, &game, true, &|_| {}).is_err());
        assert!(!game.join("MelonLoader/net6/MelonLoader.dll").exists());
        fs::write(payload.join("version.dll"), original).unwrap();
        install_payload(&payload, &game, true, &|value| assert!(value <= 100)).unwrap();
        save_config(&game, 700.0, 2.0, Features::default()).unwrap();
        assert!(mod_files_present(&game).unwrap());
        for name in names.into_iter().filter(|name| name.ends_with(".dll")) {
            let path = game.join(name);
            let bytes = fs::read(&path).unwrap();
            fs::remove_file(&path).unwrap();
            assert!(!mod_files_present(&game).unwrap(), "missing {name}");
            assert!(read_config(&game).unwrap().is_some());
            fs::write(&path, bytes).unwrap();
            assert!(mod_files_present(&game).unwrap());
        }
        let config = game.join("UserData/Loader.cfg");
        let custom = format!(
            "{}\n# user settings preserved\n",
            fs::read_to_string(&config).unwrap()
        );
        fs::write(&config, &custom).unwrap();
        install_payload(&payload, &game, true, &|_| {}).unwrap();
        install_payload(&payload, &game, false, &|_| {}).unwrap();
        assert_eq!(fs::read_to_string(config).unwrap(), custom);
        assert!(atomic_create(&game.join("version.dll"), b"replacement").is_err());
        assert_eq!(fs::read(game.join("version.dll")).unwrap(), b"version.dll");
        for path in [
            "../escape",
            "MelonLoader/../../escape",
            "C:/escape",
            "MelonLoader/file:stream",
            "MelonLoader/NUL.txt",
            "MelonLoader/file.",
        ] {
            assert!(confined(&game, path).is_err(), "{path}");
        }
        let mut malicious = manifest.clone();
        malicious["files"][0]["path"] = serde_json::json!("MelonLoader/../version.dll");
        fs::write(payload.join("manifest.json"), malicious.to_string()).unwrap();
        assert!(install_payload(&payload, &game, true, &|_| {}).is_err());
        assert!(root.starts_with(std::env::temp_dir()));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn config_save_is_small_atomic_validated_and_reset_preserves_unrelated_files() {
        let dir = std::env::temp_dir().join(format!(
            "subspace-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("UserData")).unwrap();
        let dir = fs::canonicalize(dir).unwrap();
        fs::write(dir.join("UserData/OtherMod.json"), b"foreign").unwrap();
        let features = Features {
            resource_highlight: true,
            repeat_build: true,
        };
        save_config(&dir, 1000.0, 3.0, features).unwrap();
        let saved = read_config(&dir).unwrap().unwrap();
        assert_eq!((saved.limit, saved.movement_speed), (1000.0, 3.0));
        assert_eq!(saved.features, features);
        assert!(fs::metadata(config_path(&dir).unwrap()).unwrap().len() < 200);
        let before = fs::read(config_path(&dir).unwrap()).unwrap();
        for (limit, speed) in [
            (299.0, 1.0),
            (1001.0, 1.0),
            (f32::NAN, 1.0),
            (500.0, 0.0),
            (500.0, f32::INFINITY),
        ] {
            assert!(save_config(&dir, limit, speed, features).is_err());
            assert_eq!(fs::read(config_path(&dir).unwrap()).unwrap(), before);
        }
        save_config(&dir, 500.0, 0.5, Features::default()).unwrap();
        assert_eq!(read_config(&dir).unwrap().unwrap().limit, 500.0);
        assert_eq!(fs::read_dir(dir.join("UserData")).unwrap().count(), 2);
        remove_config(&dir).unwrap();
        remove_config(&dir).unwrap();
        assert_eq!(
            fs::read(dir.join("UserData/OtherMod.json")).unwrap(),
            b"foreign"
        );
        let foreign = b"{\"schema\":4,\"other\":true}";
        fs::write(config_path(&dir).unwrap(), foreign).unwrap();
        assert!(save_config(&dir, 500.0, 1.0, features).is_err());
        assert!(remove_config(&dir).is_err());
        assert_eq!(fs::read(config_path(&dir).unwrap()).unwrap(), foreign);
        for value in [
            serde_json::json!({"schema":1,"zoom_limit":1001,"movement_speed":1}),
            serde_json::json!({"schema":1,"zoom_limit":500,"movement_speed":0}),
            serde_json::json!({"schema":1,"zoom_limit":500,"movement_speed":1,"foreign":true}),
        ] {
            fs::write(config_path(&dir).unwrap(), value.to_string()).unwrap();
            assert!(read_config(&dir).is_err());
        }
        assert!(dir.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(dir).unwrap();
    }
}
