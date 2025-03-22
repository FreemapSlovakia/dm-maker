mod options;
mod progress;
mod rasterization;
mod read;
mod schema;
mod shading;
mod shared_types;

use clap::Parser;
use options::Options;
use rasterization::rasterize;
use read::read;
use shared_types::Job;

fn main() {
    let options = Options::parse();

    // let bbox_3857 = BBox::new(2273080.0, 6204962.0, 2273494.0, 6205186.0); // SMALL
    // let bbox_3857 = BBox::new(2272240.0, 6203413.0, 2274969.0, 6205873.0); // BIG
    // let bbox_3857 = BBox::new(2269316.0, 6199572.0, 2279288.0, 6218237.0); // Plesivecka
    // let bbox_3857 = BBox::new(2279885.0, 6197892.0, 2290779.0, 6212053.0); // Silica
    // let bbox_3857 = BBox::new(2247108.0, 6186062.0, 2257303.0, 6196843.0); // Gemer
    // let bbox_3857 = BBox::new(2003828.0, 6146494.0, 2023330.0, 6171158.0); // Nitra

    let tile_metas = read(&options);

    let mut jobs: Vec<_> = tile_metas.into_iter().map(Job::Rasterize).collect();

    jobs.sort_by_cached_key(|job| job.tile().morton_code());

    rasterize(&options, jobs);
}
