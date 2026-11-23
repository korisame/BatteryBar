use crate::{
    history::{ActivityDb, Interval, Report, Row, RETENTION},
    platform,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Power {
    Charging,
    Discharging,
    Charged,
    Plugged,
}
impl Power {
    pub fn label(self) -> &'static str {
        match self {
            Self::Charging => "In carica",
            Self::Discharging => "A batteria",
            Self::Charged => "Carica completa",
            Self::Plugged => "Collegato, carica in pausa",
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub percent: i64,
    pub state: Power,
    pub voltage: Option<f64>,
    pub ma: Option<i64>,
    pub current: i64,
    pub maximum: i64,
    pub design: i64,
    pub temperature: Option<f64>,
    pub cycles: Option<i64>,
    pub adapter_watts: Option<i64>,
    pub adapter_volts: Option<i64>,
    pub average: Option<f64>,
    pub hours: Option<f64>,
    pub eta: Option<f64>,
    pub quality: String,
    pub insufficient: bool,
    pub stale: bool,
    pub has_reading: bool,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            percent: 0,
            state: Power::Discharging,
            voltage: None,
            ma: None,
            current: 0,
            maximum: 0,
            design: 0,
            temperature: None,
            cycles: None,
            adapter_watts: None,
            adapter_volts: None,
            average: None,
            hours: None,
            eta: None,
            quality: "Stima in assestamento".into(),
            insufficient: false,
            stale: false,
            has_reading: false,
        }
    }
}
impl Snapshot {
    pub fn decode(v: &serde_json::Value) -> Option<Self> {
        let percent = v["CurrentCapacity"].as_i64()?;
        let maximum = v["AppleRawMaxCapacity"].as_i64()?;
        let current = v["AppleRawCurrentCapacity"].as_i64()?;
        let connected = v["AppleRawExternalConnected"]
            .as_bool()
            .or_else(|| v["ExternalConnected"].as_bool())?;
        let charging = v["ControllerCharging"]
            .as_bool()
            .or_else(|| v["IsCharging"].as_bool())?;
        if !(0..=100).contains(&percent) || maximum <= 0 || current < 0 {
            return None;
        }
        let positive = |k: &str| v[k].as_i64().filter(|n| *n > 0);
        Some(Self {
            percent,
            maximum,
            current,
            has_reading: true,
            state: Self::state(
                connected,
                charging,
                v["FullyCharged"].as_bool().unwrap_or(false),
            ),
            voltage: positive("Voltage").map(|n| n as f64 / 1000.),
            ma: v["InstantAmperage"].as_i64().filter(|n| *n != 0),
            design: v["DesignCapacity"].as_i64().unwrap_or(0),
            cycles: positive("CycleCount"),
            temperature: positive("Temperature").map(|n| n as f64 / 100.),
            adapter_watts: v["AdapterDetails"]["Watts"].as_i64().filter(|n| *n > 0),
            adapter_volts: v["AdapterDetails"]["AdapterVoltage"]
                .as_i64()
                .filter(|n| *n > 0)
                .map(|n| n / 1000),
            ..Default::default()
        })
    }
    pub fn state(connected: bool, charging: bool, full: bool) -> Power {
        if !connected {
            Power::Discharging
        } else if full {
            Power::Charged
        } else if charging {
            Power::Charging
        } else {
            Power::Plugged
        }
    }
    pub fn watts(&self) -> Option<f64> {
        Some(self.voltage? * self.ma? as f64 / 1000.)
    }
    pub fn health(&self) -> Option<i64> {
        (self.design > 0 && self.maximum > 0)
            .then(|| (self.maximum as f64 / self.design as f64 * 100.).round() as i64)
    }
}
#[derive(Default)]
pub struct Estimator {
    samples: VecDeque<(f64, f64, f64)>,
    mode: Option<Power>,
    last: Option<f64>,
}
// Recent readings retain half their weight after two seconds, so both power
// and remaining-time estimates follow real load changes sooner.
const POWER_HALF_LIFE_SECONDS: f64 = 2.0;
impl Estimator {
    pub fn reset(&mut self) {
        self.samples.clear();
        self.mode = None;
    }
    pub fn update(&mut self, s: &mut Snapshot, t: f64, window: f64) {
        if self.last.is_some_and(|p| t - p > 5.) {
            self.reset();
        }
        self.last = Some(t);
        let mode = if s.insufficient {
            Power::Discharging
        } else {
            s.state
        };
        if self.mode != Some(mode) {
            self.reset();
            self.mode = Some(mode);
        }
        s.quality = "Stima in assestamento".into();
        s.average = None;
        s.hours = None;
        s.eta = None;
        while self
            .samples
            .front()
            .is_some_and(|p| t - p.0 > window.max(1.))
        {
            self.samples.pop_front();
        }
        let valid = s.ma.is_some_and(|m| {
            mode == Power::Charging && m > 0 || mode == Power::Discharging && m < 0
        });
        if !valid {
            self.samples.clear();
            return;
        }
        if let (Some(v), Some(ma)) = (s.voltage.filter(|v| *v > 0.), s.ma) {
            let magnitude = ma.unsigned_abs() as f64;
            // A distinct operating rate should take effect even during a notification burst.
            if self.samples.back().is_some_and(|(_, previous, _)| {
                (magnitude - previous).abs() >= (previous * 0.25).max(150.)
            }) {
                self.samples.clear();
            }
            if self.samples.back().is_none_or(|p| t - p.0 >= 0.9) {
                self.samples.push_back((
                    t,
                    ma.unsigned_abs() as f64,
                    v * ma.unsigned_abs() as f64 / 1000.,
                ));
            }
        }
        if self.samples.is_empty() {
            return;
        }
        let mut weights = 0.;
        let mut current = 0.;
        let mut watts = 0.;
        for (time, ma, w) in &self.samples {
            let weight = 0.5_f64.powf((t - time) / POWER_HALF_LIFE_SECONDS);
            weights += weight;
            current += ma * weight;
            watts += w * weight;
        }
        current /= weights;
        let variance = self
            .samples
            .iter()
            .map(|(time, ma, _)| {
                (ma - current).powi(2) * 0.5_f64.powf((t - time) / POWER_HALF_LIFE_SECONDS)
            })
            .sum::<f64>()
            / weights;
        if t - self.samples.front().unwrap().0 >= 30. && self.samples.len() >= 15 {
            s.quality = if variance.sqrt() / current.max(1.) > 0.3 {
                "Stima variabile: carico in cambiamento"
            } else {
                "Stima ai consumi attuali"
            }
            .into();
        }
        s.average = Some(watts / weights);
        if current <= 0. || s.maximum <= 0 || s.current < 0 {
            return;
        }
        s.hours = match mode {
            Power::Charging => Some((s.maximum - s.current).max(0) as f64 / current),
            Power::Discharging => Some(s.current as f64 / current),
            _ => None,
        };
        s.eta = s
            .hours
            .filter(|h| *h > 0. && *h < 48.)
            .map(|h| t + h * 3600.);
    }
}
#[derive(Default)]
pub struct Assessment {
    since: Option<f64>,
    last: Option<f64>,
}
impl Assessment {
    pub fn update(&mut self, s: &mut Snapshot, t: f64) {
        if self.last.is_some_and(|p| t - p > 5.) {
            self.since = None;
        }
        self.last = Some(t);
        if s.state != Power::Discharging && s.watts().unwrap_or(0.) < -1. {
            let since = *self.since.get_or_insert(t);
            s.insufficient = t - since >= 10.;
        } else {
            self.since = None;
            s.insufficient = false;
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Counter {
    pub pid: i32,
    pub started: u64,
    pub path: String,
    pub cpu: f64,
    pub energy: Option<f64>,
}
#[derive(Default)]
pub struct Sampler {
    previous: HashMap<i32, Counter>,
    date: Option<f64>,
}
impl Sampler {
    pub fn sample(
        &mut self,
        counters: Vec<Counter>,
        t: f64,
        devices: Vec<String>,
    ) -> Option<Interval> {
        let mut rows = Vec::new();
        let interval = self
            .date
            .filter(|d| t - d > 0. && t - d <= 45.)
            .map(|date| {
                for c in &counters {
                    if let Some(p) = self.previous.get(&c.pid) {
                        if p.started != c.started || p.path != c.path || c.cpu < p.cpu {
                            continue;
                        }
                        let cpu = c.cpu - p.cpu;
                        let energy = c
                            .energy
                            .zip(p.energy)
                            .filter(|(a, b)| a >= b)
                            .map(|(a, b)| a - b);
                        if cpu > 0.001 || energy.unwrap_or(0.) > 0.001 {
                            rows.push(Row {
                                path: c.path.clone(),
                                cpu,
                                joules: energy,
                            });
                        }
                    }
                }
                Interval {
                    start: date,
                    end: t,
                    rows,
                    devices,
                }
            });
        self.previous = counters.into_iter().map(|c| (c.pid, c)).collect();
        self.date = Some(t);
        interval
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Point {
    pub ts: f64,
    pub pct: i64,
    pub watts: f64,
    pub charging: bool,
}
#[derive(Default)]
pub struct Settings {
    values: HashMap<String, serde_json::Value>,
}
impl Settings {
    pub fn flag(&self, key: &str) -> bool {
        self.value(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(matches!(
                key,
                "bar_percentage"
                    | "notify_full"
                    | "notify_low"
                    | "track_history"
                    | "activity_group_apps"
            ))
    }
    fn value(&self, key: &str) -> Option<serde_json::Value> {
        self.values.get(key).cloned().or_else(|| platform::get(key))
    }
    pub fn int(&self, key: &str) -> i64 {
        self.value(key)
            .and_then(|v| v.as_i64())
            .filter(|n| *n != 0)
            .unwrap_or(match key {
                "notify_low_pct" => 20,
                "notify_limit_pct" => 80,
                "activity_history_hours" => 24,
                _ => 0,
            })
    }
    pub fn rolling(&self) -> f64 {
        self.value("rolling_min")
            .and_then(|v| v.as_f64())
            .filter(|v| *v != 0.)
            .unwrap_or(3.)
    }
    pub fn set(&mut self, key: &str, value: serde_json::Value) {
        platform::set(key, value.clone());
        self.values.insert(key.into(), value);
    }
    pub fn toggle(&mut self, key: &str) {
        self.set(key, serde_json::json!(!self.flag(key)));
    }
}
pub struct Store {
    pub snapshot: Snapshot,
    pub settings: Settings,
    pub history: Vec<Point>,
    pub consumers: Report,
    pub activity_updated: Option<f64>,
    pub last_update: Option<f64>,
    pub activity_error: Option<String>,
    pub db: ActivityDb,
    pub sampler: Sampler,
    pub estimator: Estimator,
    assessment: Assessment,
    dir: PathBuf,
    last_state: Option<Power>,
    notified: [bool; 3],
    last_compact: f64,
    storage_available: bool,
}
impl Store {
    pub fn open(dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let disk = (|| -> Result<ActivityDb, Box<dyn std::error::Error>> {
            std::fs::create_dir_all(dir)?;
            let mut db = ActivityDb::open(&dir.join("activity-v3.sqlite3"))?;
            db.import_if_needed(dir, now(), platform::timebase())?;
            db.prune(now())?;
            Ok(db)
        })();
        let (db,activity_error,storage_available)=match disk {Ok(db)=>(db,None,true),Err(e)=>(ActivityDb::open(Path::new(":memory:"))?,Some(format!("Storico non disponibile: {e}. Monitoraggio batteria attivo; registrazione non salvata.")),false)};
        let cutoff = now() - RETENTION;
        let history = File::open(dir.join("history.jsonl"))
            .ok()
            .map(|f| {
                BufReader::new(f)
                    .lines()
                    .map_while(Result::ok)
                    .filter_map(|l| serde_json::from_str::<Point>(&l).ok())
                    .filter(|p| p.ts >= cutoff)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            snapshot: Snapshot::default(),
            settings: Settings::default(),
            history,
            consumers: Report::default(),
            activity_updated: None,
            last_update: None,
            activity_error,
            db,
            sampler: Sampler::default(),
            estimator: Estimator::default(),
            assessment: Assessment::default(),
            dir: dir.into(),
            last_state: None,
            notified: [false; 3],
            last_compact: now(),
            storage_available,
        })
    }
    pub fn refresh(&mut self) {
        self.refresh_at(
            platform::battery().as_ref().and_then(Snapshot::decode),
            now(),
        );
    }
    fn refresh_at(&mut self, read: Option<Snapshot>, t: f64) {
        let Some(mut s) = read else {
            self.snapshot.stale = true;
            self.snapshot.hours = None;
            self.snapshot.eta = None;
            self.estimator.reset();
            return;
        };
        if self.last_state != Some(s.state) {
            self.estimator.reset();
            self.notified = [false; 3];
            self.last_state = Some(s.state);
        }
        self.assessment.update(&mut s, t);
        self.estimator
            .update(&mut s, t, (self.settings.rolling() * 60.).max(60.));
        if self.settings.flag("track_history") {
            self.append_point(&s, t);
        }
        self.notifications(&s);
        self.snapshot = s;
        self.last_update = Some(t);
    }
    fn append_point(&mut self, s: &Snapshot, t: f64) {
        if self
            .history
            .last()
            .is_some_and(|p| t - p.ts < 60. || (t - p.ts < 300. && p.pct == s.percent))
        {
            return;
        }
        let p = Point {
            ts: t,
            pct: s.percent,
            watts: s.average.unwrap_or(0.),
            charging: s.state != Power::Discharging,
        };
        let result = (|| -> std::io::Result<()> {
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.join("history.jsonl"))?;
            serde_json::to_writer(&mut f, &p)?;
            f.write_all(b"\n")
        })();
        if let Err(e) = result {
            self.activity_error = Some(format!("Storico batteria non salvato: {e}"));
            return;
        }
        self.history.push(p);
        self.history.retain(|p| p.ts >= t - RETENTION);
    }
    pub fn collect(&mut self) {
        let counters = platform::counters();
        if counters.is_empty() {
            self.sampler = Sampler::default();
            self.activity_error =
                Some("Contatori dei processi temporaneamente non disponibili".into());
            return;
        }
        let t = now();
        if let Some(i) = self.sampler.sample(counters, t, platform::devices()) {
            let mut report = Report::summarize(std::slice::from_ref(&i), i.start, i.end, true);
            report.rows.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
            report.rows.truncate(6);
            self.consumers = report;
            self.activity_updated = Some(i.end);
            if self.settings.flag("track_history") && self.storage_available {
                match self.db.append(&i) {
                    Ok(()) => self.activity_error = None,
                    Err(e) => self.activity_error = Some(format!("Storico non salvato: {e}")),
                };
            }
        }
        if t - self.last_compact >= 3600. {
            if let Err(e) = self.db.prune(t) {
                self.activity_error = Some(format!("Storico non compattato: {e}"));
            }
            self.compact_battery(t);
            self.last_compact = t;
        }
    }
    fn compact_battery(&mut self, t: f64) {
        self.history.retain(|p| p.ts >= t - RETENTION);
        let result = (|| -> std::io::Result<()> {
            let temp = self.dir.join("history.rust.tmp");
            let mut f = File::create(&temp)?;
            for p in &self.history {
                serde_json::to_writer(&mut f, p)?;
                f.write_all(b"\n")?;
            }
            f.sync_all()?;
            std::fs::rename(temp, self.dir.join("history.jsonl"))
        })();
        if let Err(e) = result {
            self.activity_error = Some(format!("Storico batteria non compattato: {e}"));
        }
    }
    pub fn report(&mut self) -> Report {
        if !self.settings.flag("track_history") {
            return Report::default();
        }
        let t = now();
        match self.db.report(
            t - self.settings.int("activity_history_hours") as f64 * 3600.,
            t,
            self.settings.flag("activity_group_apps"),
        ) {
            Ok(report) => report,
            Err(e) => {
                self.activity_error = Some(format!("Storico non leggibile: {e}"));
                Report::default()
            }
        }
    }
    pub fn toggle(&mut self, key: &str) {
        self.settings.toggle(key);
        if key == "track_history" && !self.settings.flag(key) {
            self.history.clear();
        }
    }
    pub fn hour_delta(&self) -> Option<i64> {
        let t = now();
        let mut i = self.history.iter().filter(|p| p.ts > t - 3600.);
        let first = i.next()?;
        let last = i.next_back()?;
        (last.ts - first.ts > 600.).then_some(last.pct - first.pct)
    }
    fn notifications(&mut self, s: &Snapshot) {
        let low = self.settings.int("notify_low_pct").max(5);
        let limit = self.settings.int("notify_limit_pct").max(50);
        for (i, trigger, title, body) in [
            (
                0,
                s.state == Power::Charged && self.settings.flag("notify_full"),
                "Carica completa",
                "La batteria e' al 100%".to_string(),
            ),
            (
                1,
                s.state == Power::Discharging
                    && s.percent <= low
                    && self.settings.flag("notify_low"),
                "Batteria bassa",
                format!("Sei al {}%", s.percent),
            ),
            (
                2,
                s.state == Power::Charging
                    && s.percent >= limit
                    && self.settings.flag("notify_limit"),
                "Limite di carica",
                format!("Raggiunto il {}%, puoi staccare", s.percent),
            ),
        ] {
            if trigger && !self.notified[i] {
                platform::notify(title, &body);
                self.notified[i] = true;
            }
        }
    }
}
pub fn duration(hours: f64) -> String {
    let m = (hours * 60.).round() as i64;
    if m >= 60 {
        format!("{}h {:02}m", m / 60, m % 60)
    } else {
        format!("{m}m")
    }
}
pub fn activity_duration(s: f64) -> String {
    if s < 60. {
        format!("{s:.1} s")
    } else {
        format!("{:.1} min", s / 60.)
    }
}
pub fn energy(j: f64) -> String {
    if j < 3600. {
        format!("{:.1} mWh", j / 3.6)
    } else {
        format!("{:.3} Wh", j / 3600.)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn snap() -> Snapshot {
        Snapshot {
            voltage: Some(12.),
            maximum: 6000,
            current: 3000,
            ma: Some(-1000),
            has_reading: true,
            ..Default::default()
        }
    }
    #[test]
    fn states_and_corrupt() {
        assert_eq!(Snapshot::state(false, true, true), Power::Discharging);
        assert_eq!(Snapshot::state(true, false, false), Power::Plugged);
        assert_eq!(Snapshot::state(true, true, true), Power::Charged);
        assert!(Snapshot::decode(&serde_json::json!({})).is_none());
    }
    #[test]
    fn cable_and_charger_telemetry_lead_delayed_summary_flags() {
        let mut raw = serde_json::json!({
            "CurrentCapacity": 50, "AppleRawCurrentCapacity": 3000, "AppleRawMaxCapacity": 6000,
            "ExternalConnected": false, "IsCharging": false, "Voltage": 12000,
            "InstantAmperage": -1000
        });
        let mut estimator = Estimator::default();
        let mut read = |raw: &serde_json::Value, t| {
            let mut snapshot = Snapshot::decode(raw).unwrap();
            estimator.update(&mut snapshot, t, 180.);
            (snapshot.state, snapshot.hours)
        };
        assert_eq!(read(&raw, 0.), (Power::Discharging, Some(3.)));
        raw["AppleRawExternalConnected"] = true.into();
        assert_eq!(read(&raw, 0.1), (Power::Plugged, None));
        raw["ControllerCharging"] = true.into();
        assert_eq!(read(&raw, 0.2), (Power::Charging, None));
        raw["InstantAmperage"] = 2000.into();
        assert_eq!(read(&raw, 0.3), (Power::Charging, Some(1.5)));
        raw["InstantAmperage"] = 1000.into();
        assert_eq!(read(&raw, 0.4), (Power::Charging, Some(3.)));
        raw["ExternalConnected"] = true.into();
        raw["AppleRawExternalConnected"] = false.into();
        assert_eq!(read(&raw, 0.5), (Power::Discharging, None));
        raw["InstantAmperage"] = (-1500).into();
        assert_eq!(read(&raw, 0.6), (Power::Discharging, Some(2.)));
    }
    #[test]
    fn estimation_transition_bursts_and_gap() {
        let mut e = Estimator::default();
        let mut s = snap();
        e.update(&mut s, 100., 180.);
        assert!((s.hours.unwrap() - 3.0).abs() < 1e-10);
        s.ma = Some(-3000);
        for n in 1..=8 {
            e.update(&mut s, 100. + n as f64 / 10., 180.);
        }
        assert!((s.hours.unwrap() - 1.0).abs() < 1e-10);
        for n in 1..=15 {
            e.update(&mut s, 100. + n as f64, 180.);
        }
        assert!((1.0..1.1).contains(&s.hours.unwrap()));
        s.state = Power::Charging;
        e.update(&mut s, 116., 180.);
        assert!(s.hours.is_none());
        s.ma = Some(2000);
        e.update(&mut s, 117., 180.);
        assert_eq!(s.hours, Some(1.5));
        e.update(&mut s, 200., 180.);
        assert_eq!(s.quality, "Stima in assestamento");
    }
    #[test]
    fn confidence_and_drain() {
        let mut e = Estimator::default();
        let mut a = Assessment::default();
        let mut s = snap();
        for n in 0..=30 {
            e.update(&mut s, n as f64, 180.);
        }
        assert_eq!(s.quality, "Stima ai consumi attuali");
        s.state = Power::Plugged;
        for n in 0..10 {
            a.update(&mut s, n as f64);
            assert!(!s.insufficient);
        }
        a.update(&mut s, 10.);
        assert!(s.insufficient);
        e.update(&mut s, 31., 180.);
        assert!((s.hours.unwrap() - 3.0).abs() < 1e-10);
        s.state = Power::Discharging;
        a.update(&mut s, 11.);
        assert!(!s.insufficient);
    }
    #[test]
    fn estimate_follows_load_change_in_both_directions() {
        let mut estimator = Estimator::default();
        let mut s = snap();
        for n in 0..=30 {
            estimator.update(&mut s, n as f64, 180.);
        }
        s.ma = Some(-3000);
        estimator.update(&mut s, 30.2, 180.);
        assert_eq!(s.hours, Some(1.0));
        for n in 31..=36 {
            estimator.update(&mut s, n as f64, 180.);
        }
        assert!(s.average.unwrap() > 20.0);
        assert!(s.hours.unwrap() < 1.8);
        s.ma = Some(-1000);
        estimator.update(&mut s, 36.2, 180.);
        assert_eq!(s.hours, Some(3.0));
        for n in 37..=42 {
            estimator.update(&mut s, n as f64, 180.);
        }
        assert!(s.average.unwrap() < 23.0);
        assert!(s.hours.unwrap() > 1.5);
        s.state = Power::Charging;
        s.ma = Some(2000);
        estimator.update(&mut s, 43., 180.);
        assert_eq!(s.hours, Some(1.5));
    }
    #[test]
    fn counter_reset_sleep_and_rollbacks() {
        let mut s = Sampler::default();
        let c = Counter {
            pid: 42,
            started: 100,
            path: "/test".into(),
            cpu: 10.,
            energy: Some(100.),
        };
        assert!(s.sample(vec![c.clone()], 0., vec![]).is_none());
        let mut c2 = c.clone();
        c2.cpu = 12.;
        c2.energy = Some(130.);
        let i = s.sample(vec![c2.clone()], 30., vec![]).unwrap();
        assert_eq!((i.rows[0].cpu, i.rows[0].joules), (2., Some(30.)));
        c2.started = 200;
        c2.cpu = 1000.;
        assert!(s
            .sample(vec![c2.clone()], 60., vec![])
            .unwrap()
            .rows
            .is_empty());
        assert!(s.sample(vec![c2], 160., vec![]).is_none());
    }
    #[test]
    fn live_counters_use_cpu_seconds_and_accessible_energy() {
        let rows = platform::counters();
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|r| r.cpu >= 0.));
        assert!(rows.iter().any(|r| r.energy.is_some()));
        let own = rows
            .iter()
            .find(|r| r.pid == std::process::id() as i32)
            .unwrap();
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
        let expected = (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64
            + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1e6;
        assert!(
            (expected - own.cpu).abs() < 0.2,
            "counter scale differs from CPU seconds"
        );
    }
}
