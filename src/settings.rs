use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub zoom_limit: Option<f32>,
    pub movement_speed: Option<f32>,
    pub language: Option<String>,
    pub game_dir: Option<PathBuf>,
    pub features: Features,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Features {
    pub resource_highlight: bool,
    pub repeat_build: bool,
    pub move_buildings: bool,
}
impl Features {
    pub fn parse(value: &serde_json::Value) -> Result<Self, String> {
        let flag = |key: &str| {
            value.get(key).map_or(Ok(false), |v| {
                v.as_bool().ok_or_else(|| format!("{key} must be boolean"))
            })
        };
        Ok(Self {
            resource_highlight: flag("resource_highlight")?,
            repeat_build: flag("repeat_build")?,
            move_buildings: flag("move_buildings")?,
        })
    }
}

impl Settings {
    pub fn load() -> Result<Self, String> {
        Self::load_from(&settings_path()?)
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&settings_path()?)
    }

    fn load_from(path: &Path) -> Result<Self, String> {
        let data = match fs::read(path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.to_string()),
        };
        Self::parse(&data)
    }

    fn parse(data: &[u8]) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_slice(data).map_err(|e| e.to_string())?;
        let object = value
            .as_object()
            .ok_or("settings.json must contain an object")?;
        let zoom_limit = match object.get("zoom_limit") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => {
                let value = value
                    .as_f64()
                    .ok_or("zoom_limit must be a number or null")?;
                if !value.is_finite() || !(1.0..=1000.0).contains(&value) {
                    return Err("zoom_limit must be between 1 and 1000".into());
                }
                Some(value as f32)
            }
        };
        let movement_speed = match object.get("movement_speed") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => {
                let n = v.as_f64().ok_or("movement_speed must be numeric")?;
                if !(0.25..=3.0).contains(&n) {
                    return Err("movement_speed must be between 0.25 and 3".into());
                }
                Some(n as f32)
            }
        };
        let text = |key: &str| -> Result<Option<String>, String> {
            match object.get(key) {
                None | Some(serde_json::Value::Null) => Ok(None),
                Some(value) => value
                    .as_str()
                    .map(|s| Some(s.to_owned()))
                    .ok_or_else(|| format!("{key} must be text or null")),
            }
        };
        let language = text("language")?;
        if language.as_ref().is_some_and(|s| {
            s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-')
        }) {
            return Err("Invalid language code".into());
        }
        let game_dir = text("game_dir")?.map(PathBuf::from);
        if game_dir.as_ref().is_some_and(|p| !p.is_absolute()) {
            return Err("Game folder must be absolute".into());
        }
        Ok(Self {
            zoom_limit,
            movement_speed,
            language,
            game_dir,
            features: Features::parse(&value)?,
        })
    }

    pub(crate) fn save_to(&self, path: &Path) -> Result<(), String> {
        if self == &Self::default() {
            return match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            };
        }
        if self
            .zoom_limit
            .is_some_and(|value| !value.is_finite() || !(1.0..=1000.0).contains(&value))
        {
            return Err("zoom_limit must be between 1 and 1000".into());
        }
        if self
            .movement_speed
            .is_some_and(|n| !n.is_finite() || !(0.25..=3.0).contains(&n))
        {
            return Err("movement_speed must be between 0.25 and 3".into());
        }
        let data = serde_json::to_vec_pretty(&serde_json::json!({"zoom_limit": self.zoom_limit, "movement_speed": self.movement_speed, "language": self.language, "game_dir": self.game_dir, "resource_highlight": self.features.resource_highlight, "repeat_build": self.features.repeat_build, "move_buildings": self.features.move_buildings}))
            .map_err(|e| e.to_string())?;
        Self::parse(&data)?;
        atomic_write(path, &data)
    }
}

pub(crate) fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
    write_atomically(path, data, true)
}

pub(crate) fn atomic_create(path: &Path, data: &[u8]) -> Result<(), String> {
    write_atomically(path, data, false)
}

fn write_atomically(path: &Path, data: &[u8], replace: bool) -> Result<(), String> {
    let parent = path.parent().ok_or("settings path has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temp = parent.join(format!("settings-{}-{stamp}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(data).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        let wide = |path: &Path| {
            use std::os::windows::ffi::OsStrExt;
            path.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        if unsafe {
            MoveFileExW(
                wide(&temp).as_ptr(),
                wide(path).as_ptr(),
                (if replace {
                    MOVEFILE_REPLACE_EXISTING
                } else {
                    0
                }) | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub(crate) fn settings_path() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(path).join("SubspaceToolkit/settings.json"))
        .ok_or_else(|| "LOCALAPPDATA is unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_settings_and_keeps_valid_limits() {
        for input in [
            r#"{"zoom_limit":0}"#,
            r#"{"zoom_limit":1000.1}"#,
            r#"{"zoom_limit":"1000"}"#,
            r#"{"movement_speed":0}"#,
            r#"{"movement_speed":3.1}"#,
            r#"{"movement_speed":"fast"}"#,
            r#"{"resource_highlight":1}"#,
            r#"{"repeat_build":"true"}"#,
            r#"{"move_buildings":1}"#,
            r#"{"move_buildings":null}"#,
            "[]",
            "not json",
        ] {
            assert!(Settings::parse(input.as_bytes()).is_err());
        }
        for limit in [1.0, 300.0, 1000.0] {
            let text = format!(r#"{{"zoom_limit":{limit}}}"#);
            assert_eq!(
                Settings::parse(text.as_bytes()).unwrap().zoom_limit,
                Some(limit)
            );
        }
        assert_eq!(
            Settings::parse(br#"{"zoom_limit":null}"#).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn atomic_replace_and_reset_preserve_previous_file_on_invalid_save() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "subspace-settings-test-{}-{stamp}",
            std::process::id()
        ));
        let path = directory.join("settings.json");
        assert_eq!(Settings::load_from(&path).unwrap(), Settings::default());
        Settings {
            zoom_limit: Some(700.0),
            movement_speed: Some(2.0),
            features: Features {
                resource_highlight: true,
                repeat_build: true,
                move_buildings: true,
            },
            ..Default::default()
        }
        .save_to(&path)
        .unwrap();
        assert_eq!(
            Settings::load_from(&path).unwrap().movement_speed,
            Some(2.0)
        );
        assert_eq!(
            Settings::load_from(&path).unwrap().features,
            Features {
                resource_highlight: true,
                repeat_build: true,
                move_buildings: true,
            }
        );
        let before = fs::read(&path).unwrap();
        assert!(
            Settings {
                movement_speed: Some(f32::NAN),
                ..Default::default()
            }
            .save_to(&path)
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        Settings {
            zoom_limit: Some(1000.0),
            ..Default::default()
        }
        .save_to(&path)
        .unwrap();
        assert!(
            Settings {
                zoom_limit: Some(f32::NAN),
                ..Default::default()
            }
            .save_to(&path)
            .is_err()
        );
        assert_eq!(Settings::load_from(&path).unwrap().zoom_limit, Some(1000.0));
        Settings::default().save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).unwrap(), Settings::default());
        assert!(!path.exists());
        Settings::default().save_to(&path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn language_and_folder_round_trip_with_legacy_defaults() {
        let settings =
            Settings::parse(br#"{"language":"de","game_dir":"E:/Game","zoom_limit":700}"#).unwrap();
        assert_eq!(settings.language.as_deref(), Some("de"));
        assert_eq!(settings.game_dir, Some(PathBuf::from("E:/Game")));
        assert_eq!(settings.features, Features::default());
        let legacy =
            Settings::parse(br#"{"resource_highlight":true,"repeat_build":true}"#).unwrap();
        assert!(legacy.features.resource_highlight && legacy.features.repeat_build);
        assert!(!legacy.features.move_buildings);
        assert_eq!(
            Settings::parse(br#"{"zoom_limit":700}"#).unwrap().language,
            None
        );
        for input in [r#"{"language":"../de"}"#, r#"{"game_dir":"relative"}"#] {
            assert!(Settings::parse(input.as_bytes()).is_err());
        }
    }
}
