# Project Context — Pulse

File này dùng làm ngữ cảnh cho AI coding agent khi implement Pulse. Gom lại toàn bộ quyết định đã chốt từ 6 tài liệu design, cộng với thông tin môi trường cụ thể.

Tham chiếu đầy đủ: `pulse-prd.md`, `pulse-architecture.md`, `pulse-database-schema.md`, `pulse-api-spec.md`, `pulse-security.md`, `pulse-ai-design.md`.

---

## 1. Thông tin dự án

- **Tên project**: Pulse — AI-powered API/Backend Monitoring SaaS
- **GitHub**: `github.com/anhtoan1103/pulse` (repo chưa tạo — tạo mới, private hoặc public tùy bạn quyết định lúc tạo)
- **Domain**: `toan.uk` — đề xuất subdomain `pulse.toan.uk` cho frontend, `api.pulse.toan.uk` cho backend API (tách subdomain giúp CORS/cấu hình rõ ràng hơn so với dùng chung 1 domain cho cả 2)
- **Hosting**: self-host tại máy cá nhân, expose qua Cloudflare Tunnel (xem `pulse-security.md` mục 7b)

## 2. Tech Stack (đã chốt)

| Layer | Công nghệ |
|---|---|
| Frontend | Next.js + TypeScript |
| Backend | Rust + Axum |
| Database | PostgreSQL + SQLx |
| Cache/Queue | Redis |
| Infra | Docker, Docker Compose, GitHub Actions (CI) |
| AI | LLM API — free tier ưu tiên Google AI Studio (Gemini Flash) hoặc Groq, thiết kế theo chuẩn OpenAI-compatible để dễ đổi provider |
| Tunnel | Cloudflare Tunnel (`cloudflared`) |

## 3. Scope MVP (không làm ngoài phạm vi này trừ khi được yêu cầu rõ)

**Có**: monitor API endpoints (giới hạn 50/user), threshold tĩnh do user tự set (có min/max), real-time AI analysis khi có incident, health digest định kỳ đơn giản (không AI), email notification, auth email/password + OAuth (Google/GitHub), 2 role đơn giản (`admin`/`user`).

**Không có ở MVP** (đừng tự ý thêm): multi-tenancy/team, RBAC phức tạp, billing, Slack/webhook, OpenTelemetry, dynamic baseline anomaly detection, mobile app.

**Giới hạn hệ thống**: tối đa 10 users, tối đa 50 endpoints/user.

## 4. Cấu trúc repo đề xuất

```
pulse/
├── backend/          # Rust/Axum — API server + Worker (2 entrypoint, chung codebase)
│   ├── src/
│   ├── migrations/    # SQLx migrations, theo schema trong pulse-database-schema.md
│   └── Cargo.toml
├── frontend/          # Next.js
│   ├── app/
│   └── package.json
├── docker-compose.yml  # Postgres + Redis + backend + frontend cho local dev
├── .github/
│   └── workflows/     # CI: test + build
├── docs/               # copy 6 file .md design vào đây làm tài liệu tham chiếu trong repo
└── README.md
```

## 5. Nguyên tắc khi AI code phần này

- Luôn đối chiếu với `pulse-security.md` mục 9 (checklist) khi code phần liên quan — đặc biệt SSRF validation (Checker Worker) và ownership check (mọi endpoint CRUD).
- Viết test song song theo `pulse-api-spec.md` mục 9, không để dồn lại cuối.
- Không hardcode secret — dùng `.env` (xem file `.env.example` đi kèm), không commit `.env` thật.
- Nếu cần quyết định kỹ thuật nhỏ chưa có trong 6 tài liệu design (VD chọn thư viện cụ thể, đặt tên field phụ) — chọn theo convention phổ biến của Rust/Axum và Next.js, không cần hỏi lại cho từng chi tiết nhỏ, nhưng ghi chú lại trong code comment nếu là quyết định đáng kể.
- Nếu phát sinh yêu cầu nằm ngoài scope MVP (mục 3) trong lúc code — dừng lại, không tự ý mở rộng, hỏi lại trước.

## 6. Thứ tự implement đề xuất

1. Setup repo + Docker Compose (Postgres, Redis) + skeleton Axum project + skeleton Next.js project.
2. Database migrations theo `pulse-database-schema.md`.
3. Auth: email/password trước, OAuth sau (OAuth cần domain thật để test callback URL — có thể làm sau khi có Cloudflare Tunnel chạy).
4. Endpoints CRUD API + ownership check + test.
5. Checker Worker + Scheduler + SSRF validation.
6. Anomaly Detector + data preprocessing (`prepare_ai_context()`).
7. AI Analysis Service (theo `pulse-ai-design.md`).
8. Notification Service (email).
9. Health Digest (định kỳ, đơn giản).
10. Frontend dashboard, kết nối API.
11. Admin panel.
12. Setup Cloudflare Tunnel, trỏ domain, test end-to-end trên domain thật.
