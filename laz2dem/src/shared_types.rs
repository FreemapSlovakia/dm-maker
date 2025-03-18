use maptile::{bbox::BBox, tile::Tile};
use spade::{HasPosition, Point2};
use std::{error::Error, fmt::Display, path::PathBuf, str::FromStr, sync::Mutex};

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

pub enum Source {
    LazTileDb(PathBuf),
    LazIndexDb(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShadingMethod {
    Igor(IgorShadingParams),
    Oblique(ObliqueShadingParams),
    Slope(SlopeShadingParams),
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
pub struct SlopeShadingParams {
    pub altitude: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shading {
    pub color: u32,
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

// igor,ff4455ff,120+...
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
                            let azimuth = params[2].parse::<f64>();

                            match azimuth {
                                Ok(azimuth) => {
                                    Ok(ShadingMethod::Igor(IgorShadingParams { azimuth }))
                                }
                                Err(_) => Err(()),
                            }
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
                                        azimuth,
                                        altitude,
                                    }))
                                }
                                _ => Err(()),
                            }
                        }
                    }
                    Some(&"slope") => {
                        if params.len() != 3 {
                            Err(())
                        } else {
                            let altitude = params[2].parse::<f64>();

                            match altitude {
                                Ok(altitude) => {
                                    Ok(ShadingMethod::Slope(SlopeShadingParams { altitude }))
                                }
                                Err(_) => Err(()),
                            }
                        }
                    }
                    _ => Err(()),
                };

                let color = u32::from_str_radix(params[0], 16);

                match (color, method) {
                    (Ok(color), Ok(method)) => Ok(Shading { color, method }),
                    _ => Err(ParseShadingError()),
                }
            })
            .collect();

        Ok(Shadings(shadings?))
    }
}
