use crate::{
    params::Params,
    shared_types::{PointWithHeight, TileMeta},
};
use core::f64;
use las::{Reader, point::Classification};
use maptile::{bbox::BBox, utils::bbox_covered_tiles};
use proj::Proj;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rusqlite::Connection;
use spade::Point2;
use std::sync::Mutex;

pub fn read(params: &Params) -> Vec<TileMeta> {
    let buffer_m = params.buffer_px as f64 / params.pixels_per_meter();

    let tile_metas: Vec<_> = bbox_covered_tiles(
        &params.bbox_3857,
        params.zoom - params.supertile_zoom_offset,
    )
    .map(|tile| TileMeta {
        tile,
        bbox: tile
            .bounds(params.tile_size << params.supertile_zoom_offset)
            .to_extended(buffer_m),
        points: Mutex::new(Vec::<PointWithHeight>::new()),
    })
    .collect();

    let proj_3857_to_8353 = Proj::new_known_crs("EPSG:3857", "EPSG:8353", None)
        .expect("Failed to create PROJ transformation");

    let bbox_8353: BBox = proj_3857_to_8353
        .transform_bounds(
            params.bbox_3857.min_x,
            params.bbox_3857.min_y,
            params.bbox_3857.max_x,
            params.bbox_3857.max_y,
            11,
        )
        .unwrap()
        .into();

    let conn = Connection::open("index.sqlite").unwrap();

    let mut stmt = conn.prepare("SELECT file FROM laz_index WHERE max_x >= ?1 AND min_x <= ?3 AND max_y >= ?2 AND min_y <= ?4").unwrap();

    let rows = stmt
        .query_map(<[f64; 4]>::from(bbox_8353), |row| row.get::<_, String>(0))
        .unwrap();

    let files: Vec<String> = rows.map(|row| row.unwrap()).collect();

    println!("Reading {} files", files.len());

    files.par_iter().for_each_init(
        || {
            Proj::new_known_crs("EPSG:8353", "EPSG:3857", None)
                .expect("Failed to create PROJ transformation")
        },
        |proj, file| {
            println!("READ {file}");

            let mut reader = Reader::from_path(file).unwrap();

            for point in reader.points() {
                let point = point.unwrap();

                if point.classification != Classification::Ground {
                    continue;
                }

                if !bbox_8353.contains(point.x, point.y) {
                    continue;
                }

                let (x, y) = proj.convert((point.x, point.y)).unwrap();

                if !params.bbox_3857.contains(x, y) {
                    continue;
                }

                for (i, tile_meta) in tile_metas.iter().enumerate() {
                    if !tile_meta.bbox.contains(x, y) {
                        continue;
                    }

                    tile_metas
                        .get(i)
                        .unwrap()
                        .points
                        .lock()
                        .unwrap()
                        .push(PointWithHeight {
                            position: Point2::new(x, y),
                            height: point.z,
                        });
                }
            }

            println!("DONE {file}");
        },
    );

    tile_metas
}
