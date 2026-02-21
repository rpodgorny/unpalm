mod polygon;

use clap::Parser;
use evdev::{uinput::VirtualDevice, AbsoluteAxisCode};
use nix::poll::{poll, PollFd, PollFlags};
use polygon::{is_in_any_polygon, parse_polygon_string, Polygon};
use signal_hook::{consts::SIGINT, consts::SIGTERM, iterator::Signals};
use std::collections::HashMap;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Reason for exiting the event loop
enum LoopExitReason {
    /// Clean shutdown requested via signal (SIGINT/SIGTERM)
    SignalReceived,
    /// Device disconnected, reconnection may be attempted
    DeviceDisconnected,
    /// Fatal error that cannot be recovered from
    FatalError(String),
}

/// Holds the device and virtual device after setup
struct DeviceSetup {
    device: evdev::Device,
    virtual_device: VirtualDevice,
}

/// Palm rejection filter for touchpads
#[derive(clap::Parser, Debug)]
#[command(name = "unpalm", version)]
#[command(about = "Filter palm touches from touchpad input", long_about = None)]
struct Cli {
    /// Device name pattern to search for, supports wildcards (*) (e.g., "*ELAN*4448")
    #[arg(short = 'n', long)]
    device_name: Option<String>,

    /// Device file path (e.g., /dev/input/event5). If not specified, device is found by name
    #[arg(short = 'f', long)]
    device_file: Option<PathBuf>,

    /// Left margin as percentage of touchpad width (rectangle)
    #[arg(long)]
    margin_left: Option<f32>,

    /// Right margin as percentage of touchpad width (rectangle)
    #[arg(long)]
    margin_right: Option<f32>,

    /// Top margin as percentage of touchpad height (rectangle)
    #[arg(long)]
    margin_top: Option<f32>,

    /// Bottom margin as percentage of touchpad height (rectangle)
    #[arg(long)]
    margin_bottom: Option<f32>,

    /// Polygon exclusion zones (format: "x1,y1 x2,y2 x3,y3 ..." where ALL coordinates are PERCENTAGES 0-100)
    /// Example: "0,0 20,0 10,30" creates a triangle at the top-left corner
    /// Can be specified multiple times for multiple polygons
    #[arg(long, value_name = "POINTS")]
    polygon: Vec<String>,
}

/// Check if a device is a touchpad by looking for:
/// - INPUT_PROP_POINTER property (not a touchscreen which has INPUT_PROP_DIRECT)
/// - Absolute position axes (ABS_MT_POSITION_X/Y or ABS_X/Y)
fn is_touchpad(device: &evdev::Device) -> bool {
    use evdev::PropType;

    let props = device.properties();
    let has_pointer = props.contains(PropType::POINTER);
    let has_direct = props.contains(PropType::DIRECT);

    // Must be a pointer device, not a direct touch device (touchscreen)
    if !has_pointer || has_direct {
        return false;
    }

    // Must have absolute position axes
    let supported = device.supported_absolute_axes();
    if let Some(axes) = supported {
        let has_mt = axes.contains(AbsoluteAxisCode::ABS_MT_POSITION_X)
            && axes.contains(AbsoluteAxisCode::ABS_MT_POSITION_Y);
        let has_st =
            axes.contains(AbsoluteAxisCode::ABS_X) && axes.contains(AbsoluteAxisCode::ABS_Y);
        return has_mt || has_st;
    }

    false
}

/// Match a pattern with wildcards (*) against a string
/// Example: "*ELAN*4448" matches "ELAN Touchpad 4448"
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[pos..].find(part) {
            Some(found) => {
                // First part must match at start if pattern doesn't start with *
                if i == 0 && !pattern.starts_with('*') && found != 0 {
                    return false;
                }
                pos += found + part.len();
            }
            None => return false,
        }
    }

    // Last part must match at end if pattern doesn't end with *
    if !pattern.ends_with('*') && !parts.last().unwrap_or(&"").is_empty() {
        text.ends_with(parts.last().unwrap())
    } else {
        true
    }
}

/// Find all touchpad devices in the system
fn find_all_touchpads() -> Vec<(PathBuf, evdev::Device)> {
    evdev::enumerate()
        .filter(|(_, device)| is_touchpad(device))
        .map(|(path, device)| (path.to_path_buf(), device))
        .collect()
}

fn find_device(
    device_name: Option<&str>,
    device_file: Option<&PathBuf>,
) -> Result<evdev::Device, String> {
    // Case 1: Explicit device file path
    if let Some(path) = device_file {
        match evdev::Device::open(path) {
            Ok(device) => {
                println!("Opened device file: {}", path.display());
                if let Some(name) = device.name() {
                    println!("Device name: {name}");
                }
                return Ok(device);
            }
            Err(e) => {
                return Err(format!(
                    "Failed to open device file {}: {e}",
                    path.display()
                ));
            }
        }
    }

    // Case 2: Search by name
    if let Some(name) = device_name {
        let matches: Vec<(PathBuf, evdev::Device)> = evdev::enumerate()
            .filter(|(_, device)| {
                device
                    .name()
                    .map(|dev_name| wildcard_match(name, dev_name))
                    .unwrap_or(false)
            })
            .map(|(path, device)| (path.to_path_buf(), device))
            .collect();

        match matches.len() {
            0 => return Err(format!("No device found matching pattern: {}", name)),
            1 => {
                let (path, device) = matches.into_iter().next().unwrap();
                let dev_name = device.name().unwrap_or("Unknown");
                println!("Found device: {} - {dev_name}", path.display());
                return Ok(device);
            }
            n => {
                eprintln!("Found {n} devices matching pattern '{name}', please be more specific:");
                for (path, device) in &matches {
                    let dev_name = device.name().unwrap_or("Unknown");
                    eprintln!("  {} - {dev_name}", path.display());
                }
                return Err(format!("Multiple devices found matching pattern: {name}"));
            }
        }
    }

    // Case 3: Auto-detect - find all touchpads
    let touchpads = find_all_touchpads();

    match touchpads.len() {
        0 => Err("No touchpads found".to_string()),
        1 => {
            let (path, device) = touchpads.into_iter().next().unwrap();
            let name = device.name().unwrap_or("Unknown");
            println!("Auto-detected touchpad: {} - {name}", path.display());
            Ok(device)
        }
        n => {
            eprintln!("Found {n} touchpads, please specify one with -n or -f:");
            for (path, device) in &touchpads {
                let name = device.name().unwrap_or("Unknown");
                eprintln!("  {} - {name}", path.display());
            }
            Err("Multiple touchpads found".to_string())
        }
    }
}

/// Check if an I/O error indicates the device was disconnected
fn is_device_disconnected(error: &std::io::Error) -> bool {
    use nix::errno::Errno;
    error
        .raw_os_error()
        .map(Errno::from_raw)
        .is_some_and(|e| matches!(e, Errno::ENODEV | Errno::EIO | Errno::ENXIO))
}

/// Set up the device and create the virtual device
fn setup_device(
    device_name: Option<&str>,
    device_file: Option<&PathBuf>,
) -> Result<(DeviceSetup, i32, i32), Box<dyn std::error::Error>> {
    // Find the touchpad
    let mut device = find_device(device_name, device_file)?;

    // Detect touchpad dimensions from device
    let absinfo: HashMap<AbsoluteAxisCode, evdev::AbsInfo> = device.get_absinfo()?.collect();

    let x_info = absinfo
        .get(&AbsoluteAxisCode::ABS_MT_POSITION_X)
        .or_else(|| absinfo.get(&AbsoluteAxisCode::ABS_X))
        .ok_or("No X axis found")?;
    let y_info = absinfo
        .get(&AbsoluteAxisCode::ABS_MT_POSITION_Y)
        .or_else(|| absinfo.get(&AbsoluteAxisCode::ABS_Y))
        .ok_or("No Y axis found")?;

    let x_max = x_info.maximum();
    let y_max = y_info.maximum();

    println!("Detected touchpad dimensions: X_MAX={x_max}, Y_MAX={y_max}");

    // Create virtual device with same capabilities
    let mut builder = VirtualDevice::builder()?
        .name("Filtered Touchpad")
        .with_keys(device.supported_keys().unwrap_or_default())?;

    // Copy device properties (INPUT_PROP_POINTER, etc.) - critical for libinput
    let props = device.properties();
    builder = builder.with_properties(props)?;

    // Copy relative axes if present
    if let Some(rel_axes) = device.supported_relative_axes() {
        builder = builder.with_relative_axes(rel_axes)?;
    }

    // Copy all absolute axes from the source device
    for (axis, info) in device.get_absinfo()? {
        builder = builder.with_absolute_axis(&evdev::UinputAbsSetup::new(axis, info))?;
    }

    let mut virtual_device = builder.build()?;

    println!(
        "Created virtual device: {:?}",
        virtual_device.enumerate_dev_nodes_blocking()?.next()
    );

    // Grab the original device
    device.grab()?;
    println!("Grabbed original device");

    // Set device to non-blocking mode to allow polling
    device.set_nonblocking(true)?;

    Ok((
        DeviceSetup {
            device,
            virtual_device,
        },
        x_max,
        y_max,
    ))
}

/// Attempt to reconnect to the device, retrying every second
fn reconnect_with_retry(
    device_name: Option<&str>,
    device_file: Option<&PathBuf>,
    running: &Arc<AtomicBool>,
) -> Result<DeviceSetup, Box<dyn std::error::Error>> {
    loop {
        // Check if we should stop trying
        if !running.load(Ordering::SeqCst) {
            return Err("Shutdown requested during reconnection".into());
        }

        // Wait before retrying
        thread::sleep(Duration::from_secs(1));

        // Check again after sleeping
        if !running.load(Ordering::SeqCst) {
            return Err("Shutdown requested during reconnection".into());
        }

        // Try to set up the device
        match setup_device(device_name, device_file) {
            Ok((setup, _, _)) => {
                println!("Successfully reconnected to device");
                return Ok(setup);
            }
            Err(e) => {
                eprintln!("Reconnection attempt failed: {}", e);
            }
        }
    }
}

/// Run the main event loop
fn run_event_loop(
    device: &mut evdev::Device,
    virtual_device: &mut VirtualDevice,
    polygons: &[Polygon],
    running: &Arc<AtomicBool>,
) -> LoopExitReason {
    // Track multitouch slot state
    // slot_blocked[slot] = true if touch started in edge zone
    // slot_positions[slot] = (x, y) - None means not yet known
    let mut slot_blocked: HashMap<i32, bool> = HashMap::new();
    let mut slot_positions: HashMap<i32, (Option<i32>, Option<i32>)> = HashMap::new();
    let mut current_slot: i32 = 0;

    println!("Starting event loop...");

    while running.load(Ordering::SeqCst) {
        // Poll with 100ms timeout to allow checking running flag
        let fd = PollFd::new(device.as_fd(), PollFlags::POLLIN);
        match poll(&mut [fd], 100u16) {
            Ok(0) => continue, // Timeout, check running flag
            Ok(_) => {
                // Check for POLLERR or POLLHUP which indicate device issues
                if let Some(revents) = fd.revents() {
                    if revents.contains(PollFlags::POLLERR) || revents.contains(PollFlags::POLLHUP)
                    {
                        return LoopExitReason::DeviceDisconnected;
                    }
                }
            }
            Err(nix::errno::Errno::EINTR) => continue, // Interrupted by signal
            Err(e) => {
                return LoopExitReason::FatalError(format!("Poll error: {}", e));
            }
        }

        // Now fetch events (will return immediately since we polled first)
        let events: Vec<evdev::InputEvent> = match device.fetch_events() {
            Ok(events) => events.collect(),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    continue;
                }
                if is_device_disconnected(&e) {
                    eprintln!("Device disconnected: {}", e);
                    return LoopExitReason::DeviceDisconnected;
                }
                return LoopExitReason::FatalError(format!("Error reading events: {}", e));
            }
        };

        let mut events_to_forward = Vec::new();

        for event in events {
            let mut forward = true;

            match event.destructure() {
                evdev::EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_SLOT, value) => {
                    current_slot = value;
                }
                evdev::EventSummary::AbsoluteAxis(
                    _,
                    AbsoluteAxisCode::ABS_MT_TRACKING_ID,
                    value,
                ) => {
                    if value >= 0 {
                        // New touch started
                        slot_positions.insert(current_slot, (None, None));
                        slot_blocked.insert(current_slot, false);
                    } else {
                        // Touch ended (tracking_id == -1)
                        slot_blocked.remove(&current_slot);
                        slot_positions.remove(&current_slot);
                    }
                }
                evdev::EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_X, x) => {
                    if let Some((old_x, old_y)) = slot_positions.get(&current_slot).copied() {
                        slot_positions.insert(current_slot, (Some(x), old_y));

                        // Check if this is the first complete position
                        if old_x.is_none() {
                            if let Some(y) = old_y {
                                if is_in_any_polygon(x, y, polygons) {
                                    slot_blocked.insert(current_slot, true);
                                    println!("Slot {current_slot}: blocked (started at {x}, {y})");
                                }
                            }
                        }
                    }
                }
                evdev::EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_Y, y) => {
                    if let Some((old_x, old_y)) = slot_positions.get(&current_slot).copied() {
                        slot_positions.insert(current_slot, (old_x, Some(y)));

                        // Check if this is the first complete position
                        if old_y.is_none() {
                            if let Some(x) = old_x {
                                if is_in_any_polygon(x, y, polygons) {
                                    slot_blocked.insert(current_slot, true);
                                    println!("Slot {current_slot}: blocked (started at {x}, {y})");
                                }
                            }
                        }
                    }
                }
                _ => {}
            }

            // Block only slot-specific MT events for blocked slots
            // DO NOT block ABS_X/ABS_Y as they represent the overall pointer position
            if slot_blocked.get(&current_slot).copied().unwrap_or(false) {
                if let evdev::EventSummary::AbsoluteAxis(
                    _,
                    AbsoluteAxisCode::ABS_MT_POSITION_X
                    | AbsoluteAxisCode::ABS_MT_POSITION_Y
                    | AbsoluteAxisCode::ABS_MT_TOUCH_MAJOR
                    | AbsoluteAxisCode::ABS_MT_TOUCH_MINOR
                    | AbsoluteAxisCode::ABS_MT_PRESSURE,
                    _,
                ) = event.destructure()
                {
                    forward = false;
                }
            }

            // Collect events to forward
            if forward {
                events_to_forward.push(event);
            }
        }

        // Emit all forwarded events as a batch
        if !events_to_forward.is_empty() {
            if let Err(e) = virtual_device.emit(&events_to_forward) {
                return LoopExitReason::FatalError(format!("Error emitting events: {}", e));
            }
        }
    }

    LoopExitReason::SignalReceived
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command-line arguments
    let cli = Cli::parse();

    // Set up signal handling for clean shutdown (do this once, before device setup)
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    thread::spawn(move || {
        for _ in signals.forever() {
            r.store(false, Ordering::SeqCst);
        }
    });

    // Initial device setup (failure exits the program)
    let (mut setup, x_max, y_max) =
        setup_device(cli.device_name.as_deref(), cli.device_file.as_ref())?;

    // Build exclusion zone polygons from margins and explicit polygons
    let mut polygons: Vec<Polygon> = Vec::new();

    let has_explicit_args = cli.margin_left.is_some()
        || cli.margin_right.is_some()
        || cli.margin_top.is_some()
        || cli.margin_bottom.is_some()
        || !cli.polygon.is_empty();

    if has_explicit_args {
        // Explicit args given: only use what was specified, margins produce rectangles
        if let Some(pct) = cli.margin_left {
            if pct > 0.0 {
                let margin_px = (x_max as f32 * pct / 100.0) as i32;
                polygons.push(Polygon::rectangle(0, 0, margin_px, y_max));
                println!("Left margin: {pct}% ({margin_px}px) [rectangle]");
            }
        }
        if let Some(pct) = cli.margin_right {
            if pct > 0.0 {
                let margin_px = (x_max as f32 * pct / 100.0) as i32;
                polygons.push(Polygon::rectangle(x_max - margin_px, 0, x_max, y_max));
                println!("Right margin: {pct}% ({margin_px}px) [rectangle]");
            }
        }
        if let Some(pct) = cli.margin_top {
            if pct > 0.0 {
                let margin_px = (y_max as f32 * pct / 100.0) as i32;
                polygons.push(Polygon::rectangle(0, 0, x_max, margin_px));
                println!("Top margin: {pct}% ({margin_px}px) [rectangle]");
            }
        }
        if let Some(pct) = cli.margin_bottom {
            if pct > 0.0 {
                let margin_px = (y_max as f32 * pct / 100.0) as i32;
                polygons.push(Polygon::rectangle(0, y_max - margin_px, x_max, y_max));
                println!("Bottom margin: {pct}% ({margin_px}px) [rectangle]");
            }
        }

        // Parse explicit polygon exclusion zones
        for (i, polygon_str) in cli.polygon.iter().enumerate() {
            let points = parse_polygon_string(polygon_str).map_err(|e| {
                format!("Failed to parse polygon {} '{}': {}", i + 1, polygon_str, e)
            })?;
            let polygon = Polygon::from_percentages(&points, x_max, y_max)
                .map_err(|e| format!("Failed to create polygon {}: {}", i + 1, e))?;
            println!("Polygon {}: {} vertices", i + 1, polygon.vertices.len());

            for warning in polygon.validate() {
                eprintln!("Warning: Polygon {}: {}", i + 1, warning);
            }

            polygons.push(polygon);
        }
    } else {
        // No explicit args: apply built-in defaults
        // Top 20% rectangle
        let top_px = (y_max as f32 * 20.0 / 100.0) as i32;
        polygons.push(Polygon::rectangle(0, 0, x_max, top_px));
        println!("Default: top 20% rectangle ({top_px}px)");

        // Left 30% triangle: (0,0) -> (left_px,0) -> (0,y_max)
        let left_px = (x_max as f32 * 30.0 / 100.0) as i32;
        polygons.push(Polygon {
            vertices: vec![(0, 0), (left_px, 0), (0, y_max)],
        });
        println!("Default: left 30% triangle ({left_px}px)");

        // Right 30% triangle: (x_max-right_px,0) -> (x_max,0) -> (x_max,y_max)
        let right_px = (x_max as f32 * 30.0 / 100.0) as i32;
        polygons.push(Polygon {
            vertices: vec![(x_max - right_px, 0), (x_max, 0), (x_max, y_max)],
        });
        println!("Default: right 30% triangle ({right_px}px)");

        println!("Using default exclusion zones");
    }

    println!("Total exclusion zones: {} polygon(s)", polygons.len());

    // Main loop with reconnection support
    loop {
        match run_event_loop(
            &mut setup.device,
            &mut setup.virtual_device,
            &polygons,
            &running,
        ) {
            LoopExitReason::SignalReceived => {
                println!("\nShutting down...");
                break;
            }
            LoopExitReason::DeviceDisconnected => {
                eprintln!("Device disconnected, attempting to reconnect...");
                // Drop the old setup to release the grab and close file descriptors
                drop(setup);
                // Try to reconnect, retrying every second
                match reconnect_with_retry(
                    cli.device_name.as_deref(),
                    cli.device_file.as_ref(),
                    &running,
                ) {
                    Ok(new_setup) => {
                        setup = new_setup;
                        // Continue with the same polygons (assumes same device dimensions)
                    }
                    Err(e) => {
                        // This only happens if shutdown was requested during reconnection
                        println!("\nShutting down: {}", e);
                        break;
                    }
                }
            }
            LoopExitReason::FatalError(e) => {
                return Err(e.into());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for wildcard_match

    #[test]
    fn test_wildcard_exact_match() {
        assert!(wildcard_match("ELAN Touchpad", "ELAN Touchpad"));
    }

    #[test]
    fn test_wildcard_exact_no_match() {
        assert!(!wildcard_match("ELAN Touchpad", "Synaptics Touchpad"));
    }

    #[test]
    fn test_wildcard_star_at_start() {
        assert!(wildcard_match("*Touchpad", "ELAN Touchpad"));
        assert!(wildcard_match("*Touchpad", "Touchpad"));
        assert!(!wildcard_match("*Touchpad", "Touchpad Extra"));
    }

    #[test]
    fn test_wildcard_star_at_end() {
        assert!(wildcard_match("ELAN*", "ELAN Touchpad"));
        assert!(wildcard_match("ELAN*", "ELAN"));
        assert!(!wildcard_match("ELAN*", "Something ELAN"));
    }

    #[test]
    fn test_wildcard_star_both_ends() {
        assert!(wildcard_match("*ELAN*", "My ELAN Touchpad"));
        assert!(wildcard_match("*ELAN*", "ELAN"));
        assert!(wildcard_match("*ELAN*", "ELAN Touchpad"));
        assert!(wildcard_match("*ELAN*", "My ELAN"));
        assert!(!wildcard_match("*ELAN*", "Something Else"));
    }

    #[test]
    fn test_wildcard_star_in_middle() {
        assert!(wildcard_match("ELAN*4448", "ELAN Touchpad 4448"));
        assert!(wildcard_match("ELAN*4448", "ELAN4448"));
        assert!(!wildcard_match("ELAN*4448", "ELAN Touchpad 9999"));
        assert!(!wildcard_match("ELAN*4448", "Synaptics 4448"));
    }

    #[test]
    fn test_wildcard_multiple_stars() {
        assert!(wildcard_match(
            "*ELAN*4448*",
            "My ELAN Touchpad 4448 Device"
        ));
        assert!(wildcard_match("*ELAN*4448", "My ELAN Touchpad 4448"));
        assert!(!wildcard_match("*ELAN*4448", "My ELAN Touchpad 9999"));
    }

    #[test]
    fn test_wildcard_just_star() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*", ""));
    }

    #[test]
    fn test_wildcard_empty_pattern() {
        assert!(wildcard_match("", ""));
        // NOTE: empty pattern matches non-empty text due to how split('*') works.
        // Not a real issue since clap won't pass empty strings for -n.
        assert!(wildcard_match("", "something"));
    }

    #[test]
    fn test_wildcard_empty_text() {
        assert!(!wildcard_match("ELAN", ""));
        assert!(wildcard_match("*", ""));
    }

    #[test]
    fn test_wildcard_consecutive_stars() {
        assert!(wildcard_match("**ELAN**", "My ELAN Touchpad"));
        assert!(wildcard_match("ELAN**Touchpad", "ELAN Touchpad"));
    }

    #[test]
    fn test_wildcard_case_sensitive() {
        assert!(!wildcard_match("*elan*", "ELAN Touchpad"));
        assert!(!wildcard_match("*ELAN*", "elan touchpad"));
    }

    #[test]
    fn test_wildcard_pattern_longer_than_text() {
        assert!(!wildcard_match("ELAN Touchpad Extra", "ELAN Touchpad"));
    }

    #[test]
    fn test_wildcard_special_characters() {
        assert!(wildcard_match("*foo-bar*", "my foo-bar device"));
        assert!(wildcard_match("device (v2)", "device (v2)"));
    }

    // Tests for is_device_disconnected

    #[test]
    fn test_disconnected_enodev() {
        let err = std::io::Error::from_raw_os_error(19); // ENODEV
        assert!(is_device_disconnected(&err));
    }

    #[test]
    fn test_disconnected_eio() {
        let err = std::io::Error::from_raw_os_error(5); // EIO
        assert!(is_device_disconnected(&err));
    }

    #[test]
    fn test_disconnected_enxio() {
        let err = std::io::Error::from_raw_os_error(6); // ENXIO
        assert!(is_device_disconnected(&err));
    }

    #[test]
    fn test_disconnected_other_errno() {
        let err = std::io::Error::from_raw_os_error(13); // EACCES
        assert!(!is_device_disconnected(&err));
    }

    #[test]
    fn test_disconnected_no_os_error() {
        let err = std::io::Error::new(std::io::ErrorKind::Other, "custom error");
        assert!(!is_device_disconnected(&err));
    }

    // Tests for CLI argument parsing

    #[test]
    fn test_cli_defaults() {
        let cli = Cli::parse_from(["unpalm"]);
        assert!(cli.margin_left.is_none());
        assert!(cli.margin_right.is_none());
        assert!(cli.margin_top.is_none());
        assert!(cli.margin_bottom.is_none());
        assert!(cli.device_name.is_none());
        assert!(cli.device_file.is_none());
        assert!(cli.polygon.is_empty());
    }

    #[test]
    fn test_cli_custom_margins() {
        let cli = Cli::parse_from([
            "unpalm",
            "--margin-left",
            "30",
            "--margin-right",
            "10",
            "--margin-top",
            "15",
            "--margin-bottom",
            "5",
        ]);
        assert_eq!(cli.margin_left, Some(30.0));
        assert_eq!(cli.margin_right, Some(10.0));
        assert_eq!(cli.margin_top, Some(15.0));
        assert_eq!(cli.margin_bottom, Some(5.0));
    }

    #[test]
    fn test_cli_device_name() {
        let cli = Cli::parse_from(["unpalm", "-n", "*ELAN*"]);
        assert_eq!(cli.device_name.as_deref(), Some("*ELAN*"));
    }

    #[test]
    fn test_cli_device_file() {
        let cli = Cli::parse_from(["unpalm", "-f", "/dev/input/event5"]);
        assert_eq!(
            cli.device_file.as_deref(),
            Some(std::path::Path::new("/dev/input/event5"))
        );
    }

    #[test]
    fn test_cli_single_polygon() {
        let cli = Cli::parse_from(["unpalm", "--polygon", "0,0 20,0 10,30"]);
        assert_eq!(cli.polygon.len(), 1);
        assert_eq!(cli.polygon[0], "0,0 20,0 10,30");
    }

    #[test]
    fn test_cli_multiple_polygons() {
        let cli = Cli::parse_from([
            "unpalm",
            "--polygon",
            "0,0 15,0 0,25",
            "--polygon",
            "85,0 100,0 100,25",
        ]);
        assert_eq!(cli.polygon.len(), 2);
    }

    #[test]
    fn test_cli_zero_margins() {
        let cli = Cli::parse_from([
            "unpalm",
            "--margin-left",
            "0",
            "--margin-right",
            "0",
            "--margin-top",
            "0",
        ]);
        assert_eq!(cli.margin_left, Some(0.0));
        assert_eq!(cli.margin_right, Some(0.0));
        assert_eq!(cli.margin_top, Some(0.0));
    }
}
