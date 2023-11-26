# lines2pmtiles

(Title pending... "and Tyler Too" is an alternative)

This is an experimental Rust crate to convert GeoJSON Points and LineStrings to PMTiles. The goals are:

- pure Rust, to be compatible with WASM
- only target limited inputs for od2net; don't reinvent tippecanoe
  - Only Points and LineStrings, and sorting features by a numeric property to decide what to drop
- Explore if parallelism can help performance

The output is not correct yet; do not use this in production.
