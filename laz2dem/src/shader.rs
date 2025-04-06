use crate::{
    aspect_slope::{AspectSlope, compute_aspect_slope},
    shadings::{
        IgorShadingParams, ObliqueShadingParams, ObliqueSlopeShadingParams, Shading, ShadingMethod,
    },
};
use image::{Rgba, RgbaImage};
use ndarray::Array2;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

pub fn compute_aspect_slopes(
    ele_grid: &Array2<f64>,
    z_factor: f64,
    rows: usize,
    cols: usize,
) -> Array2<AspectSlope> {
    let mut out = Array2::<AspectSlope>::default((cols, rows));

    for y in 1..rows - 1 {
        for x in 1..cols - 1 {
            out[[x, y]] = compute_aspect_slope(ele_grid, z_factor, x, y);
        }
    }

    out
}

pub fn compute_hillshade<F>(
    ele_grid: &Array2<f64>,
    z_factor: f64,
    rows: usize,
    cols: usize,
    compute_rgb: F,
) -> RgbaImage
where
    F: Fn(AspectSlope) -> Rgba<u8>,
{
    let mut hillshade = RgbaImage::new(cols as u32, rows as u32);

    for y in 1..rows - 1 {
        for x in 1..cols - 1 {
            *hillshade.get_pixel_mut(x as u32, (rows - y) as u32) =
                compute_rgb(compute_aspect_slope(ele_grid, z_factor, x, y));
        }
    }

    hillshade
}

pub fn shade(
    aspect_slope: AspectSlope,
    shadings: &[Shading],
    contrast: f64,
    brightness: f64,
) -> Rgba<u8> {
    let AspectSlope { aspect, slope } = aspect_slope;

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
