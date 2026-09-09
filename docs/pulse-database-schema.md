# Database Schema — Pulse

Version: 0.1 (Draft)
Dựa trên: `pulse-prd.md`, `pulse-architecture.md`
Database: PostgreSQL

---

## 1. Tổng quan các bảng

- `users` — tài khoản (giới hạn 10 users ở MVP)
- `endpoints` — API endpoints được đăng ký để monitor (giới hạn 50/hệ thống)
- `checks` — kết quả mỗi lần check định kỳ (time-series data)
- `incidents` — các lần phát hiện bất thường + kết quả AI phân tích
- `health_digests` — báo cáo định kỳ đơn giản (không qua AI)

---

## 2. Chi tiết từng bảng

### 2.1 `users`

| Field | Type | Ghi chú |
|---|---|---|
| id | UUID (PK) | |
| email | TEXT UNIQUE | dùng cho login + notification |
| password_hash | TEXT | NULL nếu user chỉ đăng nhập qua OAuth (không set password) |
| role | TEXT | `admin` hoặc `user`, default `user` |
| is_active | BOOLEAN | admin có thể disable user, default `true` |
| created_at | TIMESTAMPTZ | |

### 2.1b `auth_identities`

Tách riêng để 1 user có thể đăng nhập bằng nhiều phương thức (email/password, Google, GitHub) cùng lúc.

| Field | Type | Ghi chú |
|---|---|---|
| id | UUID (PK) | |
| user_id | UUID (FK → users.id) | |
| provider | TEXT | `google`, `github`, `email` |
| provider_user_id | TEXT | ID phía provider trả về (VD Google `sub`, GitHub user id) — NULL nếu provider = `email` |
| created_at | TIMESTAMPTZ | |

Index: `UNIQUE (provider, provider_user_id)` — tránh 2 user khác nhau map vào cùng 1 tài khoản Google/GitHub.

**Luồng đăng nhập OAuth**: khi user login qua Google/GitHub lần đầu → tạo record trong `users` (nếu email chưa tồn tại) + record tương ứng trong `auth_identities`. Nếu email đã tồn tại (VD trước đó đăng ký bằng password) → chỉ thêm record `auth_identities` mới, liên kết vào `user_id` đã có — cho phép "link" nhiều phương thức vào cùng 1 tài khoản.

**Admin đầu tiên**: MVP có thể seed 1 admin account trực tiếp qua migration/script khi deploy, thay vì xây flow "promote user thành admin" phức tạp — vì chỉ có 10 users, không cần UI phức tạp cho việc này ở bản đầu.

---

### 2.2 `endpoints`

| Field | Type | Ghi chú |
|---|---|---|
| id | UUID (PK) | |
| user_id | UUID (FK → users.id) | |
| name | TEXT | tên hiển thị, VD "Orders API" |
| url | TEXT | endpoint đầy đủ |
| method | TEXT | GET/POST/... |
| check_interval_seconds | INTEGER | user tự cấu hình |
| latency_threshold_ms | INTEGER | ngưỡng cảnh báo latency |
| error_rate_threshold_percent | NUMERIC(5,2) | ngưỡng cảnh báo error rate |
| is_active | BOOLEAN | cho phép tạm dừng monitor mà không xóa |
| last_checked_at | TIMESTAMPTZ | Scheduler dùng field này để biết endpoint nào "đến hạn" |
| created_at | TIMESTAMPTZ | |

**Ràng buộc áp dụng ở tầng application** (không nhất thiết ở DB):
- `check_interval_seconds`: giới hạn min/max (VD: 10s – 3600s) để tránh spam hoặc quá thưa.
- `latency_threshold_ms`, `error_rate_threshold_percent`: giới hạn min/max hợp lý.
- Tổng số endpoints active của 1 user ≤ 50 (kiểm tra ở application layer khi tạo mới).

Index: `(user_id)`, `(is_active, last_checked_at)` — phục vụ Scheduler quét nhanh endpoint đến hạn.

---

### 2.3 `checks`

| Field | Type | Ghi chú |
|---|---|---|
| id | UUID (PK) | hoặc BIGSERIAL nếu muốn nhẹ hơn UUID cho bảng volume lớn |
| endpoint_id | UUID (FK → endpoints.id) | |
| checked_at | TIMESTAMPTZ | |
| status_code | INTEGER | NULL nếu request timeout/fail hoàn toàn |
| latency_ms | INTEGER | NULL nếu không nhận được response |
| success | BOOLEAN | derived: status_code trong range 2xx và không timeout |
| error_message | TEXT | NULL nếu thành công, ghi lại lỗi nếu có (timeout, DNS fail...) |

Index: `(endpoint_id, checked_at DESC)` — bắt buộc, vì hầu hết query đều là "lấy N lần check gần nhất của endpoint X" hoặc "lấy checks trong khoảng thời gian Y".

**Retention policy (ghi chú, chưa bắt buộc code ở MVP):** bảng này tăng nhanh nhất (mỗi endpoint check định kỳ liên tục). Nên có job dọn dẹp dữ liệu cũ hơn N ngày (VD 30 ngày) để tránh phình DB — có thể để làm sau MVP, nhưng nên thiết kế sẵn field `checked_at` có index tốt để việc xóa theo range dễ dàng.

---

### 2.4 `incidents`

| Field | Type | Ghi chú |
|---|---|---|
| id | UUID (PK) | |
| endpoint_id | UUID (FK → endpoints.id) | |
| triggered_at | TIMESTAMPTZ | thời điểm phát hiện vượt threshold |
| trigger_reason | TEXT | VD: "latency_threshold_exceeded", "error_rate_threshold_exceeded" |
| metric_before | JSONB | snapshot giá trị trước đó (baseline gần nhất) |
| metric_after | JSONB | snapshot giá trị lúc phát hiện |
| ai_possible_cause | TEXT | kết quả AI phân tích |
| ai_evidence | JSONB | mảng evidence, VD ["DB query time +640%", ...] |
| ai_suggested_steps | JSONB | mảng suggested investigation steps |
| ai_status | TEXT | "pending" / "completed" / "failed" — vì gọi AI là async, cần track trạng thái |
| resolved_at | TIMESTAMPTZ | NULL nếu chưa resolve; user có thể tự đánh dấu resolved |
| created_at | TIMESTAMPTZ | |

Index: `(endpoint_id, triggered_at DESC)`.

**Lý do dùng JSONB cho evidence/suggested_steps**: đây là dữ liệu có cấu trúc động (số lượng item thay đổi tùy tình huống), JSONB linh hoạt hơn là tạo bảng con — hợp lý cho MVP. Có thể tách bảng riêng sau nếu cần query sâu vào từng evidence.

---

### 2.5 `health_digests`

| Field | Type | Ghi chú |
|---|---|---|
| id | UUID (PK) | |
| endpoint_id | UUID (FK → endpoints.id) | |
| period_start | TIMESTAMPTZ | |
| period_end | TIMESTAMPTZ | |
| total_checks | INTEGER | |
| success_count | INTEGER | |
| avg_latency_ms | INTEGER | |
| status | TEXT | "healthy" / "degraded" — tính đơn giản, không qua AI |
| created_at | TIMESTAMPTZ | |

Đây là bảng đơn giản, tính toán bằng aggregate query trên `checks` — không cần AI, đúng theo quyết định "định kỳ thì chạy đơn giản thôi".

---

## 3. Quan hệ tổng thể

```
users (1) ──── (N) auth_identities

users (1) ──── (N) endpoints (1) ──── (N) checks
                        │
                        ├──── (N) incidents
                        │
                        └──── (N) health_digests
```

---

## 4. Ghi chú kỹ thuật khác

- Dùng **UUID** cho các bảng chính (`users`, `endpoints`, `incidents`) để tránh lộ thông tin số lượng record qua ID tuần tự, và thuận tiện nếu sau này cần merge/replicate dữ liệu.
- Bảng `checks` có thể cân nhắc dùng `BIGSERIAL` thay vì UUID nếu muốn tối ưu write throughput — quyết định này có thể để lúc implement, không ảnh hưởng thiết kế tổng thể.
- Tất cả timestamp dùng `TIMESTAMPTZ` (có timezone) — tránh lỗi thường gặp khi hệ thống có user ở nhiều múi giờ khác nhau.

---

## Next Steps
1. Viết **API Specification** — các endpoints của chính Pulse (không phải endpoints user monitor): auth, CRUD endpoints, lấy metrics, lấy incidents...
2. Thiết kế **AI prompt/schema** cho AI Analysis Service — input context nào cần đưa vào để có `ai_possible_cause`, `ai_evidence`, `ai_suggested_steps` chính xác.
