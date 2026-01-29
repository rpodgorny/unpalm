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

/// Palm rejection filter for touchpads
#[derive(Parser, Debug)]
#[command(name = "unpalm")]
#[command(about = "Filter palm touches from touchpad input", long_about = None)]
struct Cli {
    /// Device name pattern to search for, supports wildcards (*) (e.g., "*ELAN*4448")
    #[arg(short = 'n', long)]
    device_name: Option<String>,

    /// Device file path (e.g., /dev/input/event5). If not specified, device is found by name
    #[arg(short = 'f', long)]
    device_file: Option<PathBuf>,

    /// Left margin as percentage of touchpad width
    #[arg(long, default_value_t = 20)]
    margin_left: i32,

    /// Right margin as percentage of touchpad width
    #[arg(long, default_value_t = 20)]
    margin_right: i32,

    /// Top margin as percentage of touchpad height
    #[arg(long, default_value_t = 20)]
    margin_top: i32,

    /// Bottom margin as percentage of touchpad height
    #[arg(long, default_value_t = 0)]
    margin_bottom: i32,

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command-line arguments
    let cli = Cli::parse();

    // Find the touchpad
    let mut device = find_device(cli.device_name.as_deref(), cli.device_file.as_ref())?;

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

    // Build exclusion zone polygons from margins and explicit polygons
    let mut polygons: Vec<Polygon> = Vec::new();

    // Convert margins to exclusion polygons
    if cli.margin_left > 0 {
        let margin_px = (x_max * cli.margin_left) / 100;
        polygons.push(Polygon::rectangle(0, 0, margin_px, y_max));
        println!("Left margin: {}% ({}px)", cli.margin_left, margin_px);
    }
    if cli.margin_right > 0 {
        let margin_px = (x_max * cli.margin_right) / 100;
        polygons.push(Polygon::rectangle(x_max - margin_px, 0, x_max, y_max));
        println!("Right margin: {}% ({}px)", cli.margin_right, margin_px);
    }
    if cli.margin_top > 0 {
        let margin_px = (y_max * cli.margin_top) / 100;
        polygons.push(Polygon::rectangle(0, 0, x_max, margin_px));
        println!("Top margin: {}% ({}px)", cli.margin_top, margin_px);
    }
    if cli.margin_bottom > 0 {
        let margin_px = (y_max * cli.margin_bottom) / 100;
        polygons.push(Polygon::rectangle(0, y_max - margin_px, x_max, y_max));
        println!("Bottom margin: {}% ({}px)", cli.margin_bottom, margin_px);
    }

    // Parse explicit polygon exclusion zones
    for (i, polygon_str) in cli.polygon.iter().enumerate() {
        let points = parse_polygon_string(polygon_str)
            .map_err(|e| format!("Failed to parse polygon {} '{}': {}", i + 1, polygon_str, e))?;
        let polygon = Polygon::from_percentages(&points, x_max, y_max)
            .map_err(|e| format!("Failed to create polygon {}: {}", i + 1, e))?;
        println!("Polygon {}: {} vertices", i + 1, polygon.vertices.len());

        // Validate polygon and print warnings
        for warning in polygon.validate() {
            eprintln!("Warning: Polygon {}: {}", i + 1, warning);
        }

        polygons.push(polygon);
    }

    println!("Total exclusion zones: {} polygon(s)", polygons.len());

    // Create virtual device with same capabilities
    let mut builder = VirtualDevice::builder()?
        .name("Filtered Touchpad")
        .with_keys(device.supported_keys().unwrap_or_default())?;

    // Copy device properties (INPUT_PROP_POINTER, etc.) - critical for libinput
    let props = device.properties();
    builder = builder.with_properties(&props)?;

    // Copy relative axes if present
    if let Some(rel_axes) = device.supported_relative_axes() {
        builder = builder.with_relative_axes(&rel_axes)?;
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

    // Set up signal handling for clean shutdown
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    thread::spawn(move || {
        for _ in signals.forever() {
            r.store(false, Ordering::SeqCst);
        }
    });

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
            Ok(0) => continue,                         // Timeout, check running flag
            Ok(_) => {}                                // Events ready
            Err(nix::errno::Errno::EINTR) => continue, // Interrupted by signal
            Err(e) => {
                eprintln!("Poll error: {}", e);
                break;
            }
        }

        // Now fetch events (will return immediately since we polled first)
        let events: Vec<evdev::InputEvent> = match device.fetch_events() {
            Ok(events) => events.collect(),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    continue;
                }
                eprintln!("Error reading events: {}", e);
                break;
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
                                if is_in_any_polygon(x, y, &polygons) {
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
                                if is_in_any_polygon(x, y, &polygons) {
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
                if let evdev::EventSummary::AbsoluteAxis(_, code, _) = event.destructure() {
                    match code {
                        AbsoluteAxisCode::ABS_MT_POSITION_X
                        | AbsoluteAxisCode::ABS_MT_POSITION_Y
                        | AbsoluteAxisCode::ABS_MT_TOUCH_MAJOR
                        | AbsoluteAxisCode::ABS_MT_TOUCH_MINOR
                        | AbsoluteAxisCode::ABS_MT_PRESSURE => {
                            forward = false;
                        }
                        _ => {}
                    }
                }
            }

            // Collect events to forward
            if forward {
                events_to_forward.push(event);
            }
        }

        // Emit all forwarded events as a batch
        if !events_to_forward.is_empty() {
            virtual_device.emit(&events_to_forward)?;
        }
    }

    println!("\nShutting down...");
    drop(device); // Releases grab
    Ok(())
}
