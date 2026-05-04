//! Human-readable summary of a KG-Q336 codeplug image.
//!
//! Used by the `narm info` CLI verb to pretty-print settings,
//! VFO state, FM broadcast presets, scan groups, and channel
//! list from either a `.kg` (de-mojibaked) image or a 32 KiB
//! raw radio dump that's been re-shaped to the `.kg` layout.

use std::path::Path;

use anyhow::{Context, Result};

use super::{DecodeReport, decode_channels};

/// Pretty-print a decoded image to stdout.
///
/// `source_label` is shown next to the file path in the
/// header (`kg-file` for `.kg` inputs, `raw-radio-dump` for
/// 32 KiB raw images, etc.).
pub fn print_report(file: &Path, source_label: &str, raw: &[u8]) -> Result<()> {
    let report = decode_channels(raw).context("decoding codeplug")?;

    println!("file:       {} ({})", file.display(), source_label);
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

fn print_startup(r: &DecodeReport) {
    println!("[startup message]");
    match r.startup_message.as_deref() {
        Some(s) => println!("  {s:?}"),
        None => println!("  (blank)"),
    }
    println!();
}

fn print_settings(r: &DecodeReport) {
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
        "  work_mode:           A={:?} B={:?}",
        s.work_mode_a, s.work_mode_b
    );
    println!("  work_channel:        A={} B={}", s.work_ch_a, s.work_ch_b);
    println!(
        "  vfostep:             A={:?} B={:?}",
        s.vfostep_a, s.vfostep_b
    );
    println!(
        "  vfo_squelch:         A={} B={}",
        s.vfo_squelch_a, s.vfo_squelch_b
    );
    println!("  busy_lockout:        A={} B={}", s.bcl_a, s.bcl_b);
    println!(
        "  vfoband:             A={:?} B={:?}",
        s.vfoband_a, s.vfoband_b
    );
    println!("  scn_grp_a_act:       {} (encoding TBD)", s.scn_grp_a_act);
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

fn print_vfo_state(r: &DecodeReport) {
    if r.vfo_state.is_empty() {
        return;
    }
    println!("[vfo state] ({} entries)", r.vfo_state.len());
    for (i, v) in r.vfo_state.iter().enumerate() {
        println!("  slot {i}: rx={:>11} Hz  tx={:>11} Hz", v.rx_hz, v.tx_hz);
    }
    println!();
}

fn print_fm_broadcast(r: &DecodeReport) {
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

fn print_scan_groups(r: &DecodeReport) {
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

fn print_call_group(r: &DecodeReport) {
    println!("[call group 1]");
    match r.call_group_1_name.as_deref() {
        Some(s) => println!("  name: {s:?}"),
        None => println!("  (blank)"),
    }
    println!();
}

fn print_channels(r: &DecodeReport) {
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

fn print_warnings(r: &DecodeReport) {
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
