# Subspace Toolkit

[![Windows CI](https://github.com/DcBexter/subspace-toolkit/actions/workflows/ci.yml/badge.svg)](https://github.com/DcBexter/subspace-toolkit/actions/workflows/ci.yml)

Native Rust-Anwendung für **Star Trek: Outposts Unknown** auf Windows x64. Erweitert maximalen Kameraabstand von **300 auf bis zu 1000**. Mausradsteuerung und Zoomgeschwindigkeit bleiben erhalten.

**Status: experimentell.** Automatisierte Tests prüfen Speicherzugriff, Grenzen, Übersetzungen und Wiederherstellung. Abschließender Spielneustart mit installiertem Permanent-Mod ist noch nicht geprüft. CI benötigt keine Spieldateien und bestätigt keinen vollständigen Spieltest.

## Download

Unter [Actions → Windows CI](https://github.com/DcBexter/subspace-toolkit/actions/workflows/ci.yml) einen erfolgreichen Lauf öffnen und **SubspaceToolkit-windows-x64** herunterladen. ZIP vollständig entpacken, dann `subspace-toolkit.exe` starten. GitHub-Anmeldung mit Repositoryzugriff ist für private Artefakte erforderlich. Es gibt noch keine veröffentlichte Release-Version.

## Zwei Betriebsarten

### Trainer

1. Spiel und Toolkit starten, Spielstand laden. Verbindung erfolgt automatisch.
2. Gewünschtes Zoomlimit einstellen; Loslassen des Reglers oder **Für diese Sitzung anwenden** übernimmt Wert.
3. Toolkit während des Spiels geöffnet lassen. Auswahl wird lokal gespeichert und bei nächster Verbindung erneut angewendet.

Normales Schließen des Toolkits stellt ursprüngliches Laufzeitlimit wieder her. Nach erzwungenem Beenden kann Änderung bis zum Spielneustart bestehen bleiben. Rücksetzen ändert das Limit; Mausrad bewegen, um eine bereits weiter entfernte Kamera wieder innerhalb der Grenze zu bringen.

### Permanent-Mod

1. Toolkit bei laufendem Spiel einmal Installation erkennen lassen.
2. Spielstand speichern und **Spiel schließen**, Toolkit geöffnet lassen.
3. Limit wählen und **Permanent speichern** drücken.
4. Nach bestätigter Installation Toolkit schließen und Spiel starten.

Das neue Limit wird aus geändertem Szenenpaket geladen. Toolkit muss anschließend nicht mitlaufen. Zum Ändern des installierten Limits Spiel schließen und erneut permanent speichern.

**Mod & Einstellungen zurücksetzen** stellt Originalpaket wieder her, entfernt eigene Originalsicherung, Installationsdaten und gespeicherte Trainerauswahl. Bei installiertem Mod muss Spiel geschlossen sein. Im reinen Trainermodus funktioniert Reset auch während des Spiels.

## Kompatibilität und Sicherung

- Unterstützt ausschließlich untersuchten Windows-Build: `version.txt` Windows **273416**, Unity **6000.3.19f1**, IL2CPP-Metadaten **39**. Steam-Zeile **272131** ist keine Prüfung eines separaten Steam-Builds.
- SHA-256-Prüfungen binden Änderungen an konkrete Binärdateien, Metadaten, Katalog und Szenenpaket. Unbekannte Builds werden abgelehnt.
- Permanent-Mod ändert ausschließlich serialisiertes `maxZoomDistance`; kein DLL-Loader erforderlich. Andere Programme, ReShade und DLSS-Dateien werden nicht verändert.
- Originalsicherung liegt neben betroffenem Szenenpaket mit Endung `.bundle.subspace-original`. Für Installation und Austausch ausreichend freien Platz vorhalten, ungefähr 450 MB.
- `%LOCALAPPDATA%\SubspaceToolkit\settings.json` enthält Trainerauswahl; `permanent.json` enthält Installationspfad und Wiederherstellungsjournal. Diese Dateien und Backup bei installiertem Mod behalten.
- Vor Spielupdates Mod zurücksetzen. Wurde Paket extern verändert, verweigert Reset Überschreiben; Originalbackup aufbewahren. Während Installation/Reset Spiel nicht starten und kein Update ausführen.
- Sichtbarkeit bei Abstand 1000 hängt von Szene und Spiel ab. Keine Änderungen an Sichtweite, LOD oder Zoomgeschwindigkeit.

## Übersetzungen

Windows-Anzeigesprache wird automatisch erkannt. Deutsch und Englisch sind enthalten; nicht unterstützte Sprache fällt auf Englisch zurück.

Neue Sprache: `locales/en.json` kopieren, beispielsweise als `locales/fr.json`, Werte übersetzen und neben EXE im Ordner `locales` ablegen. Schlüssel und Platzhalter wie `{limit}` unverändert lassen. Regionale Kataloge wie `pt-BR.json` werden unterstützt; fehlende Schlüssel verwenden englischen Text. Ungültiges JSON fällt auf eingebauten Katalog zurück.

## Entwicklung

Voraussetzungen: Windows x64, Rust über rustup, Visual Studio Build Tools mit C++-Toolchain und Windows SDK. `rust-toolchain.toml` fixiert Compiler und Komponenten; `Cargo.lock` fixiert Abhängigkeiten.

```powershell
cargo fmt --all -- --check
cargo clippy --locked --bin subspace-toolkit -- -D warnings
cargo test --locked --bin subspace-toolkit
cargo build --locked --release --bin subspace-toolkit
```

EXE: `target/release/subspace-toolkit.exe`. Eingebaute EN/DE-Kataloge funktionieren auch ohne externe Dateien.

Zwei zusätzliche Tests benötigen eigene, unveränderte Installation des unterstützten Spiels. Sie lesen Originalpaket und schreiben ausschließlich Testkopien in temporäre Ordner beziehungsweise `analysis/`; sie installieren keinen Mod:

```powershell
$env:OUTPOSTS_GAME_DIR = 'D:\Games\Star Trek Outposts Unknown'
cargo test --locked --bin subspace-toolkit -- --ignored
```

## CI

Push auf `main`, Pull Requests und manueller Start lösen Windows-Workflow aus. Er prüft Formatierung, Clippy mit Fehlern bei Warnungen und Tests ohne Spielinstallation, baut Release-EXE und lädt EXE, Kataloge, README und SHA-256-Prüfsumme als 14 Tage verfügbares Artefakt hoch. Workflow besitzt ausschließlich Lesezugriff auf Repositoryinhalt. Verwendete [Checkout](https://github.com/actions/checkout)- und [Upload-Artifact](https://github.com/actions/upload-artifact)-Actions sind auf Commit-SHAs fixiert.

Repository enthält eigenen Toolkit-Quellcode. Spielpakete, dekompilierte Quellen, Speicherabbilder und Originalbackups sind ausgeschlossen. Unabhängiges Fan-Werkzeug; keine offizielle Verbindung zu Spielentwicklern oder Star-Trek-Rechteinhabern.
