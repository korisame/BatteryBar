use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
};

pub const RETENTION: f64 = 48.0 * 3600.0;
const SWIFT_EPOCH: f64 = 978_307_200.0;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Row {
    pub path: String,
    #[serde(rename = "cpuSeconds")]
    pub cpu: f64,
    pub joules: Option<f64>,
}
impl Row {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Interval {
    pub start: f64,
    pub end: f64,
    pub rows: Vec<Row>,
    pub devices: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub rows: Vec<Row>,
    pub seconds: f64,
    pub devices: Vec<String>,
}

pub fn app_path(path: &str) -> &str {
    path.find(".app/").map_or(path, |i| &path[..i + 4])
}
impl Report {
    pub fn summarize(intervals: &[Interval], since: f64, until: f64, group: bool) -> Self {
        let mut report = Self::default();
        let mut totals: HashMap<String, Row> = HashMap::new();
        let mut devices = BTreeSet::new();
        for i in intervals {
            let duration = i.end - i.start;
            let overlap = i.end.min(until) - i.start.max(since);
            if duration <= 0.0 || overlap <= 0.0 {
                continue;
            }
            report.seconds += overlap;
            let fraction = (overlap / duration).min(1.0);
            devices.extend(i.devices.iter().cloned());
            for r in &i.rows {
                let key = if group { app_path(&r.path) } else { &r.path };
                let entry = totals.entry(key.to_string()).or_insert(Row {
                    path: key.to_string(),
                    cpu: 0.0,
                    joules: r.joules.map(|_| 0.0),
                });
                entry.cpu += r.cpu * fraction;
                if let Some(j) = r.joules {
                    entry.joules = Some(entry.joules.unwrap_or(0.0) + j * fraction);
                }
            }
        }
        report.rows = totals.into_values().collect();
        report.finish(devices);
        report
    }
    fn finish(&mut self, devices: BTreeSet<String>) {
        self.rows.sort_by(|a, b| {
            b.joules
                .unwrap_or(0.0)
                .total_cmp(&a.joules.unwrap_or(0.0))
                .then(b.cpu.total_cmp(&a.cpu))
                .then(a.path.cmp(&b.path))
        });
        self.devices = devices.into_iter().collect();
    }
}
#[derive(Debug, Default, Serialize)]
pub struct ImportStats {
    pub lines: u64,
    pub intervals: u64,
    pub rows: u64,
    pub corrupt: u64,
    pub expired: u64,
}

pub struct ActivityDb {
    connection: Connection,
}
impl ActivityDb {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-512; PRAGMA mmap_size=0; PRAGMA temp_store=FILE; PRAGMA wal_autocheckpoint=256;
          CREATE TABLE IF NOT EXISTS paths(id INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE,app TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS intervals(id INTEGER PRIMARY KEY,start REAL NOT NULL,end REAL NOT NULL,devices TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS interval_end ON intervals(end);
          CREATE TABLE IF NOT EXISTS activity(interval_id INTEGER NOT NULL REFERENCES intervals(id) ON DELETE CASCADE,path_id INTEGER NOT NULL REFERENCES paths(id),cpu REAL NOT NULL,joules REAL);
          CREATE INDEX IF NOT EXISTS activity_interval ON activity(interval_id);
          CREATE INDEX IF NOT EXISTS activity_path ON activity(path_id);
          CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);")?;
        Ok(Self { connection })
    }
    pub fn import_if_needed(
        &mut self,
        dir: &Path,
        now: f64,
        v1_scale: f64,
    ) -> Result<Option<ImportStats>, Box<dyn std::error::Error>> {
        let imported: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM metadata WHERE key='legacy_import'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if imported.is_some() {
            return Ok(None);
        }
        let v2 = dir.join("activity-v2.jsonl");
        let (source, scale) = if v2.exists() {
            (v2, 1.0)
        } else {
            (dir.join("activity-v1.jsonl"), v1_scale)
        };
        let stats = if source.exists() {
            self.import_jsonl_marked(&source, now, scale, true, true)?
        } else {
            ImportStats::default()
        };
        self.connection.execute(
            "INSERT OR REPLACE INTO metadata VALUES('legacy_import',?1)",
            [source.to_string_lossy().as_ref()],
        )?;
        Ok(Some(stats))
    }
    // A single transaction preserves atomicity. BufReader and one decoded interval bound
    // migration memory independently of file length. The source is never rewritten.
    pub fn import_jsonl(
        &mut self,
        path: &Path,
        now: f64,
        cpu_scale: f64,
        swift_dates: bool,
    ) -> Result<ImportStats, Box<dyn std::error::Error>> {
        self.import_jsonl_marked(path, now, cpu_scale, swift_dates, false)
    }
    fn import_jsonl_marked(
        &mut self,
        path: &Path,
        now: f64,
        cpu_scale: f64,
        swift_dates: bool,
        mark: bool,
    ) -> Result<ImportStats, Box<dyn std::error::Error>> {
        let file = BufReader::with_capacity(64 * 1024, File::open(path)?);
        let tx = self.connection.transaction()?;
        let mut stats = ImportStats::default();
        for line in file.lines() {
            let line = line?;
            stats.lines += 1;
            let mut i: Interval = match serde_json::from_str(&line) {
                Ok(i) => i,
                Err(_) => {
                    stats.corrupt += 1;
                    continue;
                }
            };
            if swift_dates {
                i.start += SWIFT_EPOCH;
                i.end += SWIFT_EPOCH;
            }
            if !valid(&i) {
                stats.corrupt += 1;
                continue;
            }
            if i.end < now - RETENTION {
                stats.expired += 1;
                continue;
            }
            for r in &mut i.rows {
                r.cpu *= cpu_scale;
            }
            insert(&tx, &i)?;
            stats.intervals += 1;
            stats.rows += i.rows.len() as u64;
        }
        if mark {
            tx.execute(
                "INSERT OR REPLACE INTO metadata VALUES('legacy_import',?1)",
                [path.to_string_lossy().as_ref()],
            )?;
        }
        tx.commit()?;
        Ok(stats)
    }
    pub fn append(&mut self, i: &Interval) -> rusqlite::Result<()> {
        if !valid(i) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let tx = self.connection.transaction()?;
        insert(&tx, i)?;
        tx.commit()
    }
    pub fn prune(&mut self, now: f64) -> rusqlite::Result<usize> {
        let tx = self.connection.transaction()?;
        let n = tx.execute("DELETE FROM intervals WHERE end < ?1", [now - RETENTION])?;
        tx.execute(
            "DELETE FROM paths WHERE NOT EXISTS(SELECT 1 FROM activity WHERE path_id=paths.id)",
            [],
        )?;
        tx.commit()?;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(n)
    }
    pub fn report(&self, since: f64, until: f64, group: bool) -> rusqlite::Result<Report> {
        let mut report = Report::default();
        let mut devices = BTreeSet::new();
        let mut intervals = self.connection.prepare_cached(
            "SELECT start,end,devices FROM intervals WHERE end>?1 AND start<?2 AND end>start",
        )?;
        let mut intervals = intervals.query(params![since, until])?;
        while let Some(r) = intervals.next()? {
            let start: f64 = r.get(0)?;
            let end: f64 = r.get(1)?;
            report.seconds += end.min(until) - start.max(since);
            let text: String = r.get(2)?;
            if let Ok(names) = serde_json::from_str::<Vec<String>>(&text) {
                devices.extend(names);
            }
        }
        // Return one aggregate per unique path (or app), never one object per historical row.
        let key = if group { "p.app" } else { "p.path" };
        let query=format!("SELECT {key},SUM(a.cpu*MIN(1.0,(MIN(i.end,?2)-MAX(i.start,?1))/(i.end-i.start))),SUM(a.joules*MIN(1.0,(MIN(i.end,?2)-MAX(i.start,?1))/(i.end-i.start))) FROM intervals i JOIN activity a ON a.interval_id=i.id JOIN paths p ON p.id=a.path_id WHERE i.end>?1 AND i.start<?2 AND i.end>i.start GROUP BY {key}");
        let mut statement = self.connection.prepare(&query)?;
        let rows = statement.query_map(params![since, until], |r| {
            Ok(Row {
                path: r.get(0)?,
                cpu: r.get(1)?,
                joules: r.get(2)?,
            })
        })?;
        report.rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        report.finish(devices);
        Ok(report)
    }
    pub fn counts(&self) -> rusqlite::Result<(i64, i64, i64)> {
        Ok((
            self.connection
                .query_row("SELECT COUNT(*) FROM intervals", [], |r| r.get(0))?,
            self.connection
                .query_row("SELECT COUNT(*) FROM activity", [], |r| r.get(0))?,
            self.connection
                .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))?,
        ))
    }
    pub fn integrity(&self) -> rusqlite::Result<String> {
        self.connection
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
    }
    pub fn export_legacy(&self, path: &Path) -> Result<u64, Box<dyn std::error::Error>> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        let mut file = BufWriter::new(file);
        let mut count = 0;
        let mut intervals = self
            .connection
            .prepare("SELECT id,start,end,devices FROM intervals ORDER BY id")?;
        let mut intervals = intervals.query([])?;
        let mut rows=self.connection.prepare("SELECT p.path,a.cpu,a.joules FROM activity a JOIN paths p ON p.id=a.path_id WHERE a.interval_id=?1")?;
        while let Some(record) = intervals.next()? {
            let id: i64 = record.get(0)?;
            let mut interval = Interval {
                start: record.get::<_, f64>(1)? - SWIFT_EPOCH,
                end: record.get::<_, f64>(2)? - SWIFT_EPOCH,
                devices: serde_json::from_str(&record.get::<_, String>(3)?)?,
                rows: Vec::new(),
            };
            interval.rows = rows
                .query_map([id], |r| {
                    Ok(Row {
                        path: r.get(0)?,
                        cpu: r.get(1)?,
                        joules: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            serde_json::to_writer(&mut file, &interval)?;
            file.write_all(b"\n")?;
            count += 1;
        }
        file.flush()?;
        file.get_ref().sync_all()?;
        Ok(count)
    }
}
fn valid(i: &Interval) -> bool {
    i.start.is_finite()
        && i.end.is_finite()
        && i.end > i.start
        && i.rows.iter().all(|r| {
            r.cpu.is_finite() && r.cpu >= 0.0 && r.joules.is_none_or(|j| j.is_finite() && j >= 0.0)
        })
}
fn insert(tx: &rusqlite::Transaction<'_>, i: &Interval) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO intervals(start,end,devices) VALUES(?1,?2,?3)",
        params![i.start, i.end, serde_json::to_string(&i.devices).unwrap()],
    )?;
    let id = tx.last_insert_rowid();
    let mut path_insert =
        tx.prepare_cached("INSERT OR IGNORE INTO paths(path,app) VALUES(?1,?2)")?;
    let mut row_insert=tx.prepare_cached("INSERT INTO activity(interval_id,path_id,cpu,joules) SELECT ?1,id,?3,?4 FROM paths WHERE path=?2")?;
    for r in &i.rows {
        path_insert.execute(params![r.path, app_path(&r.path)])?;
        row_insert.execute(params![id, r.path, r.cpu, r.joules])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> ActivityDb {
        ActivityDb::open(Path::new(":memory:")).unwrap()
    }
    #[test]
    fn clipping_grouping_and_nullable_energy() {
        let i = Interval {
            start: 1000.,
            end: 1030.,
            rows: vec![
                Row {
                    path: "/Apps/Test.app/Contents/MacOS/Test".into(),
                    cpu: 10.,
                    joules: Some(3600.),
                },
                Row {
                    path: "/Apps/Test.app/Contents/Helpers/H".into(),
                    cpu: 20.,
                    joules: Some(3600.),
                },
                Row {
                    path: "/daemon".into(),
                    cpu: 2.,
                    joules: None,
                },
            ],
            devices: vec!["Hub".into()],
        };
        let mut db = db();
        db.append(&i).unwrap();
        let expected = Report::summarize(&[i], 1015., 1030., true);
        let got = db.report(1015., 1030., true).unwrap();
        assert_eq!(got.rows, expected.rows);
        assert_eq!(got.seconds, 15.);
        assert_eq!(got.devices, vec!["Hub"]);
        assert_eq!(db.counts().unwrap(), (1, 3, 3));
        assert_eq!(db.report(2000., 3000., false).unwrap().seconds, 0.);
    }
    #[test]
    fn retention_and_path_interning() {
        let mut db = db();
        let now = 200000.;
        for n in 0..4 {
            db.append(&Interval {
                start: now - RETENTION - 30. + n as f64 * 30.,
                end: now - RETENTION + n as f64 * 30.,
                rows: vec![Row {
                    path: "/same".into(),
                    cpu: 1.,
                    joules: None,
                }],
                devices: vec![],
            })
            .unwrap();
        }
        assert_eq!(db.counts().unwrap(), (4, 4, 1));
        db.prune(now + 1.).unwrap();
        assert_eq!(db.counts().unwrap(), (3, 3, 1));
        assert_eq!(db.integrity().unwrap(), "ok");
    }
    #[test]
    fn legacy_streaming_dates_scale_and_corruption() {
        use std::io::Write;
        let path =
            std::env::temp_dir().join(format!("batterybar-fixture-{}.jsonl", std::process::id()));
        let mut f = File::create(&path).unwrap();
        writeln!(f,"{{\"start\":1000,\"end\":1030,\"rows\":[{{\"path\":\"/fixture\",\"cpuSeconds\":2,\"joules\":null}}],\"devices\":[]}}\nbroken\n{{\"start\":-999999,\"end\":-999969,\"rows\":[],\"devices\":[]}}").unwrap();
        drop(f);
        let mut db = db();
        let stats = db
            .import_jsonl(&path, SWIFT_EPOCH + 1030., 2., true)
            .unwrap();
        assert_eq!((stats.intervals, stats.corrupt, stats.expired), (1, 1, 1));
        let r = db.report(SWIFT_EPOCH, SWIFT_EPOCH + 2000., false).unwrap();
        assert_eq!(r.rows[0].cpu, 4.);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn migration_is_idempotent_and_rollback_export_roundtrips() {
        let dir = std::env::temp_dir().join(format!(
            "batterybar-migration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let source = Interval {
            start: 1000.,
            end: 1030.,
            rows: vec![Row {
                path: "/fixture.app/Contents/MacOS/F".into(),
                cpu: 2.,
                joules: Some(10.),
            }],
            devices: vec!["Fixture USB".into()],
        };
        std::fs::write(
            dir.join("activity-v2.jsonl"),
            format!("{}\n", serde_json::to_string(&source).unwrap()),
        )
        .unwrap();
        let path = dir.join("history.sqlite");
        let mut db = ActivityDb::open(&path).unwrap();
        assert_eq!(
            db.import_if_needed(&dir, SWIFT_EPOCH + 1030., 99.)
                .unwrap()
                .unwrap()
                .rows,
            1
        );
        drop(db);
        let mut db = ActivityDb::open(&path).unwrap();
        assert!(db
            .import_if_needed(&dir, SWIFT_EPOCH + 1030., 99.)
            .unwrap()
            .is_none());
        assert_eq!(db.counts().unwrap(), (1, 1, 1));
        let output = dir.join("rollback.jsonl");
        assert_eq!(db.export_legacy(&output).unwrap(), 1);
        assert!(db.export_legacy(&output).is_err());
        let restored: Interval =
            serde_json::from_str(std::fs::read_to_string(&output).unwrap().trim()).unwrap();
        assert_eq!(restored.start, source.start);
        assert_eq!(restored.end, source.end);
        assert_eq!(restored.rows, source.rows);
        assert_eq!(restored.devices, source.devices);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
