use anyhow::{Context, Result};
use clap::Args;

use narm::{LatLng, encode};

const DEFAULT_ENCODE_LENGTH: usize = 6;

#[derive(Args, Debug)]
pub struct GridArgs {
    /// Either a Maidenhead locator (e.g. JO67AT) — one arg —
    /// or a "lat lng" pair (e.g. 57.8125 12.0417) — two args.
    #[arg(num_args = 1..=2, value_name = "LOCATOR | LAT LNG")]
    pub input: Vec<String>,
}

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
