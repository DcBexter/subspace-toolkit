use std::{collections::HashMap, path::Path};
use windows_sys::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};

type Messages = HashMap<String, String>;
const ENGLISH: &str = include_str!("../locales/en.json");
const GERMAN: &str = include_str!("../locales/de.json");

pub struct Catalog {
    messages: Messages,
}

impl Catalog {
    pub fn system() -> Self {
        let mut locale = [0u16; 85];
        let length = unsafe {
            LCIDToLocaleName(
                GetUserDefaultUILanguage() as u32,
                locale.as_mut_ptr(),
                locale.len() as i32,
                0,
            )
        };
        let locale = if length > 1 {
            String::from_utf16_lossy(&locale[..length as usize - 1])
        } else {
            "en".into()
        };
        let executable = std::env::current_exe().ok();
        let directory = executable
            .as_deref()
            .and_then(Path::parent)
            .map(|path| path.join("locales"));
        Self::load(&locale, directory.as_deref())
    }

    fn load(locale: &str, directory: Option<&Path>) -> Self {
        let mut messages: Messages =
            serde_json::from_str(ENGLISH).expect("valid built-in English catalog");
        let read = |language: &str| -> Option<Messages> {
            let data = std::fs::read(directory?.join(format!("{language}.json"))).ok()?;
            serde_json::from_slice(&data).ok()
        };
        if let Some(english) = read("en") {
            messages.extend(english);
        }
        let locale = locale.to_ascii_lowercase();
        if locale.is_empty() || !locale.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-') {
            return Self { messages };
        }
        let language = locale.split('-').next().unwrap_or("en");
        if language == "de" {
            let german: Messages =
                serde_json::from_str(GERMAN).expect("valid built-in German catalog");
            messages.extend(german);
        }
        if let Some(translated) = read(language) {
            messages.extend(translated);
        }
        if locale != language
            && let Some(regional) = read(&locale)
        {
            messages.extend(regional);
        }
        Self { messages }
    }

    pub fn text<'a>(&'a self, key: &'a str) -> &'a str {
        self.messages.get(key).map(String::as_str).unwrap_or(key)
    }

    pub fn format(&self, key: &str, values: &[(&str, String)]) -> String {
        let mut text = self.text(key).to_owned();
        for (name, value) in values {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }

    pub fn error(&self, error: &crate::game::GameError) -> String {
        match &error.detail {
            Some(detail) => format!("{}: {detail}", self.text(error.key)),
            None => self.text(error.key).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_language_selection_and_english_fallback() {
        assert_eq!(
            Catalog::load("de-DE", None).text("apply"),
            "Für diese Sitzung anwenden"
        );
        for locale in ["en-US", "es-ES", "../de", ""] {
            assert_eq!(
                Catalog::load(locale, None).text("apply"),
                "Apply for this session"
            );
        }
        let english: Messages = serde_json::from_str(ENGLISH).unwrap();
        let german: Messages = serde_json::from_str(GERMAN).unwrap();
        assert_eq!(english.len(), german.len());
        assert!(english.keys().all(|key| german.contains_key(key)));
    }

    #[test]
    fn new_catalogs_partial_translations_and_malformed_fallback() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("subspace-i18n-test-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let french = directory.join("fr.json");
        std::fs::write(&french, r#"{"apply":"Appliquer"}"#).unwrap();
        let catalog = Catalog::load("fr-FR", Some(&directory));
        assert_eq!(catalog.text("apply"), "Appliquer");
        assert_eq!(
            catalog.text("reset"),
            Catalog::load("en", None).text("reset")
        );
        assert_eq!(
            catalog.format(
                "readout",
                &[
                    ("current", "300".into()),
                    ("maximum", "1000".into()),
                    ("original", "300".into())
                ]
            ),
            "Distance: 300 / 1000 · Original: 300"
        );
        std::fs::write(&french, b"invalid json").unwrap();
        assert_eq!(
            Catalog::load("fr-FR", Some(&directory)).text("apply"),
            "Apply for this session"
        );
        let german = directory.join("de.json");
        std::fs::write(&german, b"invalid json").unwrap();
        assert_eq!(
            Catalog::load("de-DE", Some(&directory)).text("apply"),
            "Für diese Sitzung anwenden"
        );
        std::fs::remove_file(french).unwrap();
        std::fs::remove_file(german).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
