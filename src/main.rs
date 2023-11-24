use std::io::BufReader;

use anyhow::Result;
use fs_err::File;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        panic!("Pass in a .geojson file");
    }

    // TODO use clap
    let options = lines2pmtiles::Options {
        layer_name: "layer1".to_string(),
        sort_by_key: Some("count".to_string()),
        zoom_levels: (0..13).collect(),
        // This is so much less than 500KB, but the final tile size is still big
        limit_size_bytes: Some(200 * 1024),
    };

    let reader = BufReader::new(File::open(&args[1])?);
    let pmtiles = lines2pmtiles::geojson_to_pmtiles(reader, options)?;
    println!("Writing out.pmtiles");
    let mut file = File::create("out.pmtiles")?;
    pmtiles.to_writer(&mut file)?;
    Ok(())
}
