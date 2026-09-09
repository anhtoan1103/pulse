# Product Requirements Document — Pulse

**AI-powered API/Backend Monitoring SaaS**
Version: 0.1 (Draft)

---

## 1. Problem Statement

Đội ngũ backend nhỏ/vừa thường không có đủ nguồn lực để vận hành một stack observability đầy đủ (Datadog, New Relic...) vì chi phí cao và độ phức tạp setup lớn. Khi có incident (latency tăng, error rate tăng), họ mất nhiều thời gian để:

- Phát hiện vấn đề (thường phát hiện qua complaint của user, không phải chủ động)
- Xác định nguyên nhân gốc (root cause) — phải tự đọc logs, dashboard, correlate nhiều nguồn dữ liệu thủ công
- Biết nên bắt đầu điều tra từ đâu

**Pulse** giải quyết vấn đề này bằng cách: monitor API endpoints tự động, phát hiện anomaly, và dùng AI để phân tích + đề xuất hướng điều tra ngay khi có incident — thay vì chỉ hiển thị số liệu thô.

---

## 2. Target Users

- **Primary**: Solo developers / small backend teams (1–10 người) đang tự vận hành API, không có dedicated DevOps/SRE.
- **Secondary**: Indie hackers, side-project founders cần biết API "sống" hay "chết" mà không muốn setup Prometheus/Grafana đầy đủ.

*(Không nhắm tới enterprise ở giai đoạn MVP — enterprise cần multi-tenancy, RBAC, compliance phức tạp hơn nhiều.)*

---

## 3. Goals & Non-Goals (MVP)

### Goals
- Cho phép user đăng ký một hoặc nhiều API endpoints để monitor.
- Tự động ping/check định kỳ, thu thập metrics: availability, latency, status code, error rate.
- Phát hiện anomaly (dựa trên threshold hoặc so sánh với baseline).
- Khi có incident: AI phân tích dữ liệu thu thập được → đưa ra "possible cause" + "suggested investigation steps".
- Dashboard hiển thị lịch sử incidents + metrics theo thời gian.
- Notification cơ bản (email) khi có incident.

### Non-Goals (để sau, không làm ở MVP)
- Multi-tenancy / team collaboration
- RBAC phức tạp (permission matrix, nhiều role, scoped access) — MVP chỉ có 2 role đơn giản: `admin` và `user`
- Billing / subscription
- Slack/webhook integration
- OpenTelemetry / distributed tracing
- Custom alerting rules phức tạp (chỉ threshold đơn giản ở MVP)
- Mobile app

---

## 4. User Stories

1. **Là một developer**, tôi muốn thêm một API endpoint (URL + method) vào hệ thống, để Pulse bắt đầu monitor nó.
2. **Là một developer**, tôi muốn xem dashboard hiển thị latency/error rate/availability theo thời gian của từng endpoint, để biết tình trạng hiện tại.
3. **Là một developer**, khi có incident xảy ra, tôi muốn nhận email thông báo ngay, để không bỏ lỡ downtime.
4. **Là một developer**, khi xem một incident, tôi muốn thấy AI đã phân tích: nguyên nhân khả nghi là gì, bằng chứng nào hỗ trợ, và nên kiểm tra gì trước — để rút ngắn thời gian debug.
5. **Là một developer**, tôi muốn xem lại lịch sử các incidents đã xảy ra với từng endpoint, để nhận diện pattern lặp lại.
6. **Là một developer**, tôi muốn cấu hình threshold (VD: latency > 2s, error rate > 5%) cho từng endpoint, để kiểm soát khi nào coi là "incident".
7. **Là một user**, tôi muốn đăng nhập bằng Google hoặc GitHub thay vì tạo password riêng, để tiện lợi hơn khi test.
8. **Là admin**, tôi muốn xem danh sách tất cả users trong hệ thống, để biết ai đang dùng Pulse.
9. **Là admin**, tôi muốn có khả năng vô hiệu hóa (disable) một user, để kiểm soát ai được phép tiếp tục dùng hệ thống trong giai đoạn giới hạn 10 users.

---

## 5. Success Metrics (cho bản thân dự án, không phải sản phẩm thật)

Vì đây là portfolio project chứ không phải sản phẩm thương mại thật, "success" được đo bằng:

- Hệ thống chạy được end-to-end: từ thêm endpoint → thu thập metrics → phát hiện anomaly → AI phân tích → hiển thị/notify.
- Codebase thể hiện được: clean architecture, testing coverage hợp lý, CI/CD hoạt động, có thể demo live.
- Có thể viết case study rõ ràng (vấn đề → giải pháp → kiến trúc → kết quả) để đưa vào CV/portfolio.

---

## 6. Quyết định MVP scope

- **Check interval**: cho phép user tùy chỉnh (không cố định 1 giá trị chung).
- **AI analysis trigger**: hai chế độ —
  - **Real-time**: chạy ngay + notify khi phát hiện incident (chỉ gọi AI khi thật sự vượt threshold, không gọi cho mọi lần check — tránh burn LLM credits).
  - **Định kỳ (health digest)**: chạy đơn giản theo lịch, không cần AI reasoning sâu — chỉ xác nhận hệ thống vẫn hoạt động bình thường.
- **Anomaly detection**: **threshold tĩnh, do user tự cấu hình theo từng endpoint** (VD: latency > 2000ms, error rate > 5%), nhưng có **giới hạn min/max** cho giá trị được set (tránh threshold vô lý — quá thấp gây báo động giả liên tục, quá cao làm mất tác dụng detect). Dynamic baseline (tự học từ lịch sử) để lại cho v2.
- **Giới hạn MVP**: tối đa 10 users, mỗi hệ thống tổng cộng tối đa 50 API endpoints được monitor.

---

## Next Steps

1. Chốt lại các Open Questions ở trên.
2. Viết **Architecture Document** (system components, data flow, deployment topology).
3. Thiết kế **Database Schema**.
4. Viết **API Specification**.
