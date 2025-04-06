use crate::{shadings::Shadings, shared_types::Source};
use clap::{ArgGroup, Parser, ValueEnum};
use maptile::{bbox::BBox, constants::WEB_MERCATOR_EXTENT};
use std::{
    fmt::{Display, Formatter},
    num::ParseIntError,
    path::PathBuf,
    str::FromStr,
};

#[derive(Clone, Debug, Parser, PartialEq)]
#[clap(group = ArgGroup::new("exclusive").required(true))]
pub struct Options {
    /// Output mbtiles file
    pub output: PathBuf,

    /// Source as LAZ tile DB
    #[clap(long, group = "exclusive")]
    pub laz_tile_db: Option<PathBuf>,

    /// Source as LAZ index DB referring *.laz files
    #[clap(long, group = "exclusive")]
    pub laz_index_db: Option<PathBuf>,

    /// EPSG:3857 bounding box to render
    #[clap(long)]
    pub bbox: BBox,

    /// Projection of points if reading from *.laz; default is EPSG:3857
    #[clap(long, conflicts_with = "laz_tile_db")]
    pub source_projection: Option<String>,

    /// Max zoom level of tiles to generate
    #[clap(long)]
    pub zoom_level: u8,

    /// If LAZ tile DB is used then use value of `--zoom-level` argument of `laztile`
    /// If LAZ index is used then use zoom level to determine size of tile to process at once.
    #[clap(long, default_value_t = 16)]
    pub unit_zoom_level: u8,

    /// Shadings; `+` separated componets of shading. Shading component is <method>,method_param1[,method_param2...].
    /// ‎
    /// Methods:
    /// - `oblique` - params: azimuth in degrees, alitutde in degrees
    /// - `igor` - params: azimuth in degrees
    /// - `slope` - params: alitutde in degrees
    #[clap(long, verbatim_doc_comment)]
    pub shadings: Shadings,

    /// Increase (> 1.0) or decrease (< 1.0) contrast of the shading. Use value higher than 0.0.
    #[clap(long, default_value_t = 1.0)]
    pub contrast: f64,

    /// Increase (> 0.0) or decrease (< 0.0) brightness of the shading. Use value between -1.0 and 1.0.
    #[clap(long, default_value_t = 0.0)]
    pub brightness: f64,

    /// Z-factor
    #[clap(long, default_value_t = 1.0)]
    pub z_factor: f64,

    /// Tile size
    #[clap(long, default_value_t = 256)]
    pub tile_size: u16,

    /// Buffer size in pixels to prevent artifacts at tieledges
    #[clap(long, default_value_t = 40)]
    pub buffer: u32,

    /// Tile image format. For alpha (transparency) support use `png`.
    #[clap(long, value_enum, default_value_t = Format::JPEG)]
    pub format: Format,

    /// Quality from 0 to 100 when writing to JPEG
    #[clap(long, default_value_t = 80)]
    pub jpeg_quality: u8,

    /// Background color when writing to JPEG as it does not support alpha
    #[clap(long, default_value = "FFFFFF")]
    pub background_color: Rgb,

    #[clap(long, value_enum)]
    pub existing_file_action: Option<ExistingFileAction>,
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum ExistingFileAction {
    Overwrite,
    Continue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rgb(pub image::Rgb<u8>);

impl FromStr for Rgb {
    type Err = ParseIntError;

    fn from_str(string: &str) -> Result<Self, Self::Err> {
        u32::from_str_radix(string, 16).map(|color| {
            let [_, r, g, b] = color.to_be_bytes();

            Self(image::Rgb([r, g, b]))
        })
    }
}

#[derive(ValueEnum, Debug, Clone, PartialEq)]
pub enum Format {
    JPEG,
    PNG,
}

impl Display for Format {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}",
            match self {
                Format::JPEG => "jpeg",
                Format::PNG => "png",
            }
        )
    }
}
