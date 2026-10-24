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

Vollständiges Paket inklusive Loader, privater .NET-Laufzeit und eigenem Mod:

```powershell
./scripts/prepare-loader.ps1
dotnet run --project scripts/check-mod -c Release
$env:SUBSPACE_TEST_PAYLOAD = (Resolve-Path dist/SubspaceToolkit/payload).Path
cargo test --locked --bin subspace-toolkit release_payload_installs -- --ignored
```

Letzter Test installiert nur in temporären Ordner. Keine Spielinstallation erforderlich. Payload, Versionsbindung und Quellen/Lizenzen: [Loader-Build](../scripts/LOADER.md). Erste Installation kopiert Loader-Dateien mit Fortschrittsanzeige; spätere Speichervorgänge aktualisieren kleine JSON-Konfiguration. Abweichende vorhandene Dateien werden nicht überschrieben. Beim ersten Start erzeugt MelonLoader zusätzlich spielabhängige Wrapper; das kann dauern.

## Integration

Untersuchter Build: Windows **273416**, Unity **6000.3.19f1**, IL2CPP-Metadaten **39**. Künftige Spielversionen sind nicht automatisch kompatibel.

Der Trainer lädt seinen Live-Bootstrap in den laufenden Spielprozess, auch ohne permanent installierten Loader. Mit vorhandenem MelonLoader nutzt er dessen Update-Ereignis, sonst einen Kamera-Update-Hook. Beide Wege verwenden `mod/Engine.cs` und kurzlebige JSON-Befehle. Nur „Permanent speichern“ schreibt `UserData/SubspaceToolkit.json` im Spielordner. Regler ändern Trainer-Werte und lokale Voreinstellungen. Nach Toolkit-Ende oder spätestens nach Ablauf der Drei-Sekunden-Frist gelten wieder permanente Werte, sonst Originalwerte. Kameraeigenschaften werden über benannte IL2CPP-Mitglieder angesprochen.

Kamera: Zoomlimit 300–1000, gemeinsame Geschwindigkeit 25–300 % für WASD/Randscroll und Mauspan. Mausrad-, Rotations- und Neigungsfelder bleiben unverändert. Reset entfernt eigene dauerhafte Konfiguration und Trainer-Voreinstellungen. Alte Datei-Patches lassen sich nur noch über den geprüften Wiederherstellungspfad zurücksetzen; neue Asset-Patches sind im regulären Build ausgeschlossen.

`mod/ResourceHighlight.cs` öffnet Ressourcenübersicht bei Mouseover in Anfragen/Nachschub und markiert passenden Kachelrahmen; Icon und Anzahl bleiben unverändert. Bereits sichtbare Übersicht bleibt nach Hover offen. Nur vom Mod geöffnete Übersicht wird wieder geschlossen. Der Spielbuild kann eine sichtbare Übersicht mit internem Zustand `Hidden` melden; deshalb wird zusätzlich tatsächliche Canvas-Sichtbarkeit geprüft. Im Anfragen-Bildschirm live bestätigt: korrekte Kacheln einschließlich Bestand 0, Entfernen des Rahmens und beide Menü-Ausgangszustände. Nachschub-Test steht aus.

`mod/RepeatBuild.cs` hält Platzierungsmodus bei Shift gedrückt; Spiel prüft weiterhin Kosten und Bauplatz. Beide Funktionen sind standardmäßig aus, über Reiter Mods wählbar und haben unabhängige Fehlermeldungen. Flags folgen derselben Trainer-/Permanent-Trennung wie Kamera. Hook- und Metadatenprüfungen bestanden; echter Shift-Platzierungstest steht aus.

### Architektur

- `Controller` in `main.rs` steuert Verbindung, Wiederverbindung, Einstellungen und Installationsaufträge. `Toolkit` rendert Oberfläche; Reiterzustand bleibt dort. Keine zusätzliche Datei oder Weiterleitungsschicht.
- `GameSession::apply` übergibt Kamera und Mod-Schalter gemeinsam. Status zeigt tatsächlich gemeldete Werte; Befehlsdatei allein bestätigt keine Anwendung.
- `permanent::reset` führt alte Wiederherstellung, Entfernen dauerhafter Konfiguration und Löschen lokaler Voreinstellungen zusammen. Dateisperre nach Teilabschluss bleibt sichtbar und ist wiederholbar.
- Versionsgebundene Asset-Prüfung existiert nur im Wiederherstellungspfad `legacy.rs`. Regulärer Mod bindet benannte Eigenschaften; alter Asset-Patcher und LZ4-Abhängigkeit wurden entfernt.
- EN/DE-Oberfläche trennt Kamera und Zusatzmods in zwei Reiter. Kamera-Ablesewerte stehen im Status-Tooltip; Fortschritt erscheint nur beim Speichern. Beide Reiter, Sprachwechsel, manueller Ordnerdialog und anschließende automatische Wiederverbindung wurden mit gebauter EXE visuell geprüft.

### Loader-Kompatibilität (24.10.2026)

Unveränderte MelonLoader-Versionen 0.7.3 und 0.7.4-ci.2630 stürzten ohne Mods ab. Der IL2CPP-Fallback setzte einen Hook mitten in eine Funktion. Neun Zeilen aus [Il2CppInterop PR #291](https://github.com/BepInEx/Il2CppInterop/pull/291) wurden auf die exakt verwendete Version zurückportiert. Mit korrigierter Loader-Bibliothek erreicht das Spiel Hauptmenü und Spielszene. Der Fix betrifft keine Spiel-Assets, ist aber noch nicht upstream freigegeben. Paket enthält vollständige geänderte Quellen, Patch und Lizenzen; reproduzierbarer Build steht in `scripts/prepare-loader.ps1`. Diagnose-Nachweise liegen lokal unter `analysis/loader-upstream/`.

Bisherige Datei-Patches sind lokal vollständig zurückgesetzt. Live bestätigt: Originalwerte ohne Toolkit, automatische Verbindung mit 1000/300 %, explizites permanentes Speichern, temporäres Überschreiben mit 600/50 % ohne Änderung der permanenten Datei, Rückkehr zu 1000/300 % nach Toolkit-Ende. Zusätzlich bestanden: Neustart ohne Toolkit mit gespeicherten 1000/300 %, Reset auf 300/100 % einschließlich Entfernen der Konfiguration sowie automatische Wiederverbindung. Mod-Status und unabhängiges Lesen der tatsächlichen Kamera-Felder stimmen überein.

Ein vorübergehend verweigerter Dateizugriff darf die Kamera nicht dauerhaft sperren. Nach Korrektur mit absichtlicher Statusdatei-Sperre geprüft: 1,2 Sekunden Konflikt sowie 12 Sekunden mit Trainer-Timeout. Nach Freigabe verschwanden Fehler ohne Neustart, Trainer verband sich selbst und setzte seine temporären 50 % wieder durch. Permanente Datei blieb unverändert. Testskript und Nachweise: `analysis/melon-migration/test-status-recovery.ps1`, `recovered-status-timeout-camera.json`.

Der bekannte Beenden-Absturz `UnityPlayer.dll+0xb15f91` trat auch mit Original-Assets, geschlossenem Toolkit und deaktiviertem Loader auf. Der Alpha-Loader zeigte außerdem einen separaten NativeAOT-Fehler in `VERSION.dll`, mit und ohne Kamera-Mod. Aktuell getestet: stabiler Loader 0.7.3 mit korrigierter Interop-Bibliothek und ausgeblendeter Konsole. Dort trat der Alpha-Fehler nicht auf; Beenden scheiterte weiterhin in UnityPlayer, einmal bei `+0x7fc968` (Ursache offen), einmal bei der bekannten Stelle `+0xb15f91`. Kontrolllauf ohne Loader reproduzierte am 24.10. um 06:14 erneut `+0xb15f91`, aber nicht die zusätzliche Variante `+0x7fc968`. Kein erzwungenes Beenden als Workaround aktiviert. Sichtbarkeit von Personen-Karten bei erweitertem Zoom bleibt ebenfalls offen.

Diagnosen liegen lokal unter `analysis/melon-migration/` und `analysis/exit-crash-20261024/`. Keine Spielassemblies, Dumps oder Assets veröffentlichen. Release 0.1.0 bleibt offen.

Reproduzierbar gebautes Paket wurde nach gesicherter Migration eigener manueller DLLs installiert. Neustart ohne Toolkit bestätigt gespeicherte Kamera 1000/300 % und fehlerfreie Bindung beider Zusatzmods. Vollständiger visueller Paket-Spieltest bleibt offen. Installationsanzeige prüft neben Konfiguration auch erforderliche Loader-/Mod-Dateien; fehlende Dateien lassen sich erneut installieren, ohne gespeicherte Werte vorab zu löschen.
## Übersetzungen

Windows-Anzeigesprache entscheidet; fehlende Sprache oder Schlüssel fallen auf Englisch zurück. Neue Sprache: `locales/en.json` kopieren, beispielsweise als `fr.json` oder `pt-BR.json`, Werte übersetzen. Schlüssel und Platzhalter erhalten. Externe Kataloge liegen neben EXE im Ordner `locales`; EN/DE sind zusätzlich eingebaut.

## Automatisierung

Ein Workflow enthält Windows-Build, Tests, Clippy, CodeQL für Rust/Actions/C#, RustSec-Audit und Dependency Review. Sicherheitsscans laufen zusätzlich täglich, Windows-Prüfungen zusätzlich montags. Renovate aktualisiert Crates, Rust-Toolchain, Actions, Auditwerkzeug und Lockfile. Alle Updates einschließlich Major-Versionen, Sicherheitskorrekturen und vorhandener Submodule dürfen nach aktuellen Branches und grünen Pflichtprüfungen automatisch mergen. Dependabot-PRs erhalten ebenfalls natives Auto-Merge, ohne ihren Code im privilegierten Workflow auszuführen. Produkt-PRs bleiben ausgeschlossen. Sicherheitsdetails siehe [SECURITY.md](../SECURITY.md).

Spielpakete, dekompilierte Quellen und Backups gehören nicht ins Repository. CI-Paket enthält Rust-Anwendung, Übersetzungen, Loader/Runtime/Mod-Payload, Prüfsummen und zugehörige Interop-Quellen. Lokale Builds und Tests ersetzen keinen erfolgreichen Remote-CI-Lauf und keinen Spieltest.
