use crossbeam::channel::{Receiver, Sender, bounded};
use rusqlite::{Connection, OpenFlags, params};
use std::io::Cursor;
use std::thread::{self, available_parallelism};
use tilemath::{Tile, TileIterator};
use zstd::stream::decode_all;

type RawTile = (Tile, Vec<u8>);
type EncodedTile = (Tile, Vec<u8>);

// const OUTPUT_DB: &str = "/home/martin/14TB/sk-new-dmr/dedinky-lerc.mbtiles";
const OUTPUT_DB: &str = "/media/martin/OSM/sk-dem-lerc.mbtiles";
const INPUT_DB: &str = "/home/martin/14TB/sk-new-dmr/sk-w-water.mbtiles";

fn worker(raw_rx: Receiver<RawTile>, enc_tx: Sender<EncodedTile>) {
    while let Ok((tile, zstd_data)) = raw_rx.recv() {
        // enc_tx.send((tile, zstd_data)).expect("send encoded tile");

        let buf = decode_all(&*zstd_data).expect("zstd decompress");

        assert_eq!(buf.len() % 4, 0, "buffer length must be multiple of 4");

        let floats: Vec<_> = buf
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect();

        let len = floats.len();
        let dim = (len as f64).sqrt() as usize;
        assert_eq!(dim * dim, len, "data does not form a square matrix");

        let lerc_data = lerc::encode(
            &floats,
            None,
            dim,
            dim,
            1,
            1,
            0,
            2_f64.powf((20.0 - tile.zoom as f64) / 1.5) / 150.0,
        )
        .expect("lerc encode");

        let buffer = zstd::encode_all(Cursor::new(lerc_data), 16).unwrap();

        enc_tx.send((tile, buffer)).expect("send encoded tile");
    }
}

fn writer(enc_rx: Receiver<EncodedTile>) {
    let conn = Connection::open(OUTPUT_DB).expect("open output DB");

    conn.pragma_update(None, "synchronous", "OFF").unwrap();

    conn.pragma_update(None, "journal_mode", "WAL").unwrap();

    conn.busy_timeout(std::time::Duration::from_secs(10))
        .unwrap();

    while let Ok((tile, buffer)) = enc_rx.recv() {
        // println!("INS {tile}");

        conn.execute(
            "INSERT INTO tiles (zoom_level, tile_column, tile_row, tile_data) VALUES (?1, ?2, ?3, ?4)",
            params![tile.zoom, tile.x, tile.reversed_y(), buffer],
        ).expect("insert tile");
    }
}

fn main() {
    {
        let conn = Connection::open(OUTPUT_DB).unwrap();

        conn.execute(
            "CREATE TABLE IF NOT EXISTS tiles (
                zoom_level INTEGER,
                tile_column INTEGER,
                tile_row INTEGER,
                tile_data BLOB
            )",
            [],
        )
        .unwrap();

        conn.execute(
            "CREATE UNIQUE INDEX idx_tiles ON tiles (zoom_level, tile_column, tile_row)",
            (),
        )
        .unwrap();
    }

    let num_workers = available_parallelism().unwrap().get();

    let (raw_tx, raw_rx) = bounded::<RawTile>(num_workers * 2);
    let (enc_tx, enc_rx) = bounded::<EncodedTile>(num_workers * 2);

    for _ in 0..num_workers {
        let rx = raw_rx.clone();
        let tx = enc_tx.clone();
        thread::spawn(move || worker(rx, tx));
    }

    thread::spawn(move || writer(enc_rx));

    // {
    //     let conn = Connection::open_with_flags(INPUT_DB, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();

    //     let mut stmt = conn
    //         .prepare("SELECT zoom_level, tile_column, tile_row, tile_data FROM tiles")
    //         .unwrap();

    //     let mut rows = stmt.query([]).unwrap();

    //     while let Some(row) = rows.next().unwrap() {
    //         let zoom: u8 = row.get(0).unwrap();
    //         let x: u32 = row.get(1).unwrap();
    //         let y: u32 = row.get(2).unwrap();
    //         let data: Vec<u8> = row.get(3).unwrap();
    //         raw_tx.send((Tile { zoom, x, y }, data)).unwrap();
    //     }
    // }

    {
        let conn = Connection::open_with_flags(INPUT_DB, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();

        let mut stmt = conn
            .prepare("SELECT tile_data FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3")
            .unwrap();

        for tile in TileIterator::new(19, 286567..=295056, 178655..=182904).pyramid() {
            let mut rows = stmt.query((tile.zoom, tile.x, tile.reversed_y())).unwrap();

            while let Some(row) = rows.next().unwrap() {
                let data: Vec<u8> = row.get(0).unwrap();
                raw_tx.send((tile, data)).unwrap();
            }
        }
    }

    drop(raw_tx); // end signal to workers
    drop(enc_tx); // end signal to writer
}
