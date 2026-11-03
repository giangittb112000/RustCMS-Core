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
