use std::f64;

pub fn igor_rgb(
    aspect_rad: f64,
    slope_rad: f64,
    params: &[(f64, f64, u32)],
    contrast: f64,
    brightness: f64,
) -> [u8; 3] {
    let igor = |az: f64| {
        let aspect_diff = difference_between_angles(
            aspect_rad,
            f64::consts::PI * 1.5 - az.to_radians(),
            f64::consts::PI * 2.0,
        );

        let aspect_strength = 1.0 - aspect_diff / f64::consts::PI;

        1.0 - slope_rad * 2.0 * aspect_strength
    };

    // Compute modified hillshade values
    let mods: Vec<_> = params
        .iter()
        .map(|param| param.1 * (1.0 - igor(param.0)))
        .collect();

    // Normalization factor
    let norm = f64::MIN_POSITIVE + mods.iter().sum::<f64>();

    let alpha = 1.0 - mods.iter().map(|m| 1.0 - m).product::<f64>();

    // Compute each channel
    let compute_channel = |shift| {
        let sum: f64 = mods
            .iter()
            .enumerate()
            .map(|(i, m)| m * f64::from((params[i].2 >> shift) & 0xFF_u32) / 255.0)
            .sum();

        let value = contrast * ((sum / norm) - 0.5) + 0.5 + brightness;

        let value = value + (1.0 - value) * (1.0 - alpha);

        (value * 255.0).clamp(0.0, 255.0) as u8
    };

    let r = compute_channel(16);
    let g = compute_channel(8);
    let b = compute_channel(0);

    [r, g, b]
}

fn normalize_angle(angle: f64, normalizer: f64) -> f64 {
    let angle = angle % normalizer;

    if angle < 0.0 {
        normalizer + angle
    } else {
        angle
    }
}

fn difference_between_angles(angle1: f64, angle2: f64, normalizer: f64) -> f64 {
    let diff = (normalize_angle(angle1, normalizer) - normalize_angle(angle2, normalizer)).abs();

    if diff > normalizer / 2.0 {
        normalizer - diff
    } else {
        diff
    }
}
