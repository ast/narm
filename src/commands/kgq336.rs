use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use narm::kgq336;

#[derive(Args, Debug)]
pub struct Kgq336Args {
    #[command(subcommand)]
    pub command: Kgq336Command,
}

#[derive(Subcommand, Debug)]
pub enum Kgq336Command {
    /// Pretty-print the radio-wide settings, scan groups, VFO
    /// state, FM broadcast presets, and channel summary from a
    /// `.kg` codeplug file.
    #[command(visible_alias = "i")]
    Inspect(InspectArgs),
}

#[derive(Args, Debug)]
pub struct InspectArgs {
    /// Path to a `.kg` file produced by the Wouxun CPS.
    pub file: PathBuf,
}

pub fn run(args: Kgq336Args) -> Result<()> {
    match args.command {
        Kgq336Command::Inspect(a) => run_inspect(a),
    }
}

fn run_inspect(args: InspectArgs) -> Result<()> {
    let bytes =
        std::fs::read(&args.file).with_context(|| format!("reading {}", args.file.display()))?;
    let raw = kgq336::unmojibake(&bytes).context("de-mojibaking .kg file")?;
    let report = kgq336::decode_channels(&raw).context("decoding codeplug")?;

    println!("file:       {}", args.file.display());
    println!("size (raw): {} bytes", raw.len());
    println!();

    print_startup(&report);
    print_settings(&report);
    print_vfo_state(&report);
    print_fm_broadcast(&report);
    print_scan_groups(&report);
    print_call_group(&report);
    print_channels(&report);
    print_warnings(&report);

    Ok(())
}

fn print_startup(r: &kgq336::DecodeReport) {
    println!("[startup message]");
    match r.startup_message.as_deref() {
        Some(s) => println!("  {s:?}"),
        None => println!("  (blank)"),
    }
    println!();
}

fn print_settings(r: &kgq336::DecodeReport) {
    let s = match &r.settings {
        Some(s) => s,
        None => {
            println!("[settings] (image too short)\n");
            return;
        }
    };
    println!("[settings]");
    println!("  battery_save:        {}", s.battery_save);
    println!("  roger:               {:?}", s.roger);
    println!("  tot:                 {}", s.tot);
    println!("  tot_pre_alert:       {} s", s.tot_pre_alert_seconds);
    println!("  vox:                 {}", s.vox);
    println!("  language:            {:?}", s.language);
    println!("  voice_guide:         {}", s.voice_guide);
    println!("  beep:                {}", s.beep);
    println!("  scan_mode:           {:?}", s.scan_mode);
    println!("  backlight_seconds:   {}", s.backlight_seconds);
    println!("  brightness_active:   {}", s.brightness_active);
    println!("  startup_display:     {:?}", s.startup_display);
    println!("  ptt_id:              {:?}", s.ptt_id);
    println!("  sidetone:            {:?}", s.sidetone);
    println!("  dtmf_transmit_time:  {} ms", s.dtmf_transmit_time_ms);
    println!("  alert:               {:?}", s.alert);
    println!("  auto_lock:           {}", s.auto_lock);
    println!("  priority_channel:    {}", s.priority_channel);
    println!("  rpt_setting:         {}", s.rpt_setting);
    println!("  rpt_spk:             {}", s.rpt_spk);
    println!("  scan_det:            {}", s.scan_det);
    println!("  sub_freq_mute:       {:?}", s.sub_freq_mute);
    println!("  sc_qt:               {:?}", s.sc_qt);
    println!("  theme:               {}", s.theme);
    println!("  time_zone:           {}", s.time_zone);
    println!("  gps_on:              {}", s.gps_on);
    println!(
        "  vfo_squelch:         A={} B={}",
        s.vfo_squelch_a, s.vfo_squelch_b
    );
    println!("  top_key:             {:?}", s.top_key);
    println!("  pf1_short:           {}", s.pf1_short);
    println!("  pf2_long:            {}", s.pf2_long);
    println!("  pf3_short:           {}", s.pf3_short);
    println!(
        "  mode_switch_pwd:     {:?}",
        printable_ascii(&s.mode_switch_password)
    );
    println!(
        "  reset_password:      {:?}",
        printable_ascii(&s.reset_password)
    );
    println!("  ani_code:            {:?}", s.ani_code_string());
    println!("  scc_code:            {:?}", s.scc_code_string());
    println!();
}

fn print_vfo_state(r: &kgq336::DecodeReport) {
    if r.vfo_state.is_empty() {
        return;
    }
    println!("[vfo state] ({} entries)", r.vfo_state.len());
    for (i, v) in r.vfo_state.iter().enumerate() {
        println!("  slot {i}: rx={:>11} Hz  tx={:>11} Hz", v.rx_hz, v.tx_hz);
    }
    println!();
}

fn print_fm_broadcast(r: &kgq336::DecodeReport) {
    if r.fm_broadcast.is_empty() {
        return;
    }
    let unique: std::collections::BTreeSet<u64> = r.fm_broadcast.iter().copied().collect();
    println!(
        "[fm broadcast] {} slots, {} unique frequencies",
        r.fm_broadcast.len(),
        unique.len()
    );
    for f in &unique {
        println!("  {:.3} MHz", *f as f64 / 1_000_000.0);
    }
    println!();
}

fn print_scan_groups(r: &kgq336::DecodeReport) {
    if r.scan_groups.is_empty() {
        return;
    }
    println!("[scan groups]");
    for g in &r.scan_groups {
        let name = if g.name.is_empty() {
            "(blank)".to_string()
        } else {
            format!("\"{}\"", g.name)
        };
        let range = if g.start_channel == 0 && g.end_channel == 0 {
            "(unset)".to_string()
        } else {
            format!("CH-{:03}..CH-{:03}", g.start_channel, g.end_channel)
        };
        println!("  {:>2}. {:<20} {}", g.index, name, range);
    }
    println!();
}

fn print_call_group(r: &kgq336::DecodeReport) {
    println!("[call group 1]");
    match r.call_group_1_name.as_deref() {
        Some(s) => println!("  name: {s:?}"),
        None => println!("  (blank)"),
    }
    println!();
}

fn print_channels(r: &kgq336::DecodeReport) {
    println!("[channels] {} populated", r.channels.len());
    if r.channels.is_empty() {
        println!();
        return;
    }
    for ch in &r.channels {
        let tx_hz = ch.rx_hz as i64 + ch.shift_hz;
        println!("  rx={:>9} Hz  tx={:>9} Hz  {}", ch.rx_hz, tx_hz, ch.name);
    }
    println!();
}

fn print_warnings(r: &kgq336::DecodeReport) {
    if r.warnings.is_empty() {
        return;
    }
    println!("[warnings] ({} entries)", r.warnings.len());
    for w in &r.warnings {
        println!("  - {w}");
    }
}

/// Render a fixed-width ASCII byte slot as a quoted string,
/// stopping at the first NUL or `0xFF` so trailing padding
/// doesn't leak in.
fn printable_ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0 && b != 0xFF)
        .map(|&b| b as char)
        .collect()
}
