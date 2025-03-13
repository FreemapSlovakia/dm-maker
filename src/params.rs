use maptile::{bbox::BBox, constants::WEB_MERCATOR_EXTENT};

pub struct Params {
    pub zoom: u8,
    pub supertile_zoom_offset: u8,
    pub buffer_px: u32,
    pub tile_size: u16,
    pub bbox_3857: BBox,
}

impl Params {
    pub fn get_pixels_per_meter(&self) -> f64 {
        (((self.tile_size as u64) << self.zoom) as f64) / 2.0 / WEB_MERCATOR_EXTENT
    }
}
