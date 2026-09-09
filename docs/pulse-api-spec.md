# API Specification — Pulse

Version: 0.1 (Draft)
Dựa trên: `pulse-prd.md`, `pulse-architecture.md`, `pulse-database-schema.md`

Format: REST, JSON. Base path: `/api/v1`

---

## 1. Authentication

Tất cả endpoints (trừ auth endpoints) yêu cầu header:
```
Authorization: Bearer <jwt_token>
```

### 1.1 Email/Password

**POST** `/api/v1/auth/register`
Body: `{ "email": string, "password": string }`
Response: `201 { "user_id": uuid, "email": string }`

**POST** `/api/v1/auth/login`
Body: `{ "email": string, "password": string }`
Response: `200 { "token": string, "user": { id, email, role } }`

### 1.2 OAuth (Google, GitHub)

**GET** `/api/v1/auth/oauth/:provider` (`provider` = `google` | `github`)
→ Redirect tới trang OAuth consent của provider.

**GET** `/api/v1/auth/oauth/:provider/callback`
→ Provider redirect về đây kèm `code`. Backend đổi `code` lấy user info, tạo/liên kết `users` + `auth_identities`, trả về JWT.
Response: redirect về frontend kèm token (hoặc set cookie, tùy quyết định implement).

### 1.3 Session

**GET** `/api/v1/auth/me`
Response: `200 { id, email, role, created_at }`

**POST** `/api/v1/auth/logout`
Response: `204`

---

## 2. Endpoints (API monitoring targets)

**POST** `/api/v1/endpoints`
Tạo endpoint mới để monitor. Kiểm tra giới hạn 50 endpoints/user ở tầng application trước khi insert.
Body:
```json
{
  "name": "Orders API",
  "url": "https://api.example.com/orders",
  "method": "GET",
  "check_interval_seconds": 60,
  "latency_threshold_ms": 2000,
  "error_rate_threshold_percent": 5.0
}
```
Response: `201 { ...endpoint object }`
Errors: `422` nếu threshold/interval vượt giới hạn min/max cho phép.

**GET** `/api/v1/endpoints`
List tất cả endpoints của user hiện tại.
Response: `200 { "endpoints": [...] }`

**GET** `/api/v1/endpoints/:id`
Chi tiết 1 endpoint.

**PATCH** `/api/v1/endpoints/:id`
Cập nhật threshold, interval, is_active, v.v.

**DELETE** `/api/v1/endpoints/:id`
Xóa endpoint (và cascade xóa checks/incidents liên quan, hoặc soft-delete — quyết định lúc implement).

---

## 3. Metrics (Checks)

**GET** `/api/v1/endpoints/:id/checks`
Query params: `from`, `to` (ISO timestamp), `limit` (default 100, max 1000)
Response:
```json
{
  "checks": [
    { "checked_at": "...", "status_code": 200, "latency_ms": 240, "success": true }
  ]
}
```
Dùng để vẽ biểu đồ latency/error rate theo thời gian trên dashboard.

**GET** `/api/v1/endpoints/:id/checks/summary`
Query params: `period` (`24h` | `7d` | `30d`)
Response: `200 { "avg_latency_ms": ..., "success_rate_percent": ..., "total_checks": ... }`
Dùng cho card tổng quan trên dashboard, tránh phải kéo raw data về client để tự tính.

---

## 4. Incidents

**GET** `/api/v1/incidents`
Query params: `endpoint_id` (optional filter), `status` (`open` | `resolved`), `limit`
Response: `200 { "incidents": [...] }`

**GET** `/api/v1/incidents/:id`
Chi tiết 1 incident, bao gồm `ai_possible_cause`, `ai_evidence`, `ai_suggested_steps`.

**PATCH** `/api/v1/incidents/:id/resolve`
Đánh dấu incident đã resolved (set `resolved_at`).
Response: `200 { ...incident object }`

---

## 5. Health Digests

**GET** `/api/v1/endpoints/:id/health-digests`
Query params: `limit`
Response: `200 { "digests": [...] }`

---

## 6. Admin

Tất cả endpoints dưới đây yêu cầu `role = admin`, trả `403` nếu không phải admin.

**GET** `/api/v1/admin/users`
List tất cả users trong hệ thống.
Response: `200 { "users": [{ id, email, role, is_active, created_at }] }`

**PATCH** `/api/v1/admin/users/:id`
Cập nhật `is_active` (disable/enable user) hoặc `role`.
Body: `{ "is_active": false }` hoặc `{ "role": "admin" }`
Response: `200 { ...user object }`

**GET** `/api/v1/admin/stats`
Thống kê tổng quan: tổng số users, tổng số endpoints đang active, tổng incidents trong 24h gần nhất — hữu ích cho admin dashboard.

---

## 7. Error Response Format (chuẩn hóa toàn bộ API)

```json
{
  "error": {
    "code": "VALIDATION_ERROR",
    "message": "check_interval_seconds must be between 10 and 3600"
  }
}
```

Mã lỗi HTTP dùng chuẩn: `400` (bad request), `401` (chưa đăng nhập), `403` (không đủ quyền), `404` (không tồn tại), `422` (validation), `429` (rate limit — nếu áp dụng), `500` (lỗi hệ thống).

---

## 8. Rate Limiting (ghi chú)

Vì MVP giới hạn 10 users, rate limiting chưa phải ưu tiên cao, nhưng nên có ở mức cơ bản cho auth endpoints (`/auth/login`, `/auth/register`) để tránh brute-force — VD giới hạn 5 requests/phút/IP.

---

## 9. Testing Strategy

### 9.1 Cách tổ chức

- **Integration test** là trọng tâm cho API — vì giá trị chính nằm ở hành vi end-to-end (request → DB → response), không phải logic thuần túy đơn lẻ.
- **Unit test** cho phần logic tách biệt được: validate threshold/interval, tính `success_rate_percent`, parse response từ AI Analysis Service.
- Dùng DB riêng cho test (test database, migrate + seed trước mỗi test run, rollback/truncate sau mỗi test) — tránh test đụng vào dữ liệu dev thật.
- CI (GitHub Actions) chạy toàn bộ test suite mỗi lần push/merge — đã note trong Architecture Document.

### 9.2 Test case theo từng nhóm endpoint

**Auth**
- Register: thành công với email/password hợp lệ; thất bại nếu email đã tồn tại; thất bại nếu password quá ngắn.
- Login: thành công với credential đúng; thất bại với sai password; thất bại với email không tồn tại.
- OAuth callback: tạo user mới nếu email chưa tồn tại; link `auth_identities` vào user có sẵn nếu email đã tồn tại (không tạo user trùng).
- `/auth/me`: trả đúng thông tin user khi có token hợp lệ; `401` khi token thiếu/hết hạn/sai.

**Endpoints (CRUD)**
- Tạo endpoint: thành công với data hợp lệ; `422` khi threshold/interval ngoài giới hạn min/max; `422` khi vượt quá 50 endpoints/user.
- List/Get: chỉ trả về endpoints thuộc về user hiện tại (không thấy endpoint của user khác) — **đây là test quan trọng nhất về authorization**.
- Update: cập nhật đúng field; `403`/`404` khi update endpoint không thuộc về mình.
- Delete: xóa thành công; kiểm tra cascade tới `checks`/`incidents` đúng như thiết kế (xóa hoặc soft-delete tùy quyết định implement).

**Checks/Metrics**
- `/checks`: trả đúng dữ liệu trong khoảng `from`/`to`; giới hạn đúng theo `limit`/`max`.
- `/checks/summary`: tính đúng `avg_latency_ms`, `success_rate_percent` với các trường hợp: không có check nào (empty), tất cả đều fail, mix success/fail.

**Incidents**
- Tạo incident tự động khi checker phát hiện vượt threshold (test ở tầng Anomaly Detector, không phải qua API trực tiếp — nhưng verify được qua `GET /incidents`).
- `ai_status` chuyển đúng trạng thái `pending` → `completed`/`failed` sau khi AI Analysis Service trả kết quả.
- Resolve: chỉ owner của endpoint mới resolve được incident của mình.

**Admin**
- Non-admin gọi bất kỳ `/admin/*` endpoint nào → `403`.
- Admin disable user → user đó không login được nữa (test luôn cả tác động phụ, không chỉ field `is_active` đổi giá trị).
- Admin list users → trả đúng toàn bộ users, không bị giới hạn theo `user_id` như các endpoint thường.

**Error format**
- Mọi lỗi trả về đúng format chuẩn ở mục 7 (test snapshot chung, áp dụng cho tất cả endpoint thay vì lặp lại ở từng test case).

### 9.3 Ngoài phạm vi API test (ghi chú, thuộc phần khác của hệ thống)

- Test cho Checker Worker (có thực sự gọi đúng HTTP request, xử lý timeout đúng không) — thuộc integration test riêng cho Worker, không nằm trong API test.
- Test cho Anomaly Detector logic (so sánh threshold đúng chưa) — có thể unit test độc lập vì đây là pure logic.
- Load test cho Scheduler + Worker khi có 50 endpoints check đồng thời — để sau, không phải ưu tiên MVP.

---

## Next Steps
1. Thiết kế **AI prompt/schema** cho AI Analysis Service (input context, output format cụ thể để parse vào `ai_possible_cause`, `ai_evidence`, `ai_suggested_steps`).
2. Bắt đầu implement: setup project skeleton (Rust/Axum backend, Next.js frontend, Docker Compose cho Postgres+Redis local dev), viết test theo mục 9 song song với từng endpoint.
