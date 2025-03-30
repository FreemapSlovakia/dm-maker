use crate::shared_types::{
    IgorShadingParams, ObliqueShadingParams, ObliqueSlopeShadingParams, Shading, ShadingMethod,
};
use image::{Rgba, RgbaImage};
use std::f64::{
    self,
    consts::{FRAC_PI_2, PI, TAU},
};

pub fn compute_hillshade<F>(
    elevation: &[f64],
    z_factor: f64,
    rows: usize,
    cols: usize,
    compute_rgb: F,
) -> RgbaImage
where
    F: Fn(f64, f64) -> Rgba<u8>,
{
    let mut hillshade = RgbaImage::new(cols as u32, rows as u32);

    for y in 1..rows - 1 {
        for x in 1..cols - 1 {
            let (slope, aspect) = compute_slope_and_aspect(elevation, z_factor, cols, x, y);

            *hillshade.get_pixel_mut(x as u32, (rows - y) as u32) = compute_rgb(aspect, slope);
        }
    }

    hillshade
}

fn compute_slope_and_aspect(
    elevation: &[f64],
    z_factor: f64,
    cols: usize,
    x: usize,
    y: usize,
) -> (f64, f64) {
    let off = y * cols;

    // Extract 3x3 window
    let z1 = elevation[off - cols + x - 1];
    let z2 = elevation[off - cols + x];
    let z3 = elevation[off - cols + x + 1];
    let z4 = elevation[off + x - 1];
    let z6 = elevation[off + x + 1];
    let z7 = elevation[off + cols + x - 1];
    let z8 = elevation[off + cols + x];
    let z9 = elevation[off + cols + x + 1];

    // Compute raw derivatives (Horn method)
    let dz_dx = (-z1 + z3 - 2.0 * z4 + 2.0 * z6 - z7 + z9) / 8.0;
    let dz_dy = (-z1 - 2.0 * z2 - z3 + z7 + 2.0 * z8 + z9) / 8.0;

    // Apply z-factor
    let dz_dx = dz_dx * z_factor;
    let dz_dy = dz_dy * z_factor;

    // Compute slope
    let mut slope = dz_dx.hypot(dz_dy).atan();

    // Compute aspect
    let mut aspect = dz_dy.atan2(-dz_dx);

    if aspect < 0.0 {
        aspect += TAU;
    }

    if aspect.is_nan() || slope.is_nan() {
        slope = 0.0;
        aspect = 0.0;
    }

    (slope, aspect)
}

pub fn shade(
    aspect: f64,
    slope: f64,
    shadings: &[Shading],
    contrast: f64,
    brightness: f64,
) -> Rgba<u8> {
    let alphas: Vec<_> = shadings
        .iter()
        .map(|shading| {
            let intensity = match &shading.method {
                ShadingMethod::Igor(IgorShadingParams { azimuth }) => {
                    let aspect_diff = difference_between_angles(aspect, azimuth - FRAC_PI_2, TAU);

                    let aspect_strength = 1.0 - aspect_diff / PI;

                    slope / FRAC_PI_2 * 2.0 * aspect_strength
                }
                ShadingMethod::Oblique(ObliqueShadingParams { azimuth, altitude }) => {
                    let zenith = FRAC_PI_2 - altitude;

                    zenith.cos() * slope.cos()
                        + zenith.sin() * slope.sin() * (azimuth - FRAC_PI_2 - aspect).cos()
                }
                ShadingMethod::IgorSlope => slope / FRAC_PI_2,
                ShadingMethod::ObliqueSlope(ObliqueSlopeShadingParams { altitude }) => {
                    let zenith = FRAC_PI_2 - altitude;

                    zenith.cos() * slope.cos() + zenith.sin() * slope.sin()
                }
            };

            let intensity = shading.contrast * (intensity - 0.5) + 0.5 + shading.brightness;

            let alpha = (shading.color & 0xFF) as f64 / 255.0;

            alpha * intensity
        })
        .collect();

    let alphas_sum = f64::MIN_POSITIVE
        + alphas
            .iter()
            .enumerate()
            .map(|(i, alpha)| alpha * shadings[i].weight)
            .sum::<f64>();

    let compute_channel = |shift| {
        let sum: f64 = alphas
            .iter()
            .enumerate()
            .map(|(i, alpha)| {
                alpha * shadings[i].weight * f64::from((shadings[i].color >> shift) & 0xFF_u32)
                    / 255.0
            })
            .sum();

        let value = contrast * ((sum / alphas_sum) - 0.5) + 0.5 + brightness;

        (value * 255.0).clamp(0.0, 255.0) as u8
    };

    let alpha = 1.0 - alphas.iter().map(|alpha| 1.0 - alpha).product::<f64>();

    Rgba([
        compute_channel(24),
        compute_channel(16),
        compute_channel(8),
        (alpha * 255.0).clamp(0.0, 255.0) as u8,
    ])
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
