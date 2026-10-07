//! The tradable universe: `config/universe.csv` loaded once into a [`Universe`].
//!
//! The file is the single list of names the screener scores, the report groups
//! ("5 of today's 8 picks are semis") and the backtest later iterates. Columns:
//!
//! ```text
//! symbol,name,sector,themes,region
//! NVDA,Nvidia,semis,ai|datacenter|gpu,US
//! ```
//!
//! `themes` is a `|`-separated list. ETFs carry `sector = etf` (or `fund`); they
//! are tracked for aggregates and context but are never buy candidates, see
//! [`Instrument::is_etf`]. An optional `watch` column (`1`, `yes`, `true`) marks the
//! names the owner always wants covered in the report whether or not the rule picks
//! them; absent column means no watchlist. Unknown columns are ignored so the file can grow
//! without a code change; a missing required column or a duplicate symbol is a
//! hard error, because a silently half-loaded universe would skew every report.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Deserializer};

use super::error::UniverseError;
use super::types::Symbol;

/// Sector values that mean "a fund, not a company". Lower-cased before comparison.
const ETF_SECTORS: &[&str] = &["etf", "fund"];

/// One row of the universe file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Instrument {
    pub symbol: Symbol,
    pub name: String,
    pub sector: String,
    #[serde(deserialize_with = "pipe_list")]
    pub themes: Vec<String>,
    pub region: String,
    /// Always covered in the report (headlines + note), with its rank shown.
    #[serde(default, deserialize_with = "yes_no")]
    pub watch: bool,
}

impl Instrument {
    /// Funds are tracked (they anchor sector and regime reads) but never bought
    /// by the screener, so `sector` doubles as the asset-type flag.
    pub fn is_etf(&self) -> bool {
        ETF_SECTORS.contains(&self.sector.to_ascii_lowercase().as_str())
    }

    pub fn has_theme(&self, theme: &str) -> bool {
        self.themes.iter().any(|t| t.eq_ignore_ascii_case(theme))
    }
}

/// The loaded file: rows in file order plus a symbol index.
#[derive(Debug, Clone, Default)]
pub struct Universe {
    instruments: Vec<Instrument>,
    by_symbol: HashMap<Symbol, usize>,
}

impl Universe {
    /// Read and parse `path`. The usual call is `Universe::load("config/universe.csv")`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, UniverseError> {
        let path = path.as_ref();
        let file = std::fs::File::open(path).map_err(|source| UniverseError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_reader(file)
    }

    /// Parse CSV text from any reader. Split out from [`Universe::load`] so tests
    /// feed string fixtures without touching the filesystem.
    pub fn from_reader<R: Read>(reader: R) -> Result<Self, UniverseError> {
        let mut rdr = csv::ReaderBuilder::new()
            .trim(csv::Trim::All)
            .flexible(true) // ignore stray trailing cells; serde still enforces required fields
            .from_reader(reader);

        // Check the header up front so the error names the column, not "missing
        // field `sector` at line 2" for every row.
        let headers = rdr
            .headers()
            .map_err(|source| UniverseError::Row { line: 1, source })?;
        for required in ["symbol", "name", "sector", "themes", "region"] {
            if !headers.iter().any(|h| h == required) {
                return Err(UniverseError::MissingColumn(required));
            }
        }

        let mut instruments: Vec<Instrument> = Vec::new();
        let mut by_symbol: HashMap<Symbol, usize> = HashMap::new();
        let mut first_line: HashMap<Symbol, u64> = HashMap::new();

        for (i, row) in rdr.deserialize::<Instrument>().enumerate() {
            // csv positions are 0-based byte records; humans read 1-based lines
            // with the header on line 1, hence `+ 2`.
            let line = i as u64 + 2;
            let inst = row.map_err(|source| UniverseError::Row { line, source })?;
            if inst.symbol.as_str().is_empty() {
                return Err(UniverseError::EmptySymbol { line });
            }
            if let Some(&first) = first_line.get(&inst.symbol) {
                return Err(UniverseError::Duplicate {
                    symbol: inst.symbol.0.clone(),
                    line,
                    first,
                });
            }
            first_line.insert(inst.symbol.clone(), line);
            by_symbol.insert(inst.symbol.clone(), instruments.len());
            instruments.push(inst);
        }

        Ok(Self {
            instruments,
            by_symbol,
        })
    }

    pub fn len(&self) -> usize {
        self.instruments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instruments.is_empty()
    }

    /// All rows, in file order.
    pub fn iter(&self) -> impl Iterator<Item = &Instrument> {
        self.instruments.iter()
    }

    pub fn get(&self, symbol: &Symbol) -> Option<&Instrument> {
        self.by_symbol.get(symbol).map(|&i| &self.instruments[i])
    }

    pub fn contains(&self, symbol: &Symbol) -> bool {
        self.by_symbol.contains_key(symbol)
    }

    /// Every symbol, in file order. The screener's fan-out list.
    pub fn symbols(&self) -> impl Iterator<Item = &Symbol> {
        self.instruments.iter().map(|i| &i.symbol)
    }

    /// Non-ETF rows: what the screener may rank as a buy.
    pub fn buy_candidates(&self) -> impl Iterator<Item = &Instrument> {
        self.instruments.iter().filter(|i| !i.is_etf())
    }

    /// ETF rows: tracked for context, never picked.
    pub fn etfs(&self) -> impl Iterator<Item = &Instrument> {
        self.instruments.iter().filter(|i| i.is_etf())
    }

    /// Names marked `watch` in the file, in file order.
    pub fn watchlist(&self) -> impl Iterator<Item = &Instrument> {
        self.instruments.iter().filter(|i| i.watch)
    }

    /// Rows per sector, sectors in alphabetical order (BTreeMap) so a report
    /// renders the same way every day.
    pub fn group_by_sector(&self) -> BTreeMap<&str, Vec<&Instrument>> {
        let mut out: BTreeMap<&str, Vec<&Instrument>> = BTreeMap::new();
        for inst in &self.instruments {
            out.entry(inst.sector.as_str()).or_default().push(inst);
        }
        out
    }

    /// Rows per theme. A row with three themes appears under all three.
    pub fn group_by_theme(&self) -> BTreeMap<&str, Vec<&Instrument>> {
        let mut out: BTreeMap<&str, Vec<&Instrument>> = BTreeMap::new();
        for inst in &self.instruments {
            for theme in &inst.themes {
                out.entry(theme.as_str()).or_default().push(inst);
            }
        }
        out
    }

    /// Rows tagged with one theme, file order. Case-insensitive on the tag.
    pub fn by_theme(&self, theme: &str) -> Vec<&Instrument> {
        self.instruments
            .iter()
            .filter(|i| i.has_theme(theme))
            .collect()
    }

    /// Rows in one sector, file order. Case-insensitive.
    pub fn by_sector(&self, sector: &str) -> Vec<&Instrument> {
        self.instruments
            .iter()
            .filter(|i| i.sector.eq_ignore_ascii_case(sector))
            .collect()
    }
}

/// `"ai|datacenter|gpu"` → `["ai", "datacenter", "gpu"]`. Whitespace around each
/// item is dropped, empty items (a trailing `|`, or an empty cell) are skipped.
/// Items are stored as written; comparisons elsewhere are case-insensitive.
/// `1`, `yes`, `y`, `true`, `x` (any case) are true; empty, `0`, `no`, `false` are false.
/// Anything else is an error so a typo in the column is caught at load.
fn yes_no<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let raw = String::deserialize(d)?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "no" | "n" | "false" => Ok(false),
        "1" | "yes" | "y" | "true" | "x" => Ok(true),
        other => Err(serde::de::Error::custom(format!(
            "watch: expected 1/yes/true or empty, got {other:?}"
        ))),
    }
}

fn pipe_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(raw
        .split('|')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREE: &str = "\
symbol,name,sector,themes,region
NVDA,Nvidia,semis,ai|datacenter|gpu,US
SPY,SPDR S&P 500,etf,equity-index,US
BABA,Alibaba,internet,china|e-commerce|cloud|ai,CN
";

    fn three() -> Universe {
        Universe::from_reader(THREE.as_bytes()).expect("fixture parses")
    }

    #[test]
    fn parses_rows_and_splits_themes() {
        let u = three();
        assert_eq!(u.len(), 3);
        let nvda = u.get(&Symbol::from("NVDA")).unwrap();
        assert_eq!(nvda.name, "Nvidia");
        assert_eq!(nvda.sector, "semis");
        assert_eq!(nvda.themes, vec!["ai", "datacenter", "gpu"]);
        assert_eq!(nvda.region, "US");
        assert!(u.contains(&Symbol::from("BABA")));
        assert!(!u.contains(&Symbol::from("AAPL")));
    }

    #[test]
    fn group_by_theme_ai_is_the_two_tagged_rows() {
        let u = three();
        let ai: Vec<&str> = u.by_theme("ai").iter().map(|i| i.symbol.as_str()).collect();
        assert_eq!(ai, vec!["NVDA", "BABA"]);
        assert_eq!(
            u.by_theme("AI").len(),
            2,
            "theme lookup is case-insensitive"
        );
        let groups = u.group_by_theme();
        assert_eq!(groups["ai"].len(), 2);
        assert_eq!(groups["equity-index"].len(), 1);
        assert!(groups.contains_key("gpu"));
    }

    #[test]
    fn group_by_sector_is_sorted_and_complete() {
        let u = three();
        let groups = u.group_by_sector();
        let sectors: Vec<&str> = groups.keys().copied().collect();
        assert_eq!(sectors, vec!["etf", "internet", "semis"]);
        assert_eq!(groups["etf"][0].symbol, Symbol::from("SPY"));
    }

    #[test]
    fn etfs_are_distinguishable_from_buy_candidates() {
        let u = three();
        let picks: Vec<&str> = u.buy_candidates().map(|i| i.symbol.as_str()).collect();
        assert_eq!(picks, vec!["NVDA", "BABA"]);
        let etfs: Vec<&str> = u.etfs().map(|i| i.symbol.as_str()).collect();
        assert_eq!(etfs, vec!["SPY"]);

        let fund = "symbol,name,sector,themes,region\nVTI,Vanguard Total,Fund,broad,US\n";
        let u = Universe::from_reader(fund.as_bytes()).unwrap();
        assert!(
            u.iter().next().unwrap().is_etf(),
            "`fund` counts as ETF, any case"
        );
    }

    #[test]
    fn duplicate_symbol_is_an_error_naming_both_lines() {
        let dup = "symbol,name,sector,themes,region\nNVDA,Nvidia,semis,ai,US\nAMD,AMD,semis,ai,US\nNVDA,Again,semis,ai,US\n";
        let err = Universe::from_reader(dup.as_bytes()).unwrap_err();
        match err {
            UniverseError::Duplicate {
                symbol,
                line,
                first,
            } => {
                assert_eq!(symbol, "NVDA");
                assert_eq!(line, 4);
                assert_eq!(first, 2);
            }
            other => panic!("expected Duplicate, got {other:?}"),
        }
    }

    #[test]
    fn missing_required_column_names_the_column() {
        let no_region = "symbol,name,sector,themes\nNVDA,Nvidia,semis,ai\n";
        let err = Universe::from_reader(no_region.as_bytes()).unwrap_err();
        assert!(
            matches!(err, UniverseError::MissingColumn("region")),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "missing required column `region` in header"
        );
    }

    #[test]
    fn unknown_columns_are_ignored_and_cells_trimmed() {
        let extra = "symbol,name,sector,themes,region,market_cap_bn,notes\n NVDA , Nvidia ,semis, ai | gpu | ,US,4400,watch\n";
        let u = Universe::from_reader(extra.as_bytes()).unwrap();
        let n = u.get(&Symbol::from("NVDA")).expect("symbol trimmed");
        assert_eq!(n.name, "Nvidia");
        assert_eq!(n.themes, vec!["ai", "gpu"], "blank trailing item dropped");
    }

    #[test]
    fn empty_symbol_is_an_error() {
        let blank = "symbol,name,sector,themes,region\n,Nobody,semis,ai,US\n";
        let err = Universe::from_reader(blank.as_bytes()).unwrap_err();
        assert!(
            matches!(err, UniverseError::EmptySymbol { line: 2 }),
            "{err:?}"
        );
    }

    #[test]
    fn missing_file_is_an_io_error_with_the_path() {
        let err = Universe::load("config/does-not-exist.csv").unwrap_err();
        assert!(matches!(err, UniverseError::Io { .. }));
        assert!(err.to_string().contains("does-not-exist.csv"));
    }

    /// The shipped file must always load. Counts here are the current file; if a
    /// name is added, update the numbers, that is the point of the test.
    #[test]
    fn shipped_universe_loads_clean() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/config/universe.csv");
        let u = Universe::load(path).expect("config/universe.csv parses");
        assert_eq!(u.len(), 293);
        assert_eq!(u.etfs().count(), 23, "22 `etf` rows + RVI, a `fund`");
        assert_eq!(u.buy_candidates().count(), 270);
        assert!(
            u.iter().all(|i| !i.themes.is_empty()),
            "every row carries a theme"
        );
        assert!(u.iter().all(|i| !i.region.is_empty()));
        assert!(u.by_theme("ai").len() >= 40);
        assert!(u.contains(&Symbol::from("SPY")));
        assert_eq!(u.group_by_sector().len(), 20);
    }

    #[test]
    fn watch_column_is_optional_and_lenient() {
        let without = Universe::from_reader(THREE.as_bytes()).unwrap();
        assert_eq!(without.watchlist().count(), 0, "no column → no watchlist");

        let with = "symbol,name,sector,themes,region,watch\n\
            NVDA,Nvidia,semis,ai,US,1\n\
            USO,Oil,etf,oil,US,yes\n\
            MCD,McDonald's,consumer,dividend,US,\n";
        let u = Universe::from_reader(with.as_bytes()).unwrap();
        let w: Vec<&str> = u.watchlist().map(|i| i.symbol.as_str()).collect();
        assert_eq!(w, vec!["NVDA", "USO"]);

        let bad = "symbol,name,sector,themes,region,watch\nNVDA,Nvidia,semis,ai,US,maybe\n";
        assert!(
            Universe::from_reader(bad.as_bytes()).is_err(),
            "a typo is an error, not a silent no"
        );
    }
}
