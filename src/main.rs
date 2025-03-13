mod index;
mod rasterization;
mod read;
mod schema;
mod shared_types;

use maptile::{bbox::BBox, constants::WEB_MERCATOR_EXTENT};
use rasterization::rasterize;
use read::read;

fn main() {
    render();

    // index();
}

fn render() {
    let bbox_3857 = BBox::new(2273080.0, 6204962.0, 2273494.0, 6205186.0); // SMALL
    // let bbox_3857 = BBox::new(2272240.0, 6203413.0, 2274969.0, 6205873.0); // BIG
    // let bbox_3857 = BBox::new(2269316.0, 6199572.0, 2279288.0, 6218237.0); // Plesivecka

    let zoom = 19;

    let buffer_px = 20;

    let tile_size = 512;

    let tile_metas = read(bbox_3857, zoom, tile_size, buffer_px);

    let pixels_per_meter = (((tile_size as u64) << zoom) as f64) / 2.0 / WEB_MERCATOR_EXTENT;

    rasterize(pixels_per_meter, buffer_px, tile_metas);
}
