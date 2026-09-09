# Security Considerations — Pulse

Version: 0.1 (Draft)
Dựa trên: `pulse-prd.md`, `pulse-architecture.md`, `pulse-database-schema.md`, `pulse-api-spec.md`

Mục tiêu tài liệu: liệt kê các rủi ro bảo mật cần xử lý trong quá trình implement, không phải chỉ ở cuối. Mỗi mục có: rủi ro, vì sao liên quan tới Pulse cụ thể, và hướng xử lý.

---

## 1. SSRF (Server-Side Request Forgery) — rủi ro đặc thù của Pulse

**Vì sao quan trọng nhất**: Checker Worker tự động gọi HTTP tới **URL do user nhập tự do**. Đây là điểm khác biệt lớn so với web app thông thường — nếu không kiểm soát, user (hoặc kẻ tấn công dùng tài khoản hợp lệ) có thể lợi dụng Pulse như một proxy để:
- Quét/gọi tới địa chỉ nội bộ (VD `http://169.254.169.254/` — metadata endpoint của cloud provider, hoặc `http://localhost:xxxx`, hoặc dải IP nội bộ `10.x.x.x`, `192.168.x.x`).
- Dùng server của Pulse làm bàn đạp tấn công hệ thống khác.

**Xử lý**:
- Validate URL khi tạo/sửa endpoint: chặn hostname resolve về private IP ranges, localhost, link-local address (`169.254.0.0/16`), và các scheme không phải `http`/`https`.
- Resolve DNS **tại thời điểm gọi request** (không chỉ lúc validate) và kiểm tra lại IP — vì DNS rebinding attack có thể đổi IP sau khi validate ban đầu đã pass.
- Set timeout ngắn cho mọi outbound request (VD 10s) — tránh worker bị treo vô hạn.
- Cân nhắc chạy Checker Worker trong network namespace/container tách biệt, không có quyền truy cập vào internal network của chính hạ tầng Pulse.

---

## 2. Authentication & Session

- **Password**: hash bằng `bcrypt` hoặc `argon2` (không tự chế thuật toán, không dùng MD5/SHA1 trần).
- **JWT**: set thời gian hết hạn hợp lý (VD 24h cho access token); cân nhắc refresh token nếu cần session dài hơn. Ký bằng secret đủ mạnh, lưu ở env variable — không hardcode trong code.
- **OAuth (Google/GitHub)**: luôn validate `state` parameter trong callback để chống CSRF trong OAuth flow. Không tin tưởng email trả về từ provider mà không xác nhận `email_verified = true` (áp dụng cho Google).
- **Brute-force**: rate limit `/auth/login` và `/auth/register` theo IP (đã note trong API Spec mục 8).

---

## 3. Authorization

- **Ownership check ở mọi endpoint liên quan tới `endpoints`/`incidents`**: luôn filter theo `user_id` từ token, không tin tưởng `id` truyền từ client để suy ra quyền truy cập. Đây chính là loại lỗi **IDOR (Insecure Direct Object Reference)** — VD nếu chỉ check `GET /endpoints/:id` mà không verify `endpoint.user_id == current_user.id`, user A có thể xem endpoint của user B chỉ bằng cách đổi UUID trên URL.
- **Admin routes**: middleware riêng kiểm tra `role == admin`, áp dụng nhất quán cho toàn bộ `/admin/*`, không kiểm tra rải rác từng handler.
- Đây cũng chính là lý do mục Testing Strategy (API Spec, mục 9.2) nhấn mạnh test authorization là quan trọng nhất — an ninh và test nên đi cùng nhau, không tách rời.

---

## 4. Input Validation

- Validate toàn bộ input ở API layer trước khi chạm DB: `check_interval_seconds`, `latency_threshold_ms`, `error_rate_threshold_percent` phải nằm trong giới hạn min/max đã định nghĩa (Database Schema mục 2.2).
- URL của endpoint: validate format hợp lệ + áp dụng SSRF check ở mục 1.
- Dùng parameterized query / ORM (SQLx với compile-time query check trong Rust) — tránh SQL injection. Không nối chuỗi SQL thủ công.

---

## 5. Secrets Management

- API keys (LLM provider), JWT secret, DB credentials, OAuth client secret — toàn bộ lưu qua environment variables, **không commit vào Git** (thêm `.env` vào `.gitignore` ngay từ đầu).
- Với CI/CD (GitHub Actions): dùng GitHub Secrets, không hardcode trong workflow file.
- Không log secrets ra console/log file — kiểm tra kỹ log statement trong AI Analysis Service (dễ vô tình log cả request body chứa key).

---

## 6. Data Exposure

- Response API không bao giờ trả `password_hash` ra ngoài (kể cả trong `/auth/me` hay `/admin/users`).
- Error message trả về user không được lộ chi tiết nội bộ (VD stack trace, SQL query lỗi) — chỉ trả message chung chung + log chi tiết ở server-side.
- CORS: chỉ cho phép origin của frontend chính thức, không để `*` ở production.

---

## 7. AI Analysis Service — rủi ro riêng

- **Prompt injection**: dữ liệu đưa vào prompt (response body/headers từ endpoint bị monitor) có thể chứa nội dung do bên thứ ba kiểm soát (chính API mà user đang monitor). Về lý thuyết, nếu API đó trả về nội dung độc hại được thiết kế để "đánh lừa" AI, cần tránh để output của AI Analysis Service có quyền hành động trực tiếp lên hệ thống (VD không cho AI tự ý gọi lại API khác, không dùng function calling có quyền ghi/xóa dữ liệu) — chỉ dùng AI để **sinh text phân tích**, không cấp quyền thực thi hành động.
- Giới hạn kích thước data đưa vào prompt (không nhồi toàn bộ response body nếu quá lớn) — vừa kiểm soát chi phí, vừa giảm bề mặt tấn công.

---

## 7b. Self-hosting tại máy cá nhân (giai đoạn hiện tại)

Vì đang host trên máy cá nhân qua Cloudflare Tunnel (không phải server cloud), có thêm vài điểm cần chú ý:

- **Không mở port trực tiếp ra ngoài** (không port forwarding trên router) — toàn bộ traffic đi qua Cloudflare Tunnel, giúp ẩn IP thật của máy và giảm bề mặt tấn công so với expose trực tiếp.
- **Không expose SSH ra internet** — nếu cần remote access vào máy, dùng Tailscale (VPN riêng tư giữa các thiết bị của mình) thay vì mở port SSH công khai.
- **Backup Postgres định kỳ** (VD script `pg_dump` chạy cron, lưu ra ổ khác hoặc cloud storage free tier) — máy cá nhân dễ gặp sự cố hơn server cloud (mất điện, restart ngoài ý muốn, ổ cứng hỏng), và không có SLA nào cả.
- **Giới hạn quyền của `cloudflared`**: chạy dưới user riêng, không chạy bằng root, chỉ forward đúng port cần thiết (API + frontend), không forward toàn bộ máy.
- Vì đây là giai đoạn demo/học tập, có thể chấp nhận downtime ngoài dự kiến (mất điện, máy tắt) — ghi rõ điều này trong README của project, không cần cam kết uptime như sản phẩm thật.

---

## 8. Dependency & Infrastructure

- Chạy `cargo audit` (Rust) và `npm audit` (Next.js) định kỳ trong CI để phát hiện dependency có lỗ hổng đã biết.
- Docker image: dùng base image tối thiểu, không chạy container với quyền root nếu không cần thiết.
- HTTPS bắt buộc ở production (kể cả giữa các internal service nếu triển khai multi-container).

---

## 9. Checklist tóm tắt để dùng khi implement từng phần

| Khi code phần... | Nhớ kiểm tra... |
|---|---|
| Tạo/sửa endpoint | SSRF validation, threshold/interval range |
| Checker Worker | Timeout, DNS re-check tại thời điểm gọi, không follow redirect tới private IP |
| Bất kỳ handler nào đọc `endpoints`/`incidents` theo `:id` | Ownership check (`user_id` từ token, không phải từ URL) |
| Admin routes | Middleware role check áp dụng nhất quán |
| Auth | Password hashing đúng chuẩn, OAuth `state` validation, rate limit login |
| AI Analysis Service | Không cấp quyền hành động cho AI, giới hạn kích thước input |
| Trước khi merge PR | Không có secret bị commit, không log dữ liệu nhạy cảm |

---

## Next Steps
1. Thiết kế **AI prompt/schema** cho AI Analysis Service (đã note thêm ràng buộc security ở mục 7 trên).
2. Setup project skeleton — áp dụng checklist mục 9 ngay từ những phần đầu tiên (auth, endpoint CRUD) thay vì để dồn security lại cuối cùng.
