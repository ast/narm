use anyhow::{Context, Result};

use narm::{LatLng, encode};

use crate::commands::GridArgs;

const DEFAULT_ENCODE_LENGTH: usize = 6;

pub fn run(args: &GridArgs) -> Result<()> {
    match args.input.as_slice() {
        [single] => {
            let coords = narm::decode(single)?;
            println!("{:.4} {:.4}", coords.lat, coords.lng);
        }
        [lat, lng] => {
            let lat: f64 = lat.parse().context("lat is not a valid number")?;
            let lng: f64 = lng.parse().context("lng is not a valid number")?;
            let loc = encode(LatLng { lat, lng }, DEFAULT_ENCODE_LENGTH)?;
            println!("{loc}");
        }
        // clap's num_args = 1..=2 prevents the empty / >2 cases.
        _ => unreachable!(),
    }
    Ok(())
}
