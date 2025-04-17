use std::f64::{self, consts::TAU};

use crate::elevation_type::Gray64FImage;

#[derive(Clone, Copy, Default)]
pub struct AspectSlope {
    pub slope: f64,
    pub aspect: f64,
}

pub fn compute_aspect_slope(
    elevation_image: &Gray64FImage,
    z_factor: f64,
    x: u32,
    y: u32,
) -> AspectSlope {
    // Extract 3x3 window
    let z1 = elevation_image.get_pixel(x, y).0[0];
    let z2 = elevation_image.get_pixel(x, y - 1).0[0];
    let z3 = elevation_image.get_pixel(x + 1, y - 1).0[0];
    let z4 = elevation_image.get_pixel(x - 1, y).0[0];
    let z6 = elevation_image.get_pixel(x + 1, y).0[0];
    let z7 = elevation_image.get_pixel(x - 1, y + 1).0[0];
    let z8 = elevation_image.get_pixel(x, y + 1).0[0];
    let z9 = elevation_image.get_pixel(x + 1, y + 1).0[0];

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

    AspectSlope { slope, aspect }
}
