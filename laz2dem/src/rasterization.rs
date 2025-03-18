use crate::{
    params::Params,
    progress::Progress,
    schema::create_schema,
    shading::{compute_hillshade, shade},
    shared_types::{Job, PointWithHeight, Shading, Source},
};
use core::f64;
use image::{
    GenericImage, ImageBuffer, Rgb, RgbImage,
    codecs::jpeg::JpegEncoder,
    imageops::{FilterType, crop_imm},
};
use las::Reader;
use maptile::tile::Tile;
use rusqlite::Connection;
use spade::{DelaunayTriangulation, Point2, Triangulation};
use std::{
    collections::HashMap,
    fs::remove_file,
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex},
    thread::{self, available_parallelism},
};

pub fn rasterize(
    output: &Path,
    source: &Source,
    params: &Params,
    jobs: Vec<Job>,
    shading: &[Shading],
) {
    remove_file(output).unwrap_or(());

    let conn = Connection::open(output).unwrap();

    create_schema(
        &conn,
        &[
            ("name", "HS"), // TODO
            ("minzoom", "0"),
            ("maxzoom", params.zoom.to_string().as_ref()),
            ("format", "jpeg"),
            // ("bounds", ...TODO)
        ],
    )
    .unwrap();

    conn.pragma_update(None, "synchronous", "OFF").unwrap();

    conn.pragma_update(None, "journal_mode", "WAL").unwrap();

    let conn = Arc::new(Mutex::new(conn));

    let state = Arc::new(Mutex::new(Progress::new(
        jobs,
        params.supertile_zoom_offset,
    )));

    thread::scope(|scope| {
        let jobs_len = state.lock().unwrap().jobs.len();

        let for_overviews = Arc::new(Mutex::new(HashMap::<Tile, ImageBuffer<_, _>>::new()));

        for _ in 0..(jobs_len.min(available_parallelism().unwrap().get())) {
            let state = Arc::clone(&state);

            let conn = Arc::clone(&conn);

            let for_overviews = Arc::clone(&for_overviews);

            scope.spawn(move || {
                let save_tile = |tile: Tile, img: ImageBuffer<Rgb<u8>, Vec<u8>>| {
                    let mut buffer = vec![];

                    img.write_with_encoder(JpegEncoder::new(Cursor::new(&mut buffer)))
                        .unwrap();

                    for_overviews.lock().unwrap().insert(tile, img);

                    conn.lock()
                        .unwrap()
                        .execute(
                            "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)",
                            (tile.zoom, tile.x, tile.reversed_y(), buffer),
                        )
                        .unwrap();

                    state.lock().unwrap().done(tile);
                };

                loop {
                    let Some(job) = state.lock().unwrap().next() else {
                        break;
                    };

                    match job {
                        Job::Rasterize(tile_meta) => {
                            println!("Processing {:?}", tile_meta.tile);

                            let points = match source {
                                Source::LazTileDb(path_buf) => {
                                    let conn = Connection::open(path_buf).unwrap();

                                    let mut stmt = conn
                                        .prepare("SELECT data FROM tiles WHERE x = ?1 AND y = ?2")
                                        .unwrap();

                                    let mut rows =
                                        stmt.query((tile_meta.tile.x, tile_meta.tile.y)).unwrap();

                                    let mut points = Vec::new();

                                    while let Some(row) = rows.next().unwrap() {
                                        let data: Vec<u8> = row.get(0).unwrap();

                                        let mut reader = Reader::new(Cursor::new(data)).unwrap();

                                        reader.read_all_points_into(&mut points).unwrap();
                                    }

                                    points
                                        .into_iter()
                                        .map(|point| PointWithHeight {
                                            position: Point2 {
                                                x: point.x,
                                                y: point.y,
                                            },
                                            height: point.z,
                                        })
                                        .collect()
                                }
                                Source::LazIndexDb(_) => tile_meta.points.into_inner().unwrap(),
                            };

                            let mut triangulation = DelaunayTriangulation::<PointWithHeight>::new();

                            for point in points {
                                triangulation.insert(point).unwrap();
                            }

                            let bbox_3857 = tile_meta.bbox;

                            let pixels_per_meter = params.pixels_per_meter();

                            let width_pixels =
                                (bbox_3857.width() * pixels_per_meter).round() as u32;

                            let height_pixels =
                                (bbox_3857.height() * pixels_per_meter).round() as u32;

                            let mut img = Vec::new();

                            let natural_neighbor = triangulation.natural_neighbor();

                            for y in 0..height_pixels {
                                let cy = bbox_3857.min_y
                                    + y as f64 * bbox_3857.height() / height_pixels as f64;

                                for x in 0..width_pixels {
                                    let cx = bbox_3857.min_x
                                        + x as f64 * bbox_3857.width() / width_pixels as f64;

                                    img.push(
                                        natural_neighbor
                                            .interpolate(|v| v.data().height, Point2::new(cx, cy))
                                            .unwrap_or(f64::NAN),
                                    );
                                }
                            }

                            // let sun_azimuth_rad = 315_f64.to_radians();
                            // let sun_zenith_rad = 45_f64.to_radians();

                            let img = compute_hillshade(
                                img,
                                height_pixels as usize,
                                width_pixels as usize,
                                |aspect_rad, slope_rad| {
                                    shade(
                                        aspect_rad, slope_rad, shading,
                                        // &[
                                        //     (-120.0, 0.8, 0x203060),
                                        //     (60.0, 0.7, 0xFFEE00),
                                        //     (-45.0, 1.0, 0x000000),
                                        // ],
                                        1.0, 0.0,
                                    )

                                    // // Compute illumination using sun angle
                                    // let illumination = sun_zenith_rad.cos() * slope_rad.cos()
                                    //     + sun_zenith_rad.sin() * slope_rad.sin() * (sun_azimuth_rad - aspect_rad).cos();

                                    // // Scale to 0-255
                                    // let shade = (illumination * 255.0).clamp(0.0, 255.0) as u8;

                                    // [shade, shade, shade]
                                },
                            );

                            let supertile_zoom_offset = params.supertile_zoom_offset;

                            let mut tiles = tile_meta.tile.descendants(supertile_zoom_offset);

                            tiles.sort_by(|a, b| a.y.cmp(&b.y).then_with(|| a.x.cmp(&b.x)));

                            let buffer_px = params.buffer_px;
                            let tile_size = params.tile_size as u32;

                            for (sector, tile) in tiles.iter().enumerate() {
                                let img = crop_imm(
                                    &img,
                                    buffer_px
                                        + ((sector as u32) & ((1 << supertile_zoom_offset) - 1))
                                            * tile_size,
                                    buffer_px
                                        + (sector as u32 >> supertile_zoom_offset) * tile_size,
                                    tile_size,
                                    tile_size,
                                )
                                .to_image();

                                save_tile(*tile, img);
                            }
                        }
                        Job::Overview(tile) => {
                            let mut for_overviews = for_overviews.lock().unwrap();

                            let imgs: Vec<_> = tile
                                .children()
                                .iter()
                                .enumerate()
                                .filter_map(|(i, tile)| {
                                    for_overviews.remove(tile).map(|img| (i, img))
                                })
                                .collect();

                            drop(for_overviews);

                            let mut tile_img = RgbImage::new(
                                u32::from(params.tile_size) << 1,
                                u32::from(params.tile_size) << 1,
                            );

                            for (i, img) in imgs {
                                tile_img
                                    .copy_from(
                                        &img,
                                        ((i & 1) as u32) * params.tile_size as u32,
                                        (i >> 1) as u32 * params.tile_size as u32,
                                    )
                                    .unwrap();
                            }

                            let img = image::imageops::resize(
                                &tile_img,
                                u32::from(params.tile_size),
                                u32::from(params.tile_size),
                                FilterType::Lanczos3,
                            );

                            save_tile(tile, img);
                        }
                    };
                }
            });
        }
    });
}
