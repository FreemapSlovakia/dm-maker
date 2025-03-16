mod igor;
mod index;
mod params;
mod progress;
mod rasterization;
mod read;
mod schema;
mod shading;
mod shared_types;

use maptile::bbox::BBox;
use params::Params;
use rasterization::rasterize;
use read::read;
use shared_types::Job;

fn main() {
    render();

    // index();
}

fn render() {
    // let bbox_3857 = BBox::new(2273080.0, 6204962.0, 2273494.0, 6205186.0); // SMALL
    // let bbox_3857 = BBox::new(2272240.0, 6203413.0, 2274969.0, 6205873.0); // BIG
    let bbox_3857 = BBox::new(2269316.0, 6199572.0, 2279288.0, 6218237.0); // Plesivecka
    // let bbox_3857 = BBox::new(2270462.0, 6205266.0, 2277836.0, 6210632.0); // BIGGER

    let zoom = 20;

    let params = Params {
        zoom,
        supertile_zoom_offset: 2,
        buffer_px: 20,
        tile_size: 256,
        bbox_3857,
    };

    let tile_metas = read(&params);

    let mut jobs: Vec<_> = tile_metas.into_iter().map(Job::Rasterize).collect();

    jobs.sort_by_cached_key(|job| job.tile().morton_code());

    rasterize(&params, jobs);
}
