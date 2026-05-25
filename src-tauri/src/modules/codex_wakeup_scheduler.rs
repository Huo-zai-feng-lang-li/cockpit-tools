use crate::modules::{codex_account, codex_wakeup, logger};
use chrono::{DateTime, Datelike, Local, TimeZone};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::AppHandle;
use tokio::time::sleep;

static STARTED: OnceLock<Mutex<bool>> = OnceLock::new();
static RUNNING_TASKS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
const FULL_QUOTA_WAKEUP_THRESHOLD: i32 = 95;
const PRIMARY_WINDOW_FALLBACK_MINUTES: i64 = 5 * 60;
const SECONDARY_WINDOW_FALLBACK_MINUTES: i64 = 7 * 24 * 60;
const FRESH_WINDOW_GRACE_SECONDS: i64 = 12 * 60 * 60;
const FULL_QUOTA_WITHOUT_RESET_COOLDOWN_SECONDS: i64 = 6 * 24 * 60 * 60;

fn started_flag() -> &'static Mutex<bool> {
    STARTED.get_or_init(|| Mutex::new(false))
}

fn running_tasks() -> &'static Mutex<HashSet<String>> {
    RUNNING_TASKS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn parse_time_to_minutes(value: &str) -> Option<i32> {
    let parts: Vec<&str> = value.trim().split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hour: i32 = parts[0].parse().ok()?;
    let minute: i32 = parts[1].parse().ok()?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }
    Some(hour * 60 + minute)
}

fn build_local_datetime(date: chrono::NaiveDate, minutes: i32) -> Option<DateTime<Local>> {
    let hour = (minutes / 60) as u32;
    let minute = (minutes % 60) as u32;
    Local
        .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0)
        .earliest()
        .or_else(|| {
            Local
                .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0)
                .latest()
        })
}

fn fresh_quota_window_due_at(
    reset_at: i64,
    percentage: i32,
    window_minutes: Option<i64>,
    fallback_window_minutes: i64,
    last_run_at: Option<i64>,
    now_ts: i64,
) -> Option<i64> {
    if reset_at <= now_ts || percentage < FULL_QUOTA_WAKEUP_THRESHOLD {
        return None;
    }

    let window_seconds = window_minutes.unwrap_or(fallback_window_minutes).max(1) * 60;
    let remaining_seconds = reset_at.saturating_sub(now_ts);
    if remaining_seconds + FRESH_WINDOW_GRACE_SECONDS < window_seconds {
        return None;
    }

    let cycle_started_at = reset_at.saturating_sub(window_seconds);
    let already_processed = last_run_at
        .map(|value| value >= cycle_started_at.saturating_sub(FRESH_WINDOW_GRACE_SECONDS))
        .unwrap_or(false);
    if already_processed {
        return None;
    }

    Some(i64::max(cycle_started_at, now_ts))
}

fn full_quota_without_reset_due_at(
    percentage: i32,
    reset_at: Option<i64>,
    has_active_reset_countdown: bool,
    last_run_at: Option<i64>,
    now_ts: i64,
) -> Option<i64> {
    let should_skip = reset_at.is_some()
        || has_active_reset_countdown
        || percentage < FULL_QUOTA_WAKEUP_THRESHOLD;
    if should_skip {
        return None;
    }

    if last_run_at
        .map(|value| now_ts.saturating_sub(value) < FULL_QUOTA_WITHOUT_RESET_COOLDOWN_SECONDS)
        .unwrap_or(false)
    {
        return None;
    }

    Some(now_ts)
}

fn has_active_reset_countdown(reset_at: Option<i64>, now_ts: i64) -> bool {
    reset_at.map(|ts| ts > now_ts).unwrap_or(false)
}

fn collect_task_reset_timestamps(task: &codex_wakeup::CodexWakeupTask) -> Vec<i64> {
    if task.account_ids.is_empty() {
        return Vec::new();
    }
    let quota_reset_window = task
        .schedule
        .quota_reset_window
        .as_deref()
        .unwrap_or("either");
    let include_primary = quota_reset_window == "either" || quota_reset_window == "primary_window";
    let include_secondary =
        quota_reset_window == "either" || quota_reset_window == "secondary_window";

    let selected: HashSet<&str> = task.account_ids.iter().map(String::as_str).collect();
    let mut timestamps: Vec<i64> = codex_account::list_accounts()
        .into_iter()
        .filter(|account| selected.contains(account.id.as_str()))
        .flat_map(|account| account.quota.into_iter())
        .flat_map(|quota| {
            let mut values = Vec::new();
            if include_primary {
                values.push(quota.hourly_reset_time);
            }
            if include_secondary {
                values.push(quota.weekly_reset_time);
            }
            values
        })
        .flatten()
        .filter(|ts| *ts > 0)
        .collect();
    timestamps.sort_unstable();
    timestamps.dedup();
    timestamps
}

fn current_due_at(
    task: &codex_wakeup::CodexWakeupTask,
    now: DateTime<Local>,
) -> Option<(i64, Option<Vec<String>>)> {
    match task.schedule.kind.as_str() {
        "daily" => {
            let minutes = parse_time_to_minutes(task.schedule.daily_time.as_deref()?)?;
            let candidate = build_local_datetime(now.date_naive(), minutes)?.timestamp();
            if candidate <= now.timestamp() && task.last_run_at.unwrap_or(0) < candidate {
                Some((candidate, None))
            } else {
                None
            }
        }
        "weekly" => {
            let minutes = parse_time_to_minutes(task.schedule.weekly_time.as_deref()?)?;
            let weekday = now.weekday().num_days_from_sunday() as i32;
            if !task.schedule.weekly_days.contains(&weekday) {
                return None;
            }
            let candidate = build_local_datetime(now.date_naive(), minutes)?.timestamp();
            if candidate <= now.timestamp() && task.last_run_at.unwrap_or(0) < candidate {
                Some((candidate, None))
            } else {
                None
            }
        }
        "interval" => {
            let interval_seconds =
                i64::from(task.schedule.interval_hours.unwrap_or(4).max(1)) * 3600;
            let due_at = task.last_run_at.unwrap_or(task.created_at) + interval_seconds;
            if due_at <= now.timestamp() {
                Some((due_at, None))
            } else {
                None
            }
        }
        "quota_reset" => {
            let selected: HashSet<&str> = task.account_ids.iter().map(String::as_str).collect();
            let mut due_accounts = Vec::new();
            let mut latest_ts = 0i64;
            let now_ts = now.timestamp();

            let quota_reset_window = task
                .schedule
                .quota_reset_window
                .as_deref()
                .unwrap_or("either");
            let include_primary =
                quota_reset_window == "either" || quota_reset_window == "primary_window";
            let include_secondary =
                quota_reset_window == "either" || quota_reset_window == "secondary_window";

            for account in codex_account::list_accounts() {
                if !selected.contains(account.id.as_str()) {
                    continue;
                }
                let last_run_at = task.last_run_map.get(&account.id).cloned();
                if let Some(quota) = account.quota {
                    let mut account_max_ts = 0i64;
                    let account_has_active_reset_countdown =
                        has_active_reset_countdown(quota.hourly_reset_time, now_ts)
                            || has_active_reset_countdown(quota.weekly_reset_time, now_ts);
                    if include_primary {
                        if let Some(due_at) = full_quota_without_reset_due_at(
                            quota.hourly_percentage,
                            quota.hourly_reset_time,
                            account_has_active_reset_countdown,
                            last_run_at,
                            now_ts,
                        ) {
                            account_max_ts = i64::max(account_max_ts, due_at);
                        }
                        if let Some(ts) = quota.hourly_reset_time {
                            if ts <= now_ts && ts > last_run_at.unwrap_or(task.created_at) {
                                account_max_ts = i64::max(account_max_ts, ts);
                            }
                            if let Some(due_at) = fresh_quota_window_due_at(
                                ts,
                                quota.hourly_percentage,
                                quota.hourly_window_minutes,
                                PRIMARY_WINDOW_FALLBACK_MINUTES,
                                last_run_at,
                                now_ts,
                            ) {
                                account_max_ts = i64::max(account_max_ts, due_at);
                            }
                        }
                    }
                    if include_secondary {
                        if let Some(due_at) = full_quota_without_reset_due_at(
                            quota.weekly_percentage,
                            quota.weekly_reset_time,
                            account_has_active_reset_countdown,
                            last_run_at,
                            now_ts,
                        ) {
                            account_max_ts = i64::max(account_max_ts, due_at);
                        }
                        if let Some(ts) = quota.weekly_reset_time {
                            if ts <= now_ts && ts > last_run_at.unwrap_or(task.created_at) {
                                account_max_ts = i64::max(account_max_ts, ts);
                            }
                            if let Some(due_at) = fresh_quota_window_due_at(
                                ts,
                                quota.weekly_percentage,
                                quota.weekly_window_minutes,
                                SECONDARY_WINDOW_FALLBACK_MINUTES,
                                last_run_at,
                                now_ts,
                            ) {
                                account_max_ts = i64::max(account_max_ts, due_at);
                            }
                        }
                    }

                    if account_max_ts > 0 {
                        due_accounts.push(account.id);
                        latest_ts = i64::max(latest_ts, account_max_ts);
                    }
                }
            }

            if !due_accounts.is_empty() {
                Some((latest_ts, Some(due_accounts)))
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn calculate_next_run_at(task: &codex_wakeup::CodexWakeupTask) -> Option<i64> {
    let now = Local::now();
    match task.schedule.kind.as_str() {
        "daily" => {
            let minutes = parse_time_to_minutes(task.schedule.daily_time.as_deref()?)?;
            for offset in 0..7 {
                let date = now.date_naive() + chrono::Duration::days(offset);
                let candidate = build_local_datetime(date, minutes)?.timestamp();
                if candidate > now.timestamp() {
                    return Some(candidate);
                }
            }
            None
        }
        "weekly" => {
            let minutes = parse_time_to_minutes(task.schedule.weekly_time.as_deref()?)?;
            for offset in 0..14 {
                let date = now.date_naive() + chrono::Duration::days(offset);
                let weekday = date.weekday().num_days_from_sunday() as i32;
                if !task.schedule.weekly_days.contains(&weekday) {
                    continue;
                }
                let candidate = build_local_datetime(date, minutes)?.timestamp();
                if candidate > now.timestamp() {
                    return Some(candidate);
                }
            }
            None
        }
        "interval" => {
            let interval_seconds =
                i64::from(task.schedule.interval_hours.unwrap_or(4).max(1)) * 3600;
            Some(task.last_run_at.unwrap_or(task.created_at) + interval_seconds)
        }
        "quota_reset" => current_due_at(task, now)
            .map(|(due_at, _)| due_at)
            .or_else(|| {
                collect_task_reset_timestamps(task)
                    .into_iter()
                    .filter(|reset_at| *reset_at > now.timestamp())
                    .min()
            }),
        _ => None,
    }
}

fn mark_running(task_id: &str) -> bool {
    let mut guard = running_tasks()
        .lock()
        .expect("codex wakeup running tasks lock");
    guard.insert(task_id.to_string())
}

fn unmark_running(task_id: &str) {
    let mut guard = running_tasks()
        .lock()
        .expect("codex wakeup running tasks lock");
    guard.remove(task_id);
}

pub async fn run_task_now(
    app: Option<&AppHandle>,
    task_id: &str,
    trigger_type: &str,
    run_id: Option<String>,
    target_account_ids: Option<Vec<String>>,
) -> Result<codex_wakeup::CodexWakeupBatchResult, String> {
    let task =
        codex_wakeup::get_task(task_id)?.ok_or_else(|| format!("唤醒任务不存在: {}", task_id))?;
    if !mark_running(&task.id) {
        return Err("该任务正在执行中".to_string());
    }

    let context = codex_wakeup::TaskRunContext {
        trigger_type: trigger_type.to_string(),
        task_id: Some(task.id.clone()),
        task_name: Some(task.name.clone()),
    };
    let execution_account_ids = target_account_ids.unwrap_or_else(|| task.account_ids.clone());
    let result = codex_wakeup::run_batch(
        app,
        execution_account_ids,
        task.prompt.clone(),
        codex_wakeup::CodexWakeupExecutionConfig {
            model: task.model.clone(),
            model_display_name: task.model_display_name.clone(),
            model_reasoning_effort: task.model_reasoning_effort.clone(),
        },
        context,
        run_id,
        None,
    )
    .await;

    if let Ok(batch) = &result {
        if let Err(err) = codex_wakeup::update_task_after_run(&task.id, &batch.records) {
            logger::log_warn(&format!("[CodexWakeup] 更新任务执行结果失败: {}", err));
        }
    }

    unmark_running(&task.id);
    result
}

async fn run_scheduler_once(app: &AppHandle) {
    let state = match codex_wakeup::load_state_for_scheduler() {
        Ok(state) => state,
        Err(err) => {
            logger::log_warn(&format!("[CodexWakeup] 读取任务状态失败: {}", err));
            return;
        }
    };

    if !state.enabled {
        return;
    }

    let now = Local::now();
    for task in state.tasks {
        if !task.enabled {
            continue;
        }
        let Some((_, target_account_ids)) = current_due_at(&task, now) else {
            continue;
        };

        let task_id = task.id.clone();
        let trigger_type = if task.schedule.kind == "quota_reset" {
            "quota_reset"
        } else {
            "scheduled"
        }
        .to_string();
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let result = run_task_now(
                Some(&app_handle),
                &task_id,
                &trigger_type,
                None,
                target_account_ids,
            )
            .await;
            if let Err(err) = result {
                logger::log_warn(&format!(
                    "[CodexWakeup] 调度任务执行失败: task_id={}, error={}",
                    task_id, err
                ));
            }
        });
    }
}

pub fn ensure_started(app: AppHandle) {
    let mut started = started_flag()
        .lock()
        .expect("codex wakeup scheduler started lock");
    if *started {
        return;
    }
    *started = true;

    tauri::async_runtime::spawn(async move {
        loop {
            run_scheduler_once(&app).await;
            sleep(Duration::from_secs(30)).await;
        }
    });
}
