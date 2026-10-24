use crate::settings::Features;
use crate::settings::atomic_write;
#[cfg(not(all(windows, target_arch = "x86_64")))]
compile_error!("Subspace Toolkit requires 64-bit Windows.");
use std::{
    fs::{self, File},
    io::Read,
    mem::size_of,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::*,
        Threading::{
            CreateMutexW, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};
const GAME: &str = "StarTrekOutpostsUnknown.exe";
pub const MAX_LIMIT: f32 = 1000.0;
const STATUS_MAX_AGE_MS: u64 = 5000;
const LEASE_MS: u64 = 3000;
#[derive(Debug)]
pub struct GameError {
    pub key: &'static str,
    pub detail: Option<String>,
}
impl GameError {
    pub(crate) fn detail(key: &'static str, detail: impl ToString) -> Self {
        Self {
            key,
            detail: Some(detail.to_string()),
        }
    }
}
impl From<&'static str> for GameError {
    fn from(key: &'static str) -> Self {
        Self { key, detail: None }
    }
}

pub(crate) struct Handle(HANDLE);
// Windows handles transfer between threads; ownership stays singular.
unsafe impl Send for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl Handle {
    pub(crate) fn new(handle: HANDLE) -> Result<Self, GameError> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(GameError::detail(
                "error.system_access",
                std::io::Error::last_os_error(),
            ))
        } else {
            Ok(Self(handle))
        }
    }
}
fn wide_text(text: &[u16]) -> String {
    String::from_utf16_lossy(&text[..text.iter().position(|&c| c == 0).unwrap_or(text.len())])
}
fn game_processes() -> Result<Vec<u32>, GameError> {
    let snapshot = Handle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = Vec::new();
    let mut valid = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while valid {
        if wide_text(&entry.szExeFile).eq_ignore_ascii_case(GAME) {
            found.push(entry.th32ProcessID);
        }
        valid = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    Ok(found)
}

pub(crate) fn ensure_game_closed() -> Result<(), GameError> {
    if game_processes()?.is_empty() {
        Ok(())
    } else {
        Err("error.game_must_close".into())
    }
}

fn find_game() -> Result<(u32, PathBuf), GameError> {
    let pid = match game_processes()?.as_slice() {
        [] => return Err("error.game_not_found".into()),
        [pid] => *pid,
        _ => return Err("error.multiple_games".into()),
    };
    let modules = Handle::new(unsafe {
        CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
    })?;
    let mut module = MODULEENTRY32W {
        dwSize: size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    let mut valid = unsafe { Module32FirstW(modules.0, &mut module) } != 0;
    while valid {
        if wide_text(&module.szModule).eq_ignore_ascii_case(GAME) {
            let exe = PathBuf::from(wide_text(&module.szExePath));
            return Ok((
                pid,
                fs::canonicalize(exe.parent().ok_or("error.invalid_game_path")?)
                    .map_err(|e| GameError::detail("error.invalid_game_path", e))?,
            ));
        }
        valid = unsafe { Module32NextW(modules.0, &mut module) } != 0;
    }
    Err("error.game_loading".into())
}

#[derive(Clone, Copy, Debug)]
pub struct CameraState {
    pub original: f32,
    pub maximum: f32,
    pub current: f32,
    pub speed: f32,
}

fn now_ms() -> Result<u64, GameError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .map_err(|e| GameError::detail("error.system_access", e))
}
fn read_json(path: &Path) -> Result<serde_json::Value, GameError> {
    let input = File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GameError::from("error.mod_unavailable")
        } else {
            GameError::detail("error.file_read", e)
        }
    })?;
    let mut bytes = Vec::new();
    input
        .take(16_385)
        .read_to_end(&mut bytes)
        .map_err(|e| GameError::detail("error.file_read", e))?;
    if bytes.len() > 16_384 {
        return Err("error.mod_incompatible".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| GameError::detail("error.mod_incompatible", e))
}
fn parse_status(
    value: &serde_json::Value,
    pid: u32,
    game_dir: &Path,
    now: u64,
) -> Result<Option<CameraState>, GameError> {
    if value["bridge_version"].as_u64() != Some(2) {
        return Err("error.mod_unavailable".into());
    }
    if value["schema"].as_u64() != Some(1) || value["pid"].as_u64() != Some(pid as u64) {
        return Err("error.mod_incompatible".into());
    }
    let updated = value["updated_utc_ms"]
        .as_u64()
        .ok_or("error.mod_incompatible")?;
    if updated > now.saturating_add(1000) || now.saturating_sub(updated) > STATUS_MAX_AGE_MS {
        return Err("error.mod_unavailable".into());
    }
    let reported_dir = value["game_dir"]
        .as_str()
        .ok_or("error.invalid_game_path")?;
    let canonical = fs::canonicalize(reported_dir)
        .map_err(|e| GameError::detail("error.invalid_game_path", e))?;
    if !canonical
        .to_string_lossy()
        .eq_ignore_ascii_case(&game_dir.to_string_lossy())
    {
        return Err("error.invalid_game_path".into());
    }
    if value["compatible"].as_bool() != Some(true) {
        return Err(GameError::detail(
            "error.mod_incompatible",
            value["error"].as_str().unwrap_or("Camera API unavailable"),
        ));
    }
    if let Some(error) = value["error"].as_str().filter(|e| !e.is_empty()) {
        return Err(GameError::detail("error.mod_incompatible", error));
    }
    match value["camera_ready"].as_bool() {
        Some(false) => return Ok(None),
        Some(true) => {}
        None => return Err("error.mod_incompatible".into()),
    }
    let number = |key: &str| -> Result<f32, GameError> {
        let n = value[key].as_f64().ok_or("error.camera_bounds")? as f32;
        if !n.is_finite() {
            return Err("error.camera_bounds".into());
        }
        Ok(n)
    };
    let state = CameraState {
        original: number("original")?,
        maximum: number("maximum")?,
        current: number("current")?,
        speed: number("speed")?,
    };
    if !(0.0..=MAX_LIMIT).contains(&state.original)
        || state.original == 0.0
        || !(300.0..=MAX_LIMIT).contains(&state.maximum)
        || !(0.0..=1_000_000.0).contains(&state.current)
        || !(0.25..=3.0).contains(&state.speed)
    {
        return Err("error.camera_bounds".into());
    }
    Ok(Some(state))
}
fn remove_owned_lease(path: &Path, owner: &str) -> Result<(), GameError> {
    let value = match read_json(path) {
        Ok(value) => value,
        Err(error) if error.key == "error.mod_unavailable" => return Ok(()),
        Err(error) => return Err(error),
    };
    if value["owner"].as_str() != Some(owner) {
        return Err("error.restore_external_change".into());
    }
    fs::remove_file(path).map_err(|e| GameError::detail("error.write_failed", e))
}

pub struct GameSession {
    pub game_dir: PathBuf,
    pub feature_error: Option<String>,
    pid: u32,
    process: Handle,
    _connection_lock: Handle,
    status_path: PathBuf,
    lease_path: PathBuf,
    owner: String,
    desired: Option<(f32, f32, Features)>,
}
impl GameSession {
    pub fn connect() -> Result<Self, GameError> {
        let (pid, game_dir) = find_game()?;
        let mutex_name: Vec<u16> = format!("Local\\SubspaceToolkit-{pid}\0")
            .encode_utf16()
            .collect();
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
        let already_connected = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let connection_lock = Handle::new(mutex)?;
        if already_connected {
            return Err("error.already_connected".into());
        }
        let process =
            Handle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
        let root = PathBuf::from(
            std::env::var_os("LOCALAPPDATA")
                .filter(|s| !s.is_empty())
                .ok_or("error.invalid_game_path")?,
        )
        .join("SubspaceToolkit");
        let mut session = Self {
            game_dir,
            feature_error: None,
            pid,
            process,
            _connection_lock: connection_lock,
            status_path: root.join(format!("status-{pid}.json")),
            lease_path: root.join(format!("live-{pid}.json")),
            owner: format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| GameError::detail("error.system_access", e))?
                    .as_nanos()
            ),
            desired: None,
        };
        let ready = read_json(&session.status_path).ok().is_some_and(|status| {
            now_ms().is_ok_and(|now| parse_status(&status, pid, &session.game_dir, now).is_ok())
        });
        if !ready {
            crate::inject::ensure_attached(pid, &session.game_dir)?;
        }
        session.poll()?;
        Ok(session)
    }
    pub fn poll(&mut self) -> Result<Option<CameraState>, GameError> {
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 || code != 259 {
            return Err("error.game_access_lost".into());
        }
        let value = read_json(&self.status_path)?;
        self.feature_error = value["feature_error"].as_str().map(str::to_owned);
        let state = parse_status(&value, self.pid, &self.game_dir, now_ms()?)?;
        if state.is_some()
            && let Some(desired) = self.desired
        {
            self.write_command(desired)?;
        }
        Ok(state)
    }
    fn write_command(&mut self, desired: (f32, f32, Features)) -> Result<(), GameError> {
        let now = now_ms()?;
        match read_json(&self.lease_path) {
            Ok(value)
                if value["owner"].as_str() != Some(&self.owner)
                    && value["expires_utc_ms"]
                        .as_u64()
                        .is_none_or(|expires| expires > now) =>
            {
                return Err("error.external_change".into());
            }
            Err(error) if error.key != "error.mod_unavailable" => return Err(error),
            _ => {}
        }
        let command = serde_json::json!({"schema":1,"owner":self.owner,"zoom_limit":desired.0,"movement_speed":desired.1,"expires_utc_ms":now+LEASE_MS,"resource_highlight":desired.2.resource_highlight,"repeat_build":desired.2.repeat_build});
        atomic_write(
            &self.lease_path,
            &serde_json::to_vec(&command)
                .map_err(|e| GameError::detail("error.write_failed", e))?,
        )
        .map_err(|e| GameError::detail("error.write_failed", e))?;
        self.desired = Some(desired);
        Ok(())
    }
    pub fn apply(
        &mut self,
        limit: f32,
        multiplier: f32,
        features: Features,
    ) -> Result<(), GameError> {
        if !limit.is_finite() || !(300.0..=MAX_LIMIT).contains(&limit) {
            return Err("error.invalid_limit".into());
        }
        if !multiplier.is_finite() || !(0.25..=3.0).contains(&multiplier) {
            return Err("error.invalid_speed".into());
        }
        self.poll()?.ok_or("error.camera_unavailable")?;
        self.write_command((limit, multiplier, features))
    }
    pub fn restore(&mut self) -> Result<(), GameError> {
        if self.desired.is_some() {
            remove_owned_lease(&self.lease_path, &self.owner)?;
            self.desired = None;
        }
        Ok(())
    }
}
impl Drop for GameSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status(dir: &Path, pid: u32, now: u64) -> serde_json::Value {
        serde_json::json!({"schema":1,"bridge_version":2,"pid":pid,"game_dir":dir,"updated_utc_ms":now,
            "compatible":true,"camera_ready":true,"original":300,"maximum":1000,"current":165,"speed":2.0,"error":null})
    }
    #[test]
    fn status_checks_identity_freshness_compatibility_and_camera_bounds() {
        let dir = fs::canonicalize(std::env::temp_dir()).unwrap();
        let now = 10_000;
        let valid = status(&dir, 42, now);
        assert_eq!(
            parse_status(&valid, 42, &dir, now).unwrap().unwrap().speed,
            2.0
        );
        for (key, value, expected) in [
            (
                "bridge_version",
                serde_json::json!(1),
                "error.mod_unavailable",
            ),
            ("pid", serde_json::json!(43), "error.mod_incompatible"),
            ("schema", serde_json::json!(2), "error.mod_incompatible"),
            (
                "updated_utc_ms",
                serde_json::json!(4999),
                "error.mod_unavailable",
            ),
            (
                "updated_utc_ms",
                serde_json::json!(11001),
                "error.mod_unavailable",
            ),
            (
                "compatible",
                serde_json::json!(false),
                "error.mod_incompatible",
            ),
            ("maximum", serde_json::json!(1001), "error.camera_bounds"),
            ("speed", serde_json::json!(0.0), "error.camera_bounds"),
            ("current", serde_json::Value::Null, "error.camera_bounds"),
            (
                "error",
                serde_json::json!("Camera modified externally"),
                "error.mod_incompatible",
            ),
        ] {
            let mut value_copy = valid.clone();
            value_copy[key] = value;
            assert_eq!(
                parse_status(&value_copy, 42, &dir, now).unwrap_err().key,
                expected,
                "{key}"
            );
        }
        let mut loading = valid.clone();
        loading["camera_ready"] = serde_json::json!(false);
        loading["maximum"] = serde_json::json!(0);
        assert!(parse_status(&loading, 42, &dir, now).unwrap().is_none());
        assert!(parse_status(&valid, 42, &dir.join("wrong"), now).is_err());
    }
    #[test]
    fn trainer_lease_preserves_permanent_settings_reports_actual_state_and_only_removes_own_file() {
        let dir = std::env::temp_dir().join(format!(
            "subspace-bridge-{}-{}",
            std::process::id(),
            now_ms().unwrap()
        ));
        fs::create_dir_all(&dir).unwrap();
        let dir = fs::canonicalize(dir).unwrap();
        let open = || {
            Handle::new(unsafe {
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, std::process::id())
            })
            .unwrap()
        };
        let status_path = dir.join("status.json");
        fs::write(
            &status_path,
            status(&dir, std::process::id(), now_ms().unwrap()).to_string(),
        )
        .unwrap();
        let permanent = dir.join("SubspaceToolkit.json");
        fs::write(&permanent, b"durable settings must stay unchanged").unwrap();
        let mut session = GameSession {
            game_dir: dir.clone(),
            feature_error: None,
            pid: std::process::id(),
            process: open(),
            _connection_lock: open(),
            status_path,
            lease_path: dir.join("live.json"),
            owner: "test-owner".into(),
            desired: None,
        };
        let features = Features {
            resource_highlight: true,
            repeat_build: true,
        };
        session.apply(500.0, 3.0, features).unwrap();
        let command = read_json(&session.lease_path).unwrap();
        assert_eq!(command["zoom_limit"], 500.0);
        assert_eq!(command["movement_speed"], 3.0);
        assert_eq!(command["resource_highlight"], true);
        assert_eq!(command["repeat_build"], true);
        assert_eq!(command["owner"], "test-owner");
        assert!(command["expires_utc_ms"].as_u64().unwrap() > now_ms().unwrap());
        // A successfully written command is not evidence the game has applied it.
        assert_eq!(session.poll().unwrap().unwrap().maximum, 1000.0);
        assert_eq!(session.poll().unwrap().unwrap().speed, 2.0);
        for invalid in [f32::NAN, f32::INFINITY, 0.0, 3.01] {
            assert!(session.apply(500.0, invalid, features).is_err());
        }
        for invalid in [f32::NAN, f32::INFINITY, 299.0, 1001.0] {
            assert!(session.apply(invalid, 3.0, features).is_err());
        }
        session.restore().unwrap();
        assert!(!session.lease_path.exists());
        session.apply(500.0, 0.25, Features::default()).unwrap();
        let mut foreign = read_json(&session.lease_path).unwrap();
        foreign["owner"] = serde_json::json!("someone-else");
        fs::write(&session.lease_path, foreign.to_string()).unwrap();
        assert_eq!(
            session.apply(500.0, 1.0, features).unwrap_err().key,
            "error.external_change"
        );
        assert_eq!(
            session.restore().unwrap_err().key,
            "error.restore_external_change"
        );
        assert_eq!(
            read_json(&session.lease_path).unwrap()["owner"],
            "someone-else"
        );
        assert_eq!(
            fs::read(&permanent).unwrap(),
            b"durable settings must stay unchanged"
        );
        drop(session);
        assert!(dir.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(dir).unwrap();
    }
}
