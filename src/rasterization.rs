use crate::{
    schema::create_schema,
    shared_types::{PointWithHeight, TileMeta},
};
use core::f64;
use image::{RgbImage, codecs::jpeg::JpegEncoder, imageops::crop_imm};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use rusqlite::Connection;
use spade::{DelaunayTriangulation, Point2, Triangulation};
use std::{fs::remove_file, io::Cursor, sync::Mutex};

pub fn rasterize(pixels_per_meter: f64, buffer_px: u32, tile_metas: Vec<TileMeta>) {
    remove_file("out.mbtiles").unwrap_or(());

    let conn = Connection::open("out.mbtiles").unwrap();

    create_schema(
        &conn,
        &[
            ("name", "HS"),
            ("minzoom", "20"),
            ("maxzoom", "20"),
            ("format", "jpeg"),
            // ("bounds", ...TODO)
        ],
    )
    .unwrap();

    conn.pragma_update(None, "synchronous", "OFF").unwrap();

    conn.pragma_update(None, "journal_mode", "WAL").unwrap();

    let conn = Mutex::new(conn);

    println!("Tiles: {}", tile_metas.len());

    tile_metas.into_par_iter().for_each(|tile_meta| {
        // println!("Processing {:?}", tile_meta.tile);

        let mut t = DelaunayTriangulation::<PointWithHeight>::new();

        let points = tile_meta.points.into_inner().unwrap();

        for point in points {
            t.insert(point).unwrap();
        }

        let bbox_3857 = tile_meta.bbox;

        let width_pixels = (bbox_3857.width() * pixels_per_meter).round() as u32;
        let height_pixels = (bbox_3857.height() * pixels_per_meter).round() as u32;

        let mut img = Vec::new();

        let nn = t.natural_neighbor();

        for y in 0..height_pixels {
            let cy = bbox_3857.min_y + y as f64 * bbox_3857.height() / height_pixels as f64;

            for x in 0..width_pixels {
                let cx = bbox_3857.min_x + x as f64 * bbox_3857.width() / width_pixels as f64;

                let v = nn.interpolate(|v| v.data().height, Point2::new(cx, cy));

                if let Some(v) = v {
                    if v.is_nan() {
                        println!("NAN {cx} {cy}")
                    }

                    img.push(v);
                } else {
                    img.push(f64::NAN);
                }
            }
        }

        // let sun_azimuth_rad = 315_f64.to_radians();
        // let sun_zenith_rad = 45_f64.to_radians();

        let img = compute_hillshade(
            img,
            height_pixels as usize,
            width_pixels as usize,
            |aspect_rad, slope_rad| {
                igor_rgb(
                    aspect_rad,
                    slope_rad,
                    &[
                        (-120.0, 0.8, 0x203060),
                        (60.0, 0.7, 0xFFEE00),
                        (-45.0, 1.0, 0x000000),
                    ],
                    1.0,
                    0.0,
                )

                // // Compute illumination using sun angle
                // let illumination = sun_zenith_rad.cos() * slope_rad.cos()
                //     + sun_zenith_rad.sin() * slope_rad.sin() * (sun_azimuth_rad - aspect_rad).cos();

                // // Scale to 0-255
                // let shade = (illumination * 255.0).clamp(0.0, 255.0) as u8;

                // [shade, shade, shade]
            },
        );

        let tiles = tile_meta.tile.get_children();

        for (sector, tile) in tiles.iter().enumerate() {
            let img = crop_imm(
                &img,
                buffer_px + ((sector as u32) & 1) * 256,
                buffer_px + (sector as u32 >> 1) * 256,
                256,
                256,
            )
            .to_image();

            let mut buffer = vec![];

            img.write_with_encoder(JpegEncoder::new(Cursor::new(&mut buffer)))
                .unwrap();

            conn.lock()
                .unwrap()
                .execute(
                    "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)",
                    (tile.zoom, tile.x, tile.reversed_y(), buffer),
                )
                .unwrap();
        }
    });
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

fn compute_hillshade<F>(elevation: Vec<f64>, rows: usize, cols: usize, compute_rgb: F) -> RgbImage
where
    F: Fn(f64, f64) -> [u8; 3],
{
    let mut hillshade = RgbImage::new(cols as u32, rows as u32);

    for y in 1..rows - 1 {
        for x in 1..cols - 1 {
            let (slope_rad, aspect_rad) = compute_slope_and_aspect(&elevation, cols, x, y);

            hillshade.get_pixel_mut(x as u32, (rows - y) as u32).0 =
                compute_rgb(aspect_rad, slope_rad);

            igor_rgb(
                aspect_rad,
                slope_rad,
                &[
                    (-120.0, 0.8, 0x203060),
                    (60.0, 0.7, 0xFFEE00),
                    (-45.0, 1.0, 0x000000),
                ],
                1.0,
                0.0,
            );
        }
    }

    hillshade
}

fn igor_rgb(
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
