use crate::{
    lanczos::resize_lanczos3,
    options::Options,
    progress::Progress,
    schema::create_schema,
    shared_types::{Job, PointWithHeight, Source},
};
use core::f64;
use las::Reader;
use maptile::tile::Tile;
use ndarray::{Array2, s};
use proj::Proj;
use rusqlite::{Connection, Error, ErrorCode, OpenFlags};
use spade::{DelaunayTriangulation, Point2, Triangulation};
use std::{
    collections::HashMap,
    io::Cursor,
    sync::{Arc, Mutex},
    thread::{self, available_parallelism},
};

const BUFFER_PX: usize = 2;

const SELECT_TILE_EXISTS_SQL: &str =
    "SELECT 1 FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3";

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
                    ("format", "application/x-zstd-f16-grid"),
                    ("minzoom", "0"),
                    ("maxzoom", options.zoom_level.to_string().as_ref()),
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
        1,
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

        let for_overviews = Arc::new(Mutex::new(HashMap::<Tile, Array2<f64>>::new()));

        for _ in 0..(jobs_len.min(available_parallelism().unwrap().get())) {
            let progress = Arc::clone(&progress);

            let conn = Arc::clone(&conn);

            let for_overviews = Arc::clone(&for_overviews);

            let laztile_conn = laztile_conn.clone();

            scope.spawn(move || {
                let ctx = Context {
                    progress,
                    options,
                    r#continue,
                    supertile_zoom_offset,
                    for_overviews,
                    conn,
                    laztile_conn,
                };

                while process_single(&ctx) {}
            });
        }
    });

    progress.lock().unwrap().print_stats();
}

struct Context<'a> {
    progress: Arc<Mutex<Progress>>,
    options: &'a Options,
    r#continue: bool,
    supertile_zoom_offset: u8,
    for_overviews: Arc<Mutex<HashMap<Tile, Array2<f64>>>>,
    conn: Arc<Mutex<Connection>>,
    laztile_conn: Option<Arc<Mutex<Connection>>>,
}

fn save_tile<'a>(ctx: &Context<'a>, tile: Tile, dem: Array2<f64>) {
    let r: Vec<_> = dem
        .clone()
        .into_iter()
        .flat_map(|value| (value as f32).to_le_bytes())
        .collect();

    let buffer = zstd::encode_all(Cursor::new(r), 0).unwrap();

    ctx.for_overviews.lock().unwrap().insert(tile, dem);

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

    // if ctx.r#continue {
    //     let (tile, tiles) = match job {
    //         Job::Rasterize(ref tile_meta) => (
    //             tile_meta.tile,
    //             tile_meta.tile.descendants(ctx.supertile_zoom_offset),
    //         ),
    //         Job::Overview(tile) => (tile, vec![tile]),
    //     };

    //     let conn = ctx.conn.lock().unwrap();

    //     let mut stmt = conn.prepare(SELECT_TILE_EXISTS_SQL).unwrap();

    //     let mut rows = stmt.query((tile.zoom, tile.x, tile.reversed_y())).unwrap();

    //     if rows.next().unwrap().is_some() {
    //         for tile in tiles {
    //             ctx.for_overviews
    //                 .lock()
    //                 .unwrap()
    //                 .insert(tile, Array2::<f64>::zeros([0, 0])); // TODO

    //             progress.lock().unwrap().done(tile);
    //         }

    //         return true;
    //     }
    // }

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

            let width_pixels = (bbox.width() * pixels_per_meter).round() as usize;

            let height_pixels = (bbox.height() * pixels_per_meter).round() as usize;

            let mut elevations = Array2::<f64>::zeros([height_pixels, width_pixels]);

            let natural_neighbor = triangulation.natural_neighbor();

            for y in 0..height_pixels {
                let cy = bbox.min_y + y as f64 * bbox.height() / height_pixels as f64;

                for x in 0..width_pixels {
                    let cx = bbox.min_x + x as f64 * bbox.width() / width_pixels as f64;

                    elevations[[height_pixels - y - 1, x]] = natural_neighbor
                        .interpolate(|v| v.data().height, Point2::new(cx, cy))
                        .unwrap_or(f64::NAN);
                }
            }

            let mut tiles = tile_meta.tile.descendants(ctx.supertile_zoom_offset);

            tiles.sort_by(|a, b| a.y.cmp(&b.y).then_with(|| a.x.cmp(&b.x)));

            let buffer_px = options.buffer as usize;

            let tile_size = options.tile_size as usize;

            for (sector, tile) in tiles.iter().enumerate() {
                let x = buffer_px + ((sector) & ((1 << ctx.supertile_zoom_offset) - 1)) * tile_size;

                let y = buffer_px + (sector >> ctx.supertile_zoom_offset) * tile_size;

                let slice = elevations
                    .slice(s![
                        (y - BUFFER_PX)..(y + tile_size + BUFFER_PX),
                        (x - BUFFER_PX)..(x + tile_size + BUFFER_PX)
                    ])
                    .to_owned();

                save_tile(ctx, *tile, slice);
            }
        }
        Job::Overview(tile) => {
            let for_overviews = ctx.for_overviews.lock().unwrap();

            let children: Vec<_> = tile
                .children_buffered(1)
                .enumerate()
                .filter_map(|(i, tile)| for_overviews.get(&tile).map(|img| (i, tile, img.clone())))
                .collect();

            // TODO missing deleting from `for_overviews`

            drop(for_overviews);

            let tile_size = options.tile_size as usize;

            let mut img = Array2::<f64>::zeros([
                (tile_size + BUFFER_PX * 2) * 2,
                (tile_size + BUFFER_PX * 2) * 2,
            ]);

            struct Resize {
                src: usize,
                dest: usize,
                size: usize,
            }

            for (i, _, sub) in children {
                let adjust = |c: usize| match c {
                    0 => Resize {
                        dest: 0,
                        src: BUFFER_PX + tile_size - 2 * BUFFER_PX,
                        size: 2 * BUFFER_PX,
                    },
                    1 => Resize {
                        dest: 2 * BUFFER_PX,
                        src: BUFFER_PX,
                        size: tile_size,
                    },
                    2 => Resize {
                        dest: 2 * BUFFER_PX + tile_size,
                        src: BUFFER_PX,
                        size: tile_size,
                    },
                    3 => Resize {
                        dest: 2 * BUFFER_PX + 2 * tile_size,
                        src: BUFFER_PX,
                        size: 2 * BUFFER_PX,
                    },
                    _ => panic!("out of range"),
                };

                let y = adjust(i & 3);
                let x = adjust(i >> 2);

                img.slice_mut(s![y.dest..(y.dest + y.size), x.dest..(x.dest + x.size)])
                    .assign(&sub.slice(s![y.src..y.src + y.size, x.src..x.src + x.size]));
            }

            let img = resize_lanczos3(&img, (tile_size + BUFFER_PX * 2, tile_size + BUFFER_PX * 2));

            if tile
                == (Tile {
                    zoom: 18,
                    x: 145915,
                    y: 90174,
                })
            {
                println!(
                    "{} {} {} {} {}",
                    tile,
                    img[[0, 0]],
                    img[[0, BUFFER_PX + tile_size + BUFFER_PX - 1]],
                    img[[BUFFER_PX + tile_size + BUFFER_PX - 1, 0]],
                    img[[
                        BUFFER_PX + tile_size + BUFFER_PX - 1,
                        BUFFER_PX + tile_size + BUFFER_PX - 1
                    ]]
                );
            }

            save_tile(ctx, tile, img);
        }
    };

    true
}
