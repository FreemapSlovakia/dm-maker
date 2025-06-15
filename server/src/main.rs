use axum::{
    Router,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use clap::Parser;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{cell::RefCell, path::Path as FsPath};
use tokio::task::spawn_blocking;

// thread-local SQLite connection
thread_local! {
    static THREAD_DB: RefCell<Option<Connection>> = RefCell::new(None);
}

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Path to the .mbtiles file
    #[arg(long)]
    mbtiles: String,

    /// Base URL path (e.g. /tiles)
    #[arg(long, default_value = "/tiles")]
    base_path: String,

    /// Host and port to bind (e.g. 0.0.0.0:3033)
    #[arg(long, default_value = "0.0.0.0:3033")]
    bind: std::net::SocketAddr,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let db_path = args.mbtiles.clone();

    let route_path = format!("{}/{{z}}/{{x}}/{{y}}", args.base_path.trim_end_matches('/'));

    let db_path_clone = db_path.clone();

    let app = Router::new().route(
        &route_path,
        get(move |path| get_tile(path, db_path_clone.clone())),
    );

    let listener = tokio::net::TcpListener::bind(&args.bind).await.unwrap();

    axum::serve(listener, app).await.unwrap();
}
async fn get_tile(Path((z, x, y)): Path<(u32, u32, u32)>, db_path: String) -> Response {
    let tms_y = (1 << z) - 1 - y;

    let result = spawn_blocking(move || {
        THREAD_DB.with(|db_cell| {
            let mut db_opt = db_cell.borrow_mut();

            if db_opt.is_none() {
                *db_opt = Some(Connection::open_with_flags(
                    FsPath::new(&db_path),
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?);
            }

            let conn = db_opt.as_ref().unwrap();

            let mut stmt = conn.prepare_cached(
                "SELECT tile_data FROM tiles WHERE zoom_level = ? AND tile_column = ? AND tile_row = ?",
            )?;

            stmt.query_row((z, x, tms_y), |row| row.get::<_, Vec<u8>>(0)).optional()
        })
    })
    .await;

    match result {
        Ok(Ok(Some(tile))) => (
            StatusCode::OK,
            [
                ("Content-Type", "application/octet-stream"),
                ("Access-Control-Allow-Origin", "*"),
            ],
            tile,
        )
            .into_response(),
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
