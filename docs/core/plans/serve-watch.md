# `rustcms serve --watch` — thiết kế cho RustCMS (v3, 2026-11-16 — Codex vòng 3: sẵn sàng triển khai)

Trạng thái: **đề xuất, chưa duyệt.** Xuất phát từ PR #1 của giangittb112000 (`crates/rustcms-cli/src/watch.rs`, commit 43eb700). Anh Đức muốn phát triển tiếp lệnh này thay vì tích hợp bacon (cồng kềnh).

Lịch sử: v2 → v3 sau Codex vòng 2 (tín hiệu bằng tokio + nix, dọn cả hai group cùng lúc, giữ thay đổi chưa xử lý qua các vòng, bỏ cam kết theo dõi `*_FILE`, sửa lời hứa về template và thời gian dừng). v1 → v2 sau Codex vòng 1 (bỏ socket handoff, bỏ "chỉ restart không build", bỏ npm tự động và cổng `APP_ENV`; thêm quản lý tín hiệu/tiến trình con là điều kiện bắt buộc; sửa bảng sự thật).

## Mục tiêu

- Một lệnh, không cài thêm gì: `rustcms serve --watch`. Chỉ ở CLI, **không đổi Core**.
- Dependency: `rustcms-cli` thêm trực tiếp `tokio` (workspace, đã có feature `signal`) và `nix` (chỉ Unix; đã có trong `Cargo.lock` cùng phiên bản) để gửi tín hiệu theo group mà không cần `unsafe` (workspace cấm `unsafe_code`). Không crate mới vào cây phụ thuộc.
- Không build lại vô ích: sửa template đã đăng ký, template mail, `uploads`… thì không làm gì.
- Build trước (prebuild) lỗi thì server cũ vẫn chạy. **Không hứa** "admin không bao giờ chết": server mới có thể lỗi lúc khởi động (config, cổng), và vẫn còn một khe nhỏ giữa lúc dừng server cũ và lúc `cargo run` chạy.
- Ctrl-C luôn dọn sạch tiến trình con (cargo, build script, server).

Ngoài phạm vi: hot patching, tăng tốc rustc, socket handoff, npm tự động, Windows (giai đoạn 1).

## Sự thật trong code (đã kiểm với Codex @ 68304f3)

| Đổi gì | RustCMS dùng thế nào | Việc cần làm |
|---|---|---|
| `src/**` (cả `src/database/migrations`), `Cargo.toml`, `Cargo.lock` (của workspace), `build.rs`, `.cargo/config.toml`, `rust-toolchain*` | Biên dịch | Build → thay server |
| `lang/*.toml` | `include_str!` (`admin_panel_provider.rs.tmpl:13`) | Build → thay server |
| `resources/admin/{js,css}/**` (bundle extension đã build), logo/favicon | `include_str!`/`include_bytes!` (`make.rs:294`, LMS provider:24) | Build → thay server, rồi F5 |
| `resources/admin/src/**`, `package.json` | `npm run build` (`rustcms-build`) | **Không tự làm** ở giai đoạn 1; in gợi ý "chạy `npm run build`" (một lần mỗi đợt) |
| `resources/views/**` | Chỉ template đăng ký bằng `view!`/`mail_template!` mới đọc lại mỗi lần render, và chỉ khi bản debug + env thực tế là `local` (`template.rs:69`, `page.rs:45`). `PageView::Template(include_str!(…))` vẫn nhúng cứng (`page.rs:16`) | **Không theo dõi** ở giai đoạn 1. Guide ghi rõ: trường hợp không tự đọc lại thì sửa một file trong `src/` hoặc khởi động lại |
| `config/*.toml` (không đệ quy), `.env` | Đọc lúc khởi động (`config.rs:228,253`) | Build check (thường chỉ kiểm fingerprint, rất nhanh) → thay server |
| File trỏ bởi `*_FILE` | Đường dẫn tùy ý (`config.rs:185`) | **Không theo dõi** (ghi trong guide: đổi secret thì lưu lại một file đã theo dõi hoặc khởi động lại) |
| Migrations | `serve` không chạy và không kiểm migration (`console.rs:145`, `app.rs:365`) | Build → thay server, in nhắc `rustcms migrate` khi file trong `src/database/migrations` đổi |
| `storage/`, uploads, DB SQLite, log | Đọc khi chạy | Không theo dõi |

Ghi chú: bảng theo đường dẫn là **quy ước**, không phải đồ thị phụ thuộc thật. Path dependency nằm ngoài app không được theo dõi (ghi trong guide).

## Thiết kế giai đoạn 1

### 1. Một pipeline tuần tự

```
quét (500 ms) → có thay đổi đáng kể? → chờ yên (debounce 300 ms, gom thêm, tính cả thêm/xoá/đổi tên)
  → cargo build --package P --bin B   (server cũ vẫn chạy)
      lỗi → giữ server cũ, chờ thay đổi tiếp
      ok  → quét lại: có file build đổi trong lúc build? → build lại (lặp)
          → dừng server cũ (SIGTERM → chờ → SIGKILL)
          → cargo run --package P --bin B -- serve …   (cùng tham số; Unix: cargo exec thành server)
```

- **Luôn build check**, kể cả khi chỉ `config/` hay `.env` đổi: tránh trường hợp đã sửa Rust lỗi trước đó, rồi đổi config làm dừng server cũ và `cargo run` gặp code lỗi. Cargo thấy không có gì đổi thì xong trong ~1 s.
- **Chạy qua `cargo run`** để giữ `[env]`, runner, đường dẫn thư viện động: giống hệt `rustcms serve` thường (`project.rs:42`). Trên Unix, cargo `exec_replace` nên PID là server (trừ khi cấu hình `target.<triple>.runner` → in cảnh báo "runner: tín hiệu tới runner").
- Không áp `Cargo.lock` "do build tự ghi" như PR #1: thay đổi này chỉ gây thêm một lần build check rỗng (an toàn hơn nuốt mất sửa của người dùng). Theo dõi lockfile của **workspace** (tìm bằng `cargo locate-project --workspace`).
- Tên binary: lấy `default-run` hoặc binary duy nhất của package qua `cargo metadata --no-deps`; nhiều binary mà không có `default-run` → báo lỗi rõ.

### 2. Tín hiệu và tiến trình con (điều kiện bắt buộc)

- Mỗi tiến trình con (`cargo build`, `cargo run`/server) chạy trong **process group riêng** bằng `CommandExt::process_group(0)` (std). **Không dùng `setsid`** (tách khỏi terminal).
- `stdin` của con là `null`; `stdout`/`stderr` kế thừa. Ctrl-C từ terminal chỉ tới watcher.
- Watcher đăng ký SIGINT/SIGTERM (tokio `signal`) **trước khi** sinh con; mọi lần chờ (debounce, build, drain) đều ngắt được.
- **Watcher sở hữu cả hai group cùng lúc**: trong lúc build trước, server cũ và `cargo build` cùng sống. Khi nhận tín hiệu hoặc gặp lỗi: SIGTERM cả hai group (`nix::sys::signal::killpg`) → chờ tối đa 6 s → SIGKILL các group còn thành viên (kể cả khi leader đã thoát) → `wait` các con trực tiếp → thoát. Đã nhận lệnh dừng thì không bao giờ khởi động server mới.
- Thay server: SIGTERM group server → thay ngay khi group đã thoát, **tối đa 6 s** rồi SIGKILL. (Drain của Core mặc định 5 s, chỉnh được: `app.rs:446`; WebSocket đóng bằng 1001.) Worker queue mặc định 30 s bị cắt ở dev: chấp nhận, ghi trong guide.
- Server thoát bất ngờ (bất kể lúc nào) → in "server đã dừng (exit N)", chờ thay đổi tiếp. Không thử lại.
- Giới hạn (ghi trong guide): watcher bị SIGKILL thì group con có thể còn sống; con tự tách group thì thoát khỏi việc dọn; Ctrl-Z/SIGQUIT từ terminal chỉ tới watcher → dùng ở foreground. Không làm watchdog.

### 3. Quét file

- Gốc theo dõi: thư mục app (`src`, `config`, `lang`, `resources`, `build.rs`, `.env`, `.cargo/`, `rust-toolchain*`, `Cargo.toml`) + `Cargo.toml`, `Cargo.lock`, `.cargo/`, `rust-toolchain*` ở gốc workspace.
- Đổi `Cargo.toml` → chọn lại package/binary qua `cargo metadata` (chỉ binary target của package app).
- Bỏ qua: `resources/views/`, `target/`, `node_modules/`, `.git/`, `storage/`, file tạm editor (`*~`, `.#*`, `*.swp`, `*.swx`, `4913`, `*.tmp`).
- Không theo dõi `resources/views/**` (xem bảng: template đăng ký tự đọc lại; trường hợp khác cần sửa `src/` hoặc khởi động lại). Template mới cần dòng `view!` trong `src/` → đã kích hoạt build.
- Lỗi đọc thư mục → in cảnh báo, giữ snapshot cũ cho đường dẫn đó và thử lại vòng sau (không coi là "file bị xoá").
- **Thay đổi chưa xử lý không bao giờ bị nuốt**: snapshot "đã xử lý" chỉ cập nhật bằng ảnh chụp *trước* lần build vừa dùng; mọi thay đổi xảy ra trong lúc build/dừng/khởi động (kể cả `Cargo.lock` do cargo ghi) được xử lý ở vòng sau (cùng lắm thêm một lần build rỗng).
- Đo chi phí quét trên `examples/demo` trước khi chốt 500 ms.

### 4. Hiển thị

- Một dòng mỗi đợt: `watch: src/models/post.rs (+2) → build 6.2s → restarted` / `→ build failed, server kept`.
- Output của cargo và server giữ nguyên.

### 5. Không làm ở giai đoạn 1

- npm build tự động (output ghi dở, `build.js` xoá CSS trước khi build → vòng lặp sự kiện).
- Socket handoff (cần truyền fd giữa tiến trình, `LISTEN_PID`, Windows… — không phải "thay đổi nhỏ"; chỉ giữ được kết nối mới trong backlog).
- Windows (cần Job Object để giết cả cây tiến trình): `--watch` trên Windows báo "chưa hỗ trợ".
- Cổng `APP_ENV=local` (muốn đúng phải tính lại config nhiều lớp; `Config::load()` trong watcher sẽ nạp `.env` vào chính watcher, làm giá trị cũ dính sang tiến trình con).

## Kiểm thử

- Unit: phân loại đường dẫn, bỏ file tạm, debounce gom đợt, `--watch` trước/sau `--`, chọn binary từ `cargo metadata`.
- Tích hợp (Unix, `--ignored`, chạy trong `test-cli`, dùng app scaffold):
  - build lỗi → server cũ vẫn trả 200; sửa đúng → server mới;
  - sửa file trong lúc build → build lại trước khi thay;
  - ghi `.env` kiểu atomic (ghi file tạm rồi rename) → một lần thay;
  - `make:resource` (nhiều file) → một lần build;
  - sửa template `view!` → không build;
  - Ctrl-C lúc đang build và lúc đang chạy → không còn tiến trình con (kiểm bằng PID group);
  - Ctrl-C khi **cả** server cũ và build đang sống, có tiến trình cháu còn sống sau leader → cả hai group bị dọn.

## Docs

- Guide: mục "Development loop" (bảng "đổi gì → điều gì xảy ra", giới hạn: Unix, path dependency, worker bị cắt, nhắc `migrate`, `npm run build`), `[profile.dev] debug = "line-tables-only"`.
- `docs/guide/src/reference/cli.md` sinh lại; CHANGELOG.

## Dependency cụ thể (Codex vòng 3 đã kiểm `Cargo.lock`)

```toml
[dependencies]
tokio.workspace = true        # feature `signal` đã bật ở workspace; thêm `process` nếu dùng tokio::process

[target.'cfg(unix)'.dependencies]
nix = { version = "=0.31.3", features = ["signal"] }   # đã có trong Cargo.lock; `signal` kéo theo `process` (killpg, Pid)
```

## Câu hỏi còn mở

Không còn (đã trả lời qua Codex vòng 1–3). Chờ anh Đức duyệt; quyết định còn lại: làm trong PR #1 của tác giả hay em tự làm trên nhánh riêng.
