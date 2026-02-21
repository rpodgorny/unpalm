# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

unpalm is a Linux palm rejection filter for touchpads written in Rust. It works at the evdev level, intercepting touchpad input and filtering out touches that begin in configurable exclusion zones (margins or custom polygons). It's compatible with any display server (X11, Wayland), any compositor, and any touchpad hardware.

**Core approach:**
1. Grabs the physical touchpad device (making it unavailable)
2. Creates a virtual touchpad device with identical capabilities
3. Tracks each touch slot individually - blocks touches that START in exclusion zones
4. Forwards all other events to the virtual device
5. Applications interact with the filtered virtual device

## Build, Test, and Run Commands

**Build:**
```bash
cargo build --release
```

**Run (requires root for device access):**
```bash
sudo ./target/release/unpalm
```

**Run tests:**
```bash
cargo test
```

**Run a single test:**
```bash
cargo test test_name
```

**Check code:**
```bash
cargo check
cargo clippy
```

**Install as systemd service:**
```bash
sudo cp target/release/unpalm /usr/local/bin/
sudo cp unpalm.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable unpalm.service
sudo systemctl start unpalm.service
```

## Architecture

### Two-File Design
- `src/main.rs` (~820 lines): Device detection, CLI, event loop, virtual device creation, reconnection logic
- `src/polygon.rs` (~580 lines): Polygon struct, ray casting, parsing, validation

### Key Components

**Device Detection (`main.rs`):**
- `is_touchpad()`: Identifies touchpads via evdev properties (INPUT_PROP_POINTER without INPUT_PROP_DIRECT)
- `find_device()`: Three-mode device discovery: explicit path, name pattern with wildcard matching, or auto-detection
- `find_all_touchpads()`: Helper that enumerates all touchpad devices
- `wildcard_match()`: Simple wildcard pattern matching for device names

**Exclusion Zones (`polygon.rs`):**
- `Polygon` struct: Represents exclusion zones as arbitrary polygons in absolute coordinates
- `Polygon::contains()`: Ray casting algorithm for point-in-polygon testing
- `Polygon::from_percentages()`: Converts percentage coordinates to absolute touchpad coordinates
- `Polygon::rectangle()`: Helper for creating rectangular exclusion zones from margins
- `Polygon::validate()`: Checks for degenerate polygons (duplicate vertices, zero area, self-intersection)
- `parse_polygon_string()`: Parses CLI polygon strings into percentage coordinates
- `is_in_any_polygon()`: Tests a point against all exclusion polygons
- `--margin-*` flags always produce rectangles
- Default behavior (no args): 30% side triangles + 20% top rectangle
- Any explicit `--margin-*` or `--polygon` arg replaces all defaults

**Device Setup & Reconnection (`main.rs`):**
- `setup_device()`: Finds touchpad, reads dimensions, creates virtual device, grabs physical device
- `reconnect_with_retry()`: Retries device setup every second after disconnection
- `is_device_disconnected()`: Detects ENODEV/EIO/ENXIO errors indicating device loss
- `LoopExitReason` enum: Distinguishes signal, disconnect, and fatal error exits
- `DeviceSetup` struct: Bundles physical and virtual device together

**Event Loop (`main.rs`, `run_event_loop()`):**
- Uses `nix::poll()` for non-blocking event reading with signal handling
- Maintains per-slot state:
  - `slot_blocked`: HashMap tracking which slots are blocked (started in exclusion zone)
  - `slot_positions`: HashMap tracking current X/Y position per slot
  - `current_slot`: Active multitouch slot
- Blocks only MT-specific events (ABS_MT_POSITION_X/Y, ABS_MT_TOUCH_MAJOR/MINOR, ABS_MT_PRESSURE)
- **Critical:** Never blocks ABS_X/ABS_Y (overall pointer position) or slot/tracking_id events
- Touch blocking logic: When first complete position (X,Y) arrives for a slot, checks if it's in any polygon. If yes, marks slot as blocked permanently until touch ends.
- Returns `LoopExitReason` so `main()` can decide whether to reconnect or shut down

**Virtual Device Creation (inside `setup_device()`):**
- Clones all capabilities from physical device (keys, axes, properties)
- Properties are critical - libinput needs INPUT_PROP_POINTER to recognize it as a touchpad
- Uses evdev's VirtualDevice/uinput API

### State Machine

Touch lifecycle per slot:
1. `ABS_MT_TRACKING_ID >= 0`: New touch → Initialize position (None, None), not blocked
2. `ABS_MT_POSITION_X/Y`: Accumulate position → Once both X and Y are known for the first time, check polygons
3. If first position is in any polygon → Mark slot blocked
4. `ABS_MT_TRACKING_ID == -1`: Touch ended → Clear slot state

### Signal Handling
- Uses `signal-hook` for clean SIGINT/SIGTERM handling
- Spawns thread to set atomic bool on signal
- Main loop checks `running` flag every 100ms poll timeout
- Device grab is released on drop

## Dependencies

- `evdev`: Linux evdev device access and uinput virtual device creation
- `signal-hook`: Async Unix signal handling
- `nix`: poll() system call for non-blocking I/O
- `clap`: CLI argument parsing with derive macros

## Important Implementation Details

**Per-touch tracking is critical:** The tool only blocks touches that START in exclusion zones. If you start a touch in an allowed area and move into an exclusion zone, it continues working. This prevents palms from being detected when you're actually using the touchpad near the edges.

**Multitouch slot protocol:** Linux multitouch uses slots (ABS_MT_SLOT) to track multiple simultaneous touches. The current slot is set via ABS_MT_SLOT events, then subsequent ABS_MT_* events apply to that slot. Tracking IDs are assigned when touches start and set to -1 when they end.

**Virtual device must clone properties:** The virtual device needs INPUT_PROP_POINTER property copied from the source device, otherwise libinput won't recognize it as a touchpad (it would be treated as a generic absolute device).

**Non-blocking I/O with polling:** Device is set to non-blocking mode and poll() is used to wait for events with a timeout. This allows checking the `running` atomic bool periodically for graceful shutdown.

**Device reconnection:** If the device disconnects (ENODEV/EIO/ENXIO), the main loop retries `setup_device()` every second until the device reappears or a shutdown signal is received. Polygons are reused across reconnections (assumes same device dimensions).

**Release profile optimization:** Cargo.toml uses `opt-level = "z"`, LTO, and stripping to produce a small (~300KB) binary.

## Configuration

All configuration is via CLI arguments (no config file):
- **No arguments:** Built-in defaults apply (top 20% rectangle + left/right 30% triangles)
- **Any explicit arg replaces all defaults** - only the specified zones apply
- Margins: `--margin-{left,right,top,bottom}` as percentages (0-100), always produce **rectangles**
- Custom polygons: `--polygon "x1,y1 x2,y2 x3,y3 ..."` where **ALL coordinates are percentages 0-100** (repeatable)
  - Example: `--polygon "0,0 20,0 10,30"` creates a triangle at top-left corner
  - Percentages make configurations portable across different touchpad sizes
- Device selection: `-n` for name pattern with wildcards, `-f` for explicit path

For systemd service, edit `/etc/systemd/system/unpalm.service` and modify the `ExecStart` line.

## Code Style Notes

This is pragmatic, "vibe-coded" Rust:
- Two files: main logic + polygon module
- No fancy abstractions or trait hierarchies
- Straightforward imperative code
- Comments explain "why" not "what"
- Tests focus on core algorithms (polygon math, parsing, wildcard matching)
- Error handling via Result types, but not exhaustive edge case coverage

The author explicitly states this was built fast to solve an immediate problem (ASUS Zenbook Duo keyboard palm rejection in detached mode) and will be cleaned up later.
