use crate::shared_types::{Shadings, Source};
use clap::{ArgGroup, Parser};
use maptile::{bbox::BBox, constants::WEB_MERCATOR_EXTENT};
use std::path::PathBuf;

#[derive(Clone, Debug, Parser, PartialEq)]
#[clap(group = ArgGroup::new("exclusive").required(true))]
pub struct Options {
    // Output mbtiles file
    pub output: PathBuf,

    /// Zoom level of tiles
    #[clap(long)]
    pub zoom_level: u8,

    /// Shadings; `+` separated componets of shading. Shading component is <method>,method_param1[,method_param2...].
    /// ‎
    /// Methods:
    /// - oblique - params: azimuth in degrees
    /// - igor - params: azimuth in degrees, alitutde in degrees
    /// - slope - params: alitutde in degrees
    #[clap(long, verbatim_doc_comment)]
    pub shadings: Shadings,

    /// Z-factor
    #[clap(long, default_value_t = 1.0)]
    pub z_factor: f64,

    /// TODO explain
    #[clap(long, default_value_t = 0)]
    pub supertile_zoom_offset: u8,

    /// Tile size
    #[clap(long, default_value_t = 256)]
    pub tile_size: u16,

    /// Buffer size in pixels
    #[clap(long, default_value_t = 40)]
    pub buffer: u32,

    #[clap(long, group = "exclusive")]
    pub laz_tile_db: Option<PathBuf>,

    #[clap(long, group = "exclusive")]
    pub laz_index_db: Option<PathBuf>,

    /// EPSG:3857 bounding box to render
    #[clap(long)]
    pub bbox: BBox,
}

impl Options {
    pub fn pixels_per_meter(&self) -> f64 {
        (((self.tile_size as u64) << self.zoom_level) as f64) / 2.0 / WEB_MERCATOR_EXTENT
    }

    pub fn source(&self) -> Source {
        self.laz_index_db.clone().map_or_else(
            || {
                self.laz_tile_db
                    .clone()
                    .map_or_else(|| unreachable!("only one"), Source::LazTileDb)
            },
            Source::LazIndexDb,
        )
    }
}
