//! Control the existing named login job, falling back to SMAppService only when absent.
//! Enabling/disabling a future login must not unload, restart or stop the current app.
use crate::platform;
use serde_json::{json, Value};
use std::{path::Path, process::Command};
const LABEL: &str = "com.user.battery-menubar";
struct Managed {
    domain: String,
}
fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let result = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("Avvio al login: {e}"))?;
    if !result.status.success() {
        return Err(format!(
            "Avvio al login: {}",
            String::from_utf8_lossy(&result.stderr)
                .trim()
                .chars()
                .take(240)
                .collect::<String>()
        ));
    }
    String::from_utf8(result.stdout).map_err(|e| format!("Avvio al login: {e}"))
}
fn extract(path: &Path, key: &str, format: &str) -> Result<String, String> {
    output(
        "/usr/bin/plutil",
        &["-extract", key, format, "-o", "-", &path.to_string_lossy()],
    )
}
fn valid_arguments(args: &[String], exe: &Path, home: &Path) -> bool {
    if exe != home.join("BatteryBar-Swift/BatteryBar.app/Contents/MacOS/BatteryBar") {
        return false;
    }
    (args.len() == 1 && Path::new(&args[0]) == exe)
        || (args.len() == 2 && Path::new(&args[1]) == exe
        && Path::new(&args[0]) == home.join("Applications/Automazioni/Batteria - Barra menu.app/Contents/MacOS/Batteria - Barra menu"))
}
fn managed() -> Result<Option<Managed>, String> {
    let home = platform::home_dir().ok_or("Directory utente non disponibile")?;
    let path = home.join("Library/LaunchAgents/com.user.battery-menubar.plist");
    if !path.exists() {
        return Ok(None);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // A separate test/candidate executable never owns the production launch job.
    if !platform::production_bundle() {
        return Ok(None);
    }
    let label = extract(&path, "Label", "raw")?;
    let args: Vec<String> = serde_json::from_str(&extract(&path, "ProgramArguments", "json")?)
        .map_err(|e| e.to_string())?;
    if label.trim() != LABEL
        || !valid_arguments(&args, &exe, &home)
        || extract(&path, "RunAtLoad", "raw")?.trim() != "true"
    {
        return Err("Avvio al login: il job esistente non corrisponde a questo bundle; nessuna modifica applicata.".into());
    }
    if args.len() == 2 {
        let wrapper =
            home.join("Applications/Automazioni/Batteria - Barra menu.app/Contents/Info.plist");
        if extract(&wrapper, "CFBundleIdentifier", "raw")?.trim()
            != "com.user.battery-menubar.named"
        {
            return Err("Avvio al login: identità del launcher non valida.".into());
        }
    } else {
        let associated: Vec<String> =
            serde_json::from_str(&extract(&path, "AssociatedBundleIdentifiers", "json")?)
                .map_err(|e| e.to_string())?;
        if associated != ["com.shaungioelegori.batterybar"] {
            return Err("Avvio al login: identità dell’app non valida.".into());
        }
    }
    Ok(Some(Managed {
        domain: format!("gui/{}", unsafe { libc::getuid() }),
    }))
}
fn disabled(text: &str) -> Result<bool, String> {
    if !text.contains("disabled services = {") || !text.trim_end().ends_with('}') {
        return Err("Avvio al login: risposta launchd non valida.".into());
    }
    let mut found = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once("=>") else {
            continue;
        };
        if key.trim().trim_matches('"') != LABEL {
            continue;
        }
        let value = match value.trim() {
            "true" | "disabled" => true,
            "false" | "enabled" => false,
            _ => return Err("Avvio al login: stato launchd non valido.".into()),
        };
        if found.replace(value).is_some() {
            return Err("Avvio al login: stato launchd ambiguo.".into());
        }
    }
    // No override means launchd uses the valid RunAtLoad job's enabled default.
    Ok(found.unwrap_or(false))
}
impl Managed {
    fn enabled(&self) -> Result<bool, String> {
        disabled(&output(
            "/bin/launchctl",
            &["print-disabled", &self.domain],
        )?)
        .map(|v| !v)
    }
    fn set(&self, enabled: bool) -> Result<(), String> {
        let target = format!("{}/{LABEL}", self.domain);
        output(
            "/bin/launchctl",
            &[if enabled { "enable" } else { "disable" }, &target],
        )?;
        if self.enabled()? != enabled {
            return Err("Avvio al login: lo stato richiesto non è stato applicato.".into());
        }
        Ok(())
    }
}
pub fn enabled() -> Result<bool, String> {
    match managed()? {
        Some(job) => job.enabled(),
        None => Ok(platform::sm_login_status() == 1),
    }
}
pub fn set(enabled: bool) -> Result<(), String> {
    match managed()? {
        Some(job) => job.set(enabled),
        None => platform::set_sm_login(enabled),
    }
}
pub fn diagnostic() -> Value {
    match managed() {
        Ok(Some(job)) => match job.enabled() {
            Ok(on) => json!({"backend":"named_launch_agent","label":LABEL,"enabled":on}),
            Err(error) => {
                json!({"backend":"named_launch_agent","label":LABEL,"enabled":null,"error":error})
            }
        },
        Ok(None) => json!({"backend":"SMAppService","enabled":platform::sm_login_status()==1}),
        Err(error) => json!({"backend":"unverified","enabled":null,"error":error}),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    #[test]
    fn launchd_override_default_exact_identity_and_errors() {
        assert!(!disabled("disabled services = {\n \"another.job\" => true\n}").unwrap());
        assert!(
            disabled("disabled services = {\n \"com.user.battery-menubar\" => true\n}").unwrap()
        );
        assert!(
            !disabled("disabled services = {\n \"com.user.battery-menubar\" => false\n}").unwrap()
        );
        assert!(disabled("permission denied").is_err());
        assert!(
            !disabled("disabled services = {\n \"com.user.battery-menubar\" => enabled\n}")
                .unwrap()
        );
        assert!(
            disabled("disabled services = {\n \"com.user.battery-menubar\" => disabled\n}")
                .unwrap()
        );
        assert!(
            disabled("disabled services = {\n \"com.user.battery-menubar\" => unknown\n}").is_err()
        );
        assert!(disabled("disabled services = {\n \"com.user.battery-menubar\" => true\n \"com.user.battery-menubar\" => false\n}").is_err());
    }
    #[test]
    fn job_must_target_this_app_and_the_named_launcher() {
        let home = PathBuf::from("/Users/test");
        let exe = home.join("BatteryBar-Swift/BatteryBar.app/Contents/MacOS/BatteryBar");
        let mut args = vec![home.join("Applications/Automazioni/Batteria - Barra menu.app/Contents/MacOS/Batteria - Barra menu").to_string_lossy().into_owned(), exe.to_string_lossy().into_owned()];
        assert!(valid_arguments(&args, &exe, &home));
        args[1] = "/tmp/other-app".into();
        assert!(!valid_arguments(&args, &exe, &home));
        args[1] = exe.to_string_lossy().into_owned();
        args[0] = "/bin/bash".into();
        assert!(!valid_arguments(&args, &exe, &home));
    }
    #[test]
    fn direct_job_accepts_only_the_canonical_app_without_extra_arguments() {
        let home = PathBuf::from("/Users/test");
        let exe = home.join("BatteryBar-Swift/BatteryBar.app/Contents/MacOS/BatteryBar");
        let args = vec![exe.to_string_lossy().into_owned()];
        assert!(valid_arguments(&args, &exe, &home));
        let staged = home.join("staging/BatteryBar.app/Contents/MacOS/BatteryBar");
        assert!(!valid_arguments(
            &[staged.to_string_lossy().into_owned()],
            &staged,
            &home
        ));
        assert!(!valid_arguments(&[], &exe, &home));
        assert!(!valid_arguments(
            &[args[0].clone(), "--production".into()],
            &exe,
            &home
        ));
        assert!(!valid_arguments(&["/bin/sh".into()], &exe, &home));
    }
}
