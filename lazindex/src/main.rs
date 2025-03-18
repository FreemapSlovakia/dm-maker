use clap::Parser;
use las::Reader;
use rusqlite::Connection;
use std::path::PathBuf;
use walkdir::WalkDir;

#[derive(Parser, Debug, PartialEq)]
struct Options {
    /// Directory with *.laz files
    directory: PathBuf, // "/home/martin/18TB"

    /// Output database file
    database: PathBuf, // "/home/martin/14TB/sk-new-dmr/laztiles.sqlite"
}

fn main() {
    let options = Options::parse();

    let conn = Connection::open(options.database).unwrap();

    conn.execute(
      "CREATE TABLE laz_index (min_x NUMBER, max_x NUMBER, min_y NUMBER, max_y NUMBER, file VARCHAR)", ()
  )
  .unwrap();

    let mut stmt = conn
        .prepare("INSERT INTO laz_index VALUES (?1, ?2, ?3, ?4, ?5)")
        .unwrap();

    for dir in WalkDir::new(options.directory) {
        let dir = dir.unwrap();

        println!("{}", dir.file_name().to_string_lossy());

        if dir
            .path()
            .extension()
            .map(|ext| ext == "laz")
            .unwrap_or(false)
        {
            let reader = Reader::from_path(dir.path()).unwrap();

            let bounds = reader.header().bounds();

            let _ = stmt
                .execute((
                    bounds.min.x,
                    bounds.max.x,
                    bounds.min.y,
                    bounds.max.y,
                    dir.path().to_string_lossy(),
                ))
                .unwrap();
        }
    }

    for query in [
        "CREATE UNIQUE INDEX laz_file_unique ON laz_index (file)",
        "CREATE INDEX laz_min_x_index ON laz_index (min_x)",
        "CREATE INDEX laz_max_x_index ON laz_index (max_x)",
        "CREATE INDEX laz_min_y_index ON laz_index (min_y)",
        "CREATE INDEX laz_max_y_index ON laz_index (max_y)",
    ] {
        conn.execute(query, ()).unwrap();
    }
}
