# mbtalerts

[![Build and Deploy](https://github.com/jluszcz/mbtalerts/actions/workflows/build-and-deploy.yml/badge.svg)](https://github.com/jluszcz/mbtalerts/actions/workflows/build-and-deploy.yml)

Fetches active MBTA subway alerts (Red, Orange, and Green Lines) from the [MBTA v3 API](https://api-v3.mbta.com) and displays them in the terminal.

## Usage

```bash
cargo run
```

Alerts are printed to stdout in the format:

```
----------------------------------------
EFFECT  Line  Start  End
Header text
```

### Options

| Flag | Description |
|------|-------------|
| `-n`, `--no-cache` | Query the MBTA API directly instead of using today's cached response |
| `-s`, `--sync-calendar` | Sync alerts to Google Calendar instead of printing them (requires `GOOGLE_SERVICE_ACCOUNT_KEY` and either `GOOGLE_CALENDAR_ID` or `GOOGLE_CALENDAR_IDS`) |
| `-v` | Enable debug logging |
| `-vv` | Enable trace logging |

By default, responses are cached daily in the OS temp directory and reused on subsequent runs.

## Calendars
- [Red Line](https://calendar.google.com/calendar/embed?src=03be1370866d53605030267cef3ac085d61a22792b521cc1e9619baa35c99ce4%40group.calendar.google.com&ctz=America%2FNew_York)
- [Orange Line](https://calendar.google.com/calendar/embed?src=f22bb6d2fb13f0ef95c84e859433bc4e9f3aac9baf2401010ed6cc54a22e78e6%40group.calendar.google.com&ctz=America%2FNew_York)
- [Blue Line](https://calendar.google.com/calendar/embed?src=efe9dbd186a99d4252233a073eb835d6ef88a55f42846c9fea7ca23d67569803%40group.calendar.google.com&ctz=America%2FNew_York)
- [Green Line](https://calendar.google.com/calendar/embed?src=87e2d9fa3c19a485ff42fa7d029a74dc67999e483763e5392fd4deb7cb30cb5a%40group.calendar.google.com&ctz=America%2FNew_York)

## Lambda

A separate `lambda` binary syncs alerts to a Google Calendar and is designed to run on AWS Lambda.

### Environment Variables

| Variable | Description |
|----------|-------------|
| `GOOGLE_SERVICE_ACCOUNT_KEY` | Service account key JSON (required) |
| `GOOGLE_CALENDAR_ID` | Single target calendar ID. Used when `GOOGLE_CALENDAR_IDS` is not set |
| `GOOGLE_CALENDAR_IDS` | JSON object mapping line names to calendar IDs. When set, takes precedence over `GOOGLE_CALENDAR_ID` |

When using `GOOGLE_CALENDAR_IDS`, provide a JSON object with keys `Red`, `Orange`, `Blue`, `Green`, and `default`. The `default` calendar is used for alerts with no route or an unrecognized route, and is required. Alerts affecting multiple lines are synced to each matching calendar. Calendar IDs without an `@group.calendar.google.com` suffix have it appended automatically.

```json
{
  "Red":     "<calendar-id>",
  "Orange":  "<calendar-id>",
  "Blue":    "<calendar-id>",
  "Green":   "<calendar-id>",
  "default": "<calendar-id>"
}
```
