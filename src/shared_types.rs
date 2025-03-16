use maptile::{bbox::BBox, tile::Tile};
use spade::{HasPosition, Point2};
use std::sync::Mutex;

pub struct PointWithHeight {
    pub position: Point2<f64>,
    pub height: f64,
}

impl HasPosition for PointWithHeight {
    type Scalar = f64;

    fn position(&self) -> Point2<f64> {
        self.position
    }
}

pub struct TileMeta {
    pub tile: Tile,
    pub bbox: BBox,
    pub points: Mutex<Vec<PointWithHeight>>,
}

pub enum Job {
    Rasterize(TileMeta),
    Overview(Tile),
}

impl Job {
    pub const fn tile(&self) -> Tile {
        match self {
            Self::Rasterize(tile_meta) => tile_meta.tile,
            Self::Overview(tile) => *tile,
        }
    }
}
