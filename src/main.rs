use clap::Parser;
use evdev::{uinput::VirtualDevice, AbsoluteAxisCode};
use nix::poll::{poll, PollFd, PollFlags};
use signal_hook::{consts::SIGINT, consts::SIGTERM, iterator::Signals};
use std::collections::HashMap;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

#[derive(Debug, Clone)]
struct Polygon {
    vertices: Vec<(i32, i32)>, // Absolute coordinates
}

impl Polygon {
    /// Create a polygon from percentage coordinates (0-100)
    /// Converts percentage points to absolute touchpad coordinates
    fn from_percentages(points: &[(f32, f32)], x_max: i32, y_max: i32) -> Result<Self, String> {
        if points.len() < 3 {
            return Err("Polygon must have at least 3 points".to_string());
        }

        let vertices: Vec<(i32, i32)> = points
            .iter()
            .map(|(x_pct, y_pct)| {
                let x = ((*x_pct / 100.0) * x_max as f32) as i32;
                let y = ((*y_pct / 100.0) * y_max as f32) as i32;
                (x, y)
            })
            .collect();

        Ok(Polygon { vertices })
    }

    /// Create a rectangle polygon from absolute coordinates
    fn rectangle(x1: i32, y1: i32, x2: i32, y2: i32) -> Self {
        Polygon {
            vertices: vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)],
        }
    }

    /// Check if a point is inside the polygon using ray casting algorithm
    fn contains(&self, px: i32, py: i32) -> bool {
        let mut inside = false;
        let n = self.vertices.len();
        let mut j = n - 1;

        for i in 0..n {
            let (xi, yi) = self.vertices[i];
            let (xj, yj) = self.vertices[j];

            if (yi > py) != (yj > py) {
                let x_intersect = xj as i64
                    + ((py as i64 - yj as i64) * (xi as i64 - xj as i64)) / (yi as i64 - yj as i64);
                if (px as i64) < x_intersect {
                    inside = !inside;
                }
            }
            j = i;
        }
        inside
    }
}

/// Parse a polygon string like "0,0 10,0 0,30" into percentage coordinates (0-100)
/// All coordinates are percentages of touchpad width/height, making configs portable
fn parse_polygon_string(s: &str) -> Result<Vec<(f32, f32)>, String> {
    let points: Result<Vec<(f32, f32)>, String> = s
        .split_whitespace()
        .map(|point_str| {
            let coords: Vec<&str> = point_str.split(',').collect();
            if coords.len() != 2 {
                return Err(format!(
                    "Invalid point format '{point_str}', expected 'x,y'"
                ));
            }

            let x = coords[0]
                .parse::<f32>()
                .map_err(|_| format!("Invalid x coordinate '{}'", coords[0]))?;
            let y = coords[1]
                .parse::<f32>()
                .map_err(|_| format!("Invalid y coordinate '{}'", coords[1]))?;

            if !(0.0..=100.0).contains(&x) || !(0.0..=100.0).contains(&y) {
                return Err(format!(
                    "Coordinates must be in range 0-100, got ({x}, {y})"
                ));
            }

            Ok((x, y))
        })
        .collect();

    let points = points?;

    if points.len() < 3 {
        return Err(format!(
            "Polygon must have at least 3 points, got {}",
            points.len()
        ));
    }

    Ok(points)
}

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

/// Check if a point is inside any of the exclusion polygons
fn is_in_any_polygon(x: i32, y: i32, polygons: &[Polygon]) -> bool {
    polygons.iter().any(|polygon| polygon.contains(x, y))
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
                evdev::EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_TRACKING_ID, value) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for parse_polygon_string

    #[test]
    fn test_parse_polygon_string_valid_triangle() {
        let result = parse_polygon_string("0,0 100,0 50,100");
        assert!(result.is_ok());
        let points = result.unwrap();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0], (0.0, 0.0));
        assert_eq!(points[1], (100.0, 0.0));
        assert_eq!(points[2], (50.0, 100.0));
    }

    #[test]
    fn test_parse_polygon_string_valid_rectangle() {
        let result = parse_polygon_string("0,0 50,0 50,50 0,50");
        assert!(result.is_ok());
        let points = result.unwrap();
        assert_eq!(points.len(), 4);
    }

    #[test]
    fn test_parse_polygon_string_too_few_points() {
        let result = parse_polygon_string("0,0 100,0");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least 3 points"));
    }

    #[test]
    fn test_parse_polygon_string_invalid_format() {
        let result = parse_polygon_string("0,0 100");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid point format"));
    }

    #[test]
    fn test_parse_polygon_string_out_of_range() {
        let result = parse_polygon_string("0,0 150,0 50,100");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("range 0-100"));
    }

    #[test]
    fn test_parse_polygon_string_negative_coords() {
        let result = parse_polygon_string("-10,0 100,0 50,100");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("range 0-100"));
    }

    #[test]
    fn test_parse_polygon_string_invalid_number() {
        let result = parse_polygon_string("0,0 abc,def 50,100");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid"));
    }

    // Tests for Polygon::contains (ray casting algorithm)

    #[test]
    fn test_polygon_contains_triangle_inside() {
        let polygon = Polygon {
            vertices: vec![(0, 0), (100, 0), (50, 100)],
        };
        assert!(polygon.contains(50, 30));
        assert!(polygon.contains(25, 10));
        assert!(polygon.contains(75, 10));
    }

    #[test]
    fn test_polygon_contains_triangle_outside() {
        let polygon = Polygon {
            vertices: vec![(0, 0), (100, 0), (50, 100)],
        };
        assert!(!polygon.contains(10, 90));
        assert!(!polygon.contains(90, 90));
        assert!(!polygon.contains(50, 110));
        assert!(!polygon.contains(-10, 0));
    }

    #[test]
    fn test_polygon_contains_rectangle() {
        let polygon = Polygon {
            vertices: vec![(10, 10), (90, 10), (90, 90), (10, 90)],
        };
        assert!(polygon.contains(50, 50));
        assert!(polygon.contains(15, 15));
        assert!(polygon.contains(85, 85));
        assert!(!polygon.contains(5, 5));
        assert!(!polygon.contains(95, 50));
        assert!(!polygon.contains(50, 95));
    }

    #[test]
    fn test_polygon_contains_edge_cases() {
        let polygon = Polygon {
            vertices: vec![(0, 0), (100, 0), (100, 100), (0, 100)],
        };
        // Points on edges or vertices might behave differently
        // The important thing is consistency
        assert!(polygon.contains(50, 50));
        assert!(polygon.contains(1, 1));
        assert!(polygon.contains(99, 99));
    }

    #[test]
    fn test_polygon_from_percentages() {
        let points = vec![(0.0, 0.0), (100.0, 0.0), (50.0, 100.0)];
        let result = Polygon::from_percentages(&points, 1000, 2000);
        assert!(result.is_ok());
        let polygon = result.unwrap();
        assert_eq!(polygon.vertices.len(), 3);
        assert_eq!(polygon.vertices[0], (0, 0));
        assert_eq!(polygon.vertices[1], (1000, 0));
        assert_eq!(polygon.vertices[2], (500, 2000));
    }

    #[test]
    fn test_polygon_from_percentages_too_few_points() {
        let points = vec![(0.0, 0.0), (100.0, 0.0)];
        let result = Polygon::from_percentages(&points, 1000, 2000);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least 3 points"));
    }

    // Tests for helper functions

    #[test]
    fn test_is_in_any_polygon_empty_list() {
        let polygons: Vec<Polygon> = vec![];
        assert!(!is_in_any_polygon(50, 50, &polygons));
    }

    #[test]
    fn test_is_in_any_polygon_single_polygon() {
        let polygon = Polygon {
            vertices: vec![(0, 0), (100, 0), (50, 100)],
        };
        let polygons = vec![polygon];
        assert!(is_in_any_polygon(50, 30, &polygons));
        assert!(!is_in_any_polygon(10, 90, &polygons));
    }

    #[test]
    fn test_is_in_any_polygon_multiple_polygons() {
        let polygon1 = Polygon {
            vertices: vec![(0, 0), (50, 0), (25, 50)],
        };
        let polygon2 = Polygon {
            vertices: vec![(200, 0), (250, 0), (225, 50)],
        };
        let polygons = vec![polygon1, polygon2];

        assert!(is_in_any_polygon(25, 20, &polygons)); // In polygon1
        assert!(is_in_any_polygon(225, 20, &polygons)); // In polygon2
        assert!(!is_in_any_polygon(125, 20, &polygons)); // In neither
    }

    // Tests for Polygon::rectangle

    #[test]
    fn test_polygon_rectangle() {
        let rect = Polygon::rectangle(10, 20, 50, 80);
        assert_eq!(rect.vertices.len(), 4);
        assert_eq!(rect.vertices[0], (10, 20));
        assert_eq!(rect.vertices[1], (50, 20));
        assert_eq!(rect.vertices[2], (50, 80));
        assert_eq!(rect.vertices[3], (10, 80));
    }

    #[test]
    fn test_polygon_rectangle_contains() {
        let rect = Polygon::rectangle(10, 10, 90, 90);
        assert!(rect.contains(50, 50));
        assert!(rect.contains(15, 15));
        assert!(rect.contains(85, 85));
        assert!(!rect.contains(5, 50)); // Left of rect
        assert!(!rect.contains(95, 50)); // Right of rect
        assert!(!rect.contains(50, 5)); // Above rect
        assert!(!rect.contains(50, 95)); // Below rect
    }

    // Tests for margins as polygons

    #[test]
    fn test_margins_as_polygons() {
        // Simulate 10% margins on a 100x100 touchpad
        let x_max = 100;
        let y_max = 100;
        let margin = 10;

        let mut polygons = vec![];
        polygons.push(Polygon::rectangle(0, 0, margin, y_max)); // Left
        polygons.push(Polygon::rectangle(x_max - margin, 0, x_max, y_max)); // Right
        polygons.push(Polygon::rectangle(0, 0, x_max, margin)); // Top
        polygons.push(Polygon::rectangle(0, y_max - margin, x_max, y_max)); // Bottom

        // Center should not be blocked
        assert!(!is_in_any_polygon(50, 50, &polygons));

        // Margins should be blocked
        assert!(is_in_any_polygon(5, 50, &polygons)); // Left margin
        assert!(is_in_any_polygon(95, 50, &polygons)); // Right margin
        assert!(is_in_any_polygon(50, 5, &polygons)); // Top margin
        assert!(is_in_any_polygon(50, 95, &polygons)); // Bottom margin
    }

    #[test]
    fn test_combined_margins_and_polygon() {
        // Simulate margins + custom triangle polygon
        let mut polygons = vec![];

        // Left margin (10%)
        polygons.push(Polygon::rectangle(0, 0, 10, 100));

        // Custom triangle in center
        polygons.push(Polygon {
            vertices: vec![(40, 40), (60, 40), (50, 60)],
        });

        // Left margin blocks
        assert!(is_in_any_polygon(5, 50, &polygons));

        // Center of triangle blocks
        assert!(is_in_any_polygon(50, 45, &polygons));

        // Outside both doesn't block
        assert!(!is_in_any_polygon(80, 50, &polygons));
    }
}
