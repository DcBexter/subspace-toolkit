# Entwicklung

## Bauen und prüfen

Windows x64, rustup und Visual Studio Build Tools mit C++/Windows SDK erforderlich. Compiler steht in `rust-toolchain.toml`, Abhängigkeiten in `Cargo.lock`.

```powershell
cargo fmt --all -- --check
cargo clippy --locked --bin subspace-toolkit -- -D warnings
cargo test --locked --bin subspace-toolkit
cargo build --locked --release --bin subspace-toolkit
```

EXE: `target/release/subspace-toolkit.exe`. CI-Artefakte: [Actions](https://github.com/DcBexter/subspace-toolkit/actions/workflows/ci.yml).

Zwei zusätzliche Tests lesen eigene, unveränderte Spielinstallation und schreiben nur Testkopien:

```powershell
$env:OUTPOSTS_GAME_DIR = 'D:\Games\Star Trek Outposts Unknown'
cargo test --locked --bin subspace-toolkit -- --ignored
```

## Integration

Unterstützter Build: Windows **273416**, Unity **6000.3.19f1**, IL2CPP-Metadaten **39**. SHA-256-Prüfungen binden Änderungen an untersuchte Dateien; neue Spielversionen erfordern erneute Analyse. Die Steam-Zeile **272131** bestätigt keinen separat getesteten Steam-Build.

Trainer verändert `maxZoomDistance` der aktiven Kamera im Speicher. Permanent-Mod ändert dasselbe serialisierte Feld im Szenenpaket; Zoomgeschwindigkeit, Sichtweite und andere Spieldateien bleiben unverändert. Normales Beenden stellt Laufzeitlimit wieder her; erzwungenes Beenden benötigt gegebenenfalls Spielneustart.

Originalbackup liegt neben Szenenpaket als `.bundle.subspace-original`; etwa 450 MB freien Platz bereithalten. `%LOCALAPPDATA%\SubspaceToolkit` enthält Trainereinstellung (`settings.json`) und Wiederherstellungsjournal (`permanent.json`). Bei installiertem Mod Backup und Journal behalten. Bei fremden Dateiänderungen verweigert Reset Überschreiben. Während Installation/Reset weder Spiel starten noch updaten.

## Übersetzungen

Windows-Anzeigesprache entscheidet; fehlende Sprache oder Schlüssel fallen auf Englisch zurück. Neue Sprache: `locales/en.json` kopieren, beispielsweise als `fr.json` oder `pt-BR.json`, Werte übersetzen. Schlüssel und Platzhalter erhalten. Externe Kataloge liegen neben EXE im Ordner `locales`; EN/DE sind zusätzlich eingebaut.

## Automatisierung

Ein Workflow enthält Windows-Build, Tests, Clippy, CodeQL für Rust/Actions, RustSec-Audit und Dependency Review. Sicherheitsscans laufen zusätzlich täglich. Renovate aktualisiert Crates, Rust-Toolchain, Actions, Auditwerkzeug und Lockfile, einschließlich Major-Versionen. Automerge benötigt aktuelle Branches und grüne Pflichtprüfungen; Sicherheitsdetails siehe [SECURITY.md](../SECURITY.md).

Spielpakete, dekompilierte Quellen und Backups gehören nicht ins Repository. Spieltest nach permanenter Installation steht noch aus.
