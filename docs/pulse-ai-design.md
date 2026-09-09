# AI Prompt & Schema Design — Pulse

Version: 0.1 (Draft)
Dựa trên: `pulse-prd.md`, `pulse-architecture.md`, `pulse-database-schema.md`, `pulse-api-spec.md`, `pulse-security.md`

Mục tiêu: định nghĩa chính xác **input context** đưa vào AI Analysis Service, **output schema** cần AI trả về, và cách xử lý khi AI lỗi/trả sai format.

---

## 1. Khi nào AI Analysis Service được gọi

Chỉ gọi khi Anomaly Detector xác nhận **vượt threshold** (đã chốt ở PRD mục 6) — không gọi cho mỗi lần check thường. Input được chuẩn bị từ dữ liệu đã có sẵn trong DB tại thời điểm trigger, không cần gọi thêm request nào khác.

---

## 2. Input Context — dữ liệu đưa vào prompt

Lấy từ các bảng đã thiết kế (`endpoints`, `checks`, `incidents`):

```json
{
  "endpoint": {
    "name": "Orders API",
    "url": "https://api.example.com/orders",
    "method": "GET"
  },
  "trigger_reason": "latency_threshold_exceeded",
  "threshold_configured": {
    "latency_threshold_ms": 2000,
    "error_rate_threshold_percent": 5.0
  },
  "metric_before": {
    "period": "avg 15 phút trước đó",
    "avg_latency_ms": 320,
    "error_rate_percent": 0.5
  },
  "metric_after": {
    "period": "5 phút gần nhất",
    "avg_latency_ms": 2800,
    "error_rate_percent": 18.7
  },
  "recent_status_codes": ["200", "200", "500", "500", "200", "504"],
  "recent_error_messages": ["timeout after 10s", "connection reset"]
}
```

**Giới hạn kích thước** (theo Security doc mục 7): chỉ lấy tối đa N bản ghi `checks` gần nhất (VD 20 record) để tính `recent_status_codes`/`recent_error_messages` — không nhồi toàn bộ lịch sử, vừa kiểm soát token cost vừa giảm rủi ro injection nếu `error_message` chứa nội dung lạ.

**Không đưa vào prompt**: response body thật của API bị monitor (vì đây là dữ liệu của bên thứ ba, có thể chứa thông tin nhạy cảm của customer user đang monitor, và cũng là vector prompt injection theo Security doc mục 7) — chỉ dùng status code, latency, error message ở tầng network, không đọc nội dung response.

---

## 2b. Data Preprocessing — làm gọn dữ liệu trước khi đưa vào prompt

Dữ liệu thô từ DB chưa sẵn sàng để đưa thẳng vào prompt — cần một bước xử lý trung gian, vì 3 lý do: kiểm soát token cost, tránh nhiễu khiến AI phân tích sai hướng, và giảm bề mặt tấn công (prompt injection).

**Aggregate thay vì raw dump**:
- `recent_status_codes`: thay vì liệt kê từng record, nên gộp thành tần suất — VD `{"200": 12, "500": 6, "504": 2}` — vừa gọn hơn, vừa là dạng dữ liệu AI dễ suy luận hơn danh sách rời rạc.
- Tương tự `recent_error_messages`: dedupe các message giống nhau, chỉ giữ lại các loại lỗi khác nhau kèm số lần xuất hiện — VD `"timeout after 10s (x4)"` thay vì lặp lại 4 dòng giống hệt nhau.

**Truncate dữ liệu dài bất thường**:
- `error_message` từ thực tế đôi khi rất dài (stack trace, HTML error page bị log nhầm vào message...) — cắt về độ dài tối đa cố định (VD 200 ký tự), tránh 1 record bất thường chiếm hết token budget của cả request.

**Strip/sanitize nội dung nghi vấn trước khi đưa vào prompt**:
- Loại bỏ pattern trông giống secret/token bị lộ trong error message (VD chuỗi dài toàn chữ+số liền nhau, pattern giống API key/JWT) trước khi gửi cho LLM provider bên thứ ba — vì error message đôi khi vô tình chứa thông tin nhạy cảm (VD lỗi kết nối DB in luôn connection string).
- Loại bỏ ký tự/pattern có thể là chỉ thị injection (VD chuỗi kiểu "ignore previous instructions", markdown code block giả) khỏi các field lấy từ nguồn không tin cậy (`error_message` do chính API bị monitor trả về) — dù đã không đưa response body vào, error message vẫn có thể bị bên thứ ba control gián tiếp (VD cố tình trả lỗi có nội dung độc hại để đánh lừa AI).

**Chuẩn hóa format nhất quán**:
- Timestamp, số liệu (latency, percent) format thống nhất trước khi đưa vào prompt — tránh AI phải tự suy luận đơn vị/format, giảm rủi ro hiểu sai dữ liệu.

**Vị trí đặt bước này trong pipeline**: preprocessing chạy ngay sau khi Anomaly Detector trigger, trước khi build prompt — nên tách thành 1 hàm/module riêng (VD `prepare_ai_context()`), dễ unit test độc lập (đưa raw data giả định vào, kiểm tra output đã gọn/sanitize đúng chưa) mà không cần gọi LLM thật.

---

## 3. Output Schema — bắt buộc structured output

Yêu cầu AI trả về đúng JSON schema sau (dùng tool calling / structured output mode của LLM provider, không parse free-text bằng regex — tránh lỗi vặt khi AI đổi cách diễn đạt):

```json
{
  "possible_cause": "string, 1-2 câu, mô tả nguyên nhân khả nghi nhất",
  "confidence": "high | medium | low",
  "evidence": [
    "string ngắn, 1 dòng, một bằng chứng cụ thể hỗ trợ possible_cause"
  ],
  "suggested_steps": [
    "string ngắn, 1 hành động cụ thể user nên kiểm tra"
  ]
}
```

Ràng buộc khi thiết kế prompt:
- `evidence`: tối đa 5 item, mỗi item phải bắt nguồn trực tiếp từ dữ liệu trong input context (không được suy diễn ngoài dữ liệu có) — tránh AI "bịa" số liệu không có thật.
- `suggested_steps`: tối đa 5 item, ưu tiên hành động cụ thể ("Kiểm tra connection pool tới DB") hơn lời khuyên chung chung ("Kiểm tra hệ thống kỹ hơn").
- `confidence`: bắt buộc AI tự đánh giá — vì với ít dữ liệu (VD chỉ mới xảy ra 1 lần, chưa có pattern rõ), AI nên thừa nhận "low confidence" thay vì đoán chắc chắn.

---

## 4. System Prompt (khung nội dung, không phải final copy)

Nội dung chính cần có trong system prprompt:
- Vai trò: "Bạn là hệ thống phân tích incident cho một API monitoring platform."
- Ràng buộc: chỉ dùng dữ liệu được cung cấp trong context, không suy diễn thêm thông tin không có; trả lời ngắn gọn, thực tế, hướng tới hành động; không đưa ra lời khuyên chung chung không gắn với dữ liệu cụ thể.
- Nhắc rõ: đây là dữ liệu network-level (status code, latency), **không phải nội dung response thật** của API — tránh AI tự "tưởng tượng" thêm ngữ cảnh business không có căn cứ.

---

## 5. Xử lý lỗi / edge case

| Tình huống | Xử lý |
|---|---|
| LLM provider timeout/lỗi | `ai_status = "failed"`, vẫn hiển thị incident cho user (kèm raw metrics) nhưng không có AI analysis — không để lỗi AI chặn việc notify user |
| LLM trả JSON không đúng schema | Retry 1 lần với instruction nhắc rõ format; nếu vẫn fail → `ai_status = "failed"`, log lại để debug prompt sau |
| Free-tier rate limit bị chặn (theo `topics/ai-tooling` — dùng Gemini/Groq free tier) | Có fallback provider thứ 2 trong config, hoặc queue lại request để retry sau vài phút thay vì fail ngay |
| `evidence` rỗng (AI không tìm được bằng chứng rõ ràng) | Vẫn chấp nhận, hiển thị `confidence: "low"` — không ép AI phải luôn có câu trả lời chắc chắn |

---

## 6. Vì sao dùng structured output thay vì free-text + parse

- Tránh lỗi parse khi AI đổi cách hành văn (không có ràng buộc cứng như regex/markdown parsing).
- Hầu hết free-tier providers đã đề cập (Google AI Studio/Gemini, Groq) đều hỗ trợ structured output hoặc tool-calling theo chuẩn OpenAI-compatible — không cần tự chế cơ chế riêng.
- Dễ validate ở tầng application trước khi lưu vào `incidents` (JSONB fields `ai_evidence`, `ai_suggested_steps`) — nếu AI trả sai type/thiếu field, phát hiện ngay thay vì lưu rác vào DB.

---

## Next Steps

Đã hoàn thành đủ 6 tài liệu design nền tảng:
1. PRD
2. Architecture Document
3. Database Schema
4. API Specification (kèm Testing Strategy)
5. Security Considerations
6. AI Prompt & Schema Design

Bước tiếp theo (khi sẵn sàng code): setup project skeleton — Rust/Axum backend, Next.js frontend, Docker Compose (Postgres + Redis), cấu hình Cloudflare Tunnel để expose ra domain đã có.
