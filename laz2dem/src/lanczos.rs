// Optimized Lanczos3 resizing from 520x520 to 260x260 for Array2<f64> with statically cached kernel
use ndarray::{Array2, ArrayView2, ArrayViewMut2, Axis};
use std::f64::consts::PI;
use std::sync::OnceLock;

const LANCZOS_RADIUS: usize = 3;
const INPUT_SIZE: usize = 520;
const OUTPUT_SIZE: usize = 260;

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let pix = PI * x;
        (pix.sin()) / pix
    }
}

fn lanczos3(x: f64) -> f64 {
    if x.abs() < LANCZOS_RADIUS as f64 {
        sinc(x) * sinc(x / LANCZOS_RADIUS as f64)
    } else {
        0.0
    }
}

fn generate_kernel(src_len: usize, dst_len: usize) -> Vec<Vec<(isize, f64)>> {
    let scale = dst_len as f64 / src_len as f64;
    let mut kernel = Vec::with_capacity(dst_len);

    for i in 0..dst_len {
        let center = (i as f64 + 0.5) / scale - 0.5;
        let left = center.floor() as isize - LANCZOS_RADIUS as isize + 1;
        let mut weights = Vec::with_capacity(2 * LANCZOS_RADIUS);
        let mut sum = 0.0;

        for j in 0..2 * LANCZOS_RADIUS {
            let idx = left + j as isize;
            let w = lanczos3(center - idx as f64);
            weights.push((idx, w));
            sum += w;
        }

        for (_idx, w) in weights.iter_mut() {
            *w /= sum;
        }

        kernel.push(weights);
    }

    kernel
}

static KERNEL_X: OnceLock<Vec<Vec<(isize, f64)>>> = OnceLock::new();
static KERNEL_Y: OnceLock<Vec<Vec<(isize, f64)>>> = OnceLock::new();

fn resize_1d_const_kernel(
    src: ArrayView2<f64>,
    mut dst: ArrayViewMut2<f64>,
    kernel: &Vec<Vec<(isize, f64)>>,
    axis: Axis,
) {
    let out_len = dst.len_of(axis);
    let other_axis = axis.index() ^ 1;
    let in_len = src.len_of(axis);

    for out_i in 0..out_len {
        for j in 0..src.len_of(Axis(other_axis)) {
            let mut val = 0.0;
            for &(src_i, weight) in &kernel[out_i] {
                if src_i >= 0 && (src_i as usize) < in_len {
                    let idx = if axis == Axis(0) {
                        [src_i as usize, j]
                    } else {
                        [j, src_i as usize]
                    };
                    val += weight * src[idx];
                }
            }
            if axis == Axis(0) {
                dst[[out_i, j]] = val;
            } else {
                dst[[j, out_i]] = val;
            }
        }
    }
}

pub fn resize_520_to_260_lanczos3(input: &Array2<f64>) -> Array2<f64> {
    let kernel_x = KERNEL_X.get_or_init(|| generate_kernel(INPUT_SIZE, OUTPUT_SIZE));
    let kernel_y = KERNEL_Y.get_or_init(|| generate_kernel(INPUT_SIZE, OUTPUT_SIZE));

    let mut tmp = Array2::<f64>::zeros((INPUT_SIZE, OUTPUT_SIZE));

    resize_1d_const_kernel(input.view(), tmp.view_mut(), kernel_x, Axis(1));

    let mut output = Array2::<f64>::zeros((OUTPUT_SIZE, OUTPUT_SIZE));

    resize_1d_const_kernel(tmp.view(), output.view_mut(), kernel_y, Axis(0));

    output
}
