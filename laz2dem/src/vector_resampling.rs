use ndarray::{Array2, s};
use std::f64::consts::{PI, TAU};

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

fn lanczos3_kernel(x: f64) -> f64 {
    let a = 3.0;
    if x.abs() < a {
        sinc(x) * sinc(x / a)
    } else {
        0.0
    }
}

fn precompute_kernel() -> [[f64; 7]; 7] {
    let mut kernel = [[0.0; 7]; 7];
    let mut sum = 0.0;

    for j in -3..=3 {
        for i in -3..=3 {
            let dx = i as f64;
            let dy = j as f64;
            let w = lanczos3_kernel(dx) * lanczos3_kernel(dy);
            kernel[(j + 3) as usize][(i + 3) as usize] = w;
            sum += w;
        }
    }

    for row in kernel.iter_mut() {
        for val in row.iter_mut() {
            *val /= sum;
        }
    }

    kernel
}

fn aspect_slope_to_vector(aspect_deg: f64, slope_deg: f64) -> [f64; 3] {
    let az = aspect_deg.to_radians();
    let el = slope_deg.to_radians();
    let cos_el = el.cos();
    [cos_el * az.cos(), cos_el * az.sin(), el.sin()]
}

fn vector_to_aspect_slope(v: [f64; 3]) -> (f64, f64) {
    let norm = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let x = v[0] / norm;
    let y = v[1] / norm;
    let z = v[2] / norm;
    let aspect = (y.atan2(x) + TAU) % TAU;
    let slope = z.asin();
    (aspect.to_degrees(), slope.to_degrees())
}

pub fn downscale_lanczos3_ndarray(
    aspect_grid: &Array2<f64>,
    slope_grid: &Array2<f64>,
) -> (Array2<f64>, Array2<f64>) {
    let kernel = precompute_kernel();
    let (in_h, in_w) = aspect_grid.dim();
    let out_h = in_h / 2;
    let out_w = in_w / 2;

    let mut out_aspect = Array2::<f64>::zeros((out_h, out_w));
    let mut out_slope = Array2::<f64>::zeros((out_h, out_w));

    for oy in 0..out_h {
        for ox in 0..out_w {
            let cx = ox * 2 + 1;
            let cy = oy * 2 + 1;
            let mut sum = [0.0f64; 3];

            for j in -3..=3 {
                for i in -3..=3 {
                    let sx = cx as isize + i;
                    let sy = cy as isize + j;
                    if sx < 0 || sx >= in_w as isize || sy < 0 || sy >= in_h as isize {
                        continue;
                    }

                    let w = kernel[(j + 3) as usize][(i + 3) as usize];
                    if w == 0.0 {
                        continue;
                    }

                    let asp = aspect_grid[[sy as usize, sx as usize]];
                    let slp = slope_grid[[sy as usize, sx as usize]];
                    let vec = aspect_slope_to_vector(asp, slp);
                    for k in 0..3 {
                        sum[k] += vec[k] * w;
                    }
                }
            }

            let (asp, slp) = vector_to_aspect_slope(sum);
            out_aspect[[oy, ox]] = asp;
            out_slope[[oy, ox]] = slp;
        }
    }

    (out_aspect, out_slope)
}
