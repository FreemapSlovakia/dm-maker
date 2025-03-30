use maptile::{bbox::BBox, tile::Tile};
use spade::{HasPosition, Point2};
use std::{
    error::Error,
    fmt::{Debug, Display},
    path::PathBuf,
    str::FromStr,
    sync::Mutex,
};

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

impl Debug for TileMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileMeta")
            .field("tile", &self.tile)
            .field("bbox", &self.bbox)
            .finish()
    }
}

#[derive(Debug)]
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

#[derive(Clone)]
pub enum Source {
    LazTileDb(PathBuf),
    LazIndexDb(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShadingMethod {
    Igor(IgorShadingParams),
    Oblique(ObliqueShadingParams),
    IgorSlope,
    ObliqueSlope(ObliqueSlopeShadingParams),
}

#[derive(Clone, Debug, PartialEq)]
pub struct IgorShadingParams {
    pub azimuth: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObliqueShadingParams {
    pub azimuth: f64,
    pub altitude: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObliqueSlopeShadingParams {
    pub altitude: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shading {
    pub color: u32,
    pub weight: f64,
    pub brightness: f64,
    pub contrast: f64,
    pub method: ShadingMethod,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shadings(pub Vec<Shading>);

#[derive(Debug)]
pub struct ParseShadingError();

impl Display for ParseShadingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error parsing shading")
    }
}

impl Error for ParseShadingError {}

impl FromStr for Shadings {
    type Err = ParseShadingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let shadings: Result<_, _> = s
            .split('+')
            .map(|shading| {
                let params: Vec<&str> = shading.split(',').collect();

                let method = match params.get(0) {
                    Some(&"igor") => {
                        if params.len() != 3 {
                            Err(())
                        } else {
                            params[2].parse::<f64>().map_or(Err(()), |azimuth| {
                                Ok(ShadingMethod::Igor(IgorShadingParams {
                                    azimuth: azimuth.to_radians(),
                                }))
                            })
                        }
                    }
                    Some(&"oblique") => {
                        if params.len() != 4 {
                            Err(())
                        } else {
                            let azimuth = params[2].parse::<f64>();

                            let altitude = params[3].parse::<f64>();

                            match (azimuth, altitude) {
                                (Ok(azimuth), Ok(altitude)) => {
                                    Ok(ShadingMethod::Oblique(ObliqueShadingParams {
                                        azimuth: azimuth.to_radians(),
                                        altitude: altitude.to_radians(),
                                    }))
                                }
                                _ => Err(()),
                            }
                        }
                    }
                    Some(&"oblique-slope") => {
                        if params.len() != 3 {
                            Err(())
                        } else {
                            params[2].parse::<f64>().map_or(Err(()), |altitude| {
                                Ok(ShadingMethod::ObliqueSlope(ObliqueSlopeShadingParams {
                                    altitude: altitude.to_radians(),
                                }))
                            })
                        }
                    }
                    Some(&"igor-slope") => {
                        if params.len() != 2 {
                            Err(())
                        } else {
                            Ok(ShadingMethod::IgorSlope)
                        }
                    }
                    _ => Err(()),
                };

                let color = u32::from_str_radix(params[1], 16);

                match (color, method) {
                    (Ok(color), Ok(method)) => Ok(Shading {
                        color,
                        method,
                        brightness: 0.0,
                        contrast: 1.0,
                        weight: 1.0, // (i * 10 + 1) as f64 / 2.0,
                    }),
                    _ => Err(ParseShadingError()),
                }
            })
            .collect();

        Ok(Self(shadings?))
    }
}
