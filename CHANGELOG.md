# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses CalVer `YYYY.M.D.N`, where N is the daily build counter
starting at 0.

## Unreleased










### Added
- Add a Windows tray client (2026.9.6.0-E932) (7d74382)
- Restore the saved status on shutdown (2026.9.6.0-E932) (b46ab2c)
- Check for and install new releases from the tray (2026.9.7.0-A528) (5b5280d)
- Add release selection and integrity parsing for the updater (2026.9.7.0-A528) (f62c1b9)
- React to session events so idle lands as the phone connects (2026.9.15.0-E86F) (eb82a14)
- Go offline when nothing else is connected (2026.9.17.0-EB82) (29f3233)

### Changed
- Share the Discord adapter between entry points (2026.9.6.0-E932) (c599161)
- **deps-dev:** Bump the minor-and-patch group with 2 updates (#2) (a396952)
- Update README.md (d67a983)

### Fixed
- Log what the update check found (2026.9.7.0-A528) (16b1f53)
- Keep the commit stamp out of the tracked version file (2026.9.8.0-4260) (97a1460)
- **tests:** Keep the suite runnable without the runtime dependency (2026.9.17.0-EB82) (9e15d96)

