use las::{point::Classification, Reader};
use proj::Proj;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rusqlite::Connection;
use std::{
    f64,
    fs::File,
    ops::DerefMut,
    sync::{atomic::AtomicU64, Arc, Mutex},
};
use tiff::encoder::{colortype::Gray64Float, compression::Lzw, TiffEncoder};
use walkdir::WalkDir;

fn main() {
    render();

    // index();
}

fn render() {
    let proj_3857_to_8353 = Proj::new_known_crs("EPSG:3857", "EPSG:8353", None)
        .expect("Failed to create PROJ transformation");

    let (min_x, min_y, max_x, max_y) = (2272613_f64, 6205250_f64, 2275920_f64, 6208041_f64);

    let bbox_8353 = proj_3857_to_8353
        .transform_bounds(min_x, min_y, max_x, max_y, 11)
        .unwrap();

    let conn = Connection::open("index.sqlite").unwrap();

    println!("{:?}", bbox_8353);

    let mut stmt = conn.prepare("SELECT file FROM laz_index WHERE max_x >= ?1 AND min_x <= ?3 AND max_y >= ?2 AND min_y <= ?4").unwrap();

    let rows = stmt
        .query_map(bbox_8353, |row| row.get::<_, String>(0))
        .unwrap();

    let dt = Arc::new(Mutex::new(startin::Triangulation::new()));

    let files: Vec<String> = rows.map(|row| row.unwrap()).collect();

    println!("Reading {} files", files.len());

    let dt_clone = Arc::clone(&dt);

    let count = AtomicU64::new(0);

    files.par_iter().for_each_init(
        || {
            Proj::new_known_crs("EPSG:8353", "EPSG:3857", None)
                .expect("Failed to create PROJ transformation")
        },
        |proj, file| {
            println!("Reading {file}");

            let mut reader = Reader::from_path(file).unwrap();

            // let mut batch = Vec::new();

            for point in reader.points() {
                let point = point.unwrap();

                if point.classification == Classification::Ground {
                    let (x, y) = proj.convert((point.x, point.y)).unwrap();

                    if x > min_x && y > min_y && x < max_x && y < max_y {
                        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                        let _ = dt.lock().unwrap().insert_one_pt(x, y, point.z);

                        // batch.push([x, y, point.z]);

                        // if batch.len() > 1000000 {
                        //     print!(".");
                        //     stdout().flush().unwrap();

                        //     let _ = dt
                        //         .lock()
                        //         .unwrap()
                        //         .insert(&batch, startin::InsertionStrategy::BBox);

                        //     batch.clear();
                        // }
                    }
                }
            }

            // let _ = dt
            //     .lock()
            //     .unwrap()
            //     .insert(&batch, startin::InsertionStrategy::BBox);

            println!("Done {file}");
        },
    );

    println!(
        "{}",
        count.fetch_add(0, std::sync::atomic::Ordering::Relaxed)
    );

    let interpolant = startin::interpolation::NNI { precompute: true };

    let mut coords = vec![];

    let world_extent = 2.0 * f64::consts::PI * 6378137.0;
    let zoom = 20;

    let pixels_per_meter = ((256 << zoom) as f64) / world_extent;

    let width_pixels = ((max_x - min_x) * pixels_per_meter).round() as u32;
    let height_pixels = ((max_y - min_y) * pixels_per_meter).round() as u32;

    println!("Interpolating {width_pixels}x{height_pixels}");

    for y in 0..height_pixels {
        let cy = min_y + y as f64 * (max_y - min_y) / height_pixels as f64;

        for x in 0..width_pixels {
            let cx = min_x + x as f64 * (max_x - min_x) / width_pixels as f64;

            coords.push([cx, cy]);
        }
    }

    let mut img = Vec::new();

    let mut dt = dt_clone.lock().unwrap();

    let dt = dt.deref_mut();

    let zs = startin::interpolation::interpolate(&interpolant, dt, &coords);

    for y in 0..height_pixels {
        for x in 0..width_pixels {
            let v = zs.get((x + y * width_pixels) as usize).unwrap().as_ref();

            if let Ok(v) = v {
                img.push(*v);
            } else {
                img.push(f64::NAN);
            }
        }
    }

    let mut tif = TiffEncoder::new(File::create("out-nni.tif").unwrap()).unwrap();

    tif.write_image_with_compression::<Gray64Float, _>(
        width_pixels,
        height_pixels,
        Lzw::default(),
        &img,
    )
    .unwrap();
}

fn index() {
    let conn = Connection::open("index.sqlite").unwrap();

    conn.execute(
        "CREATE TABLE laz_index (min_x NUMBER, max_x NUMBER, min_y NUMBER, max_y NUMBER, file VARCHAR)", ()
    )
    .unwrap();

    let mut stmt = conn
        .prepare("INSERT INTO laz_index VALUES (?1, ?2, ?3, ?4, ?5)")
        .unwrap();

    for dir in WalkDir::new("/home/martin/18TB") {
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

            // println!("{:?}", header.bounds());

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
