# Pulse

AI-powered API/Backend Monitoring SaaS — monitors API endpoints, detects
anomalies against user-set thresholds, and uses an LLM to analyze real
incidents (possible cause, evidence, suggested investigation steps).

Full design docs live in [`docs/`](docs/) — start with
[`docs/pulse-project-context.md`](docs/pulse-project-context.md), which
indexes the rest (PRD, architecture, database schema, API spec, security,
AI design).

## Status

Implement-order steps 1–5 of
[`docs/pulse-project-context.md`](docs/pulse-project-context.md) §6 done:

- **Auth** (email/password): register, login (JWT), `me`, logout; Argon2id,
  per-IP rate limiting, 10-user cap, first-admin seed from `ADMIN_SEED_*`.
  OAuth is deferred until the Cloudflare Tunnel domain exists.
- **Endpoints CRUD** (`/api/v1/endpoints`): ownership-scoped (other users'
  endpoints are a 404), range validation, 50 endpoints/user, and SSRF
  validation of target URLs (private/loopback/link-local/reserved addresses
  rejected after DNS resolution).

- **Scheduler + Checker Worker** (`worker` binary): claims due endpoints
  every second (`FOR UPDATE SKIP LOCKED`, safe with several replicas),
  queues jobs in Redis, runs up to 100 concurrent HTTP checks and writes
  `checks` rows. SSRF is enforced again at request time: DNS answers,
  literal IPs and every redirect hop are checked; 10s timeout; graceful
  shutdown on SIGTERM.

Next step is the Anomaly Detector (step 6).

## Tech stack

| Layer | Tech |
|---|---|
| Frontend | Next.js (App Router) + TypeScript + Tailwind |
| Backend | Rust + Axum (2 binaries, 1 codebase: `api`, `worker`) |
| Database | PostgreSQL + SQLx |
| Cache/Queue | Redis |
| AI | LLM API, OpenAI-compatible (Gemini Flash / Groq free tier) |
| Infra | Docker, Docker Compose, GitHub Actions, Cloudflare Tunnel |

## Repo layout

```
pulse/
├── backend/    # Rust/Axum — src/bin/api.rs (API server), src/bin/worker.rs (Scheduler + Checker)
├── frontend/   # Next.js dashboard
├── docker-compose.yml
├── .github/workflows/ci.yml
└── docs/       # design docs (source of truth — read before changing scope)
```

## Local dev

1. `cp .env.example .env` and fill in values (never commit the real `.env`).
   The API refuses to start without a `JWT_SECRET` of at least 32 characters
   (`openssl rand -hex 32`). If `ADMIN_SEED_EMAIL`/`ADMIN_SEED_PASSWORD` are
   set, the first admin is created on startup when no admin exists yet.
2. `docker compose up --build` — starts Postgres, Redis, `backend-api`
   (`:8080`), `backend-worker`, and `frontend` (`:3000`).

Without Docker:

- **Backend**: `cd backend && cargo run --bin api` (or `--bin worker`).
  Needs `DATABASE_URL`/`REDIS_URL` pointing at a running Postgres/Redis
  (e.g. `docker compose up postgres redis`).
- **Frontend**: `cd frontend && npm run dev`.

### Windows note

Building the backend needs a Rust host toolchain that can actually link.
If you don't have Visual Studio Build Tools (C++ workload) installed, the
default `stable-x86_64-pc-windows-msvc` toolchain will fail at the link
step. Easiest fix — use the GNU toolchain instead (works with MSYS2/MinGW,
which is much smaller than VS Build Tools):

```
rustup toolchain install stable-x86_64-pc-windows-gnu
cd backend
rustup override set stable-x86_64-pc-windows-gnu   # local-only, not committed
```

Then make sure a MinGW-w64 `gcc` (e.g. MSYS2's `mingw64/bin`) is on `PATH`
when running `cargo build`/`run`. This only matters for native Windows
builds — Docker and CI both build on Linux and are unaffected.

## Testing

- Backend: `cargo test` (integration-test-first per
  [`docs/pulse-api-spec.md`](docs/pulse-api-spec.md) §9). Needs a running
  Postgres + Redis (`docker compose up -d postgres redis`) and `DATABASE_URL` set (the
  `.env` value works) — `#[sqlx::test]` creates a throwaway database per test
  and applies the migrations to it, so dev data is never touched.

### Database migrations

Plain SQL files in `backend/migrations/`, embedded into the binary at compile
time (`sqlx::migrate!()`). The `api` binary applies pending ones on startup;
the `worker` never migrates. To add one, create
`backend/migrations/<YYYYMMDDHHMMSS>_<description>.sql` — never edit a
migration that has already been applied anywhere.
- Frontend: TBD when dashboard work starts.
- CI (`.github/workflows/ci.yml`) runs fmt/clippy/test/build for backend
  and lint/build for frontend on every push/PR to `main`.

## Scope

MVP scope, limits, and what's explicitly **not** in scope are pinned down
in [`docs/pulse-project-context.md`](docs/pulse-project-context.md) §3 —
check there before adding anything that looks like a feature.
