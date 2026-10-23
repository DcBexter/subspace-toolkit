use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

#[derive(Debug, Default, PartialEq)]
pub struct Settings {
    pub zoom_limit: Option<f32>,
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
        Ok(Self { zoom_limit })
    }

    fn save_to(&self, path: &Path) -> Result<(), String> {
        if self.zoom_limit.is_none() {
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
        let data = serde_json::to_vec_pretty(&serde_json::json!({"zoom_limit": self.zoom_limit}))
            .map_err(|e| e.to_string())?;
        atomic_write(path, &data)
    }
}

pub(crate) fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
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
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
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

fn settings_path() -> Result<PathBuf, String> {
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
        }
        .save_to(&path)
        .unwrap();
        Settings {
            zoom_limit: Some(1000.0),
        }
        .save_to(&path)
        .unwrap();
        assert!(
            Settings {
                zoom_limit: Some(f32::NAN)
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
}
