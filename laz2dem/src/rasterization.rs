use crate::{
    lanczos::resize_lanczos3,
    options::Options,
    progress::Progress,
    schema::create_schema,
    shared_types::{Job, PointWithHeight, Source},
};
use core::f64;
use itertools::{Either, Itertools};
use las::Reader;
use lru::LruCache;
use ndarray::{Array2, s};
use proj::Proj;
use rusqlite::{Connection, Error, ErrorCode, OpenFlags};
use spade::{DelaunayTriangulation, Point2, Triangulation};
use std::{
    io::Cursor,
    num::NonZero,
    sync::{Arc, Mutex},
    thread::{self, available_parallelism},
};
use tilemath::tile::Tile;

const BUFFER_PX: usize = 2;

const SELECT_TILE_EXISTS_SQL: &str =
    "SELECT 1 FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3";

const INSERT_TILE_SQL: &str = "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)";

const SELECT_LAZTILE_SQL: &str = "SELECT data FROM tiles WHERE x = ?1 AND y = ?2";

struct Resize {
    src: usize,
    dest: usize,
    size: usize,
}

pub fn rasterize(options: &Options, r#continue: bool, jobs: Vec<Job>) {
    let output = &options.output;

    {
        let proj_3857_to_4326 = Proj::new_known_crs("EPSG:3857", "EPSG:4326", None)
            .expect("Failed to create PROJ transformation");

        let mut bounds = vec![
            (options.bbox.min_x, options.bbox.min_y),
            (options.bbox.max_x, options.bbox.max_y),
        ];

        proj_3857_to_4326.project_array(&mut bounds, false).unwrap();

        if !r#continue {
            let conn = Connection::open(output).unwrap();

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

    // let conn = Arc::new(Mutex::new(conn));

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

        let for_overviews = Arc::new(Mutex::new(LruCache::<Tile, Array2<f64>>::new(
            NonZero::new(options.lru_size).unwrap(),
        )));

        for _ in 0..(jobs_len.min(available_parallelism().unwrap().get())) {
            let progress = Arc::clone(&progress);

            // let conn = Arc::clone(&conn);

            let for_overviews = Arc::clone(&for_overviews);

            let laztile_conn = laztile_conn.clone();

            scope.spawn(move || {
                let conn = Connection::open(output).unwrap();

                conn.pragma_update(None, "synchronous", "OFF").unwrap();

                conn.pragma_update(None, "journal_mode", "WAL").unwrap();

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
    for_overviews: Arc<Mutex<LruCache<Tile, Array2<f64>>>>,
    conn: Connection,
    laztile_conn: Option<Arc<Mutex<Connection>>>,
}

fn save_tile<'a>(ctx: &Context<'a>, tile: Tile, dem: Array2<f64>) {
    let dem32: Vec<_> = dem.as_slice().unwrap().iter().map(|v| *v as f32).collect();

    let r = lerc::encode(
        &dem32[..],
        None,
        dem.ncols(),
        dem.nrows(),
        1,
        1,
        0,
        0.0, //(0.0005 * (1 << (20 - tile.zoom)) as f64).max(0.001),
    )
    .unwrap();

    // let r: Vec<_> = dem32
    //     .into_iter()
    //     .flat_map(|v| v.to_le_bytes().into_iter())
    //     .collect();

    let buffer = zstd::encode_all(Cursor::new(r), 0).unwrap();

    ctx.for_overviews.lock().unwrap().put(tile, dem);

    let res = ctx.conn.execute(
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

        let mut stmt = ctx.conn.prepare(SELECT_TILE_EXISTS_SQL).unwrap();

        let mut rows = stmt.query((tile.zoom, tile.x, tile.reversed_y())).unwrap();

        if rows.next().unwrap().is_some() {
            for tile in tiles {
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
                    let chunks = {
                        let laztile_conn = laztile_conn.lock().unwrap();

                        let mut stmt = laztile_conn.prepare(SELECT_LAZTILE_SQL).unwrap();

                        let mut rows = stmt.query((tile_meta.tile.x, tile_meta.tile.y)).unwrap();

                        let mut chunks = Vec::new();

                        while let Some(row) = rows.next().unwrap() {
                            chunks.push(row.get::<_, Vec<u8>>(0).unwrap());
                        }

                        chunks
                    };

                    let mut points = Vec::new();

                    for chunk in chunks {
                        let mut reader = Reader::new(Cursor::new(chunk)).unwrap();

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
            let mut for_overviews = ctx.for_overviews.lock().unwrap();

            let children_buffered = tile.children_buffered(1);

            let (children_with_data, children_without_data): (Vec<_>, Vec<_>) = children_buffered
                .enumerate()
                .map(|(sector, tile)| {
                    for_overviews.get(&tile).map_or_else(
                        || Either::Right((sector, tile)),
                        |dem| Either::Left((sector, dem.clone())),
                    )
                })
                .partition_map(|e| e);

            drop(for_overviews);

            let children = if children_without_data.is_empty() {
                children_with_data
            } else {
                let (sql, flat_params): (Vec<_>, Vec<_>) = children_without_data
                    .iter()
                    .enumerate()
                    .map(|(i, (_sector, tile))| {
                        let j = i * 3;
                        (
                            format!(
                                "(zoom_level = ?{} AND tile_column = ?{} AND tile_row = ?{})",
                                j + 1,
                                j + 2,
                                j + 3
                            ),
                            vec![u32::from(tile.zoom), tile.x, tile.reversed_y()],
                        )
                    })
                    .unzip();

                let sql = format!(
                    "SELECT tile_data, zoom_level, tile_column, tile_row FROM tiles WHERE {}",
                    sql.join(" OR ")
                );

                let flat_params: Vec<u32> = flat_params.into_iter().flatten().collect();

                let flat_refs: Vec<&dyn rusqlite::ToSql> = flat_params
                    .iter()
                    .map(|v| v as &dyn rusqlite::ToSql)
                    .collect();

                {
                    let mut stmt = ctx.conn.prepare(&sql).unwrap();

                    stmt.query_map(&flat_refs[..], |row| {
                        let zoom = row.get(1)?;

                        Ok((
                            Tile {
                                zoom,
                                x: row.get(2)?,
                                y: (1u32 << zoom) - 1 - row.get::<_, u32>(3)?,
                            },
                            row.get::<_, Vec<u8>>(0)?,
                        ))
                    })
                    .unwrap()
                    .map(|row| row.unwrap())
                    .collect::<Vec<_>>()
                }
                .into_iter()
                .filter_map(|row| {
                    children_without_data
                        .iter()
                        .find(|(_, tile)| tile == &row.0)
                        .map(|(sector, _)| {
                            let buf = zstd::decode_all(Cursor::new(row.1)).unwrap();

                            assert_eq!(buf.len() % 4, 0, "buffer length must be multiple of 4");

                            let floats: Vec<f64> = buf
                                .chunks_exact(4)
                                .map(|chunk| {
                                    f64::from(f32::from_le_bytes(chunk.try_into().unwrap()))
                                })
                                .collect();

                            let len = floats.len();
                            let dim = (len as f64).sqrt() as usize;
                            assert_eq!(dim * dim, len, "data does not form a square matrix");

                            (*sector, Array2::from_shape_vec((dim, dim), floats).unwrap())
                        })
                })
                .chain(children_with_data)
                .collect::<Vec<_>>()
            };

            let tile_size = options.tile_size as usize;

            let mut dem = Array2::<f64>::zeros([
                (tile_size + BUFFER_PX * 2) * 2,
                (tile_size + BUFFER_PX * 2) * 2,
            ]);

            for (sector, child_dem) in children {
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

                let y = adjust(sector & 3);
                let x = adjust(sector >> 2);

                dem.slice_mut(s![y.dest..(y.dest + y.size), x.dest..(x.dest + x.size)])
                    .assign(&child_dem.slice(s![y.src..y.src + y.size, x.src..x.src + x.size]));
            }

            let dem = resize_lanczos3(&dem, (tile_size + BUFFER_PX * 2, tile_size + BUFFER_PX * 2));

            save_tile(ctx, tile, dem);
        }
    };

    true
}
