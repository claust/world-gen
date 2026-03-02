use glam::Vec3;

/// Test whether a point is inside the crown envelope for the given shape.
pub fn is_inside_crown(
    shape: &str,
    point: Vec3,
    tree_height: f32,
    crown_base: f32,
    aspect_ratio: f32,
) -> bool {
    let crown_bottom = tree_height * crown_base;
    let crown_height = tree_height - crown_bottom;
    if crown_height <= 0.0 {
        return true;
    }

    let crown_center_y = (tree_height + crown_bottom) / 2.0;
    let crown_radius_y = crown_height / 2.0;
    let crown_radius_h = crown_radius_y * aspect_ratio;

    let h_dist = (point.x * point.x + point.z * point.z).sqrt();
    let nh = h_dist / crown_radius_h;
    let nv = (point.y - crown_center_y) / crown_radius_y;
    let t_in_crown = (point.y - crown_bottom) / crown_height;
    let slack = 1.15;

    match shape {
        "dome" | "oval" => nh * nh + nv * nv <= slack,
        "conical" => {
            if !(-0.05..=1.05).contains(&t_in_crown) {
                return false;
            }
            h_dist <= crown_radius_h * (1.0 - t_in_crown).max(0.0) * slack
        }
        "columnar" => {
            if !(-0.05..=1.05).contains(&t_in_crown) {
                return false;
            }
            h_dist <= crown_radius_h * 0.35 * slack
        }
        "vase" => {
            if !(-0.05..=1.05).contains(&t_in_crown) {
                return false;
            }
            h_dist <= crown_radius_h * (0.3 + 0.7 * t_in_crown) * slack
        }
        "umbrella" => {
            if !(-0.05..=1.05).contains(&t_in_crown) {
                return false;
            }
            let factor = if t_in_crown > 0.6 {
                1.0
            } else {
                0.2 + 0.8 * t_in_crown
            };
            h_dist <= crown_radius_h * factor * slack
        }
        "weeping" => {
            (h_dist / (crown_radius_h * 1.3)).powi(2)
                + ((point.y - crown_center_y) / (crown_radius_y * 1.4)).powi(2)
                <= 1.5
        }
        "fan_top" => t_in_crown > 0.8,
        _ => true,
    }
}

/// Compute the branch length scaling factor for a given profile at position t (0..1).
pub fn length_profile(profile: &str, t: f32) -> f32 {
    match profile {
        "conical" => (1.0 - t).max(0.0),
        "dome" => (t * std::f32::consts::PI).sin(),
        "columnar" => 0.6 + 0.4 * (t * std::f32::consts::PI).sin(),
        "vase" => 0.3 + 0.7 * t,
        "layered" => 0.4 + 0.6 * (t * std::f32::consts::PI * 3.0).sin().abs(),
        _ => 1.0,
    }
}
