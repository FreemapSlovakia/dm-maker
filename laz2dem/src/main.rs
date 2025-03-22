mod params;
mod progress;
mod rasterization;
mod read;
mod schema;
mod shading;
mod shared_types;

use std::path::PathBuf;

use clap::{ArgGroup, Parser};
use maptile::bbox::BBox;
use params::Params;
use rasterization::rasterize;
use read::read;
use shared_types::{Job, Shadings, Source};

#[derive(Clone, Debug, Parser, PartialEq)]
#[clap(group = ArgGroup::new("exclusive").required(true))]
struct Options {
    // Output mbtiles file
    output: PathBuf,

    /// Zoom level of tiles
    #[clap(long)]
    zoom_level: u8,

    /// Shadings; `+` separated componets of shading. Shading component is <method>,method_param1[,method_param2...].
    /// ‎
    /// Methods:
    /// - oblique - params: azimuth in degrees
    /// - igor - params: azimuth in degrees, alitutde in degrees
    /// - slope - params: alitutde in degrees
    #[clap(long, verbatim_doc_comment)]
    shadings: Shadings,

    /// TODO explain
    #[clap(long, default_value_t = 0)]
    supertile_zoom_offset: u8,

    /// Tile size
    #[clap(long, default_value_t = 256)]
    tile_size: u16,

    /// Buffer size in pixels
    #[clap(long, default_value_t = 40)]
    buffer: u32,

    #[clap(long, group = "exclusive")]
    laz_tile_db: Option<PathBuf>,

    #[clap(long, group = "exclusive")]
    laz_index_db: Option<PathBuf>,

    /// EPSG:3857 bounding box to render
    #[clap(long)]
    bbox: BBox,
}

fn main() {
    let options = Options::parse();

    // let bbox_3857 = BBox::new(2273080.0, 6204962.0, 2273494.0, 6205186.0); // SMALL
    // let bbox_3857 = BBox::new(2272240.0, 6203413.0, 2274969.0, 6205873.0); // BIG
    // let bbox_3857 = BBox::new(2269316.0, 6199572.0, 2279288.0, 6218237.0); // Plesivecka
    // let bbox_3857 = BBox::new(2279885.0, 6197892.0, 2290779.0, 6212053.0); // Silica
    // let bbox_3857 = BBox::new(2247108.0, 6186062.0, 2257303.0, 6196843.0); // Gemer
    // let bbox_3857 = BBox::new(2003828.0, 6146494.0, 2023330.0, 6171158.0); // Nitra

    let source = options.laz_index_db.map_or_else(
        || {
            options
                .laz_tile_db
                .map_or_else(|| unreachable!("only one"), Source::LazTileDb)
        },
        Source::LazIndexDb,
    );

    let params = Params {
        zoom: options.zoom_level,
        supertile_zoom_offset: options.supertile_zoom_offset,
        buffer_px: options.buffer,
        tile_size: options.tile_size,
        bbox_3857: options.bbox,
    };

    let tile_metas = read(&source, &params);

    let mut jobs: Vec<_> = tile_metas.into_iter().map(Job::Rasterize).collect();

    jobs.sort_by_cached_key(|job| job.tile().morton_code());

    rasterize(
        options.output.as_ref(),
        &source,
        &params,
        jobs,
        &options.shadings.0,
    );
}
