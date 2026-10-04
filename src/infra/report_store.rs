//! `ReportStore` — one `MorningReport` per day as `data/reports/YYYY-MM-DD.json`,
//! plus `latest.json` pointing at the newest. Writes are atomic (temp file, then
//! rename) so a crash mid-write never leaves a half report for the page to choke on.
//! The scorecard (#35) reads old reports back through `read`.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::domain::report::MorningReport;

pub const LATEST: &str = "latest.json";

pub struct ReportStore {
    dir: PathBuf,
}

impl ReportStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, date: NaiveDate) -> PathBuf {
        self.dir.join(format!("{date}.json"))
    }

    /// Write `report` as its date's file and as `latest.json`. Returns the dated path.
    pub fn write(&self, report: &MorningReport) -> std::io::Result<PathBuf> {
        fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_vec_pretty(report).map_err(std::io::Error::other)?;
        let dated = self.path(report.date);
        atomic_write(&dated, &json)?;
        atomic_write(&self.dir.join(LATEST), &json)?;
        Ok(dated)
    }

    pub fn read(&self, date: NaiveDate) -> std::io::Result<Option<MorningReport>> {
        read_opt(&self.path(date))
    }

    pub fn latest(&self) -> std::io::Result<Option<MorningReport>> {
        read_opt(&self.dir.join(LATEST))
    }

    /// Every dated report on disk, oldest first.
    pub fn dates(&self) -> std::io::Result<Vec<NaiveDate>> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let name = entry?.file_name();
            if let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json")) {
                if let Ok(d) = stem.parse::<NaiveDate>() {
                    out.push(d);
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

fn read_opt(path: &Path) -> std::io::Result<Option<MorningReport>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| std::io::Error::other(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Write to `<path>.tmp` then rename over `path`. Rename is atomic on the same
/// filesystem, so readers see either the old file or the new one, never a prefix.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::report::RunStats;

    fn report(date: &str, n: usize) -> MorningReport {
        MorningReport {
            date: date.parse().unwrap(),
            generated_at: 1,
            regime: None,
            picks: Vec::new(),
            earnings_this_week: Vec::new(),
            by_sector: Vec::new(),
            by_theme: Vec::new(),
            skipped: Vec::new(),
            universe_size: n,
            scorecard: None,
            run: RunStats::default(),
        }
    }

    fn scratch() -> ReportStore {
        ReportStore::new(std::env::temp_dir().join(format!(
            "report_store_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )))
    }

    #[test]
    fn write_read_latest_and_dates() {
        let s = scratch();
        assert!(s.latest().unwrap().is_none());
        assert!(s.dates().unwrap().is_empty());

        let a = s.write(&report("2026-10-03", 1)).unwrap();
        assert_eq!(a, s.dir().join("2026-10-03.json"));
        s.write(&report("2026-10-06", 2)).unwrap();

        assert_eq!(
            s.read("2026-10-03".parse().unwrap())
                .unwrap()
                .unwrap()
                .universe_size,
            1
        );
        assert_eq!(
            s.latest().unwrap().unwrap().universe_size,
            2,
            "latest follows the last write"
        );
        assert_eq!(
            s.dates().unwrap(),
            vec!["2026-10-03".parse().unwrap(), "2026-10-06".parse().unwrap()]
        );
        assert!(s.read("2026-10-04".parse().unwrap()).unwrap().is_none());

        // Same date again replaces, and no temp file survives.
        s.write(&report("2026-10-06", 3)).unwrap();
        assert_eq!(
            s.read("2026-10-06".parse().unwrap())
                .unwrap()
                .unwrap()
                .universe_size,
            3
        );
        assert!(!s.dir().join("2026-10-06.json.tmp").exists());
        let _ = fs::remove_dir_all(s.dir());
    }
}
