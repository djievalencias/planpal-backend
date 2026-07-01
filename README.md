# PlanPal Backend

A production-grade **Rust** backend for **PlanPal** — a meeting and calendar scheduling
application. It exposes a REST API, processes background work through NATS workers, and
integrates with Google Calendar, AWS Bedrock (Claude Haiku for chat-based scheduling),
AWS SES / SMTP for email, and Firebase Cloud Messaging for push notifications.

> **New here?** Read [`CLAUDE.md`](CLAUDE.md) for a command-first developer cheat sheet,
> and [`CODE_REVIEW.md`](CODE_REVIEW.md) for the current code-health assessment and known issues.

## Tech Stack

| Concern | Technology |
| --- | --- |
| Language / Edition | Rust 2021 |
| HTTP framework | `actix-web` 4 (+ `actix-cors`) |
| Database | PostgreSQL via `sqlx` 0.8 (compile-time-checked queries, SQL migrations) |
| Message queue | NATS (`async-nats` 0.33) |
| Auth | JWT (`jsonwebtoken` 9) + Argon2id (`argon2` 0.5) |
| AI | AWS Bedrock (`aws-sdk-bedrockruntime`), with Anthropic / DeepSeek adapters |
| Email | SMTP (`lettre`) and AWS SES v2 (`aws-sdk-sesv2`) |
| Push | Firebase Cloud Messaging (via `reqwest`) |
| Calendar | Google Calendar API + iCal (`icalendar`) |
| Secrets | Pluggable: AWS Secrets Manager, HashiCorp Vault, or env vars |
| Observability | Prometheus metrics, OpenTelemetry → Grafana Tempo, Pyroscope profiling, `loggix` logging |

## Architecture

The service ships as **five binaries** built from one workspace:

```
                       ┌─────────────────────┐
   HTTP clients  ─────▶│  planpal-server     │  REST API under /api/v1
                       │  (actix-web)        │
                       └──────────┬──────────┘
                                  │ publishes jobs
                                  ▼
                            ┌───────────┐
                            │   NATS    │
                            └─────┬─────┘
              ┌───────────────────┼───────────────────┐
              ▼                   ▼                   ▼
   planpal-schedule-worker  planpal-notification-  planpal-ai-worker
   (sync_calendar,          worker                 (process_ai_chat)
    schedule_meeting)       (email / push)
```

`seed_admin` is a one-shot utility that creates the initial admin user.

The codebase follows a layered design:

- `adapter/rest` — HTTP handlers (one module per resource), `adapter/slack` — Slack webhook
- `repository` — data access (parameterized `sqlx` queries), `model` — domain entities
- `queue/jobs` — background job definitions and handlers
- `auth`, `ai`, `notification`, `provider`, `scheduler`, `secrets`, `telemetry` — supporting subsystems
- `config.rs`, `error.rs`, `logging.rs`, `worker.rs` — cross-cutting infrastructure

## Prerequisites

- Rust toolchain (stable, 2021 edition) — install via [rustup](https://rustup.rs)
- PostgreSQL (a reachable instance for the connection URL)
- A NATS server (for the workers and job publishing)
- `sqlx-cli` for migrations: `cargo install sqlx-cli --no-default-features --features rustls,postgres`

## Setup

```bash
# 1. Clone, then create your local env file from the template
cp .env.example .env        # then edit values (DB URL, JWT secret, etc.)

# 2. Run database migrations
sqlx migrate run            # applies everything in migrations/

# 3. Build
cargo build                 # debug
cargo build --release --features profiling   # release (with continuous profiling)
```

## Configuration

Configuration is layered: built-in defaults → environment variables → secret manager.

- **Non-secret settings** use the `APP__` prefix with `__` as the nesting separator,
  e.g. `APP__SERVER__PORT=8080`, `APP__DATABASE__MAX_CONNECTIONS=20`,
  `APP__SERVER__METRICS_PORT=9090`, `APP__EMAIL__PROVIDER=smtp`.
- **Secrets** (`database.url`, `jwt.secret`, Google client id/secret, SMTP credentials,
  FCM service account JSON, Slack signing secret / bot token, etc.) are loaded by a
  secret manager selected at **compile time**:
  - `SECRET_SOURCE` — `env` (default), `aws`, or `vault`
  - `SECRET_PATH` — secret path/prefix (default `planpal/production`)

See [`.env.example`](.env.example) for the full list of variables. **Never commit a real
`.env`** — `.env` files are git-ignored.

## Running

```bash
cargo run --bin planpal-server                # REST API
cargo run --bin planpal-schedule-worker       # calendar sync + meeting scheduling
cargo run --bin planpal-notification-worker   # email + push delivery
cargo run --bin planpal-ai-worker             # AI chat scheduling
cargo run --bin seed_admin                    # create initial admin user
```

The HTTP server listens on `APP__SERVER__HOST:APP__SERVER__PORT`. Prometheus metrics are
served on a **separate** port (`APP__SERVER__METRICS_PORT`, default `9090`) so it can be
firewalled off from the public internet; set it to `0` to disable.

## API Surface

All REST routes are mounted under `/api/v1`:

| Group | Purpose |
| --- | --- |
| `health` | Liveness / readiness checks |
| `auth` | Register, login, refresh, Google OAuth, admin login |
| `users` | Profile (`/me`), availability checks |
| `calendars` | Connect / list calendar providers |
| `meetings` | Create, check availability, confirm, RSVP |
| `notifications` | List / mark-read notifications |
| `ai` | Chat-based scheduling (`POST /ai/chat`) |
| `admin`, `admin_users`, `admin_holidays` | Admin-only management (guarded by `AdminUser`) |

A Slack webhook scope is mounted at the root (outside `/api/v1`).

## Observability

- **Metrics** — Prometheus exposition at `:<metrics_port>/metrics`
- **Tracing** — OpenTelemetry OTLP export to Grafana Tempo (configure `APP__OTLP__*`)
- **Profiling** — Pyroscope continuous CPU profiling (release builds with `--features profiling`)
- **Logging** — structured logging via `loggix`, with a runtime-adjustable log level

## Testing

```bash
cargo test          # runs the inline unit tests (~166 across auth, scheduler, config, telemetry)
cargo clippy        # lints
cargo fmt           # format
```

Unit tests live inline (`#[cfg(test)]` modules). There is currently **no `tests/`
integration directory** — see [`CODE_REVIEW.md`](CODE_REVIEW.md) for coverage gaps.

## Deployment

Artifact packaging/versioning is handled by [`scripts/artifact.sh`](scripts/artifact.sh)
(S3 + Bitbucket Downloads), consistent with a Bitbucket Pipelines workflow.

## Documentation

- [`CLAUDE.md`](CLAUDE.md) — developer/AI cheat sheet (commands, layout, conventions, gotchas)
- [`CODE_REVIEW.md`](CODE_REVIEW.md) — code-health scorecard and prioritized findings
- [`docs/security-review.md`](docs/security-review.md) — detailed security findings
- [`docs/code-quality-review.md`](docs/code-quality-review.md) — detailed quality findings

## Architecture context (initial findings)
- Verdict: **modular monolith** — one Rust/Cargo workspace producing 4 process roles (HTTP `server` :8088 + `ai_worker` / `schedule_worker` / `notification_worker`) plus a seed tool, over **one shared PostgreSQL**. NOT microservices (shared DB); workers communicate **async via NATS**, not synchronous service calls.
- Hexagonal layout: `adapter/`, `provider/`, `repository/`, `queue/`, `scheduler/`. Pluggable AI provider (Bedrock / Anthropic / DeepSeek) and pluggable secret source (AWS Secrets Manager / Vault / env, `SECRET_SOURCE`).
- This phase deploys on **EC2 (no containerization)** — run each binary as a systemd service; ASG for prd. NATS, PostgreSQL (RDS), Redis (ElastiCache) are private; only the ALB fronting `server` is public.
- Known gaps: no IaC / no CI pipeline in-repo; CORS `allow_any_origin` in `src/bin/server.rs`; Google Calendar watch-channel renewal not implemented (real-time sync stops after ~7 days).
