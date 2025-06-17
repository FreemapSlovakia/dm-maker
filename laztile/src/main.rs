use clap::Parser;
use las::{
    Builder, Point, Reader, Transform, Vector, Writer,
    point::{Classification, Format},
};
use proj::Proj;
use rusqlite::Connection;
use std::{
    collections::{HashMap, HashSet},
    io::{BufReader, Cursor, Read},
    path::Path,
    sync::{Arc, Mutex},
    thread::{self, available_parallelism},
};
use std::{fs::File, path::PathBuf};
use tilemath::tile::{Tile, mercator_to_tile_coords};
use walkdir::WalkDir;
use zip::ZipArchive;

#[derive(Parser, Debug, PartialEq)]
struct Options {
    #[clap(long, default_value_t = false)]
    r#continue: bool,

    /// Directory with *.laz files
    directory: PathBuf, // "/home/martin/18TB"

    /// Output database file
    database: PathBuf, // "/home/martin/14TB/sk-new-dmr/laztiles.sqlite"

    /// Source projection; default is EPSG:3857
    #[clap(long)]
    source_projection: Option<String>,

    /// Zoom level of a tile
    #[clap(long, default_value_t = 16)]
    zoom_level: u8,

    /// Buffer in mercator meters
    #[clap(long, default_value_t = 30.0)]
    buffer: f64,
}

fn main() {
    let options = Options::parse();

    if options.r#continue && !options.database.exists() {
        panic!("Database file doesn't exist");
    }

    if !options.r#continue && options.database.exists() {
        panic!("Database file already exists");
    }

    let conn = Connection::open(options.database).unwrap();

    conn.pragma_update(None, "synchronous", "OFF").unwrap();

    conn.pragma_update(None, "journal_mode", "WAL").unwrap();

    if !options.r#continue {
        conn.execute(
            "CREATE TABLE tiles (x NUMBER, y NUMBER, laz_id INTEGER PRIMARY KEY AUTOINCREMENT, data BLOB)",
            (),
        )
        .unwrap();

        conn.execute("CREATE TABLE processed_file (name VARCHAR PRIMARY KEY)", ())
            .unwrap();
    }

    let conn = Arc::new(Mutex::new(conn));

    let laz_iter = WalkDir::new(options.directory).into_iter();

    let laz_iter = Arc::new(Mutex::new(laz_iter));

    let source_projection = options.source_projection;

    let zoom_level = options.zoom_level;

    let buffer = options.buffer;

    thread::scope(|scope| {
        for _ in 0..available_parallelism().unwrap().get() {
            let conn = Arc::clone(&conn);

            let laz_iter = Arc::clone(&laz_iter);

            let source_projection = source_projection.clone();

            scope.spawn(move || {
                let proj = source_projection.map(|proj| {
                    Proj::new_known_crs(proj.as_ref(), "EPSG:3857", None)
                        .expect("Failed to create PROJ transformation")
                });

                'main: loop {
                    if Path::new("STOP").exists() {
                        break;
                    }

                    let mut lock = laz_iter.lock().unwrap();

                    let Some(file) = lock.next() else {
                        break;
                    };

                    drop(lock);

                    let file = file.unwrap();

                    if !file
                        .path()
                        .extension()
                        .map(|ext| ext == "zip")
                        .unwrap_or(false)
                    {
                        continue;
                    }

                    let file_name = file
                        .path()
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();

                    if conn
                        .lock()
                        .unwrap()
                        .prepare("SELECT COUNT(*) FROM processed_file WHERE name = ?1")
                        .unwrap()
                        .query([&file_name])
                        .unwrap()
                        .next()
                        .unwrap()
                        .unwrap()
                        .get::<_, u32>(0)
                        .unwrap()
                        > 0
                    {
                        println!("ALREADY PROCESSED {file_name}");

                        continue;
                    }

                    println!("START {file_name}");

                    let reader = BufReader::new(File::open(file.path()).unwrap());

                    let mut zip = ZipArchive::new(reader).unwrap();

                    let mut f = 'found: {
                        for i in 0..zip.len() {
                            let file = zip.by_index(i).unwrap();

                            let name = file.name().to_lowercase();

                            if name.ends_with(".las") || name.ends_with(".laz") {
                                break 'found file;
                            }
                        }

                        eprint!("no laz/las in {}", file.file_name().to_string_lossy());

                        continue 'main;
                    };

                    let mut buf = Vec::new();
                    f.read_to_end(&mut buf).unwrap();
                    let cursor = Cursor::new(buf);

                    let mut reader = Reader::new(cursor).unwrap();

                    let mut map = HashMap::new();

                    for point in reader.points() {
                        let point = point.unwrap();

                        // if !matches!(
                        //     point.classification,
                        //     Classification::Ground
                        //         | Classification::BridgeDeck
                        //         | Classification::Water
                        //         | Classification::Building
                        //         | Classification::Rail
                        //         | Classification::RoadSurface
                        // ) {
                        //     continue;
                        // }

                        let (x, y) = proj.as_ref().map_or_else(
                            || (point.x, point.y),
                            |proj| proj.convert((point.x, point.y)).unwrap(),
                        );

                        let tile_coords: HashSet<_> = (0..4)
                            .map(|sector| {
                                mercator_to_tile_coords(
                                    x + (((sector >> 1) << 1) as f64 - 1.0) * buffer,
                                    y + (((sector & 1) << 1) as f64 - 1.0) * buffer,
                                    zoom_level,
                                )
                            })
                            .collect();

                        for tile_coord in tile_coords {
                            map.entry(tile_coord)
                                .or_insert_with(|| {
                                    let mut builder = Builder::from((1, 4));

                                    let tile = Tile {
                                        x: tile_coord.0,
                                        y: tile_coord.1,
                                        zoom: zoom_level,
                                    };

                                    let bounds = tile.bounds(256);

                                    builder.point_format = Format::new(0).unwrap();

                                    builder.point_format.is_compressed = true;

                                    builder.transforms = Vector {
                                        x: Transform {
                                            scale: 0.001,
                                            offset: bounds.min_x - buffer,
                                        },
                                        y: Transform {
                                            scale: 0.001,
                                            offset: bounds.min_y - buffer,
                                        },
                                        z: Transform {
                                            scale: 0.001,
                                            offset: 0.0,
                                        },
                                    };

                                    Writer::new(
                                        Cursor::new(Vec::new()),
                                        builder.into_header().unwrap(),
                                    )
                                    .unwrap()
                                })
                                .write_point(Point {
                                    x,
                                    y,
                                    z: point.z,
                                    classification: point.classification,
                                    ..Default::default()
                                })
                                .unwrap();
                        }
                    }

                    for ((x, y), writer) in map {
                        let data = writer.into_inner().unwrap().into_inner();

                        conn.lock()
                            .unwrap()
                            .execute(
                                "INSERT INTO tiles (x, y, data) VALUES (?1, ?2, ?3)",
                                (x, y, data.as_slice()),
                            )
                            .unwrap();
                    }

                    conn.lock()
                        .unwrap()
                        .execute(
                            "INSERT INTO processed_file (name) VALUES (?1)",
                            [&file_name],
                        )
                        .unwrap();

                    println!("FIN {file_name}");
                }
            });
        }
    });

    let conn = Arc::try_unwrap(conn).unwrap().into_inner().unwrap();

    if !options.r#continue {
        conn.execute("CREATE INDEX idx_tiles_xy ON tiles (x, y)", ())
            .unwrap();
    }
}
