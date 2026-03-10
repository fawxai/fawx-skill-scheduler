//! Scheduler skill — cron-based scheduling, reminders, and periodic jobs for Fawx agents.
//!
//! Actions: add, remove, list, check (for due jobs).
//! Jobs persist across invocations via the host KV store.

mod cron;

use cron::CronExpr;
use serde::{Deserialize, Serialize};

// ── Data types ──────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Job {
    name: String,
    cron_expr: String,
    message: String,
    tz_offset_hours: i32,
    last_fired_unix: Option<i64>,
    created_unix: i64,
}

#[derive(Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    schedule: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    tz_offset_hours: Option<i32>,
    #[serde(default)]
    now_unix: Option<i64>,
}

#[derive(Serialize)]
struct AddResponse {
    status: String,
    name: String,
    schedule: String,
}

#[derive(Serialize)]
struct RemoveResponse {
    status: String,
    name: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct ListJobEntry {
    name: String,
    schedule: String,
    message: String,
}

#[derive(Serialize)]
struct ListResponse {
    jobs: Vec<ListJobEntry>,
}

#[derive(Serialize)]
struct DueJobEntry {
    name: String,
    message: String,
}

#[derive(Serialize)]
struct CheckResponse {
    due: Vec<DueJobEntry>,
}

// ── Host API imports ────────────────────────────────────────────────────────

#[cfg(not(test))]
#[link(wasm_import_module = "host_api_v1")]
extern "C" {
    #[link_name = "log"]
    fn host_log(level: u32, msg_ptr: *const u8, msg_len: u32);
    #[link_name = "get_input"]
    fn host_get_input() -> u32;
    #[link_name = "set_output"]
    fn host_set_output(text_ptr: *const u8, text_len: u32);
    #[link_name = "kv_get"]
    fn host_kv_get(key_ptr: *const u8, key_len: u32) -> u32;
    #[link_name = "kv_set"]
    fn host_kv_set(key_ptr: *const u8, key_len: u32, val_ptr: *const u8, val_len: u32);
}

// ── Host wrappers (production) ──────────────────────────────────────────────

#[cfg(not(test))]
const MAX_HOST_STRING_LEN: usize = 65536;

#[cfg(not(test))]
const KV_KEY: &str = "scheduler:jobs";

/// Read a null-terminated string from WASM linear memory.
///
/// # Safety
/// The caller must ensure `ptr` points to valid WASM linear memory
/// containing a null-terminated string.
#[cfg(not(test))]
unsafe fn read_host_string(ptr: u32) -> String {
    if ptr == 0 {
        return String::new();
    }
    let slice = core::slice::from_raw_parts(ptr as *const u8, MAX_HOST_STRING_LEN);
    let len = slice
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(MAX_HOST_STRING_LEN);
    String::from_utf8_lossy(&slice[..len]).to_string()
}

#[cfg(not(test))]
fn log_msg(level: u32, message: &str) {
    unsafe { host_log(level, message.as_ptr(), message.len() as u32) }
}

#[cfg(not(test))]
fn get_input_str() -> String {
    unsafe {
        let ptr = host_get_input();
        read_host_string(ptr)
    }
}

#[cfg(not(test))]
fn set_output_str(text: &str) {
    unsafe { host_set_output(text.as_ptr(), text.len() as u32) }
}

#[cfg(not(test))]
fn kv_get_str(key: &str) -> Option<String> {
    unsafe {
        let ptr = host_kv_get(key.as_ptr(), key.len() as u32);
        if ptr == 0 {
            None
        } else {
            Some(read_host_string(ptr))
        }
    }
}

#[cfg(not(test))]
fn kv_set_str(key: &str, value: &str) {
    unsafe {
        host_kv_set(
            key.as_ptr(),
            key.len() as u32,
            value.as_ptr(),
            value.len() as u32,
        )
    }
}

// ── Job storage helpers ─────────────────────────────────────────────────────

fn load_jobs(stored: Option<String>) -> Vec<Job> {
    match stored {
        Some(data) if !data.is_empty() => serde_json::from_str(&data).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn serialize_jobs(jobs: &[Job]) -> Result<String, String> {
    serde_json::to_string(jobs).map_err(|e| format!("failed to serialize jobs: {}", e))
}

fn serialize_output<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|e| format!(r#"{{"error":"serialization failed: {}"}}"#, e))
}

// ── Action handlers ─────────────────────────────────────────────────────────

fn handle_add(input: &Input, jobs: &mut Vec<Job>) -> String {
    let name = match &input.name {
        Some(n) => n.clone(),
        None => {
            return serialize_output(&ErrorResponse {
                error: "missing 'name' field".to_string(),
            })
        }
    };
    let schedule = match &input.schedule {
        Some(s) => s.clone(),
        None => {
            return serialize_output(&ErrorResponse {
                error: "missing 'schedule' field".to_string(),
            })
        }
    };
    let message = input.message.clone().unwrap_or_default();
    let tz_offset = input.tz_offset_hours.unwrap_or(0);

    // Validate cron expression
    if let Err(e) = CronExpr::parse(&schedule) {
        return serialize_output(&ErrorResponse {
            error: format!("invalid cron expression: {}", e),
        });
    }

    // Remove existing job with same name (update semantics)
    jobs.retain(|j| j.name != name);

    let now_unix = input.now_unix.unwrap_or(0);
    jobs.push(Job {
        name: name.clone(),
        cron_expr: schedule.clone(),
        message,
        tz_offset_hours: tz_offset,
        last_fired_unix: None,
        created_unix: now_unix,
    });

    serialize_output(&AddResponse {
        status: "added".to_string(),
        name,
        schedule,
    })
}

fn handle_remove(input: &Input, jobs: &mut Vec<Job>) -> String {
    let name = match &input.name {
        Some(n) => n.clone(),
        None => {
            return serialize_output(&ErrorResponse {
                error: "missing 'name' field".to_string(),
            })
        }
    };

    let before = jobs.len();
    jobs.retain(|j| j.name != name);

    if jobs.len() < before {
        serialize_output(&RemoveResponse {
            status: "removed".to_string(),
            name,
        })
    } else {
        serialize_output(&ErrorResponse {
            error: "job not found".to_string(),
        })
    }
}

fn handle_list(jobs: &[Job]) -> String {
    let entries: Vec<ListJobEntry> = jobs
        .iter()
        .map(|j| ListJobEntry {
            name: j.name.clone(),
            schedule: j.cron_expr.clone(),
            message: j.message.clone(),
        })
        .collect();

    serialize_output(&ListResponse { jobs: entries })
}

/// Break a unix timestamp (+ tz offset in hours) into (minute, hour, day, month, weekday).
fn unix_to_components(unix: i64, tz_offset_hours: i32) -> (u8, u8, u8, u8, u8) {
    let adjusted = unix + (tz_offset_hours as i64) * 3600;

    // Days since epoch (1970-01-01 is Thursday = weekday 4)
    let day_seconds = 86400_i64;
    let mut remaining = adjusted;

    // Floor division for negative timestamps
    let total_days = if remaining >= 0 {
        remaining / day_seconds
    } else {
        (remaining - day_seconds + 1) / day_seconds
    };
    remaining = adjusted - total_days * day_seconds;

    let hour = (remaining / 3600) as u8;
    let minute = ((remaining % 3600) / 60) as u8;

    // Weekday: 1970-01-01 was Thursday (4)
    let weekday = ((total_days % 7 + 4 + 7) % 7) as u8; // 0=Sun

    // Date from total_days using a civil calendar algorithm
    let (year, month, day) = days_to_date(total_days);
    let _ = year; // we only need month/day

    (minute, hour, day as u8, month as u8, weekday)
}

/// Convert days since 1970-01-01 to (year, month, day).
/// Adapted from Howard Hinnant's civil_from_days algorithm.
fn days_to_date(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

fn handle_check(input: &Input, jobs: &mut [Job]) -> String {
    let now_unix = match input.now_unix {
        Some(t) => t,
        None => {
            return serialize_output(&ErrorResponse {
                error: "missing 'now_unix' field".to_string(),
            })
        }
    };

    // Floor to the start of the current minute
    let current_minute_start = now_unix - (now_unix % 60);

    let mut due = Vec::new();

    for job in jobs.iter_mut() {
        // Double-fire prevention: skip if already fired in this minute
        if let Some(last) = job.last_fired_unix {
            let last_minute_start = last - (last % 60);
            if last_minute_start == current_minute_start {
                continue;
            }
        }

        let expr = match CronExpr::parse(&job.cron_expr) {
            Ok(e) => e,
            Err(_) => continue, // skip invalid jobs silently
        };

        let (minute, hour, day, month, weekday) = unix_to_components(now_unix, job.tz_offset_hours);

        if expr.matches(minute, hour, day, month, weekday) {
            due.push(DueJobEntry {
                name: job.name.clone(),
                message: job.message.clone(),
            });
            job.last_fired_unix = Some(now_unix);
        }
    }

    serialize_output(&CheckResponse { due })
}

// ── Core processing (testable) ──────────────────────────────────────────────

fn process_input(input_str: &str, stored_jobs: Option<String>) -> (String, Option<String>) {
    let input: Input = match serde_json::from_str(input_str) {
        Ok(i) => i,
        Err(e) => {
            let output = serialize_output(&ErrorResponse {
                error: format!("invalid input: {}", e),
            });
            return (output, None);
        }
    };

    let mut jobs = load_jobs(stored_jobs);

    let output = match input.action.as_str() {
        "add" => handle_add(&input, &mut jobs),
        "remove" => handle_remove(&input, &mut jobs),
        "list" => handle_list(&jobs),
        "check" => handle_check(&input, &mut jobs),
        other => serialize_output(&ErrorResponse {
            error: format!("unknown action: {}", other),
        }),
    };

    // For mutating actions, serialize jobs back
    let save = match input.action.as_str() {
        "add" | "remove" | "check" => serialize_jobs(&jobs).ok(),
        _ => None,
    };

    (output, save)
}

// ── Entry point ─────────────────────────────────────────────────────────────

#[no_mangle]
#[cfg(not(test))]
pub extern "C" fn run() {
    log_msg(2, "Scheduler skill starting");

    let input_str = get_input_str();
    log_msg(2, &format!("Input: {}", input_str));

    let stored = kv_get_str(KV_KEY);
    let (output, save) = process_input(&input_str, stored);

    if let Some(jobs_json) = save {
        kv_set_str(KV_KEY, &jobs_json);
    }

    log_msg(2, "Scheduler skill complete");
    set_output_str(&output);
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_job() {
        let input = r#"{"action":"add","name":"standup","schedule":"0 9 * * *","message":"Time for standup!","tz_offset_hours":-7}"#;
        let (output, save) = process_input(input, None);

        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(resp["status"], "added");
        assert_eq!(resp["name"], "standup");
        assert_eq!(resp["schedule"], "0 9 * * *");

        // Jobs should be saved
        assert!(save.is_some());
        let jobs: Vec<Job> = serde_json::from_str(&save.unwrap()).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "standup");
        assert_eq!(jobs[0].tz_offset_hours, -7);
    }

    #[test]
    fn test_add_records_created_unix() {
        let input =
            r#"{"action":"add","name":"standup","schedule":"0 9 * * *","now_unix":1709888400}"#;
        let (_, save) = process_input(input, None);

        let jobs: Vec<Job> = serde_json::from_str(&save.unwrap()).unwrap();
        assert_eq!(jobs[0].created_unix, 1709888400);
    }

    #[test]
    fn test_add_invalid_cron() {
        let input = r#"{"action":"add","name":"bad","schedule":"60 * * * *","message":"nope"}"#;
        let (output, _) = process_input(input, None);

        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(resp["error"].as_str().unwrap().contains("invalid cron"));
    }

    #[test]
    fn test_add_missing_fields() {
        let input = r#"{"action":"add"}"#;
        let (output, _) = process_input(input, None);
        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(resp["error"].as_str().unwrap().contains("missing"));
    }

    #[test]
    fn test_remove_job() {
        // First add a job
        let add = r#"{"action":"add","name":"standup","schedule":"0 9 * * *","message":"hi"}"#;
        let (_, save) = process_input(add, None);

        // Then remove it
        let remove = r#"{"action":"remove","name":"standup"}"#;
        let (output, save2) = process_input(remove, save);

        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(resp["status"], "removed");
        assert_eq!(resp["name"], "standup");

        let jobs: Vec<Job> = serde_json::from_str(&save2.unwrap()).unwrap();
        assert_eq!(jobs.len(), 0);
    }

    #[test]
    fn test_remove_nonexistent() {
        let input = r#"{"action":"remove","name":"nope"}"#;
        let (output, _) = process_input(input, None);

        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(resp["error"], "job not found");
    }

    #[test]
    fn test_list_empty() {
        let input = r#"{"action":"list"}"#;
        let (output, save) = process_input(input, None);

        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(resp["jobs"].as_array().unwrap().len(), 0);
        assert!(save.is_none()); // list doesn't save
    }

    #[test]
    fn test_list_with_jobs() {
        let add1 = r#"{"action":"add","name":"a","schedule":"0 9 * * *","message":"msg a"}"#;
        let (_, save) = process_input(add1, None);
        let add2 = r#"{"action":"add","name":"b","schedule":"30 12 * * *","message":"msg b"}"#;
        let (_, save) = process_input(add2, save);

        let list = r#"{"action":"list"}"#;
        let (output, _) = process_input(list, save);

        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        let jobs = resp["jobs"].as_array().unwrap();
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn test_check_due() {
        // Add a job: 0 9 * * * (9:00 AM UTC)
        let add = r#"{"action":"add","name":"morning","schedule":"0 9 * * *","message":"Good morning!","tz_offset_hours":0}"#;
        let (_, save) = process_input(add, None);

        // Check at 2024-03-08 09:00:00 UTC (Friday)
        // 2024-03-08 = 1709856000 + 9*3600 = 1709888400
        // Actually let's compute: 2024-03-08 00:00 UTC = 1709856000
        // 09:00 UTC = 1709856000 + 32400 = 1709888400
        let check = r#"{"action":"check","now_unix":1709888400}"#;
        let (output, save2) = process_input(check, save);

        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        let due = resp["due"].as_array().unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0]["name"], "morning");
        assert_eq!(due[0]["message"], "Good morning!");

        // Verify last_fired_unix was set
        let jobs: Vec<Job> = serde_json::from_str(&save2.unwrap()).unwrap();
        assert_eq!(jobs[0].last_fired_unix, Some(1709888400));
    }

    #[test]
    fn test_check_not_due() {
        let add = r#"{"action":"add","name":"morning","schedule":"0 9 * * *","message":"hi","tz_offset_hours":0}"#;
        let (_, save) = process_input(add, None);

        // Check at 10:00 — not due
        let check = r#"{"action":"check","now_unix":1709892000}"#;
        let (output, _) = process_input(check, save);

        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(resp["due"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_double_fire_prevention() {
        let add = r#"{"action":"add","name":"morning","schedule":"0 9 * * *","message":"hi","tz_offset_hours":0}"#;
        let (_, save) = process_input(add, None);

        // First check at 09:00:00
        let check1 = r#"{"action":"check","now_unix":1709888400}"#;
        let (output1, save) = process_input(check1, save);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output1).unwrap()["due"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        // Second check at 09:00:30 (same minute) — should NOT fire again
        let check2 = r#"{"action":"check","now_unix":1709888430}"#;
        let (output2, _) = process_input(check2, save);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output2).unwrap()["due"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn test_timezone_offset() {
        // Job at 9:00 AM with tz_offset -7 (Mountain Time)
        // That means it fires when UTC time is 16:00 (9 + 7 = 16)
        let add = r#"{"action":"add","name":"standup","schedule":"0 9 * * *","message":"standup","tz_offset_hours":-7}"#;
        let (_, save) = process_input(add, None);

        // Check at 16:00 UTC = 09:00 MT — should fire
        // 2024-03-08 16:00 UTC = 1709856000 + 57600 = 1709913600
        let check = r#"{"action":"check","now_unix":1709913600}"#;
        let (output, _) = process_input(check, save.clone());
        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(resp["due"].as_array().unwrap().len(), 1);

        // Check at 09:00 UTC = 02:00 MT — should NOT fire
        let check2 = r#"{"action":"check","now_unix":1709888400}"#;
        let (output2, _) = process_input(check2, save);
        let resp2: serde_json::Value = serde_json::from_str(&output2).unwrap();
        assert_eq!(resp2["due"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_unknown_action() {
        let input = r#"{"action":"dance"}"#;
        let (output, _) = process_input(input, None);
        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(resp["error"].as_str().unwrap().contains("unknown action"));
    }

    #[test]
    fn test_invalid_json() {
        let (output, _) = process_input("not json", None);
        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(resp["error"].as_str().unwrap().contains("invalid input"));
    }

    #[test]
    fn test_unix_to_components_epoch() {
        // 1970-01-01 00:00 UTC = Thursday (4)
        let (min, hour, day, month, weekday) = unix_to_components(0, 0);
        assert_eq!((min, hour, day, month, weekday), (0, 0, 1, 1, 4));
    }

    #[test]
    fn test_unix_to_components_known_date() {
        // 2024-03-08 09:30:00 UTC = Friday (5)
        // 1709888400 = 2024-03-08 09:00 UTC, + 1800 = 09:30
        let ts = 1709888400 + 1800;
        let (min, hour, day, month, weekday) = unix_to_components(ts, 0);
        assert_eq!(min, 30);
        assert_eq!(hour, 9);
        assert_eq!(day, 8);
        assert_eq!(month, 3);
        assert_eq!(weekday, 5); // Friday
    }

    #[test]
    fn test_unix_to_components_with_tz() {
        // 2024-03-08 16:00 UTC with tz_offset -7 => 09:00 local
        let (min, hour, _, _, _) = unix_to_components(1709913600, -7);
        assert_eq!(min, 0);
        assert_eq!(hour, 9);
    }

    #[test]
    fn test_add_updates_existing() {
        let add1 = r#"{"action":"add","name":"test","schedule":"0 9 * * *","message":"old"}"#;
        let (_, save) = process_input(add1, None);

        let add2 = r#"{"action":"add","name":"test","schedule":"30 10 * * *","message":"new"}"#;
        let (output, save) = process_input(add2, save);

        let resp: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(resp["status"], "added");

        let jobs: Vec<Job> = serde_json::from_str(&save.unwrap()).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].message, "new");
        assert_eq!(jobs[0].cron_expr, "30 10 * * *");
    }
}
