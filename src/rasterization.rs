use crate::{
    igor::igor_rgb,
    params::Params,
    schema::create_schema,
    shading::compute_hillshade,
    shared_types::{PointWithHeight, TileMeta},
};
use core::f64;
use image::{codecs::jpeg::JpegEncoder, imageops::crop_imm};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use rusqlite::Connection;
use spade::{DelaunayTriangulation, Point2, Triangulation};
use std::{fs::remove_file, io::Cursor, sync::Mutex};

pub fn rasterize(params: &Params, tile_metas: Vec<TileMeta>) {
    remove_file("out.mbtiles").unwrap_or(());

    let conn = Connection::open("out.mbtiles").unwrap();

    create_schema(
        &conn,
        &[
            ("name", "HS"),
            ("minzoom", "20"),
            ("maxzoom", "20"),
            ("format", "jpeg"),
            // ("bounds", ...TODO)
        ],
    )
    .unwrap();

    conn.pragma_update(None, "synchronous", "OFF").unwrap();

    conn.pragma_update(None, "journal_mode", "WAL").unwrap();

    let conn = Mutex::new(conn);

    println!("Tiles: {}", tile_metas.len());

    tile_metas.into_par_iter().for_each(|tile_meta| {
        // println!("Processing {:?}", tile_meta.tile);

        let mut t = DelaunayTriangulation::<PointWithHeight>::new();

        let points = tile_meta.points.into_inner().unwrap();

        for point in points {
            t.insert(point).unwrap();
        }

        let bbox_3857 = tile_meta.bbox;

        let pixels_per_meter = params.get_pixels_per_meter();

        let width_pixels = (bbox_3857.width() * pixels_per_meter).round() as u32;
        let height_pixels = (bbox_3857.height() * pixels_per_meter).round() as u32;

        let mut img = Vec::new();

        let nn = t.natural_neighbor();

        for y in 0..height_pixels {
            let cy = bbox_3857.min_y + y as f64 * bbox_3857.height() / height_pixels as f64;

            for x in 0..width_pixels {
                let cx = bbox_3857.min_x + x as f64 * bbox_3857.width() / width_pixels as f64;

                let value = nn.interpolate(|v| v.data().height, Point2::new(cx, cy));

                if let Some(value) = value {
                    img.push(value);
                } else {
                    img.push(f64::NAN);
                }
            }
        }

        // let sun_azimuth_rad = 315_f64.to_radians();
        // let sun_zenith_rad = 45_f64.to_radians();

        let img = compute_hillshade(
            img,
            height_pixels as usize,
            width_pixels as usize,
            |aspect_rad, slope_rad| {
                igor_rgb(
                    aspect_rad,
                    slope_rad,
                    &[
                        (-120.0, 0.8, 0x203060),
                        (60.0, 0.7, 0xFFEE00),
                        (-45.0, 1.0, 0x000000),
                    ],
                    1.0,
                    0.0,
                )

                // // Compute illumination using sun angle
                // let illumination = sun_zenith_rad.cos() * slope_rad.cos()
                //     + sun_zenith_rad.sin() * slope_rad.sin() * (sun_azimuth_rad - aspect_rad).cos();

                // // Scale to 0-255
                // let shade = (illumination * 255.0).clamp(0.0, 255.0) as u8;

                // [shade, shade, shade]
            },
        );

        let mut tiles = vec![tile_meta.tile];

        let supertile_zoom_offset = params.supertile_zoom_offset;

        for _ in 0..supertile_zoom_offset {
            tiles = tiles.iter().flat_map(|tile| tile.get_children()).collect();
        }

        tiles.sort_by(|a, b| a.y.cmp(&b.y).then_with(|| a.x.cmp(&b.x)));

        let buffer_px = params.buffer_px;
        let tile_size = params.tile_size as u32;

        for (sector, tile) in tiles.iter().enumerate() {
            let img = crop_imm(
                &img,
                buffer_px + ((sector as u32) & ((1 << supertile_zoom_offset) - 1)) * tile_size,
                buffer_px + (sector as u32 >> supertile_zoom_offset) * tile_size,
                tile_size,
                tile_size,
            )
            .to_image();

            let mut buffer = vec![];

            img.write_with_encoder(JpegEncoder::new(Cursor::new(&mut buffer)))
                .unwrap();

            conn.lock()
                .unwrap()
                .execute(
                    "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)",
                    (tile.zoom, tile.x, tile.reversed_y(), buffer),
                )
                .unwrap();
        }
    });
}
