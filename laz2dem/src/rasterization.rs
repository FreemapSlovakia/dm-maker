use crate::{
    options::{Format, Options},
    progress::Progress,
    schema::create_schema,
    shading::{SlopeAndAspect, compute_hillshade, compute_slopes_with_aspects, shade},
    shared_types::{Job, PointWithHeight, Source},
};
use core::f64;
use half::f16;
use image::{
    GenericImage, Pixel, Rgb, RgbImage, RgbaImage,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
    imageops::{FilterType, crop_imm, resize},
    load_from_memory_with_format,
};
use las::Reader;
use maptile::tile::Tile;
use ndarray::{Array2, ArrayView2, s};
use proj::Proj;
use rusqlite::{Connection, Error, ErrorCode, OpenFlags};
use spade::{DelaunayTriangulation, Point2, Triangulation};
use std::{
    collections::HashMap,
    io::Cursor,
    sync::{Arc, Mutex},
    thread::{self, available_parallelism},
};

const SELECT_TILE_EXISTS_SQL: &str =
    "SELECT 1 FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3";

const SELECT_TILE_SQL: &str =
    "SELECT tile_data FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3";

const INSERT_TILE_SQL: &str = "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)";

const SELECT_LAZTILE_SQL: &str = "SELECT data FROM tiles WHERE x = ?1 AND y = ?2";

pub fn rasterize(options: &Options, r#continue: bool, jobs: Vec<Job>) {
    let output = &options.output;

    let conn = Connection::open(output).unwrap();

    {
        let proj_3857_to_4326 = Proj::new_known_crs("EPSG:3857", "EPSG:4326", None)
            .expect("Failed to create PROJ transformation");

        let mut bounds = vec![
            (options.bbox.min_x, options.bbox.min_y),
            (options.bbox.max_x, options.bbox.max_y),
        ];

        proj_3857_to_4326.project_array(&mut bounds, false).unwrap();

        if !r#continue {
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

    let progress = Arc::new(Mutex::new(Progress::new(
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
        let jobs_len = progress.lock().unwrap().jobs.len();

        let for_overviews = Arc::new(Mutex::new(HashMap::<Tile, RgbaImage>::new()));

        let for_overviews2 = Arc::new(Mutex::new(HashMap::<Tile, Array2<SlopeAndAspect>>::new()));

        for _ in 0..(jobs_len.min(available_parallelism().unwrap().get())) {
            let progress = Arc::clone(&progress);

            let conn = Arc::clone(&conn);

            let for_overviews = Arc::clone(&for_overviews);

            let for_overviews2 = Arc::clone(&for_overviews2);

            let laztile_conn = laztile_conn.clone();

            scope.spawn(move || {
                let ctx = Context {
                    progress,
                    options,
                    r#continue,
                    supertile_zoom_offset,
                    for_overviews,
                    for_overviews2,
                    conn,
                    laztile_conn,
                };

                while process_single(&ctx) {}
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

struct Context<'a> {
    progress: Arc<Mutex<Progress>>,
    options: &'a Options,
    r#continue: bool,
    supertile_zoom_offset: u8,
    for_overviews: Arc<Mutex<HashMap<Tile, RgbaImage>>>,
    for_overviews2: Arc<Mutex<HashMap<Tile, Array2<SlopeAndAspect>>>>,
    conn: Arc<Mutex<Connection>>,
    laztile_conn: Option<Arc<Mutex<Connection>>>,
}

fn save_tile<'a>(
    ctx: &Context<'a>,
    tile: Tile,
    img: RgbaImage,
    slopes_and_aspects: ArrayView2<SlopeAndAspect>,
) {
    //     let buffer = {
    //         let (slopes, aspects): (Vec<_>, Vec<_>) = slopes_and_aspects
    //             .iter()
    //             .map(|slope_and_aspect| {
    //                 (
    //                     f16::from_f64(slope_and_aspect.slope),
    //                     f16::from_f64(slope_and_aspect.aspect),
    //                 )
    //             })
    //             .unzip();

    //         let values: Vec<_> = slopes
    //             .iter()
    //             .flat_map(|slope| slope.to_be_bytes().to_vec())
    //             .chain(
    //                 aspects
    //                     .iter()
    //                     .flat_map(|aspect| aspect.to_be_bytes().to_vec()),
    //             )
    //             .collect();

    //         zstd::encode_all(Cursor::new(values), 0).unwrap()
    //     };

    ctx.for_overviews2
        .lock()
        .unwrap()
        .insert(tile, slopes_and_aspects.to_owned());

    let mut buffer = vec![];

    match ctx.options.format {
        Format::JPEG => {
            let img = rgba_to_rgb(&img, ctx.options.background_color.0);
            img.write_with_encoder(JpegEncoder::new_with_quality(
                Cursor::new(&mut buffer),
                ctx.options.jpeg_quality,
            ))
            .unwrap()
        }
        Format::PNG => img
            .write_with_encoder(PngEncoder::new(Cursor::new(&mut buffer)))
            .unwrap(),
    }

    ctx.for_overviews.lock().unwrap().insert(tile, img);

    let res = ctx.conn.lock().unwrap().execute(
        INSERT_TILE_SQL,
        (tile.zoom, tile.x, tile.reversed_y(), buffer),
    );

    match res {
        Err(Error::SqliteFailure(ref err, _)) if err.code == ErrorCode::ConstraintViolation => {
            println!("DUPLICATE");
        }
        _ => {
            res.unwrap();
        }
    }

    ctx.progress.lock().unwrap().done(tile);
}

fn process_single<'a>(ctx: &Context<'a>) -> bool {
    let progress = &ctx.progress;

    let Some(job) = progress.lock().unwrap().next() else {
        return false;
    };

    let options = ctx.options;

    // println!("Processing {:?}", job);

    if ctx.r#continue {
        let (tile, tiles) = match job {
            Job::Rasterize(ref tile_meta) => (
                tile_meta.tile,
                tile_meta.tile.descendants(ctx.supertile_zoom_offset),
            ),
            Job::Overview(tile) => (tile, vec![tile]),
        };

        let conn = ctx.conn.lock().unwrap();

        let mut stmt = conn.prepare(SELECT_TILE_EXISTS_SQL).unwrap();

        let mut rows = stmt.query((tile.zoom, tile.x, tile.reversed_y())).unwrap();

        if rows.next().unwrap().is_some() {
            for tile in tiles {
                ctx.for_overviews
                    .lock()
                    .unwrap()
                    .insert(tile, RgbaImage::default());

                progress.lock().unwrap().done(tile);
            }

            return true;
        }
    }

    match job {
        Job::Rasterize(tile_meta) => {
            let points = ctx.laztile_conn.as_ref().map_or_else(
                || tile_meta.points.into_inner().unwrap(),
                |laztile_conn| {
                    let laztile_conn = laztile_conn.lock().unwrap();

                    let mut stmt = laztile_conn.prepare(SELECT_LAZTILE_SQL).unwrap();

                    let mut rows = stmt.query((tile_meta.tile.x, tile_meta.tile.y)).unwrap();

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
                progress.lock().unwrap().done(tile_meta.tile);

                return true;
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
                let cy = bbox.min_y + y as f64 * bbox.height() / height_pixels as f64;

                for x in 0..width_pixels {
                    let cx = bbox.min_x + x as f64 * bbox.width() / width_pixels as f64;

                    img.push(
                        natural_neighbor
                            .interpolate(|v| v.data().height, Point2::new(cx, cy))
                            .unwrap_or(f64::NAN),
                    );
                }
            }

            let slopes_and_aspects = compute_slopes_with_aspects(
                &img,
                options.z_factor,
                height_pixels as usize,
                width_pixels as usize,
            );

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

            let mut tiles = tile_meta.tile.descendants(ctx.supertile_zoom_offset);

            tiles.sort_by(|a, b| a.y.cmp(&b.y).then_with(|| a.x.cmp(&b.x)));

            let buffer_px = options.buffer;
            let tile_size = options.tile_size as u32;

            for (sector, tile) in tiles.iter().enumerate() {
                let x = buffer_px
                    + ((sector as u32) & ((1 << ctx.supertile_zoom_offset) - 1)) * tile_size;

                let y = buffer_px + (sector as u32 >> ctx.supertile_zoom_offset) * tile_size;

                let slice: ArrayView2<_> = slopes_and_aspects.slice(s![
                    x as usize..(x + tile_size) as usize,
                    y as usize..(y + tile_size) as usize
                ]);

                let img = crop_imm(&img, x, y, tile_size, tile_size).to_image();

                save_tile(ctx, *tile, img, slice);
            }
        }
        Job::Overview(tile) => {
            let mut for_overviews = ctx.for_overviews.lock().unwrap();

            let imgs: Vec<_> = tile
                .children()
                .into_iter()
                .enumerate()
                .filter_map(|(i, tile)| for_overviews.remove(&tile).map(|img| (i, tile, img)))
                .collect();

            drop(for_overviews);

            if imgs.is_empty() {
                progress.lock().unwrap().done(tile);

                return true;
            }

            let mut tile_img = RgbaImage::new(
                u32::from(options.tile_size) << 1,
                u32::from(options.tile_size) << 1,
            );

            // for (i, tile, img) in imgs {
            //     let img = if ctx.r#continue && img.width() == 0 {
            //         let data: Vec<u8> = {
            //             let conn = ctx.conn.lock().unwrap();

            //             let mut stmt = conn.prepare(SELECT_TILE_SQL).unwrap();

            //             let mut rows = stmt.query((tile.zoom, tile.x, tile.reversed_y())).unwrap();

            //             let row = rows.next().unwrap().unwrap();

            //             row.get(0).unwrap()
            //         };

            //         load_from_memory_with_format(
            //             data.as_slice(),
            //             image::ImageFormat::Jpeg, // TODO also support PNG
            //         )
            //         .unwrap()
            //         .to_rgba8()
            //     } else {
            //         img
            //     };

            //     tile_img
            //         .copy_from(
            //             &img,
            //             ((i & 1) as u32) * options.tile_size as u32,
            //             (i >> 1) as u32 * options.tile_size as u32,
            //         )
            //         .unwrap();
            // }

            // let img = resize(
            //     &tile_img,
            //     u32::from(options.tile_size),
            //     u32::from(options.tile_size),
            //     FilterType::Lanczos3,
            // );

            let mut for_overviews2 = ctx.for_overviews2.lock().unwrap();

            let imgs: Vec<_> = tile;
            l.children()
                .into_iter()
                .enumerate()
                .filter_map(|(i, tile)| for_overviews2.remove(&tile).map(|img| (i, tile, img)))
                .collect();

            drop(for_overviews2);

            save_tile(ctx, tile, img);
        }
    };

    true
}
