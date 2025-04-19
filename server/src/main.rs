use axum::{
    Router,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{cell::RefCell, path::Path as FsPath};
use tokio::task::spawn_blocking;

// path to your .mbtiles file
const DB_PATH: &str = "/home/martin/fm/dm-maker/laz2dem/tilesets/dedinky-dem.mbtiles";

// thread-local SQLite connection
thread_local! {
    static THREAD_DB: RefCell<Option<Connection>> = RefCell::new(None);
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/tiles/{z}/{x}/{y}", get(get_tile));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3033").await.unwrap();

    axum::serve(listener, app).await.unwrap();
}

async fn get_tile(Path((z, x, y)): Path<(u32, u32, u32)>) -> Response {
    let tms_y = (1 << z) - 1 - y;

    let result = spawn_blocking(move || {
        THREAD_DB.with(|db_cell| {
            let mut db_opt = db_cell.borrow_mut();

            if db_opt.is_none() {
                *db_opt = Some(Connection::open_with_flags(
                    FsPath::new(DB_PATH),
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
