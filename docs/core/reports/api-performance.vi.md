# Hiệu năng API: API mode (báo cáo đo)

Đo bằng `just bench-api <nhãn> [VAR=giá trị ...]`. Script `scripts/bench-api.sh` làm các việc sau:
- build `bench/api` bản release (package nằm ngoài workspace);
- tạo database PostgreSQL mới trong docker compose (port 55439, chạy trong RAM, `fsync=off`);
- seed 1.000 item, một user có role API `bench` (`items.view`, `items.create`) và một PAT;
- chạy `serve` rồi bắn `oha` 15 giây với 32 kết nối cho mỗi kịch bản.

Kịch bản:
- `public`: `GET /api/v1/ping`, `Access::public()`, không truy vấn DB.
- `pat_read`: `GET /api/v1/items` (20 dòng) dùng PAT, cần quyền `items.view`.
- `jwt_read`: như trên nhưng dùng JWT access token.
- `pat_write`: `POST /api/v1/items` dùng PAT.
- `admin_path`: `GET /admin/`. Node Full trả HTML shell của SPA; node API phải trả 404.

Máy đo: MacBook Pro (Apple Silicon), PostgreSQL 17 trong Docker Desktop. Mỗi round-trip tới DB qua VM của Docker Desktop tốn khoảng 1 ms, nên chỉ so sánh **tương đối** giữa các lần chạy. Trên Linux, với DB ở gần, số tuyệt đối sẽ cao hơn nhiều. CPU tính bằng giây CPU của process server trong 15 giây đo.

## Baseline (e47744b, trước API mode)

Cấu hình mặc định (Full; queue 2 workers, scheduler bật; pool 10):

| Kịch bản | req/s | p50 ms | p99 ms | non-2xx | CPU s | RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| public | 96441 | 0.31 | 0.78 | 0 | 72.48 | 43 |
| pat_read | 1921 | 16.06 | 21.6 | 0 | 15.14 | 46 |
| jwt_read | 2369 | 13.07 | 17.75 | 0 | 15.33 | 46 |
| pat_write | 1941 | 15.97 | 21.1 | 0 | 15.15 | 46 |
| admin_path | 92921 | 0.33 | 0.77 | 0 | 73.65 | 47 |

Tắt queue và scheduler (`RUSTCMS_QUEUE__ENABLED=false RUSTCMS_SCHEDULE__ENABLED=false`): không khác biệt, chênh lệch nằm trong nhiễu (pat_read 1891, jwt_read 2380).

Pool 32 (`RUSTCMS_DATABASE__MAX_CONNECTIONS=32`): pat_read 2398, jwt_read 2877, pat_write 2426, tức chỉ tăng khoảng 25%.

Nhận xét:
- **Request có xác thực bị giới hạn bởi số câu SQL**, không phải CPU (server dùng khoảng 1 core).
  - PAT tốn 4 câu trước handler: token, user, `UPDATE last_used_at` (chạy ở mọi request), và generation RBAC.
  - JWT tốn 2 câu: family và generation.
  - Tăng pool chỉ giúp một phần, vì DB phía sau cũng là giới hạn.
  - Giảm số câu SQL trên mỗi request là đòn bẩy chính.
- **Route admin hay worker trong router không làm chậm route API.** `public` chạy khoảng 95k req/s, ngang với HTML shell của SPA. Lợi ích của API mode nằm ở chỗ khác:
  - thu hẹp bề mặt (không có session, CSRF, SPA hay stream trên node API);
  - không có việc nền cạnh tranh pool DB;
  - vận hành nhiều node.

## Sau khi làm (939df68)

Full mode, cấu hình mặc định:

| Kịch bản | req/s | p50 ms | p99 ms | non-2xx | CPU s | RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| public | 91135 | 0.33 | 0.96 | 0 | 67.40 | 43 |
| pat_read | 3059 | 10.15 | 14.32 | 0 | 16.62 | 46 |
| jwt_read | 2400 | 12.99 | 18.09 | 0 | 16.05 | 46 |
| pat_write | 3111 | 9.93 | 15.12 | 0 | 16.70 | 47 |
| admin_path | 87335 | 0.34 | 1 | 0 | 68.63 | 47 |

API mode (`RUSTCMS_APP__MODE=api`):

| Kịch bản | req/s | p50 ms | p99 ms | non-2xx | CPU s | RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| public | 90642 | 0.33 | 0.93 | 0 | 67.31 | 41 |
| pat_read | 3038 | 10.18 | 15.26 | 0 | 16.71 | 45 |
| jwt_read | 2403 | 13.03 | 17.32 | 0 | 15.96 | 45 |
| pat_write | 3147 | 9.91 | 13.79 | 0 | 16.91 | 45 |
| admin_path | 105189 | 0.28 | 0.81 | tất cả (404, đúng thiết kế) | 60.31 | 45 |

Tách theo từng yếu tố:
- **(c) Tối ưu PAT** là nguồn tăng chính.
  - pat_read tăng từ 1921 lên 3059 req/s (+59%), p50 giảm từ 16.1 xuống 10.2 ms.
  - pat_write tăng từ 1941 lên 3111 req/s (+60%).
  - Nguyên nhân: từ 4 câu SQL trước handler còn 2 (token join user; generation RBAC), và không còn `UPDATE last_used_at` trên mỗi request.
- **(a) Bỏ route admin** không ảnh hưởng tới route API: public, pat_read và jwt_read của Full và API ngang nhau, chênh lệch nằm trong nhiễu (khoảng ±5% giữa các lần chạy trên cùng máy).
  - Riêng public đo khoảng 91k, so với 96k ở baseline. Layer chặn path panel được thêm ở mọi mode, nên đã đổi lại thành chỉ thêm khi có path cần chặn (tức là chỉ ở API mode).
  - Path admin trên node API trả 404 ở khoảng 105k req/s và không đụng DB.
- **(b) Tắt worker và scheduler** (đo ở baseline-quiet) không thay đổi throughput trong 15 giây đo. Lợi ích là pool DB không bị việc nền chiếm, và có ý nghĩa khi có job thật.
- **jwt_read** không đổi (2400), vì vẫn tốn 2 câu SQL: kiểm tra family (revoke) và generation.

## Hai cache đã được duyệt (ADR 0012 amendment, 2026-11-04)

Anh Đức đã duyệt cả hai cache. Cấu hình mặc định:
- `auth.permission_generation_cache_ms = 1000`: generation RBAC đọc tối đa 1 lần/giây trên mỗi instance.
- `auth.jwt.session_cache_ms = 1000`: family và user của JWT được đọc chung trong một câu join, rồi dùng lại trong 1 giây. Cache được khoá theo generation cục bộ.

Kết quả ở API mode:

| Kịch bản | req/s | p50 ms | p99 ms | non-2xx | CPU s | RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| public | 99597 | 0.31 | 0.73 | 0 | 70.96 | 41 |
| pat_read | 4370 | 7.09 | 10.6 | 0 | 16.93 | 46 |
| jwt_read | 8425 | 3.66 | 6.24 | 0 | 19.19 | 46 |
| pat_write | 4479 | 6.9 | 10.67 | 0 | 17.14 | 47 |
| admin_path | 109360 | 0.28 | 0.67 | tất cả (404) | 64.87 | 47 |

So với baseline:
- **pat_read:** 1921 → 4370 (×2,3).
- **pat_write:** 1941 → 4479 (×2,3).
- **jwt_read:** 2369 → 8425 (×3,6).

Lúc này request JWT chỉ còn câu SQL của chính handler. Request PAT còn 1 câu (token join user) cộng với câu của handler.

**Đánh đổi:**
- Trên instance thực hiện thay đổi, thay đổi có hiệu lực ngay ở request kế tiếp. Các instance khác thấy thay đổi trong vòng 1 giây.
- Đặt `0` để quay lại hành vi cũ (đọc DB trên mỗi request).
- Session và PAT không bị cache, nên revoke chúng không bao giờ bị trễ.

## Đề xuất ban đầu (đã làm 1 và 2, xem mục trên)

1. **Câu đọc generation RBAC trên mỗi request.** Có thể cache vài trăm ms theo từng instance.
   - Lợi ích: PAT còn 1 câu, JWT còn 1 câu, khoảng +30–50% theo cách đo này.
   - Đánh đổi: thay đổi phân quyền có thể trễ tối đa bằng TTL trên các instance khác, phá cam kết "áp dụng từ request kế tiếp" của ADR 0012.
2. **Kiểm tra revoke JWT** (`jwt_session::verify_access` đọc family). Có thể cache trạng thái family theo TTL ngắn.
   - Đánh đổi tương tự: đăng xuất hoặc revoke có độ trễ.
3. **Pool DB:** pool 32 cho thêm khoảng 25% ở baseline. Các key `min_connections`/`acquire_timeout` chưa cần. Hướng dẫn chỉnh `database.max_connections` theo số instance đã có trong guide.
