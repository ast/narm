use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};

use narm::repeaters::{self, NearMatch, Repeater, RepeaterFilter};

#[derive(Args, Debug)]
pub struct RepeatersArgs {
    /// SQLite database path. Defaults to $XDG_DATA_HOME/narm/repeaters.db.
    #[arg(long, env = "NARM_DB", global = true)]
    pub db: Option<PathBuf>,
    #[command(subcommand)]
    pub command: RepeatersCommand,
}

#[derive(Subcommand, Debug)]
pub enum RepeatersCommand {
    /// Import a SSA repeater CSV (https://www.ssa.se/vushf/repeatrar-fyrar/).
    #[command(visible_alias = "i")]
    Import(ImportRepeatersArgs),
    /// List repeaters within a radius of a location.
    #[command(visible_alias = "n")]
    Near(NearArgs),
    /// Full-text search over call, city, district, network (FTS5).
    #[command(visible_alias = "s")]
    Search(SearchArgs),
}

#[derive(Args, Debug)]
pub struct ImportRepeatersArgs {
    /// Path to the SSA repeaters CSV.
    pub csv: PathBuf,
}

#[derive(Args, Debug)]
pub struct NearArgs {
    /// Maidenhead locator (one arg) or "lat lng" coords (two args).
    #[arg(num_args = 1..=2, value_name = "LOCATOR | LAT LNG")]
    pub location: Vec<String>,
    /// Search radius in kilometres.
    #[arg(long, default_value_t = 50.0)]
    pub radius: f64,
    /// Filter by band (e.g. 2, 70, 23). Comma-separated and/or
    /// repeated: --band 2,70 or --band 2 --band 70.
    #[arg(long, value_delimiter = ',')]
    pub band: Vec<String>,
    /// Filter by mode (case-insensitive: fm, dmr, c4fm, dstar).
    /// Comma-separated and/or repeated.
    #[arg(long, value_delimiter = ',')]
    pub mode: Vec<String>,
    /// Maximum number of results (default: no limit).
    #[arg(long)]
    pub limit: Option<usize>,
    /// Emit tab-separated output instead of an aligned table.
    #[arg(long)]
    pub tsv: bool,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Free-text query (terms ANDed together, FTS5 metacharacters like
    /// `-`, `:`, `*` are treated literally). Pass --raw to use FTS5
    /// syntax directly (e.g. `call:SK6*`, `A AND B`, `-noise`).
    pub query: String,
    /// Filter by band (comma-separated and/or repeated).
    #[arg(long, value_delimiter = ',')]
    pub band: Vec<String>,
    /// Filter by mode (comma-separated and/or repeated, case-insensitive).
    #[arg(long, value_delimiter = ',')]
    pub mode: Vec<String>,
    /// Maximum number of results (default: no limit).
    #[arg(long)]
    pub limit: Option<usize>,
    /// Emit tab-separated output instead of an aligned table.
    #[arg(long)]
    pub tsv: bool,
    /// Pass the query verbatim to FTS5 (no escaping).
    #[arg(long)]
    pub raw: bool,
}

pub fn run(args: &RepeatersArgs) -> Result<()> {
    let db_path = match &args.db {
        Some(p) => p.clone(),
        None => repeaters::default_db_path()?,
    };
    match &args.command {
        RepeatersCommand::Import(a) => run_import(&db_path, a),
        RepeatersCommand::Near(a) => run_near(&db_path, a),
        RepeatersCommand::Search(a) => run_search(&db_path, a),
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
    let filter = RepeaterFilter {
        bands: args.band.clone(),
        modes: args.mode.clone(),
        limit: args.limit,
    };
    let hits = repeaters::find_near(&conn, lat, lng, args.radius, &filter)?;

    if hits.is_empty() {
        println!("no repeaters within {} km", args.radius);
        return Ok(());
    }
    let rows: Vec<Vec<String>> = hits.iter().map(near_row).collect();
    render(NEAR_COLS, &rows, args.tsv);
    Ok(())
}

fn run_search(db_path: &Path, args: &SearchArgs) -> Result<()> {
    let conn = repeaters::open_db(db_path)?;
    let filter = RepeaterFilter {
        bands: args.band.clone(),
        modes: args.mode.clone(),
        limit: args.limit,
    };
    let query = if args.raw {
        args.query.clone()
    } else {
        repeaters::escape_fts_query(&args.query)
    };
    let hits = repeaters::fts_search(&conn, &query, &filter)?;

    if hits.is_empty() {
        println!("no repeaters matched {:?}", args.query);
        return Ok(());
    }
    let rows: Vec<Vec<String>> = hits.iter().map(search_row).collect();
    render(SEARCH_COLS, &rows, args.tsv);
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

const NEAR_COLS: &[&str] = &[
    "dist_km", "call", "freq", "shift", "band", "mode", "ch", "city", "locator",
];

const SEARCH_COLS: &[&str] = &[
    "call", "freq", "shift", "band", "mode", "ch", "city", "locator",
];

fn near_row(m: &NearMatch) -> Vec<String> {
    let mut cells = vec![format!("{:.1}", m.distance_km)];
    cells.extend(repeater_cells(&m.repeater));
    cells
}

fn search_row(r: &Repeater) -> Vec<String> {
    repeater_cells(r)
}

fn repeater_cells(r: &Repeater) -> Vec<String> {
    vec![
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

fn render(cols: &[&str], rows: &[Vec<String>], tsv: bool) {
    if tsv {
        print_tsv(cols, rows);
    } else {
        print_table(cols, rows);
    }
}

fn print_tsv(cols: &[&str], rows: &[Vec<String>]) {
    println!("{}", cols.join("\t"));
    for row in rows {
        println!("{}", row.join("\t"));
    }
}

fn print_table(cols: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = cols.iter().map(|c| c.len()).collect();
    for row in rows {
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
    let header: Vec<String> = cols.iter().map(|s| s.to_string()).collect();
    println!("{}", line(&header));
    println!(
        "{}",
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for row in rows {
        println!("{}", line(row));
    }
}
