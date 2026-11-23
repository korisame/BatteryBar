#![allow(deprecated)]
mod activity_table;
mod history;
mod instance;
mod model;
mod platform;
mod startup;
mod ui;
use std::{path::PathBuf, time::Duration};
fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|v| v == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let isolated = args.iter().any(|s| s == "--isolated");
    if isolated && arg_value(&args, "--config-dir").is_none() {
        return Err("--isolated requires an explicit --config-dir copy".into());
    }
    let dir = arg_value(&args, "--config-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME is required"))
                .join(".config/batterybar")
        });
    platform::preferences(isolated);
    if let Some(value) = arg_value(&args, "--set-login") {
        if isolated || !platform::production_bundle() {
            return Err("--set-login requires the installed production bundle".into());
        }
        let enabled = match value.as_str() {
            "on" => true,
            "off" => false,
            _ => return Err("--set-login expects on or off".into()),
        };
        platform::set_login(enabled)?;
        println!("{}", platform::system_status());
        return Ok(());
    }
    if args.iter().any(|s| s == "--system-status") {
        let mut status = platform::system_status();
        let preferences: serde_json::Map<String, serde_json::Value> = [
            "bar_compact",
            "bar_percentage",
            "bar_time",
            "bar_wattage",
            "notify_full",
            "notify_low",
            "notify_limit",
            "notify_low_pct",
            "notify_limit_pct",
            "track_history",
            "rolling_min",
            "activity_history_hours",
            "activity_group_apps",
        ]
        .into_iter()
        .map(|key| {
            (
                key.into(),
                platform::get(key).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
        status["preferences"] = serde_json::Value::Object(preferences);
        println!("{status}");
        return Ok(());
    }
    if let Some(output) = arg_value(&args, "--export-legacy") {
        let db = history::ActivityDb::open(&dir.join("activity-v3.sqlite3"))?;
        let count = db.export_legacy(std::path::Path::new(&output))?;
        println!(
            "{}",
            serde_json::json!({"exported_intervals":count,"integrity":db.integrity()?})
        );
        return Ok(());
    }
    if let Some(source) = arg_value(&args, "--verify-import") {
        let target = arg_value(&args, "--database").ok_or("--database required")?;
        let mut db = history::ActivityDb::open(std::path::Path::new(&target))?;
        let start = std::time::Instant::now();
        let stats = db.import_jsonl(std::path::Path::new(&source), model::now(), 1., true)?;
        let mut reports = Vec::new();
        for hours in [1., 6., 12., 24., 48.] {
            for group in [false, true] {
                let t = model::now();
                let report = db.report(t - hours * 3600., t, group)?;
                reports.push(serde_json::json!({"hours":hours,"group":group,"seconds":report.seconds,"paths":report.rows.len(),"cpu_seconds":report.rows.iter().map(|r|r.cpu).sum::<f64>(),"joules":report.rows.iter().filter_map(|r|r.joules).sum::<f64>(),"devices":report.devices.len()}));
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"import":stats,"counts":db.counts()?,"integrity":db.integrity()?,"elapsed_seconds":start.elapsed().as_secs_f64(),"memory":platform::memory(),"reports":reports})
            )?
        );
        return Ok(());
    }
    if let Some(source) = arg_value(&args, "--compare-import") {
        // Independent reference streams legacy rows and compares every app/process aggregate,
        // coverage and USB set to SQL at a single fixed cutoff. Never prints process paths.
        use std::{
            collections::{BTreeSet, HashMap},
            fs::File,
            io::{BufRead, BufReader},
        };
        let mut db = history::ActivityDb::open(&dir.join("activity-v3.sqlite3"))?;
        db.import_if_needed(&dir, model::now(), platform::timebase())?;
        let t = model::now();
        let mut results = Vec::new();
        for hours in [1., 6., 12., 24., 48.] {
            for group in [false, true] {
                let since = t - hours * 3600.;
                let mut total = HashMap::<String, (f64, Option<f64>)>::new();
                let mut seconds = 0.;
                let mut devices = BTreeSet::new();
                for line in BufReader::new(File::open(&source)?).lines() {
                    let line = line?;
                    let Ok(mut i) = serde_json::from_str::<history::Interval>(&line) else {
                        continue;
                    };
                    i.start += 978307200.;
                    i.end += 978307200.;
                    let overlap = i.end.min(t) - i.start.max(since);
                    let duration = i.end - i.start;
                    if overlap <= 0. || duration <= 0. {
                        continue;
                    }
                    seconds += overlap;
                    let f = (overlap / duration).min(1.);
                    devices.extend(i.devices);
                    for r in i.rows {
                        let key = if group {
                            history::app_path(&r.path).into()
                        } else {
                            r.path
                        };
                        let e = total.entry(key).or_insert((0., r.joules.map(|_| 0.)));
                        e.0 += r.cpu * f;
                        if let Some(j) = r.joules {
                            e.1 = Some(e.1.unwrap_or(0.) + j * f);
                        }
                    }
                }
                let report = db.report(since, t, group)?;
                let mut max_cpu_error = 0_f64;
                let mut max_joule_error = 0_f64;
                let mut equal = report.rows.len() == total.len()
                    && (report.seconds - seconds).abs() < 0.001
                    && report.devices.iter().cloned().collect::<BTreeSet<_>>() == devices;
                for r in &report.rows {
                    if let Some((cpu, j)) = total.get(&r.path) {
                        max_cpu_error = max_cpu_error.max((cpu - r.cpu).abs());
                        max_joule_error =
                            max_joule_error.max((j.unwrap_or(0.) - r.joules.unwrap_or(0.)).abs());
                        equal &= j.is_some() == r.joules.is_some();
                    } else {
                        equal = false;
                    }
                }
                equal &= max_cpu_error < 0.001 && max_joule_error < 0.001;
                results.push(serde_json::json!({"hours":hours,"group_apps":group,"equal":equal,"rows":report.rows.len(),"max_cpu_error":max_cpu_error,"max_joule_error":max_joule_error,"coverage_error":(report.seconds-seconds).abs()}));
            }
        }
        let pass = results.iter().all(|r| r["equal"].as_bool() == Some(true));
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"pass":pass,"checks":results,"integrity":db.integrity()?,"memory":platform::memory()})
            )?
        );
        if !pass {
            return Err("aggregate comparison failed".into());
        }
        return Ok(());
    }
    if !isolated
        && arg_value(&args, "--config-dir").is_none()
        && !args.iter().any(|s| s == "--production")
        && !platform::production_bundle()
    {
        return Err("Candidate requires --isolated --config-dir PATH; use --production only during coordinated deployment.".into());
    }
    let Some(_instance) = instance::InstanceLock::acquire(&dir)? else {
        eprintln!("BatteryBar è già in esecuzione per questi dati.");
        return Ok(());
    };
    let mut store = model::Store::open(&dir)?;
    if args.iter().any(|s| s == "--headless") {
        platform::notifications(false);
        let seconds = arg_value(&args, "--seconds")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(65.);
        let start = model::now();
        let mut collected = start;
        let mut samples = Vec::new();
        store.collect();
        while model::now() - start < seconds {
            store.refresh();
            if model::now() - collected >= 30. {
                store.collect();
                collected = model::now();
            }
            samples.push(platform::memory());
            std::thread::sleep(Duration::from_secs(1));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"seconds":model::now()-start,"samples":samples,"counts":store.db.counts()?,"battery_readable":store.snapshot.has_reading,"consumers":store.consumers.rows.len(),"error":store.activity_error})
            )?
        );
        return Ok(());
    }
    platform::notifications(!isolated);
    ui::run(
        store,
        ui::Options {
            isolated,
            show_panel: args.iter().any(|s| s == "--show-panel"),
            quit_after: arg_value(&args, "--quit-after").and_then(|v| v.parse().ok()),
            capture: arg_value(&args, "--capture-panel").map(PathBuf::from),
            settings_open: args.iter().any(|s| s == "--settings-open"),
            close_after: arg_value(&args, "--close-after").and_then(|v| v.parse().ok()),
            panel_cycles: arg_value(&args, "--panel-cycles")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            exercise_ui: isolated && args.iter().any(|s| s == "--exercise-ui"),
            // Explicit finite measurement aid, including the authorized real
            // production-data gate. Normal launches retain transient dismissal.
            hold_panel: args.iter().any(|s| s == "--hold-panel"),
            regression_scroll: isolated && args.iter().any(|s| s == "--regression-scroll"),
            capture_viewport: args.iter().any(|s| s == "--capture-viewport"),
            expanded_cycles: isolated && args.iter().any(|s| s == "--expanded-cycles"),
            memory_every: arg_value(&args, "--memory-every")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .map(|v| v.clamp(1., 60.))
                .unwrap_or(15.),
        },
    );
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("BatteryBar: {error}");
        std::process::exit(1);
    }
}
