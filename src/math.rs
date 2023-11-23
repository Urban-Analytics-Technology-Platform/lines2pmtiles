use std::f64;

use geojson::{Feature, Value};

// TODO Everything in here can hopefully be replaced with a crate

// WGS84 to Mercator
pub fn wgs84_to_web_mercator(c: [f64; 2]) -> [f64; 2] {
    static A: f64 = 6378137.0;
    static MAXEXTENT: f64 = 20037508.342789244;
    static D2R: f64 = f64::consts::PI / 180.0;

    [
        (A * c[0] * D2R).max(-MAXEXTENT).min(MAXEXTENT) as f64,
        (A * (((f64::consts::PI * 0.25f64) + (0.5f64 * c[1] * D2R)).tan()).ln())
            .max(-MAXEXTENT)
            .min(MAXEXTENT) as f64,
    ]
}

#[derive(Debug)]
pub struct BBox {
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
}

impl BBox {
    pub fn empty() -> Self {
        Self {
            min_lon: f64::MAX,
            max_lon: f64::MIN,
            min_lat: f64::MAX,
            max_lat: f64::MIN,
        }
    }

    pub fn add(&mut self, f: &Feature) {
        if let Some(ref geometry) = f.geometry {
            if let Value::LineString(ref line_string) = geometry.value {
                for pt in line_string {
                    self.min_lon = self.min_lon.min(pt[0]);
                    self.min_lat = self.min_lat.min(pt[1]);
                    self.max_lon = self.max_lon.max(pt[0]);
                    self.max_lat = self.max_lat.max(pt[1]);
                }
            }
        }
    }

    pub fn to_tiles(&self, zoom: u32) -> (u32, u32, u32, u32) {
        let (x1, y1) = lon_lat_to_tile(self.min_lon, self.min_lat, zoom);
        let (x2, y2) = lon_lat_to_tile(self.max_lon, self.max_lat, zoom);
        // TODO Not sure why y gets swapped sometimes
        (x1, y1.min(y2), x2, y2.max(y1))
    }
}

// Thanks to https://github.com/MilesMcBain/slippymath/blob/master/R/slippymath.R
// Use https://crates.io/crates/tile-grid or something instead?
// Alternatively https://wiki.openstreetmap.org/wiki/Slippy_map_tilenames#Python
fn lon_lat_to_tile(lon: f64, lat: f64, zoom: u32) -> (u32, u32) {
    let lon_radians = lon.to_radians();
    let lat_radians = lat.to_radians();

    let x = lon_radians;
    let y = lat_radians.tan().asinh();

    let x = (1.0 + (x / f64::consts::PI)) / 2.0;
    let y = (1.0 - (y / f64::consts::PI)) / 2.0;

    let num_tiles = 2u32.pow(zoom) as f64;

    (
        (x * num_tiles).floor() as u32,
        (y * num_tiles).floor() as u32,
    )
}
