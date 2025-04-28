use crossbeam::channel::{Receiver, Sender, bounded};
use rusqlite::{Connection, OpenFlags, params};
use std::io::Cursor;
use std::thread::{self, available_parallelism};
use zstd::stream::decode_all;

type RawTile = (u8, u32, u32, Vec<u8>);
type EncodedTile = (u8, u32, u32, Vec<u8>);

const OUTPUT_DB: &str = "/home/martin/14TB/sk-new-dmr/sk-dem-f32-lerc-0-zstd.mbtiles";
const INPUT_DB: &str = "/home/martin/14TB/sk-new-dmr/sk-dem-f32-zstd.mbtiles";

fn worker(raw_rx: Receiver<RawTile>, enc_tx: Sender<EncodedTile>) {
    while let Ok((z, x, y, zstd_data)) = raw_rx.recv() {
        let buf = decode_all(&*zstd_data).expect("zstd decompress");

        assert_eq!(buf.len() % 4, 0, "buffer length must be multiple of 4");

        let floats: Vec<_> = buf
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect();

        let len = floats.len();
        let dim = (len as f64).sqrt() as usize;
        assert_eq!(dim * dim, len, "data does not form a square matrix");

        let lerc_data = lerc::encode(&floats, None, dim, dim, 1, 1, 0, 0.0).expect("lerc encode");

        let buffer = zstd::encode_all(Cursor::new(lerc_data), 0).unwrap();

        enc_tx.send((z, x, y, buffer)).expect("send encoded tile");
    }
}

fn writer(enc_rx: Receiver<EncodedTile>) {
    let conn = Connection::open(OUTPUT_DB).expect("open output DB");

    conn.pragma_update(None, "synchronous", "OFF").unwrap();

    conn.pragma_update(None, "journal_mode", "WAL").unwrap();

    conn.busy_timeout(std::time::Duration::from_secs(10))
        .unwrap();

    while let Ok((z, x, y, buffer)) = enc_rx.recv() {
        conn.execute(
            "INSERT INTO tiles (zoom_level, tile_column, tile_row, tile_data) VALUES (?1, ?2, ?3, ?4)",
            params![z, x, y, buffer],
        ).expect("insert tile");
    }
}

fn main() {
    Connection::open(OUTPUT_DB)
        .unwrap()
        .execute(
            "CREATE TABLE IF NOT EXISTS tiles (
                zoom_level INTEGER,
                tile_column INTEGER,
                tile_row INTEGER,
                tile_data BLOB
            )",
            [],
        )
        .unwrap();

    let num_workers = available_parallelism().unwrap().get();

    let (raw_tx, raw_rx) = bounded::<RawTile>(num_workers * 2);
    let (enc_tx, enc_rx) = bounded::<EncodedTile>(num_workers * 2);

    for _ in 0..num_workers {
        let rx = raw_rx.clone();
        let tx = enc_tx.clone();
        thread::spawn(move || worker(rx, tx));
    }

    thread::spawn(move || writer(enc_rx));

    {
        let conn = Connection::open_with_flags(INPUT_DB, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();

        let mut stmt = conn
            .prepare("SELECT zoom_level, tile_column, tile_row, tile_data FROM tiles")
            .unwrap();

        let mut rows = stmt.query([]).unwrap();

        while let Some(row) = rows.next().unwrap() {
            let z: u8 = row.get(0).unwrap();
            let x: u32 = row.get(1).unwrap();
            let y: u32 = row.get(2).unwrap();
            let data: Vec<u8> = row.get(3).unwrap();
            raw_tx.send((z, x, y, data)).unwrap();
        }
    }

    drop(raw_tx); // end signal to workers
    drop(enc_tx); // end signal to writer
}
