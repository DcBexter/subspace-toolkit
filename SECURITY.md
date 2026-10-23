# Sicherheit

Sicherheitslücke gefunden? Bitte [privat melden](https://github.com/DcBexter/subspace-toolkit/security/advisories/new), statt vertrauliche Details in ein öffentliches Issue zu schreiben. Keine Spielstände, Zugangsdaten oder vollständigen Spieldateien mitsenden.

Das Toolkit ist experimentell. Sicherheitskorrekturen erfolgen auf `main`; ältere Builds werden nicht separat gepflegt. Unterstützte Spielversionen stehen in der [Entwicklerdokumentation](docs/DEVELOPMENT.md).

## Automatische Prüfungen

- Windows-CI: Formatierung, Clippy, Tests, Release-Build.
- CodeQL: Rust und GitHub-Actions-Workflows, einschließlich erweiterter Sicherheitsabfragen.
- Dependency Review: neue verwundbare Abhängigkeiten blockieren Pull Requests.
- GitHub: Dependency Graph, Dependabot-Warnungen und Sicherheitsupdates, Secret Scanning und Push Protection.
- Renovate: Updates einschließlich Major-Versionen und Lockfile-Wartung, Automerge erst nach erforderlichen grünen Prüfungen.

Grüne Prüfungen ersetzen keinen Spieltest. Insbesondere Major-Updates können Verhalten verändern, das automatisierte Tests nicht abdecken.
