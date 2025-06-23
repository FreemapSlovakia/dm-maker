use crate::{
    options::{ExistingFileAction, Format, Options},
    progress::Progress,
    schema::create_schema,
    shading::{compute_hillshade, shade},
    shared_types::{Job, PointWithHeight, Source},
};
use core::f64;
use image::{
    GenericImage, Pixel, Rgb, RgbImage, RgbaImage,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
    imageops::{FilterType, crop_imm, resize},
    load_from_memory_with_format,
};
use las::Reader;
use proj::Proj;
use rusqlite::{Connection, Error, ErrorCode, OpenFlags};
use spade::{DelaunayTriangulation, Point2, Triangulation};
use std::{
    collections::HashMap,
    fs::{exists, remove_file},
    io::Cursor,
    sync::{Arc, Mutex},
    thread::{self, available_parallelism},
};
use tilemath::tile::Tile;

const SELECT_TILE_EXISTS_SQL: &str =
    "SELECT 1 FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3";

const SELECT_TILE_SQL: &str =
    "SELECT tile_data FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3";

const INSERT_TILE_SQL: &str = "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)";

const SELECT_LAZTILE_SQL: &str = "SELECT data FROM tiles WHERE x = ?1 AND y = ?2";

pub fn rasterize(options: &Options, jobs: Vec<Job>) {
    let output = &options.output;

    let cont = exists(output).unwrap()
        && match options.existing_file_action {
            Some(ExistingFileAction::Overwrite) => {
                remove_file(output).unwrap();

                false
            }
            Some(ExistingFileAction::Continue) => true,
            None => panic!("Output file already exitsts. Specify --existing-file-action."),
        };

    let conn = Connection::open(output).unwrap();

    {
        let proj_3857_to_4326 = Proj::new_known_crs("EPSG:3857", "EPSG:4326", None)
            .expect("Failed to create PROJ transformation");

        let mut bounds = vec![
            (options.bbox.min_x, options.bbox.min_y),
            (options.bbox.max_x, options.bbox.max_y),
        ];

        proj_3857_to_4326.project_array(&mut bounds, false).unwrap();

        if !cont {
            create_schema(
                &conn,
                &[
                    ("name", "Hillshade"), // TODO
                    ("minzoom", "0"),
                    ("maxzoom", options.zoom_level.to_string().as_ref()),
                    ("format", &options.format.to_string()),
                    (
                        "bounds",
                        &format!(
                            "{},{},{},{}",
                            bounds[0].0, bounds[0].1, bounds[1].0, bounds[1].1
                        ),
                    ),
                ],
            )
            .unwrap();
        }
    }

    conn.pragma_update(None, "synchronous", "OFF").unwrap();

    conn.pragma_update(None, "journal_mode", "WAL").unwrap();

    let conn = Arc::new(Mutex::new(conn));

    let state = Arc::new(Mutex::new(Progress::new(
        jobs,
        options.zoom_level - options.unit_zoom_level,
    )));

    let laztile_conn = match options.source() {
        Source::LazTileDb(path_buf) => Some(Arc::new(Mutex::new(
            Connection::open_with_flags(path_buf, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap(),
        ))),
        Source::LazIndexDb(_) => None,
    };

    let supertile_zoom_offset = options.zoom_level - options.unit_zoom_level;

    thread::scope(|scope| {
        let jobs_len = state.lock().unwrap().jobs.len();

        let for_overviews = Arc::new(Mutex::new(HashMap::<Tile, RgbaImage>::new()));

        for _ in 0..(jobs_len.min(available_parallelism().unwrap().get())) {
            let state = Arc::clone(&state);

            let conn = Arc::clone(&conn);

            let for_overviews = Arc::clone(&for_overviews);

            let laztile_conn = laztile_conn.clone();

            scope.spawn(move || {
                let save_tile = |tile: Tile, img: RgbaImage| {
                    let mut buffer = vec![];

                    match options.format {
                        Format::JPEG => {
                            let img = rgba_to_rgb(&img, options.background_color.0);
                            img.write_with_encoder(JpegEncoder::new_with_quality(
                                Cursor::new(&mut buffer),
                                options.jpeg_quality,
                            ))
                            .unwrap()
                        }
                        Format::PNG => img
                            .write_with_encoder(PngEncoder::new(Cursor::new(&mut buffer)))
                            .unwrap(),
                    }

                    for_overviews.lock().unwrap().insert(tile, img);

                    let res = conn.lock().unwrap().execute(
                        INSERT_TILE_SQL,
                        (tile.zoom, tile.x, tile.reversed_y(), buffer),
                    );

                    match res {
                        Err(Error::SqliteFailure(ref err, _))
                            if err.code == ErrorCode::ConstraintViolation =>
                        {
                            println!("DUPLICATE");
                        }
                        _ => {
                            res.unwrap();
                        }
                    }

                    state.lock().unwrap().done(tile);
                };

                loop {
                    let Some(job) = state.lock().unwrap().next() else {
                        break;
                    };

                    // println!("Processing {:?}", job);

                    if cont {
                        let (tile, tiles) = match job {
                            Job::Rasterize(ref tile_meta) => (
                                tile_meta.tile,
                                tile_meta.tile.descendants(supertile_zoom_offset),
                            ),
                            Job::Overview(tile) => (tile, vec![tile]),
                        };

                        let conn = conn.lock().unwrap();

                        let mut stmt = conn.prepare(SELECT_TILE_EXISTS_SQL).unwrap();

                        let mut rows = stmt.query((tile.zoom, tile.x, tile.reversed_y())).unwrap();

                        if rows.next().unwrap().is_some() {
                            for tile in tiles {
                                for_overviews
                                    .lock()
                                    .unwrap()
                                    .insert(tile, RgbaImage::default());

                                state.lock().unwrap().done(tile);
                            }

                            continue;
                        }
                    }

                    match job {
                        Job::Rasterize(tile_meta) => {
                            let points = laztile_conn.as_ref().map_or_else(
                                || tile_meta.points.into_inner().unwrap(),
                                |laztile_conn| {
                                    let laztile_conn = laztile_conn.lock().unwrap();

                                    let mut stmt =
                                        laztile_conn.prepare(SELECT_LAZTILE_SQL).unwrap();

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
                                },
                            );

                            if points.is_empty() {
                                state.lock().unwrap().done(tile_meta.tile);

                                continue;
                            }

                            let mut triangulation = DelaunayTriangulation::<PointWithHeight>::new();

                            for point in points {
                                triangulation.insert(point).unwrap();
                            }

                            let bbox = tile_meta.bbox;

                            let pixels_per_meter = options.pixels_per_meter();

                            let width_pixels = (bbox.width() * pixels_per_meter).round() as u32;

                            let height_pixels = (bbox.height() * pixels_per_meter).round() as u32;

                            let mut img = Vec::new();

                            let natural_neighbor = triangulation.natural_neighbor();

                            for y in 0..height_pixels {
                                let cy =
                                    bbox.min_y + y as f64 * bbox.height() / height_pixels as f64;

                                for x in 0..width_pixels {
                                    let cx =
                                        bbox.min_x + x as f64 * bbox.width() / width_pixels as f64;

                                    img.push(
                                        natural_neighbor
                                            .interpolate(|v| v.data().height, Point2::new(cx, cy))
                                            .unwrap_or(f64::NAN),
                                    );
                                }
                            }

                            let img = compute_hillshade(
                                &img,
                                options.z_factor,
                                height_pixels as usize,
                                width_pixels as usize,
                                |aspect, slope| {
                                    shade(
                                        aspect,
                                        slope,
                                        options.shadings.0.as_ref(),
                                        options.contrast,
                                        options.brightness,
                                    )
                                },
                            );

                            let mut tiles = tile_meta.tile.descendants(supertile_zoom_offset);

                            tiles.sort_by(|a, b| a.y.cmp(&b.y).then_with(|| a.x.cmp(&b.x)));

                            let buffer_px = options.buffer;
                            let tile_size = options.tile_size as u32;

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
                                .into_iter()
                                .enumerate()
                                .filter_map(|(i, tile)| {
                                    for_overviews.remove(&tile).map(|img| (i, tile, img))
                                })
                                .collect();

                            drop(for_overviews);

                            if imgs.is_empty() {
                                state.lock().unwrap().done(tile);

                                continue;
                            }

                            let mut tile_img = RgbaImage::new(
                                u32::from(options.tile_size) << 1,
                                u32::from(options.tile_size) << 1,
                            );

                            for (i, tile, img) in imgs {
                                let img = if cont && img.width() == 0 {
                                    let data: Vec<u8> = {
                                        let conn = conn.lock().unwrap();

                                        let mut stmt = conn.prepare(SELECT_TILE_SQL).unwrap();

                                        let mut rows = stmt
                                            .query((tile.zoom, tile.x, tile.reversed_y()))
                                            .unwrap();

                                        let row = rows.next().unwrap().unwrap();

                                        row.get(0).unwrap()
                                    };

                                    load_from_memory_with_format(
                                        data.as_slice(),
                                        image::ImageFormat::Jpeg, // TODO also support PNG
                                    )
                                    .unwrap()
                                    .to_rgba8()
                                } else {
                                    img
                                };

                                tile_img
                                    .copy_from(
                                        &img,
                                        ((i & 1) as u32) * options.tile_size as u32,
                                        (i >> 1) as u32 * options.tile_size as u32,
                                    )
                                    .unwrap();
                            }

                            let img = resize(
                                &tile_img,
                                u32::from(options.tile_size),
                                u32::from(options.tile_size),
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

fn rgba_to_rgb(img: &RgbaImage, background: Rgb<u8>) -> RgbImage {
    let (width, height) = img.dimensions();

    let mut rgb_img = RgbImage::new(width, height);

    for (x, y, &rgba) in img.enumerate_pixels() {
        let mut base = background.to_rgba();

        base.channels_mut()[3] = 255;

        base.blend(&rgba);

        rgb_img.put_pixel(x, y, base.to_rgb());
    }

    rgb_img
}
