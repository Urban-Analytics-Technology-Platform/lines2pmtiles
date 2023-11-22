# lines2pmtiles

(Title pending... "and Tyler Too" is an alternative)

This is an experimental Rust crate to convert GeoJSON LineStrings to PMTiles. The goals are:

- pure Rust, to be compatible with WASM
- only target limited inputs for od2net; don't reinvent tippecanoe
  - Only LineStrings, and sorting features by a numeric property to decide what to drop
- Explore if parallelism can help performance

The output is not correct yet; do not use this in production.

Note the first commits are lost in <https://github.com/Urban-Analytics-Technology-Platform/od2net/tree/tippecanwho>
