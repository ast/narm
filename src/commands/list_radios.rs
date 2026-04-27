use anyhow::Result;

use narm::Radio;

pub fn run() -> Result<()> {
    for radio in Radio::ALL {
        let modes = radio
            .supported_modes()
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{:20}  {:20}  modes: {}",
            radio.id(),
            radio.display_name(),
            modes,
        );
    }
    Ok(())
}
