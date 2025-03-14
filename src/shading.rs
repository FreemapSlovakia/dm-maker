use image::RgbImage;

pub fn compute_hillshade<F>(
    elevation: Vec<f64>,
    rows: usize,
    cols: usize,
    compute_rgb: F,
) -> RgbImage
where
    F: Fn(f64, f64) -> [u8; 3],
{
    let mut hillshade = RgbImage::new(cols as u32, rows as u32);

    for y in 1..rows - 1 {
        for x in 1..cols - 1 {
            let (slope_rad, aspect_rad) = compute_slope_and_aspect(&elevation, cols, x, y);

            hillshade.get_pixel_mut(x as u32, (rows - y) as u32).0 =
                compute_rgb(aspect_rad, slope_rad);
        }
    }

    hillshade
}

fn compute_slope_and_aspect(elevation: &[f64], cols: usize, x: usize, y: usize) -> (f64, f64) {
    let off = y * cols;

    // Extract 3x3 window
    let z1 = elevation[off - cols + x - 1];
    let z2 = elevation[off - cols + x];
    let z3 = elevation[off - cols + x + 1];
    let z4 = elevation[off + x - 1];
    // let z5 = elevation[off + x]; // Center pixel
    let z6 = elevation[off + x + 1];
    let z7 = elevation[off + cols + x - 1];
    let z8 = elevation[off + cols + x];
    let z9 = elevation[off + cols + x + 1];

    // Compute partial derivatives (Horn method)
    let dz_dx = (-z1 + z3 - 2.0 * z4 + 2.0 * z6 - z7 + z9) / 8.0 * 1.7;

    let dz_dy = (-z1 - 2.0 * z2 - z3 + z7 + 2.0 * z8 + z9) / 8.0 * 1.7;

    // Compute slope
    let mut slope_rad = dz_dx.hypot(dz_dy).atan();

    // Compute aspect
    let mut aspect_rad = dz_dy.atan2(-dz_dx); // Negative sign because of coordinate convention

    if aspect_rad < 0.0 {
        aspect_rad += std::f64::consts::TAU; // Convert to 0 - 2π range
    }

    if aspect_rad.is_nan() || slope_rad.is_nan() {
        slope_rad = 0.0;
        aspect_rad = 0.0;
    }

    (slope_rad, aspect_rad)
}
