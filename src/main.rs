mod index;
mod params;
mod rasterization;
mod read;
mod schema;
mod shared_types;

use maptile::bbox::BBox;
use params::Params;
use rasterization::rasterize;
use read::read;

fn main() {
    render();

    // index();
}

fn render() {
    let params = Params {
        zoom: 20,
        supertile_zoom_offset: 2,
        buffer_px: 20,
        tile_size: 256,
        // bbox_3857: BBox::new(2273080.0, 6204962.0, 2273494.0, 6205186.0), // SMALL
        bbox_3857: BBox::new(2272240.0, 6203413.0, 2274969.0, 6205873.0), // BIG
                                                                          // bbox_3857: BBox::new(2269316.0, 6199572.0, 2279288.0, 6218237.0), // Plesivecka
    };

    let tile_metas = read(&params);

    rasterize(&params, tile_metas);
}
