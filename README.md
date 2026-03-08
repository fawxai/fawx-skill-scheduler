# fawx-skill-scheduler

A Fawx WASM skill for scheduling, cron jobs, and reminders. Jobs persist across invocations via the host KV store.

## Features

- **Cron-based scheduling** — standard 5-field cron expressions (minute hour day month weekday)
- **Timezone support** — per-job UTC offset
- **Double-fire prevention** — jobs won't fire twice in the same minute
- **Persistent storage** — jobs survive across invocations via host KV store

## Cron Expression Support

| Pattern | Meaning | Example |
|---------|---------|---------|
| `*` | Every value | `* * * * *` (every minute) |
| `N` | Exact value | `30 9 * * *` (9:30) |
| `*/N` | Every N | `*/15 * * * *` (every 15 min) |
| `N,M` | List | `0,30 * * * *` (on :00 and :30) |
| `N-M` | Range | `0 9-17 * * *` (9 AM to 5 PM) |

Fields: `minute(0-59) hour(0-23) day(1-31) month(1-12) weekday(0-6, 0=Sun)`

## Actions

### Add a job

```json
{
  "action": "add",
  "name": "daily-standup",
  "schedule": "0 9 * * 1-5",
  "message": "Time for standup!",
  "tz_offset_hours": -7
}
```

Response:
```json
{"status": "added", "name": "daily-standup", "schedule": "0 9 * * 1-5"}
```

### Remove a job

```json
{"action": "remove", "name": "daily-standup"}
```

Response:
```json
{"status": "removed", "name": "daily-standup"}
```

### List all jobs

```json
{"action": "list"}
```

Response:
```json
{
  "jobs": [
    {"name": "daily-standup", "schedule": "0 9 * * 1-5", "message": "Time for standup!"}
  ]
}
```

### Check for due jobs

Called periodically by the engine to trigger due jobs:

```json
{"action": "check", "now_unix": 1709888400}
```

Response:
```json
{"due": [{"name": "daily-standup", "message": "Time for standup!"}]}
```

## Building

```bash
# Native tests
cargo test

# WASM target
cargo build --target wasm32-unknown-unknown --release
```

## Technical Details

- **Storage key:** `scheduler:jobs` (JSON array in host KV store)
- **Entry point:** `run()` (extern "C", no_mangle)
- **API version:** `host_api_v1`
- **Capabilities:** `storage`
- **Zero external cron dependencies** — pure Rust cron parser
