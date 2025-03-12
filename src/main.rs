mod index;

use image::{RgbImage, codecs::jpeg::JpegEncoder};
use las::{Reader, point::Classification};
use maptile::{bbox::BBox, constants::WEB_MERCATOR_EXTENT, utils::bbox_covered_tiles};
use proj::Proj;
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use rusqlite::Connection;
use spade::{DelaunayTriangulation, HasPosition, Point2, Triangulation};
use std::{
    fs::File,
    io::{Write, stdout},
    sync::{Mutex, atomic::AtomicU64},
};

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

fn render() {
    let proj_3857_to_8353 = Proj::new_known_crs("EPSG:3857", "EPSG:8353", None)
        .expect("Failed to create PROJ transformation");

    // let bbox_3857 = BBox::new(2273080.0, 6204962.0, 2273494.0, 6205186.0);
    let bbox_3857 = BBox::new(2272240.0, 6203413.0, 2274969.0, 6205873.0); // BIG

    let zoom = 20;

    let tile_size = 256;

    let tiles: Vec<_> = bbox_covered_tiles(&bbox_3857, zoom)
        .map(|f| {
            (
                f,
                f.bounds(tile_size),
                Mutex::new(Vec::<PointWithHeight>::new()),
            )
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

    println!("{:?}", bbox_8353);

    let mut stmt = conn.prepare("SELECT file FROM laz_index WHERE max_x >= ?1 AND min_x <= ?3 AND max_y >= ?2 AND min_y <= ?4").unwrap();

    let rows = stmt
        .query_map(<[f64; 4]>::from(bbox_8353), |row| row.get::<_, String>(0))
        .unwrap();

    let files: Vec<String> = rows.map(|row| row.unwrap()).collect();

    println!("Reading {} files", files.len());

    let count = AtomicU64::new(0);

    files.par_iter().for_each_init(
        || {
            Proj::new_known_crs("EPSG:8353", "EPSG:3857", None)
                .expect("Failed to create PROJ transformation")
        },
        |proj, file| {
            println!("READ {file}");

            let mut reader = Reader::from_path(file).unwrap();

            // let mut batch = Vec::new();

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

                if count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 1_000_000 == 0 {
                    print!(".");
                    stdout().flush().unwrap();
                }

                for (i, tile) in tiles.iter().enumerate() {
                    if tile.1.contains(x, y) {
                        tiles
                            .get(i)
                            .unwrap()
                            .2
                            .lock()
                            .unwrap()
                            .push(PointWithHeight {
                                position: Point2::new(x, y),
                                height: point.z,
                            });
                    }
                }

                // t.lock()
                //     .unwrap()
                //     .insert(PointWithHeight {
                //         position: Point2::new(x, y),
                //         height: point.z,
                //     })
                //     .unwrap();
            }

            // println!("INSERT {} {file}", batch.len());

            // let _ = dt
            //     .lock()
            //     .unwrap()
            //     .insert(&batch, startin::InsertionStrategy::BBox);

            println!("DONE {file}");
        },
    );

    println!(
        "{}",
        count.fetch_add(0, std::sync::atomic::Ordering::Relaxed)
    );

    let pixels_per_meter = (((tile_size as u64) << zoom) as f64) / 2.0 / WEB_MERCATOR_EXTENT;

    tiles.into_par_iter().for_each(|tile| {
        println!("Processing {:?}", tile.0);

        let mut t = DelaunayTriangulation::<PointWithHeight>::new();

        let points = tile.2.into_inner().unwrap();

        for point in points {
            t.insert(point).unwrap();
        }

        let bbox_3857 = tile.1;

        let width_pixels = (bbox_3857.width() * pixels_per_meter).round() as u32;
        let height_pixels = (bbox_3857.height() * pixels_per_meter).round() as u32;

        println!("Interpolating {width_pixels}x{height_pixels}");

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

        let img = compute_hillshade(
            img,
            height_pixels as usize,
            width_pixels as usize,
            315.0,
            45.0,
        );

        img.write_with_encoder(JpegEncoder::new(
            File::create(format!("out-{}-{}.jpg", tile.0.x, tile.0.y)).unwrap(),
        ))
        .unwrap();
    });
}

fn compute_hillshade(
    elevation: Vec<f64>,
    rows: usize,
    cols: usize,
    sun_azimuth: f64,
    sun_zenith: f64,
) -> RgbImage {
    let mut hillshade = RgbImage::new(cols as u32, rows as u32);

    let sun_azimuth_rad = sun_azimuth.to_radians();
    let sun_zenith_rad = sun_zenith.to_radians();

    for y in 1..rows - 1 {
        let off = y * cols;

        for x in 1..cols - 1 {
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
            let dz_dx =
                (-1.0 * z1 + 1.0 * z3 + -2.0 * z4 + 2.0 * z6 + -1.0 * z7 + 1.0 * z9) / 8.0 * 2.0;

            let dz_dy =
                (-1.0 * z1 - 2.0 * z2 - 1.0 * z3 + 1.0 * z7 + 2.0 * z8 + 1.0 * z9) / 8.0 * 2.0;

            // Compute slope
            let slope_rad = (dz_dx.powi(2) + dz_dy.powi(2)).sqrt().atan();

            // Compute aspect
            let mut aspect_rad = dz_dy.atan2(-dz_dx); // Negative sign because of coordinate convention
            if aspect_rad < 0.0 {
                aspect_rad += std::f64::consts::TAU; // Convert to 0 - 2π range
            }

            // Compute illumination using sun angle
            let illumination = sun_zenith_rad.cos() * slope_rad.cos()
                + sun_zenith_rad.sin() * slope_rad.sin() * (sun_azimuth_rad - aspect_rad).cos();

            // Scale to 0-255
            let shade = (illumination * 255.0).clamp(0.0, 255.0) as u8;
            hillshade.get_pixel_mut(x as u32, (rows - y) as u32).0 = [shade, shade, shade];
        }
    }

    hillshade
}
