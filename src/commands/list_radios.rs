use anyhow::Result;

use narm::Radio;

pub fn run() -> Result<()> {
    for radio in Radio::ALL {
        println!(
            "{:20}  {:20}  modes: {}",
            radio.id(),
            radio.display_name(),
            radio.supported_modes().join(", "),
        );
    }
    Ok(())
}
