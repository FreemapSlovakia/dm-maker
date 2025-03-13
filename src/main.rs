mod index;
mod schema;

use core::f64;
use image::{RgbImage, codecs::jpeg::JpegEncoder, imageops::crop_imm};
use las::{Reader, point::Classification};
use maptile::{bbox::BBox, constants::WEB_MERCATOR_EXTENT, tile::Tile, utils::bbox_covered_tiles};
use proj::Proj;
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use rusqlite::Connection;
use schema::create_schema;
use spade::{DelaunayTriangulation, HasPosition, Point2, Triangulation};
use std::{fs::remove_file, io::Cursor, sync::Mutex};

fn main() {
    render();

    // index();
}

struct PointWithHeight {
    position: Point2<f64>,
    height: f64,
}

impl HasPosition for PointWithHeight {
    type Scalar = f64;

    fn position(&self) -> Point2<f64> {
        self.position
    }
}

struct TileMeta {
    tile: Tile,
    bbox: BBox,
    points: Mutex<Vec<PointWithHeight>>,
}

fn render() {
    let proj_3857_to_8353 = Proj::new_known_crs("EPSG:3857", "EPSG:8353", None)
        .expect("Failed to create PROJ transformation");

    let bbox_3857 = BBox::new(2273080.0, 6204962.0, 2273494.0, 6205186.0); // SMALL
    // let bbox_3857 = BBox::new(2272240.0, 6203413.0, 2274969.0, 6205873.0); // BIG
    // let bbox_3857 = BBox::new(2269316.0, 6199572.0, 2279288.0, 6218237.0); // Plesivecka

    let zoom = 20;

    let tile_size = 256;

    let pixels_per_meter = (((tile_size as u64) << zoom) as f64) / 2.0 / WEB_MERCATOR_EXTENT;

    let buffer_px = 20;

    let buffer_m = buffer_px as f64 / pixels_per_meter; // 10px in m

    let tile_metas: Vec<_> = bbox_covered_tiles(&bbox_3857, zoom)
        .map(|tile| TileMeta {
            tile,
            bbox: tile.bounds(tile_size).to_extended(buffer_m),
            points: Mutex::new(Vec::<PointWithHeight>::new()),
        })
        .collect();

    let bbox_8353: BBox = proj_3857_to_8353
        .transform_bounds(
            bbox_3857.min_x,
            bbox_3857.min_y,
            bbox_3857.max_x,
            bbox_3857.max_y,
            11,
        )
        .unwrap()
        .into();

    let conn = Connection::open("index.sqlite").unwrap();

    let mut stmt = conn.prepare("SELECT file FROM laz_index WHERE max_x >= ?1 AND min_x <= ?3 AND max_y >= ?2 AND min_y <= ?4").unwrap();

    let rows = stmt
        .query_map(<[f64; 4]>::from(bbox_8353), |row| row.get::<_, String>(0))
        .unwrap();

    let files: Vec<String> = rows.map(|row| row.unwrap()).collect();

    println!("Reading {} files", files.len());

    files.par_iter().for_each_init(
        || {
            Proj::new_known_crs("EPSG:8353", "EPSG:3857", None)
                .expect("Failed to create PROJ transformation")
        },
        |proj, file| {
            println!("READ {file}");

            let mut reader = Reader::from_path(file).unwrap();

            for point in reader.points() {
                let point = point.unwrap();

                if point.classification != Classification::Ground {
                    continue;
                }

                if !bbox_8353.contains(point.x, point.y) {
                    continue;
                }

                let (x, y) = proj.convert((point.x, point.y)).unwrap();

                if !bbox_3857.contains(x, y) {
                    continue;
                }

                for (i, tile_meta) in tile_metas.iter().enumerate() {
                    if !tile_meta.bbox.contains(x, y) {
                        continue;
                    }

                    tile_metas
                        .get(i)
                        .unwrap()
                        .points
                        .lock()
                        .unwrap()
                        .push(PointWithHeight {
                            position: Point2::new(x, y),
                            height: point.z,
                        });
                }
            }

            println!("DONE {file}");
        },
    );

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

        let img = crop_imm(
            &img,
            buffer_px,
            buffer_px,
            width_pixels - 2 * buffer_px,
            height_pixels - 2 * buffer_px,
        )
        .to_image();

        let mut buffer = vec![];

        img.write_with_encoder(JpegEncoder::new(Cursor::new(&mut buffer)))
            .unwrap();

        let tile = tile_meta.tile;

        conn.lock()
            .unwrap()
            .execute(
                "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)",
                (tile.zoom, tile.x, tile.reversed_y(), buffer),
            )
            .unwrap();
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
