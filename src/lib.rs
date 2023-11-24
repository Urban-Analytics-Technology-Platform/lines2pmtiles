use std::collections::HashMap;
use std::io::{Cursor, Read};

use anyhow::Result;
use geo::algorithm::bounding_rect::BoundingRect;
use geo::algorithm::map_coords::MapCoordsInPlace;
use geo_types::Geometry;
use geojson::FeatureReader;
use indicatif::{HumanBytes, HumanCount, MultiProgress, ProgressBar, ProgressStyle};
use mvt::{GeomEncoder, GeomType, MapGrid, Tile, TileId};
use pmtiles2::{util::tile_id as get_tile_id, Compression, PMTiles, TileType};
use pointy::Transform;
use rayon::prelude::*;
use rstar::{primitives::CachedEnvelope, RTree, RTreeObject, AABB};
use serde_json::Value;

use self::math::BBox;

mod math;

pub struct Options {
    pub layer_name: String,
    /// Descending
    pub sort_by_key: Option<String>,
    pub zoom_levels: Vec<u32>,
    pub limit_size_bytes: Option<usize>,
}

pub fn geojson_to_pmtiles<R: Read>(
    geojson_input: R,
    options: Options,
) -> Result<PMTiles<Cursor<&'static [u8]>>> {
    let (r_tree, feature_count, bbox, fields) =
        load_features(FeatureReader::from_reader(geojson_input))?;

    println!(
        "bbox of {} features: {:?}",
        HumanCount(feature_count as u64),
        bbox
    );

    let mut pmtiles = PMTiles::new(TileType::Mvt, Compression::None);
    pmtiles.min_longitude = bbox.min_lon;
    pmtiles.min_latitude = bbox.min_lat;
    pmtiles.max_longitude = bbox.max_lon;
    pmtiles.max_latitude = bbox.max_lat;
    pmtiles.min_zoom = options.zoom_levels[0] as u8;
    pmtiles.max_zoom = *options.zoom_levels.last().unwrap() as u8;
    pmtiles.meta_data = Some(serde_json::json!(
        {
            "vector_layers": [
                {
                    "id": options.layer_name,
                    "minzoom": pmtiles.min_zoom,
                    "maxzoom": pmtiles.max_zoom,
                    "fields": fields,
                }
            ]
        }
    ));

    let map_grid = MapGrid::default();

    let mut tiles_to_calculate = Vec::new();

    for z in &options.zoom_levels {
        let z = *z;
        let (x1, y1, x2, y2) = bbox.to_tiles(z);
        for x in x1..=x2 {
            for y in y1..=y2 {
                tiles_to_calculate.push(TileId::new(x, y, z)?);
            }
        }
    }

    let multi_progress = MultiProgress::new();
    let tiles: Vec<(TileId, Tile)> = tiles_to_calculate
        .into_par_iter()
        .flat_map(|tile_id| {
            let tbounds = map_grid.tile_bbox(tile_id);
            let features = r_tree.locate_in_envelope_intersecting(&AABB::from_corners(
                [tbounds.x_min(), tbounds.y_min()],
                [tbounds.x_max(), tbounds.y_max()],
            ));
            // TODO And figure out clipping
            // TODO Plumb the result
            make_tile(
                tile_id,
                features.collect(),
                &options,
                multi_progress.clone(),
            )
            .unwrap()
        })
        .collect();

    // Assemble the final thing
    for (tile_id, tile) in tiles {
        pmtiles.add_tile(
            get_tile_id(tile_id.z() as u8, tile_id.x() as u64, tile_id.y() as u64),
            tile.to_bytes()?,
        );
    }
    Ok(pmtiles)
}

struct TreeFeature {
    geometry: geo_types::Geometry<f64>,
    properties: Option<geojson::JsonObject>,
}

impl From<geojson::Feature> for TreeFeature {
    fn from(feature: geojson::Feature) -> Self {
        let properties = feature.properties;
        // Geometry must exist
        let mut geometry: geo_types::Geometry<f64> = feature.geometry.unwrap().try_into().unwrap();
        geometry.map_coords_in_place(|p| math::wgs84_to_web_mercator([p.x, p.y]).into());
        return Self {
            properties,
            geometry,
        };
    }
}

impl RTreeObject for TreeFeature {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        let bbox = self.geometry.bounding_rect().unwrap();
        AABB::from_corners([bbox.min().x, bbox.min().y], [bbox.max().x, bbox.max().y])
    }
}

impl TreeFeature {
    fn get_sort_key(&self, key: &str) -> Option<usize> {
        let props = self.properties.as_ref()?;
        let value = props.get(key)?;
        let num = value.as_f64()?;
        Some(num.round() as usize)
    }
}

fn load_features<R: Read>(
    reader: FeatureReader<R>,
) -> Result<(
    RTree<CachedEnvelope<TreeFeature>>,
    usize,
    BBox,
    HashMap<String, String>,
)> {
    // Note we calculate a bbox from WGS84 features instead of using the rtree's envelope. The
    // rtree is in web mercator space, making it harder to calculate the tiles covered
    let mut bbox = BBox::empty();
    let mut tree_features = Vec::new();
    let mut fields = HashMap::new();
    for f in reader.features() {
        let f = f?;
        bbox.add(&f);

        if let Some(ref props) = f.properties {
            for (key, _value) in props {
                // TODO Give a real description based on the JSON value type?
                fields.entry(key.to_string()).or_insert_with(String::new);
            }
        }

        tree_features.push(CachedEnvelope::new(f.into()));
    }
    let num_features = tree_features.len();
    let tree = RTree::bulk_load(tree_features);
    Ok((tree, num_features, bbox, fields))
}

fn make_tile(
    current_tile_id: TileId,
    mut features: Vec<&CachedEnvelope<TreeFeature>>,
    options: &Options,
    multi_progress: MultiProgress,
) -> Result<Option<(TileId, Tile)>> {
    // Start this early to capture the time taken to sort
    let progress = multi_progress.add(progress_bar_for_count(features.len()));

    // We have to do this to each result from RTree, because order is of course not maintained
    // between internal buckets
    if let Some(ref key) = options.sort_by_key {
        features.sort_by_key(|f| f.get_sort_key(key).unwrap_or(0));
        features.reverse();
    }

    let web_mercator_transform = MapGrid::default();
    let transform = web_mercator_transform.tile_transform(current_tile_id);
    let mut tile = Tile::new(4096);

    let mut layer = tile.create_layer(&options.layer_name);

    let mut bytes_so_far = 0;
    let mut skipped = false;
    for feature in features {
        progress.inc(1);
        let mut b = GeomEncoder::new(GeomType::Linestring, Transform::default());

        let mut any = false;
        if let Geometry::LineString(ref line_string) = feature.geometry {
            for pt in line_string {
                // Transform to mercator
                // let mercator_pt = math::wgs84_to_web_mercator([pt[0], pt[1]]);
                // Transform to 0-1 tile coords (not sure why this doesnt work with passing the
                // transform through)
                let transformed_pt = transform * (pt.x, pt.y);

                // If any part of the LineString is within this tile, keep the whole thing. No
                // clipping yet.
                if transformed_pt.x >= 0.0
                    && transformed_pt.x <= 1.0
                    && transformed_pt.y >= 0.0
                    && transformed_pt.y <= 1.0
                {
                    any = true;
                }

                //println!("{:?} becomes {:?} and then {:?}", pt, mercator_pt, transformed_pt);
                // Same as extent
                b = b.point(transformed_pt.x * 4096.0, transformed_pt.y * 4096.0)?;
            }
        }

        if !any {
            // This wasn't a LineString. Totally skip.
            // TODO Fix upstream, because b.encode() didn't fail and wound up generating something
            // that breaks the protobuf parsing in the frontend
            continue;
        }

        let encoded = b.encode()?;
        bytes_so_far += encoded.len();
        // TODO Note we don't use the layer size, because it's expensive to constantly protobuf
        // encode it. This could overcount (ignoring properties) but also undercount (the encoded
        // geometry is further compacted by protobuf?)
        if let Some(limit) = options.limit_size_bytes {
            if bytes_so_far > limit {
                skipped = true;
                progress.finish();
                break;
            }
        }

        let id = layer.num_features() as u64;
        // The ownership swaps between layer and write_feature due to how feature properties are
        // encoded
        let mut write_feature = layer.into_feature(encoded);
        write_feature.set_id(id);

        if let Some(ref props) = feature.properties {
            for (key, value) in props {
                match value {
                    Value::Null => {}
                    Value::Bool(x) => write_feature.add_tag_bool(key, *x),
                    Value::Number(x) => {
                        // TODO Other variations, and maybe use float?
                        if let Some(x) = x.as_f64() {
                            write_feature.add_tag_double(key, x);
                        }
                    }
                    Value::String(x) => write_feature.add_tag_string(key, x),
                    // Encode other cases as strings, like tippecanoe. Note this is probably bad in
                    // the input; unless the possible cases for arrays and objects are small, it'll
                    // take lots to encode these
                    Value::Array(x) => {
                        write_feature.add_tag_string(key, &serde_json::to_string(&x)?)
                    }
                    Value::Object(x) => {
                        write_feature.add_tag_string(key, &serde_json::to_string(&x)?)
                    }
                }
            }
        }

        layer = write_feature.into_layer();
    }

    let num_features = layer.num_features();
    if num_features == 0 {
        // Nothing fit in this tile, just skip it!
        return Ok(None);
    }

    tile.add_layer(layer)?;
    progress.set_style(ProgressStyle::with_template("[{elapsed_precise}] {msg}").unwrap());
    progress.finish_with_message(format!(
        "Added {} features into {}, costing {}{}",
        HumanCount(num_features as u64),
        current_tile_id,
        // TODO Maybe this is slow and we should use to_bytes() once
        HumanBytes(tile.compute_size() as u64),
        if skipped {
            " (skipping some features after hitting size limit)"
        } else {
            ""
        }
    ));

    Ok(Some((current_tile_id, tile)))
}

fn progress_bar_for_count(count: usize) -> ProgressBar {
    ProgressBar::new(count as u64).with_style(ProgressStyle::with_template(
        "[{elapsed_precise}] [{wide_bar:.cyan/blue}] {human_pos}/{human_len} ({per_sec}, {eta})").unwrap())
}
