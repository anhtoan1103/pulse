# Pulse — Frontend

Next.js (App Router) dashboard for [Pulse](../README.md). Talks to the
`backend-api` service via `NEXT_PUBLIC_API_URL` — see `docs/pulse-api-spec.md`
for the API it consumes.

## Local dev

```bash
cp .env.local.example .env.local   # sets NEXT_PUBLIC_API_URL for local dev
npm install
npm run dev
```

Needs the backend API running (`docker compose up postgres redis backend-api`
from the repo root, or `cargo run --bin api`). Without a real `AI_API_KEY`/
`SMTP_HOST` configured on the backend, incidents show "AI analysis is not
enabled" and health digests/incident emails just don't get sent — the
dashboard itself still works fully against real check/incident data.

## Structure

- `src/lib/api.ts` — typed client for every backend endpoint the dashboard
  uses; throws `ApiError` on non-2xx (standard `{error:{code,message}}` body).
- `src/lib/auth.tsx` — client-side session (JWT in `localStorage`),
  `<RequireAuth>` guards protected pages.
- `src/lib/useAsync.ts` — small fetch-in-effect hook (loading/error/data +
  `reload()`); no data-fetching library dependency.
- `src/components/` — shared UI (`ui.tsx` primitives, `EndpointForm`,
  `LatencyChart` — a dependency-free inline SVG chart, `IncidentStatusBadge`).
- `src/app/` — routes: `/login`, `/register`, `/` (endpoints list),
  `/endpoints/new`, `/endpoints/[id]` (detail: chart, incidents, health
  digests, edit/pause/delete), `/incidents`, `/incidents/[id]` (AI analysis,
  resolve).

## Notes

- `NEXT_PUBLIC_API_URL` is inlined into the client bundle at **build** time,
  not read at container runtime — `docker-compose.yml` passes it as a
  `build.args` entry, not `environment:` (see `Dockerfile`'s comment). If you
  change the backend's URL for a deployment, rebuild the frontend image.
- No admin panel yet (project-context §6 step 11, not built).
- OAuth login isn't implemented (deferred with the backend, until a real
  domain exists for callback URLs).

## Learn more

[Next.js docs](https://nextjs.org/docs) · [Learn Next.js](https://nextjs.org/learn)
