# Architecture Document — Pulse

Version: 0.1 (Draft)
Dựa trên: `pulse-prd.md`

---

## 1. Tổng quan kiến trúc

Pulse gồm 5 thành phần chính, hoạt động độc lập nhưng liên kết qua database + queue:

```
                    ┌─────────────────┐
                    │   Scheduler      │  (định kỳ trigger check)
                    └────────┬─────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │   Checker        │  (thực hiện HTTP call tới endpoint)
                    │   Worker(s)      │
                    └────────┬─────────┘
                             │ ghi metrics
                             ▼
                    ┌─────────────────┐
                    │  Metrics Store   │  (PostgreSQL)
                    └────────┬─────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │ Anomaly Detector │  (so metrics vs threshold user set)
                    └────────┬─────────┘
                    vượt threshold?
                       │           │
                      Yes          No (health digest định kỳ)
                       │           │
                       ▼           ▼
              ┌────────────────┐  ┌──────────────────┐
              │ AI Analysis     │  │ Simple Health     │
              │ Service (LLM)   │  │ Digest (no LLM)   │
              └────────┬────────┘  └─────────┬─────────┘
                       │                     │
                       └──────────┬──────────┘
                                  ▼
                       ┌─────────────────────┐
                       │ Notification Service │  (email)
                       └─────────────────────┘

                       ┌─────────────────────┐
                       │  API + Dashboard      │  (Next.js, đọc từ DB)
                       └─────────────────────┘
```

---

## 2. Thành phần chi tiết

### 2.1 Scheduler
- Trigger việc check cho từng endpoint theo interval do user cấu hình.
- Có thể implement đơn giản bằng cron job đọc bảng `endpoints`, tìm những cái "đến hạn check" (dựa trên `last_checked_at + interval`).
- Đẩy job vào queue (Redis) thay vì check trực tiếp — tách biệt scheduling khỏi thực thi.

### 2.2 Checker Worker
- Consume job từ queue, thực hiện HTTP request tới endpoint đã đăng ký.
- Ghi lại: `status_code`, `latency_ms`, `success/fail`, `timestamp`, `response_size` (optional).
- Ghi kết quả vào Metrics Store.
- Có thể chạy nhiều worker song song (horizontal scale) — vì đây là I/O-bound task.

### 2.3 Metrics Store (PostgreSQL)
- Bảng `checks`: mỗi lần check là 1 row (endpoint_id, timestamp, status_code, latency_ms, success).
- Với volume nhỏ (10 users × 50 endpoints, interval phút) — PostgreSQL đủ dùng cho MVP, chưa cần time-series DB chuyên dụng (InfluxDB/TimescaleDB) — có thể ghi chú đây là hướng mở rộng sau.

### 2.4 Anomaly Detector
- Chạy sau mỗi lần ghi metrics mới (hoặc theo batch ngắn).
- So sánh giá trị mới nhất (hoặc trung bình N lần check gần nhất) với threshold user đã set cho endpoint đó.
- Nếu vượt threshold → tạo record trong bảng `incidents`, trigger AI Analysis Service.
- Nếu không vượt và đến kỳ health digest → trigger Simple Health Digest.

### 2.5 AI Analysis Service
- Chỉ được gọi khi có incident thật sự (không gọi cho mỗi lần check — kiểm soát chi phí LLM).
- Input: dữ liệu metrics context (trước/sau incident, threshold nào bị vượt, endpoint nào).
- Output structured (JSON): `possible_cause`, `evidence[]`, `suggested_steps[]`.
- Kết quả lưu vào bảng `incidents` (liên kết với record đã tạo ở bước Anomaly Detector).

### 2.6 Notification Service
- Nhận kết quả từ AI Analysis Service (khi có incident) hoặc Health Digest (định kỳ).
- Gửi email cho user — MVP chỉ cần 1 kênh (email), chưa cần Slack/webhook.

### 2.7 API + Dashboard (Next.js)
- CRUD cho endpoints (thêm/sửa/xóa, set threshold, set interval).
- Hiển thị metrics history theo biểu đồ (latency/error rate theo thời gian).
- Hiển thị danh sách incidents + chi tiết AI analysis từng cái.
- Authentication đơn giản (email/password hoặc magic link) — vì giới hạn 10 users, chưa cần OAuth phức tạp.

---

## 3. Data Flow tóm tắt

1. Scheduler tìm endpoint đến hạn check → đẩy job vào Redis queue.
2. Checker Worker lấy job → gọi HTTP tới endpoint → ghi kết quả vào `checks` table.
3. Anomaly Detector đọc kết quả mới → so với threshold trong `endpoints` table.
4. Nếu vượt threshold → tạo `incident` → gọi AI Analysis Service → lưu kết quả → gọi Notification Service.
5. Nếu đến kỳ health digest (và không có incident) → tạo digest đơn giản → gọi Notification Service.
6. User xem toàn bộ qua Dashboard, đọc trực tiếp từ PostgreSQL qua API.

---

## 4. Vì sao chọn Queue (Redis) thay vì gọi trực tiếp?

- Tách rời "khi nào cần check" (Scheduler) khỏi "ai thực hiện check" (Worker) — cho phép scale worker độc lập.
- Retry dễ dàng nếu 1 lần check bị lỗi do network tạm thời (khác với endpoint thật sự down).
- Là điểm kiến trúc tốt để thể hiện trong CV/portfolio: hiểu về async job processing, không phải chỉ CRUD đơn giản.

---

## 5. Deployment Topology (MVP)

**Giai đoạn hiện tại: self-host tại máy cá nhân** (chưa có ngân sách thuê server).

- **Backend (Rust/Axum)**: API server + Worker chạy dạng container riêng trên máy (dù chung codebase, khác entrypoint) — thể hiện tư duy tách biệt service theo trách nhiệm, dù đang chạy chung 1 máy vật lý.
- **Frontend (Next.js)**: chạy cùng Docker Compose trên máy, hoặc deploy riêng lên Vercel free tier (giảm tải cho máy nhà, chỉ backend + DB chạy local).
- **PostgreSQL + Redis**: Docker container chạy trên máy cá nhân.
- **Expose ra internet qua Cloudflare Tunnel**: dùng `cloudflared` chạy trên máy, kết nối domain đã có sẵn qua Cloudflare — không cần port forwarding, không cần IP tĩnh, HTTPS tự động qua Cloudflare. Máy cá nhân đóng vai trò "server" mà không lộ IP thật ra ngoài.
- **CI/CD**: GitHub Actions — chạy test + build khi merge vào `main`; bước deploy có thể tạm thời là thủ công (`docker compose pull && up -d` trên máy) cho tới khi có ngân sách server thật.

**Hướng nâng cấp sau này** (khi có ngân sách): chuyển Postgres/Redis sang managed service (Supabase, Upstash) hoặc thuê VPS nhỏ, giữ nguyên kiến trúc container hóa nên việc migrate không cần viết lại code — chỉ đổi nơi chạy.

---

## 6. Open Questions cho bước tiếp theo (Database Schema)

- Cấu trúc chính xác của bảng `endpoints`, `checks`, `incidents` — cần thiết kế field, index, quan hệ.
- Retention policy cho bảng `checks` — có xóa dữ liệu cũ sau X ngày để tránh phình DB không? (Không bắt buộc MVP nhưng nên ghi chú.)
- Format chính xác của prompt gửi cho AI Analysis Service (input context nào cần đưa vào để AI phân tích chính xác).

---

## Next Steps
1. Thiết kế **Database Schema** chi tiết (bảng, field, index, relationships).
2. Viết **API Specification** (endpoints của chính Pulse, không phải endpoints user monitor).
