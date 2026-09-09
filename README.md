# Pulse

AI-powered API/Backend Monitoring SaaS — monitors API endpoints, detects
anomalies against user-set thresholds, and uses an LLM to analyze real
incidents (possible cause, evidence, suggested investigation steps).

Full design docs live in [`docs/`](docs/) — start with
[`docs/pulse-project-context.md`](docs/pulse-project-context.md), which
indexes the rest (PRD, architecture, database schema, API spec, security,
AI design).

## Status

Skeleton stage (implement-order step 1 of
[`docs/pulse-project-context.md`](docs/pulse-project-context.md) §6):
repo layout, Docker Compose (Postgres + Redis), a bare Axum API skeleton
(`/health`), a bare worker loop skeleton, and a default Next.js + TypeScript
+ Tailwind app — nothing functional yet. Next step is the database
migrations (step 2).

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
  [`docs/pulse-api-spec.md`](docs/pulse-api-spec.md) §9).
- Frontend: TBD when dashboard work starts.
- CI (`.github/workflows/ci.yml`) runs fmt/clippy/test/build for backend
  and lint/build for frontend on every push/PR to `main`.

## Scope

MVP scope, limits, and what's explicitly **not** in scope are pinned down
in [`docs/pulse-project-context.md`](docs/pulse-project-context.md) §3 —
check there before adding anything that looks like a feature.
