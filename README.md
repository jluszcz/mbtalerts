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
| `-s`, `--sync-calendar` | Sync alerts to Google Calendar instead of printing them (requires `GOOGLE_CALENDAR_ID` and `GOOGLE_SERVICE_ACCOUNT_KEY`) |
| `-v` | Enable debug logging |
| `-vv` | Enable trace logging |

By default, responses are cached daily in the OS temp directory and reused on subsequent runs.

## Lambda

A separate `lambda` binary syncs alerts to a Google Calendar and is designed to run on AWS Lambda.

### Environment Variables

| Variable | Description |
|----------|-------------|
| `GOOGLE_CALENDAR_ID` | Target Google Calendar ID |
| `GOOGLE_SERVICE_ACCOUNT_KEY` | Service account key JSON (required) |
