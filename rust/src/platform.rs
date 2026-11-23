use serde::de::DeserializeOwned;
use serde_json::Value;
use std::ffi::{CStr, CString};
unsafe extern "C" {
    fn bb_battery() -> *mut libc::c_char;
    fn bb_counters() -> *mut libc::c_char;
    fn bb_devices() -> *mut libc::c_char;
    fn bb_timebase() -> f64;
    fn bb_preferences(isolated: bool);
    fn bb_get_setting(key: *const libc::c_char) -> *mut libc::c_char;
    fn bb_set_setting(key: *const libc::c_char, value: *const libc::c_char);
    fn bb_notifications(enabled: bool);
    fn bb_notify(title: *const libc::c_char, body: *const libc::c_char);
    fn bb_login_status() -> i32;
    fn bb_home_dir() -> *mut libc::c_char;
    fn bb_system_status() -> *mut libc::c_char;
    fn bb_production_bundle() -> bool;
    fn bb_set_login(enabled: bool) -> *mut libc::c_char;
    fn bb_events(callback: extern "C" fn(i32));
    fn bb_watch_dismiss(enabled: bool);
    fn bb_stop_events();
    fn bb_memory() -> *mut libc::c_char;
    fn bb_capture_view(view: *mut libc::c_void, path: *const libc::c_char) -> bool;
    fn bb_status_appearance_check(
        button: *mut libc::c_void,
        prefix: *const libc::c_char,
    ) -> *mut libc::c_char;
}
fn string(ptr: *mut libc::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        libc::free(ptr.cast());
        Some(s)
    }
}
fn decode<T: DeserializeOwned>(ptr: *mut libc::c_char) -> Option<T> {
    serde_json::from_str(&string(ptr)?).ok()
}
fn c(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap()
}
pub fn battery() -> Option<Value> {
    unsafe { decode(bb_battery()) }
}
pub fn counters() -> Vec<crate::model::Counter> {
    unsafe { decode(bb_counters()).unwrap_or_default() }
}
pub fn devices() -> Vec<String> {
    unsafe { decode(bb_devices()).unwrap_or_default() }
}
pub fn timebase() -> f64 {
    unsafe { bb_timebase() }
}
pub fn preferences(isolated: bool) {
    unsafe { bb_preferences(isolated) }
}
pub fn get(key: &str) -> Option<Value> {
    let mut a: Vec<Value> = unsafe { decode(bb_get_setting(c(key).as_ptr())) }?;
    a.pop().filter(|v| !v.is_null())
}
pub fn set(key: &str, value: Value) {
    unsafe {
        bb_set_setting(
            c(key).as_ptr(),
            c(&serde_json::to_string(&[value]).unwrap()).as_ptr(),
        )
    }
}
pub fn notifications(enabled: bool) {
    unsafe { bb_notifications(enabled) }
}
pub fn notify(title: &str, body: &str) {
    unsafe { bb_notify(c(title).as_ptr(), c(body).as_ptr()) }
}
pub fn login() -> bool {
    crate::startup::enabled().unwrap_or(false)
}
pub fn home_dir() -> Option<std::path::PathBuf> {
    unsafe { string(bb_home_dir()).map(Into::into) }
}
pub fn sm_login_status() -> i32 {
    unsafe { bb_login_status() }
}
pub fn system_status() -> Value {
    let mut status: Value = unsafe { decode(bb_system_status()).unwrap_or(Value::Null) };
    status["startup"] = crate::startup::diagnostic();
    status
}
pub fn production_bundle() -> bool {
    unsafe { bb_production_bundle() }
}
pub fn set_login(enabled: bool) -> Result<(), String> {
    crate::startup::set(enabled)
}
pub fn set_sm_login(enabled: bool) -> Result<(), String> {
    unsafe {
        match string(bb_set_login(enabled)) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
pub fn events(cb: extern "C" fn(i32)) {
    unsafe { bb_events(cb) }
}
pub fn dismiss_watch(enabled: bool) {
    unsafe { bb_watch_dismiss(enabled) }
}
pub fn stop_events() {
    unsafe { bb_stop_events() }
}
pub fn memory() -> Value {
    unsafe { decode(bb_memory()).unwrap_or(Value::Null) }
}
pub unsafe fn capture(view: *mut libc::c_void, path: &str) -> bool {
    bb_capture_view(view, c(path).as_ptr())
}
pub unsafe fn status_appearance_check(button: *mut libc::c_void, prefix: &str) -> Vec<Value> {
    // The UI owns this NSStatusBarButton and calls on its main thread. The
    // bridge borrows it, restores its appearance, and returns malloc-owned JSON.
    decode(bb_status_appearance_check(button, c(prefix).as_ptr())).unwrap_or_default()
}
