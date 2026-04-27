use std::path::Path;

use anyhow::{Context, Result, anyhow};

use narm::repeaters::{self, NearFilter, NearMatch};

use crate::commands::{ImportRepeatersArgs, NearArgs, RepeatersArgs, RepeatersCommand};

pub fn run(args: &RepeatersArgs) -> Result<()> {
    let db_path = match &args.db {
        Some(p) => p.clone(),
        None => repeaters::default_db_path()?,
    };
    match &args.command {
        RepeatersCommand::Import(a) => run_import(&db_path, a),
        RepeatersCommand::Near(a) => run_near(&db_path, a),
    }
}

fn run_import(db_path: &Path, args: &ImportRepeatersArgs) -> Result<()> {
    let mut conn = repeaters::open_db(db_path)?;
    let stats = repeaters::import_csv(&mut conn, &args.csv)?;
    let total = repeaters::count_rows(&conn)?;
    println!(
        "imported {} rows ({} skipped) into {} — {} total",
        stats.inserted,
        stats.skipped,
        db_path.display(),
        total
    );
    Ok(())
}

fn run_near(db_path: &Path, args: &NearArgs) -> Result<()> {
    let (lat, lng) = parse_location(&args.location)?;
    let conn = repeaters::open_db(db_path)?;
    let filter = NearFilter {
        bands: args.band.clone(),
        modes: args.mode.clone(),
        limit: args.limit,
    };
    let hits = repeaters::find_near(&conn, lat, lng, args.radius, &filter)?;

    if hits.is_empty() {
        println!("no repeaters within {} km", args.radius);
        return Ok(());
    }
    if args.tsv {
        print_tsv(&hits);
    } else {
        print_table(&hits);
    }
    Ok(())
}

fn parse_location(input: &[String]) -> Result<(f64, f64)> {
    match input {
        [single] => {
            let p = narm::decode(single)
                .with_context(|| format!("expected a Maidenhead locator (got {single:?})"))?;
            Ok((p.lat, p.lng))
        }
        [lat, lng] => {
            let lat: f64 = lat.parse().context("lat is not a valid number")?;
            let lng: f64 = lng.parse().context("lng is not a valid number")?;
            Ok((lat, lng))
        }
        _ => Err(anyhow!(
            "expected a locator (1 arg) or lat lng (2 args); got {} args",
            input.len()
        )),
    }
}

const COLS: &[&str] = &[
    "dist_km", "call", "freq", "shift", "band", "mode", "ch", "city", "locator",
];

fn row_cells(m: &NearMatch) -> [String; 9] {
    let r = &m.repeater;
    [
        format!("{:.1}", m.distance_km),
        r.call.clone(),
        r.output.map(|f| format!("{f:.4}")).unwrap_or_default(),
        r.tx_shift.map(|f| format!("{f:+.3}")).unwrap_or_default(),
        r.band.clone().unwrap_or_default(),
        r.mode.clone().unwrap_or_default(),
        r.channel.clone().unwrap_or_default(),
        r.city.clone().unwrap_or_default(),
        r.locator.clone().unwrap_or_default(),
    ]
}

fn print_tsv(hits: &[NearMatch]) {
    println!("{}", COLS.join("\t"));
    for m in hits {
        println!("{}", row_cells(m).join("\t"));
    }
}

fn print_table(hits: &[NearMatch]) {
    let rows: Vec<[String; 9]> = hits.iter().map(row_cells).collect();
    let mut widths = COLS.iter().map(|c| c.len()).collect::<Vec<_>>();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let line = |cells: &[String]| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let pad = widths[i].saturating_sub(c.chars().count());
                format!("{c}{}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    let header: Vec<String> = COLS.iter().map(|s| s.to_string()).collect();
    println!("{}", line(&header));
    println!(
        "{}",
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for row in &rows {
        println!("{}", line(row));
    }
}
