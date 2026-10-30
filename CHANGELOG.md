# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

### Changed

### Deprecated

### Removed

### Fixed

### Security

## [0.2.1] - 2026-10-30

No user-visible changes.

## [0.2.0] - 2026-10-30

### Added

- The startup log lists every detected touchpad and which one was auto-selected.

### Changed

- The `Device disconnected` log message names the device.

### Fixed

- Autodetection no longer fails with `Multiple touchpads found` on machines whose touchscreens expose a ghost "Touchpad" device, such as the ASUS Zenbook Duo.
- Autodetection no longer counts another running unpalm's `Filtered Touchpad` as a physical touchpad.

## [0.1.2] - 2026-10-30

### Changed

- The binary is about half the size, since `env_logger` no longer pulls in regex support.

## [0.1.1] - 2026-10-30

### Fixed

- The `RUST_LOG` environment variable now takes effect.

## [0.1.0] - 2026-03-12

### Added

- Palm rejection at the evdev level: touches that start in an exclusion zone are blocked, while touches that start elsewhere keep working when they move into one.
- Exclusion zones set with `--margin-{left,right,top,bottom}` and `--polygon`, in percentages of the touchpad size.
- Built-in default zones: a top 20% rectangle plus left and right 30% triangles.
- Touchpad autodetection, or selection by name pattern with `-n` or by device path with `-f`.
- `--block-taps` also blocks taps and clicks in exclusion zones.
- `--debug` and `--version` flags.
- An ASCII map of the exclusion zones in the startup log.
- Automatic reconnection when the touchpad disconnects.
- Systemd user and system service files.
