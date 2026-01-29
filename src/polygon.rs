#[derive(Debug, Clone)]
pub struct Polygon {
    pub vertices: Vec<(i32, i32)>, // Absolute coordinates
}

impl Polygon {
    /// Create a polygon from percentage coordinates (0-100)
    /// Converts percentage points to absolute touchpad coordinates
    pub fn from_percentages(points: &[(f32, f32)], x_max: i32, y_max: i32) -> Result<Self, String> {
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
    pub fn rectangle(x1: i32, y1: i32, x2: i32, y2: i32) -> Self {
        Polygon {
            vertices: vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)],
        }
    }

    /// Check if a point is inside the polygon using ray casting algorithm
    pub fn contains(&self, px: i32, py: i32) -> bool {
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

    /// Validate polygon and return warnings for suspicious configurations
    pub fn validate(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        // 1. Check for duplicate consecutive points
        for i in 0..self.vertices.len() {
            let j = (i + 1) % self.vertices.len();
            if self.vertices[i] == self.vertices[j] {
                warnings.push(format!(
                    "Duplicate consecutive vertex at {:?}",
                    self.vertices[i]
                ));
            }
        }

        // 2. Check for zero/near-zero area using shoelace formula
        let area = self.signed_area().abs();
        if area < 1.0 {
            // Less than 1 square unit
            warnings.push("Polygon has near-zero area (may be degenerate)".to_string());
        }

        // 3. Check for self-intersection
        if self.is_self_intersecting() {
            warnings.push("Polygon has self-intersecting edges".to_string());
        }

        warnings
    }

    /// Calculate the signed area of the polygon using shoelace formula
    fn signed_area(&self) -> f64 {
        let mut sum = 0i64;
        for i in 0..self.vertices.len() {
            let j = (i + 1) % self.vertices.len();
            let (x1, y1) = self.vertices[i];
            let (x2, y2) = self.vertices[j];
            sum += (x1 as i64) * (y2 as i64) - (x2 as i64) * (y1 as i64);
        }
        (sum as f64).abs() / 2.0
    }

    /// Check if polygon has self-intersecting edges
    fn is_self_intersecting(&self) -> bool {
        let n = self.vertices.len();

        // Check all non-adjacent edge pairs for intersection
        for i in 0..n {
            let j = (i + 1) % n;
            let (p1, p2) = (self.vertices[i], self.vertices[j]);

            // Start checking from i+2 to avoid checking adjacent edges
            let start = (i + 2) % n;
            for k in 0..n {
                let edge_idx = (start + k) % n;
                // Stop before wrapping back to the edge adjacent to current
                if edge_idx == i || edge_idx == j {
                    continue;
                }

                let next_idx = (edge_idx + 1) % n;
                if next_idx == i || next_idx == j {
                    continue;
                }

                let (p3, p4) = (self.vertices[edge_idx], self.vertices[next_idx]);

                if Self::segments_intersect(p1, p2, p3, p4) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if two line segments intersect
    fn segments_intersect(p1: (i32, i32), p2: (i32, i32), p3: (i32, i32), p4: (i32, i32)) -> bool {
        fn ccw(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> i32 {
            let (ax, ay) = a;
            let (bx, by) = b;
            let (cx, cy) = c;
            ((cy as i64 - ay as i64) * (bx as i64 - ax as i64)
                - (by as i64 - ay as i64) * (cx as i64 - ax as i64))
                .signum() as i32
        }

        let d1 = ccw(p3, p4, p1);
        let d2 = ccw(p3, p4, p2);
        let d3 = ccw(p1, p2, p3);
        let d4 = ccw(p1, p2, p4);

        if d1 != d2 && d3 != d4 {
            return true;
        }

        // Check collinear cases (endpoints touching)
        if d1 == 0 && Self::on_segment(p3, p1, p4) {
            return true;
        }
        if d2 == 0 && Self::on_segment(p3, p2, p4) {
            return true;
        }
        if d3 == 0 && Self::on_segment(p1, p3, p2) {
            return true;
        }
        if d4 == 0 && Self::on_segment(p1, p4, p2) {
            return true;
        }

        false
    }

    /// Check if point q lies on segment pr (assuming q is collinear with p and r)
    fn on_segment(p: (i32, i32), q: (i32, i32), r: (i32, i32)) -> bool {
        q.0 <= p.0.max(r.0) && q.0 >= p.0.min(r.0) && q.1 <= p.1.max(r.1) && q.1 >= p.1.min(r.1)
    }
}

/// Parse a polygon string like "0,0 10,0 0,30" into percentage coordinates (0-100)
/// All coordinates are percentages of touchpad width/height, making configs portable
pub fn parse_polygon_string(s: &str) -> Result<Vec<(f32, f32)>, String> {
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

/// Check if a point is inside any of the exclusion polygons
pub fn is_in_any_polygon(x: i32, y: i32, polygons: &[Polygon]) -> bool {
    polygons.iter().any(|polygon| polygon.contains(x, y))
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

    // Tests for polygon validation

    #[test]
    fn test_validate_valid_polygon() {
        let polygon = Polygon {
            vertices: vec![(0, 0), (100, 0), (50, 100)],
        };
        let warnings = polygon.validate();
        assert!(warnings.is_empty(), "Valid polygon should have no warnings");
    }

    #[test]
    fn test_validate_duplicate_consecutive_points() {
        let polygon = Polygon {
            vertices: vec![(0, 0), (0, 0), (100, 0), (50, 100)],
        };
        let warnings = polygon.validate();
        assert!(!warnings.is_empty());
        assert!(warnings
            .iter()
            .any(|w| w.contains("Duplicate consecutive vertex")));
    }

    #[test]
    fn test_validate_zero_area_collinear() {
        // All points on a line - zero area
        let polygon = Polygon {
            vertices: vec![(0, 0), (50, 0), (100, 0)],
        };
        let warnings = polygon.validate();
        assert!(!warnings.is_empty());
        assert!(warnings.iter().any(|w| w.contains("near-zero area")));
    }

    #[test]
    fn test_validate_self_intersecting() {
        // Figure-eight / bowtie shape
        let polygon = Polygon {
            vertices: vec![(0, 0), (100, 100), (100, 0), (0, 100)],
        };
        let warnings = polygon.validate();
        assert!(!warnings.is_empty());
        assert!(warnings.iter().any(|w| w.contains("self-intersecting")));
    }

    #[test]
    fn test_signed_area_triangle() {
        let polygon = Polygon {
            vertices: vec![(0, 0), (100, 0), (0, 100)],
        };
        let area = polygon.signed_area();
        // Area of right triangle with base=100, height=100 is 5000
        assert!((area - 5000.0).abs() < 1.0);
    }

    #[test]
    fn test_signed_area_rectangle() {
        let polygon = Polygon {
            vertices: vec![(0, 0), (100, 0), (100, 50), (0, 50)],
        };
        let area = polygon.signed_area();
        // Area of 100x50 rectangle is 5000
        assert!((area - 5000.0).abs() < 1.0);
    }

    #[test]
    fn test_segments_intersect_crossing() {
        // Two segments that cross
        let p1 = (0, 0);
        let p2 = (100, 100);
        let p3 = (0, 100);
        let p4 = (100, 0);
        assert!(Polygon::segments_intersect(p1, p2, p3, p4));
    }

    #[test]
    fn test_segments_intersect_non_crossing() {
        // Two segments that don't intersect
        let p1 = (0, 0);
        let p2 = (50, 0);
        let p3 = (100, 0);
        let p4 = (150, 0);
        assert!(!Polygon::segments_intersect(p1, p2, p3, p4));
    }

    #[test]
    fn test_is_self_intersecting_valid_square() {
        let polygon = Polygon {
            vertices: vec![(0, 0), (100, 0), (100, 100), (0, 100)],
        };
        assert!(!polygon.is_self_intersecting());
    }

    #[test]
    fn test_is_self_intersecting_bowtie() {
        // Bowtie/figure-eight polygon
        let polygon = Polygon {
            vertices: vec![(0, 0), (100, 100), (100, 0), (0, 100)],
        };
        assert!(polygon.is_self_intersecting());
    }

    #[test]
    fn test_validate_multiple_warnings() {
        // Polygon with both duplicate points and near-zero area
        let polygon = Polygon {
            vertices: vec![(0, 0), (0, 0), (1, 0)],
        };
        let warnings = polygon.validate();
        // Should have warnings for both duplicate vertices and near-zero area
        assert!(warnings.len() >= 2);
        assert!(warnings.iter().any(|w| w.contains("Duplicate")));
        assert!(warnings.iter().any(|w| w.contains("near-zero area")));
    }
}
