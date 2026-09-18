# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses CalVer `YYYY.M.D.N`, where N is the daily build counter
starting at 0.

## Unreleased

### Changed
- **release:** Hash the zip with the shared checksums action (2026.9.18.0-2D46) (38ee728)

---

## [v2026.9.18.0](https://github.com/RealWhyKnot/discord-mobile-idle/releases/tag/v2026.9.18.0) - 2026-09-18

### Added
- React to session events so idle lands as the phone connects (2026.9.15.0-E86F) (eb82a14)
- Go offline when nothing else is connected (2026.9.17.0-EB82) (29f3233)
- Restore the saved status when the container is stopped (2026.9.17.0-EB82) (9ba9f9b)

### Changed
- **deps-dev:** Bump the minor-and-patch group with 2 updates (#2) (a396952)
- Update README.md (d67a983)

### Fixed
- Keep the commit stamp out of the tracked version file (2026.9.8.0-4260) (97a1460)
- **tests:** Keep the suite runnable without the runtime dependency (2026.9.17.0-EB82) (9e15d96)
- Take the phone over again after coming back from offline (2026.9.17.0-EB82) (bf993e1)
- Restore on shutdown even before the write echoes back (2026.9.17.0-EB82) (e81e6d4)

---

## [v2026.9.7.2](https://github.com/RealWhyKnot/discord-mobile-idle/releases/tag/v2026.9.7.2) - 2026-09-07

_No user-visible changes in this release._

---

## [v2026.9.7.1](https://github.com/RealWhyKnot/discord-mobile-idle/releases/tag/v2026.9.7.1) - 2026-09-07

### Fixed
- Log what the update check found (2026.9.7.0-A528) (16b1f53)

---

## [v2026.9.7.0](https://github.com/RealWhyKnot/discord-mobile-idle/releases/tag/v2026.9.7.0) - 2026-09-07

### Added
- Set Discord to idle while a phone session is active (5c358c4)
- Add a Windows tray client (2026.9.6.0-E932) (7d74382)
- Restore the saved status on shutdown (2026.9.6.0-E932) (b46ab2c)
- Check for and install new releases from the tray (2026.9.7.0-A528) (5b5280d)
- Add release selection and integrity parsing for the updater (2026.9.7.0-A528) (f62c1b9)

### Changed
- Share the Discord adapter between entry points (2026.9.6.0-E932) (c599161)
