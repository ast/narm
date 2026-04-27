//! SQLite-backed repeater store.
//!
//! Schema mirrors the SSA CSV (https://www.ssa.se/vushf/repeatrar-fyrar/),
//! with a contentless FTS5 virtual table over the searchable text columns.
//! Re-imports use `INSERT OR REPLACE`, keyed by the stable SSA `id`.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Deserialize;

const EARTH_RADIUS_KM: f64 = 6371.0088;

#[derive(Debug, Clone, Default)]
pub struct Repeater {
    pub id: i64,
    pub updated: Option<String>,
    pub kind: Option<String>,
    pub band: Option<String>,
    pub mode: Option<String>,
    pub network: Option<String>,
    pub network_id: Option<String>,
    pub district: Option<i64>,
    pub call: String,
    pub city: Option<String>,
    pub channel: Option<String>,
    pub output: Option<f64>,
    pub tx_shift: Option<f64>,
    pub access: Option<String>,
    pub status: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub locator: Option<String>,
    pub masl: Option<i64>,
    pub magl: Option<i64>,
    pub watt_pep: Option<f64>,
    pub dir: Option<String>,
    pub ant: Option<String>,
    pub backup: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NearMatch {
    pub repeater: Repeater,
    pub distance_km: f64,
}

#[derive(Debug, Clone, Default)]
pub struct NearFilter {
    /// Empty means "any band". Match against the `band` column verbatim.
    pub bands: Vec<String>,
    /// Empty means "any mode". Compared case-insensitively.
    pub modes: Vec<String>,
    pub limit: Option<usize>,
}

/// Resolve the default DB path: `$XDG_DATA_HOME/narm/repeaters.db`.
pub fn default_db_path() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| anyhow!("could not resolve XDG data directory"))?
        .join("narm");
    Ok(dir.join("repeaters.db"))
}

/// Open (and lazily create) the SQLite DB at `path`, ensuring schema is in
/// place. The parent directory is created if missing.
pub fn open_db(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating db directory {}", parent.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("opening sqlite db at {}", path.display()))?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS repeaters (
            id          INTEGER PRIMARY KEY,
            updated     TEXT,
            type        TEXT,
            band        TEXT,
            mode        TEXT,
            network     TEXT,
            network_id  TEXT,
            district    INTEGER,
            call        TEXT NOT NULL,
            city        TEXT,
            channel     TEXT,
            output      REAL,
            tx_shift    REAL,
            access      TEXT,
            status      TEXT,
            lat         REAL,
            lng         REAL,
            locator     TEXT,
            masl        INTEGER,
            magl        INTEGER,
            watt_pep    REAL,
            dir         TEXT,
            ant         TEXT,
            backup      INTEGER
        );

        CREATE INDEX IF NOT EXISTS repeaters_latlng ON repeaters(lat, lng);
        CREATE INDEX IF NOT EXISTS repeaters_band   ON repeaters(band);
        CREATE INDEX IF NOT EXISTS repeaters_mode   ON repeaters(mode);

        CREATE VIRTUAL TABLE IF NOT EXISTS repeaters_fts USING fts5(
            call, city, district, network,
            content='repeaters',
            content_rowid='id'
        );

        CREATE TRIGGER IF NOT EXISTS repeaters_ai AFTER INSERT ON repeaters BEGIN
            INSERT INTO repeaters_fts(rowid, call, city, district, network)
            VALUES (new.id, new.call, new.city, CAST(new.district AS TEXT), new.network);
        END;

        CREATE TRIGGER IF NOT EXISTS repeaters_ad AFTER DELETE ON repeaters BEGIN
            INSERT INTO repeaters_fts(repeaters_fts, rowid, call, city, district, network)
            VALUES ('delete', old.id, old.call, old.city, CAST(old.district AS TEXT), old.network);
        END;

        CREATE TRIGGER IF NOT EXISTS repeaters_au AFTER UPDATE ON repeaters BEGIN
            INSERT INTO repeaters_fts(repeaters_fts, rowid, call, city, district, network)
            VALUES ('delete', old.id, old.call, old.city, CAST(old.district AS TEXT), old.network);
            INSERT INTO repeaters_fts(rowid, call, city, district, network)
            VALUES (new.id, new.call, new.city, CAST(new.district AS TEXT), new.network);
        END;
        "#,
    )
    .context("initialising repeaters schema")?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct CsvRow {
    id: i64,
    updated: String,
    #[serde(rename = "type")]
    kind: String,
    band: String,
    mode: String,
    network: String,
    network_id: String,
    district: String,
    call: String,
    city: String,
    channel: String,
    output: String,
    tx_shift: String,
    access: String,
    status: String,
    lat: String,
    lng: String,
    locator: String,
    masl: String,
    magl: String,
    watt_pep: String,
    dir: String,
    ant: String,
    backup: String,
}

fn opt_str(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

fn opt_parse<T: FromStr>(s: &str) -> Option<T> {
    if s.is_empty() { None } else { s.parse().ok() }
}

impl From<CsvRow> for Repeater {
    fn from(r: CsvRow) -> Self {
        Repeater {
            id: r.id,
            updated: opt_str(r.updated),
            kind: opt_str(r.kind),
            band: opt_str(r.band),
            mode: opt_str(r.mode),
            network: opt_str(r.network),
            network_id: opt_str(r.network_id),
            district: opt_parse(&r.district),
            call: r.call,
            city: opt_str(r.city),
            channel: opt_str(r.channel),
            output: opt_parse(&r.output),
            tx_shift: opt_parse(&r.tx_shift),
            access: opt_str(r.access),
            status: opt_str(r.status),
            lat: opt_parse(&r.lat),
            lng: opt_parse(&r.lng),
            locator: opt_str(r.locator),
            masl: opt_parse(&r.masl),
            magl: opt_parse(&r.magl),
            watt_pep: opt_parse(&r.watt_pep),
            dir: opt_str(r.dir),
            ant: opt_str(r.ant),
            backup: opt_parse(&r.backup),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ImportStats {
    pub inserted: usize,
    pub skipped: usize,
}

/// Import a SSA repeater CSV into the database. Re-importing the same CSV
/// updates rows in place (stable `id`).
pub fn import_csv(conn: &mut Connection, csv_path: &Path) -> Result<ImportStats> {
    let mut text = fs::read_to_string(csv_path)
        .with_context(|| format!("reading csv {}", csv_path.display()))?;
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(b';')
        .has_headers(true)
        .from_reader(text.as_bytes());

    let tx = conn.transaction()?;
    let mut stats = ImportStats::default();
    {
        let mut stmt = tx.prepare(
            r#"
            INSERT OR REPLACE INTO repeaters (
                id, updated, type, band, mode, network, network_id, district,
                call, city, channel, output, tx_shift, access, status,
                lat, lng, locator, masl, magl, watt_pep, dir, ant, backup
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24
            )
            "#,
        )?;

        for (line_no, result) in rdr.deserialize::<CsvRow>().enumerate() {
            let row: CsvRow = match result {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("skipping line {}: {e}", line_no + 2);
                    stats.skipped += 1;
                    continue;
                }
            };
            let r: Repeater = row.into();
            stmt.execute(params![
                r.id,
                r.updated,
                r.kind,
                r.band,
                r.mode,
                r.network,
                r.network_id,
                r.district,
                r.call,
                r.city,
                r.channel,
                r.output,
                r.tx_shift,
                r.access,
                r.status,
                r.lat,
                r.lng,
                r.locator,
                r.masl,
                r.magl,
                r.watt_pep,
                r.dir,
                r.ant,
                r.backup,
            ])?;
            stats.inserted += 1;
        }
    }
    tx.commit()?;
    Ok(stats)
}

pub fn count_rows(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM repeaters", [], |r| r.get(0))
        .context("counting rows")
}

/// Find repeaters within `radius_km` of (`lat`,`lng`), with optional band/mode
/// filters and result cap. Sorted ascending by distance. Empty `bands`/`modes`
/// vecs mean "no filter".
pub fn find_near(
    conn: &Connection,
    lat: f64,
    lng: f64,
    radius_km: f64,
    filter: &NearFilter,
) -> Result<Vec<NearMatch>> {
    let (lat_min, lat_max, lng_min, lng_max) = bbox(lat, lng, radius_km);

    let mut sql = String::from(
        r#"
        SELECT id, updated, type, band, mode, network, network_id, district,
               call, city, channel, output, tx_shift, access, status,
               lat, lng, locator, masl, magl, watt_pep, dir, ant, backup
          FROM repeaters
         WHERE lat IS NOT NULL AND lng IS NOT NULL
           AND lat BETWEEN ? AND ?
           AND lng BETWEEN ? AND ?
        "#,
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![
        Box::new(lat_min),
        Box::new(lat_max),
        Box::new(lng_min),
        Box::new(lng_max),
    ];

    if !filter.bands.is_empty() {
        sql.push_str(" AND band IN (");
        sql.push_str(&vec!["?"; filter.bands.len()].join(","));
        sql.push(')');
        for b in &filter.bands {
            params.push(Box::new(b.clone()));
        }
    }
    if !filter.modes.is_empty() {
        sql.push_str(" AND LOWER(mode) IN (");
        sql.push_str(&vec!["?"; filter.modes.len()].join(","));
        sql.push(')');
        for m in &filter.modes {
            params.push(Box::new(m.to_lowercase()));
        }
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|p| &**p as &dyn rusqlite::ToSql)),
        row_to_repeater,
    )?;

    let mut hits: Vec<NearMatch> = rows
        .filter_map(|r| r.ok())
        .filter_map(|rep| {
            let (rlat, rlng) = (rep.lat?, rep.lng?);
            let d = haversine_km(lat, lng, rlat, rlng);
            (d <= radius_km).then_some(NearMatch {
                repeater: rep,
                distance_km: d,
            })
        })
        .collect();

    hits.sort_by(|a, b| a.distance_km.partial_cmp(&b.distance_km).unwrap());
    if let Some(n) = filter.limit {
        hits.truncate(n);
    }
    Ok(hits)
}

#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    /// Empty means "any band". Match against the `band` column verbatim.
    pub bands: Vec<String>,
    /// Empty means "any mode". Compared case-insensitively.
    pub modes: Vec<String>,
    pub limit: Option<usize>,
}

/// Escape a free-text user query for FTS5: split on whitespace, wrap each
/// term in FTS5 phrase quotes (doubling internal `"`), and join with
/// spaces (FTS5 implicit AND). A trailing `*` on a term is preserved as
/// the FTS5 phrase-prefix form (`"prefix"*`), so `SK6*` still does what
/// you'd expect. All other FTS5 metacharacters (`-`, `:`, `+`, parens)
/// are treated literally.
///
/// Power users who want raw FTS5 syntax (column filters like
/// `call:SK6*`, boolean `A AND B`, etc.) should bypass this and pass
/// their string through unchanged.
pub fn escape_fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| {
            if let Some(prefix) = term.strip_suffix('*')
                && !prefix.is_empty()
            {
                format!("\"{}\"*", prefix.replace('"', "\"\""))
            } else {
                format!("\"{}\"", term.replace('"', "\"\""))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Full-text search over `call`, `city`, `district`, `network`. `query` is
/// FTS5 syntax — supports column filters (`call:SA*`), prefix matching,
/// AND/OR/NEAR. Results sorted by FTS `rank` (best match first).
/// For free-text user input, run it through [`escape_fts_query`] first.
pub fn fts_search(conn: &Connection, query: &str, filter: &SearchFilter) -> Result<Vec<Repeater>> {
    let mut sql = String::from(
        r#"
        SELECT r.id, r.updated, r.type, r.band, r.mode, r.network, r.network_id, r.district,
               r.call, r.city, r.channel, r.output, r.tx_shift, r.access, r.status,
               r.lat, r.lng, r.locator, r.masl, r.magl, r.watt_pep, r.dir, r.ant, r.backup
          FROM repeaters r
          JOIN repeaters_fts f ON f.rowid = r.id
         WHERE repeaters_fts MATCH ?
        "#,
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(query.to_string())];

    if !filter.bands.is_empty() {
        sql.push_str(" AND r.band IN (");
        sql.push_str(&vec!["?"; filter.bands.len()].join(","));
        sql.push(')');
        for b in &filter.bands {
            params.push(Box::new(b.clone()));
        }
    }
    if !filter.modes.is_empty() {
        sql.push_str(" AND LOWER(r.mode) IN (");
        sql.push_str(&vec!["?"; filter.modes.len()].join(","));
        sql.push(')');
        for m in &filter.modes {
            params.push(Box::new(m.to_lowercase()));
        }
    }

    sql.push_str(" ORDER BY rank");
    if let Some(n) = filter.limit {
        sql.push_str(&format!(" LIMIT {n}"));
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|p| &**p as &dyn rusqlite::ToSql)),
        row_to_repeater,
    )?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| anyhow!("fts query failed: {e}"))
}

fn row_to_repeater(r: &Row<'_>) -> rusqlite::Result<Repeater> {
    Ok(Repeater {
        id: r.get(0)?,
        updated: r.get(1)?,
        kind: r.get(2)?,
        band: r.get(3)?,
        mode: r.get(4)?,
        network: r.get(5)?,
        network_id: r.get(6)?,
        district: r.get(7)?,
        call: r.get(8)?,
        city: r.get(9)?,
        channel: r.get(10)?,
        output: r.get(11)?,
        tx_shift: r.get(12)?,
        access: r.get(13)?,
        status: r.get(14)?,
        lat: r.get(15)?,
        lng: r.get(16)?,
        locator: r.get(17)?,
        masl: r.get(18)?,
        magl: r.get(19)?,
        watt_pep: r.get(20)?,
        dir: r.get(21)?,
        ant: r.get(22)?,
        backup: r.get(23)?,
    })
}

pub fn haversine_km(lat1: f64, lng1: f64, lat2: f64, lng2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlng = (lng2 - lng1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlng / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * a.sqrt().asin()
}

fn bbox(lat: f64, lng: f64, radius_km: f64) -> (f64, f64, f64, f64) {
    let lat_delta = radius_km / 111.0;
    // Guard against latitudes near the poles where cos(lat) ≈ 0 would blow
    // up the longitude delta. Below ≈85° this never matters in practice.
    let cos_lat = lat.to_radians().cos().abs().max(1e-6);
    let lng_delta = radius_km / (111.0 * cos_lat);
    (
        lat - lat_delta,
        lat + lat_delta,
        lng - lng_delta,
        lng + lng_delta,
    )
}

// Used by `validate` row count helper; stays here so the test isn't cross-file.
#[allow(dead_code)]
pub fn fetch_by_id(conn: &Connection, id: i64) -> Result<Option<Repeater>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, updated, type, band, mode, network, network_id, district,
               call, city, channel, output, tx_shift, access, status,
               lat, lng, locator, masl, magl, watt_pep, dir, ant, backup
          FROM repeaters WHERE id = ?1
        "#,
    )?;
    Ok(stmt.query_row([id], row_to_repeater).optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_known_distance() {
        // London (51.5074, -0.1278) → Paris (48.8566, 2.3522): about 343 km.
        let d = haversine_km(51.5074, -0.1278, 48.8566, 2.3522);
        assert!((d - 343.5).abs() < 2.0, "got {d}");
    }

    #[test]
    fn haversine_zero_for_same_point() {
        assert!(haversine_km(57.0, 12.0, 57.0, 12.0).abs() < 1e-9);
    }

    #[test]
    fn bbox_grows_with_latitude() {
        let (_, _, lng_min_low, lng_max_low) = bbox(0.0, 0.0, 100.0);
        let (_, _, lng_min_high, lng_max_high) = bbox(60.0, 0.0, 100.0);
        // At higher latitude the same km of east-west distance covers more
        // degrees of longitude, so the box should be wider.
        assert!((lng_max_high - lng_min_high) > (lng_max_low - lng_min_low));
    }

    #[test]
    fn import_then_query_roundtrip() {
        let csv = "id;updated;type;band;mode;network;network_id;district;call;city;channel;output;tx_shift;access;status;lat;lng;locator;masl;magl;watt_pep;dir;ant;backup\n\
                    10;\"\";\"Repeater\";70;\"FM\";\"\";\"\";6;\"SA6AR/R\";\"Angered\";\"RU394\";434.925;-2;1750;\"QRV\";57.8125;12.0417;\"JO67AT\";\"\";\"\";\"\";\"\";\"\";0\n\
                    20;\"\";\"Repeater\";2;\"FM\";\"\";\"\";2;\"SK2AU/R\";\"Skellefteå\";\"RV56\";145.7;-0.6;1750;\"QRV\";64.774;20.95;\"KP04LS\";248;125;75;\"\";\"\";1\n";

        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("r.csv");
        std::fs::write(&csv_path, csv).unwrap();
        let db_path = dir.path().join("r.db");
        let mut conn = open_db(&db_path).unwrap();

        let stats = import_csv(&mut conn, &csv_path).unwrap();
        assert_eq!(stats.inserted, 2);
        assert_eq!(stats.skipped, 0);
        assert_eq!(count_rows(&conn).unwrap(), 2);

        // Re-import is idempotent (same IDs → REPLACE).
        let stats2 = import_csv(&mut conn, &csv_path).unwrap();
        assert_eq!(stats2.inserted, 2);
        assert_eq!(count_rows(&conn).unwrap(), 2);

        // Near search around Gothenburg (≈SA6AR) should hit SA6AR but not the
        // Skellefteå unit ~1100 km north.
        let hits = find_near(&conn, 57.71, 11.97, 50.0, &NearFilter::default()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].repeater.call, "SA6AR/R");
        assert!(hits[0].distance_km < 25.0, "got {}", hits[0].distance_km);

        // Big radius pulls in both, ordered by distance.
        let big = find_near(&conn, 57.71, 11.97, 5000.0, &NearFilter::default()).unwrap();
        assert_eq!(big.len(), 2);
        assert!(big[0].distance_km < big[1].distance_km);

        // Single-band filter excludes the 70 cm one.
        let only_2m = find_near(
            &conn,
            57.71,
            11.97,
            5000.0,
            &NearFilter {
                bands: vec!["2".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(only_2m.len(), 1);
        assert_eq!(only_2m[0].repeater.band.as_deref(), Some("2"));

        // Multi-band filter pulls in both 2 m and 70 cm.
        let two_bands = find_near(
            &conn,
            57.71,
            11.97,
            5000.0,
            &NearFilter {
                bands: vec!["2".into(), "70".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(two_bands.len(), 2);

        // FTS hit on city.
        let fts_hits = fts_search(&conn, "Angered", &SearchFilter::default()).unwrap();
        assert_eq!(fts_hits.len(), 1);
        assert_eq!(fts_hits[0].call, "SA6AR/R");

        // FTS + band filter narrows to the 70 cm Angered repeater.
        let fts_70 = fts_search(
            &conn,
            "Angered",
            &SearchFilter {
                bands: vec!["70".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(fts_70.len(), 1);
        assert_eq!(fts_70[0].band.as_deref(), Some("70"));

        // FTS + band filter that excludes the only hit returns nothing.
        let fts_2 = fts_search(
            &conn,
            "Angered",
            &SearchFilter {
                bands: vec!["2".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(fts_2.is_empty());

        // Free-text input with FTS5 metacharacters round-trips through the
        // escape helper. "D-Star" would otherwise be parsed as `D NOT
        // Star` and fail with a column-name error.
        let escaped = escape_fts_query("D-Star");
        let dstar = fts_search(&conn, &escaped, &SearchFilter::default()).unwrap();
        assert!(
            dstar.is_empty(),
            "no D-Star rows in fixture, but query parses"
        );
    }

    #[test]
    fn escape_fts_query_basic() {
        assert_eq!(escape_fts_query(""), "");
        assert_eq!(escape_fts_query("Angered"), "\"Angered\"");
    }

    #[test]
    fn escape_fts_query_metacharacters() {
        // Hyphen, colon, plus would otherwise be FTS5 operators.
        assert_eq!(escape_fts_query("D-Star"), "\"D-Star\"");
        assert_eq!(escape_fts_query("foo+bar"), "\"foo+bar\"");
    }

    #[test]
    fn escape_fts_query_trailing_wildcard() {
        // Bare prefix → FTS5 phrase-prefix form, the wildcard escapes
        // the closing quote.
        assert_eq!(escape_fts_query("SK6*"), "\"SK6\"*");
        assert_eq!(escape_fts_query("D-Star*"), "\"D-Star\"*");
        // `call:SK6*` ends with `*` so it becomes a phrase-prefix on the
        // literal text "call:SK6" — column filters still need --raw.
        assert_eq!(escape_fts_query("call:SK6*"), "\"call:SK6\"*");
        // Bare `*` has no prefix; escape literally so FTS5 sees a phrase.
        assert_eq!(escape_fts_query("*"), "\"*\"");
    }

    #[test]
    fn escape_fts_query_multi_term_implicit_and() {
        // Whitespace splits into separate phrase-quoted terms; FTS5
        // joins adjacent terms with implicit AND.
        assert_eq!(escape_fts_query("Angered SK6"), "\"Angered\" \"SK6\"");
        assert_eq!(
            escape_fts_query("D-Star  Mölndal"),
            "\"D-Star\" \"Mölndal\"",
        );
    }

    #[test]
    fn escape_fts_query_doubles_internal_quotes() {
        assert_eq!(escape_fts_query("foo\"bar"), "\"foo\"\"bar\"");
    }
}
