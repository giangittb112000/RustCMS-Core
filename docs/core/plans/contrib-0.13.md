# Tích hợp tính năng từ 3 PR của giangittb112000 (kế hoạch v3, 2026-11-16)

Trạng thái: **v3 sau 2 vòng Codex review kế hoạch; chờ anh Đức duyệt.**
Nguồn: `docs/core/reports/2026-11-16-pr-review.vi.md` (mã lỗi 1.x, 2.x, 3.x).
Sau khi xong: `serve --watch` làm riêng theo `docs/core/plans/serve-watch.md` (v3).

## Phạm vi

| Mốc | Phần | Commit gốc (tác giả giangnt) | ADR | Rủi ro |
|---|---|---|---|---|
| A | Giới hạn ô số + chỉnh giao diện (History card, mũi tên select) | 43eb700 (bỏ phần watch), 2dc30e0, 23507ae | — | thấp |
| B | `shown_when` + `title_template` | 0c3d9fd | 0037, 0038 | **cao** |
| C | Infolist + `computed()` + `PageView::Infolist`, SDK `ui` 3 | e0425649 | 0039 | trung bình |
| D1 | Repeater JSON + `RepeatableEntry` + cột đếm + lịch sử theo dòng | cherry-pick **cả 4 commit** 8c0e550d, a77173d2, 1de9d98e, 612f1a00 một lần (1de9d98e/612f1a00 trộn phần JSON và quan hệ, không tách sạch được); D1 là lượt sửa phần JSON/đếm/UI | 0040 | trung bình |
| D2 | Repeater quan hệ (ghi bản ghi con trong form cha) | (đã vào ở D1) — lượt sửa phần quan hệ | 0040 | **cao** |

**Không đưa vào:**
- `serve --watch` (trong 43eb700: `crates/rustcms-cli/src/{main.rs,watch.rs}`, mục watch của guide `installation.md` và CHANGELOG) → làm lại sau.
- `scripts/prune-target.sh`, `just prune`, sửa `CLAUDE.md`, `bench-api.sh`/`serve.sh` (0984e9c9); merge commit 0e99ca6d (không thêm gì); các commit "Rebuilt admin assets".
- **Loạt "default view page" tác giả đẩy thêm vào PR #3 sau lúc review** (06cbfad → de99250, 9 commit). Ngoài phạm vi lần này; review riêng sau. (770afba có sửa `isLayout` — mình tự sửa 3.1 trong D1.)

## Cách làm (mọi mốc)

1. Nhánh `integrate/contrib-0.13` từ `main` @ 68304f3. Cherry-pick giữ `Author: giangnt`, kể cả 43eb700 áp một phần (`git cherry-pick -n` + bỏ file watch, commit với `--author`). Ghi commit gốc trong message (`(cherry picked from …)`). CHANGELOG ghi "contributed by @giangittb112000".
2. PR xếp chồng nên **có thể xung đột** khi sửa của mốc trước đụng file mốc sau → giải quyết tay, chạy lại test của mốc trước.
3. Sửa toàn bộ lỗi của mốc trong một lượt; **mỗi sửa có test hồi quy và kiểm tra ngược** (bỏ sửa → test fail), theo checklist.
4. Thứ tự commit trong mốc: code + test + docs + contract (`UPDATE_CONTRACT=1`, `types.gen.ts`, xem diff) → **sau thay đổi UI cuối cùng** mới build lại assets (`just admin-build`, SDK copies, `resources/admin` của demo) → commit riêng "rebuilt admin assets". Sửa sau Codex có đụng UI thì build lại lần nữa.
5. Mỗi mốc giữ đúng `VOCABULARY_VERSION` (A=12, B=13, C=14, D1=15; D2 nếu đổi schema dòng (3.14) thì 16). Lỗi validation mới cần template trong `rustcms_core::i18n::TEMPLATES`, `fixtures/contract/issues.json`, `error-codes.md`, cả hai bản dịch UI. Route mới giữ case trong `fixtures/authz/admin.toml`.
6. Gate mỗi mốc: `just lint && just test-matrix && (cd admin-ui && npx vitest run)`; commit; **Codex verify**; sửa phát hiện trong commit tiếp (tối đa 2–3 vòng).
7. Mốc rủi ro cao (B, D2): **Codex review thiết kế sửa trước khi code** (mục "thiết kế" dưới đây là đầu vào).
8. Docs cùng commit với code: guide (mẫu bằng `#region`, chụp lại ảnh khi UI đổi, `just docs`), ADR, `admin-protocol.md`, CHANGELOG, **mục upgrade "From 0.12 to 0.13"** (API Rust mới, SDK `ui: 3`, vocabulary, hành vi tìm kiếm `title`).
9. Không migration Core nào (các commit chọn không có). Guide nêu migration phía app cho `order_column` (cột + index, dùng helper `rustcms::migration`).
10. **Không push, không tag, không đụng GitHub** cho tới khi anh bảo.

## Mốc A — Giới hạn ô số

| Mã | Sửa |
|---|---|
| 1.1 | Cột `f32`: kiểm cả giá trị sau khi ép về `f32` (vượt `max`, underflow dưới `min`). Test `db_test!`. |
| 1.3 | Guide: form của action chưa có `min/max`. |
| — | Bỏ phần watch; sinh lại `docs/guide/src/reference/cli.md`. |

## Mốc B — `shown_when` + `title_template`

**Thiết kế 2.1 + 2.4 (Codex review trước khi code):**
- Điều kiện được tính trên một **bản chiếu theo người dùng**, tách khỏi dữ liệu thật:
  - giá trị **đã lưu/mặc định** của trường người dùng không đọc được (field rule; `guarded` chỉ khi không có rule cho đọc) → `null`;
  - người dùng được sửa nhưng **không được xem bản ghi** (policy VIEW từ chối; như `adapter/read.rs` không trả dữ liệu) → **mọi** giá trị đã lưu → `null`;
  - sau đó **phủ giá trị client vừa gửi** cho các ô được nhận (người ghi "mù" và ô write-only vẫn biết giá trị mình gửi) — giống SPA (`form.ts:207`, `record.tsx:121`);
  - quyền tính trong ngữ cảnh giao dịch.
- Không cấm điều kiện đọc trường `guarded` ở boot (rule đọc tường minh có thể cho phép).
- Khoá ngoại ràng buộc (`ctx.bound`, tạo từ relation manager):
  - **miễn bị bỏ/xoá** khỏi dữ liệu ghi bởi điều kiện; vẫn qua kiểm quyền ghi, kiểu, và kiểm "hook không đổi khoá";
  - với các điều kiện khác đọc trường này: **giữ quy tắc hiện có** của cả hai bộ đánh giá (trường bị điều kiện ẩn → `null`, `conditions.rs:85`, `conditions.ts:84`), giá trị lấy từ ràng buộc;
  - SPA: đưa giá trị ràng buộc vào **ngữ cảnh đánh giá** (không gửi lên như input — server từ chối, `operations.rs:425`), để server và SPA thấy cùng giá trị;
  - test parity (Rust + SPA) cho điều kiện đọc khoá ràng buộc, điều kiện nối chuỗi, và các kiểu wire (i64 chuỗi, uuid).

| Mã | Sửa |
|---|---|
| 2.1, 2.4 | Như thiết kế trên. Test: điều kiện dựa trên trường HR-only không lộ qua lưu/bỏ, 422, `clear_when_hidden`; trường đọc được nhưng bản ghi không xem được cũng không lộ; người ghi mù vẫn thấy ô phụ thuộc giá trị mình gửi. |
| 2.2 | Settings: tính giá trị cuối cùng (gồm reset ẩn) trước `kept_while`, so với lưu-hoặc-mặc-định; dùng chung cho kiểm tham chiếu media. |
| 2.3 | `clear_when_hidden` so với giá trị đã lưu. Test: bỏ trống, `null`, giá trị khác, xoá về mặc định khác null. |
| 2.5 | Schema phát cây điều kiện **đã chuẩn hoá** ở mọi bề mặt (form, layout, settings, action; sau này infolist/dòng repeater). Case `eq`, `ne`, `is_in`, lồng nhau trong `fixtures/conditions/cases.json`. |
| 2.6 | "Bắt buộc khi hiện": áp khi tạo, khi trường được gửi, hoặc khi chuyển ẩn → hiện (so trước/sau bằng cùng bản chiếu). PATCH bỏ trống giữ hành vi cũ. |
| 2.7 | Hoãn cả kiểm "required" lẫn "non-nullable" cho ô có điều kiện tới khi biết ẩn/hiện; ô hiện mà null không hợp lệ → từ chối trước hook. |
| 2.8 | SPA: tab đang mở giữ theo danh tính lấy từ khai báo gốc (chỉ số trước khi lọc), có quy tắc khi tab đang mở bị ẩn; giữ hành vi bàn phím và focus lỗi. |
| 2.9 | ADR 0038 sửa mô tả tìm kiếm; CHANGELOG/upgrade: đổi hành vi tìm `title(...)`, tiêu đề rỗng bị bỏ, export `Condition` có thể trùng `sea_orm::Condition`; thêm ô có điều kiện vào kitchen-sink; tìm kiếm: **bỏ từ trùng**, không cắt bớt từ. |
| hoãn | Secret settings bị ẩn mà không giải mã được: **không làm** lần này (nguy cơ mất envelope). |

## Mốc C — Infolist

| Mã | Sửa |
|---|---|
| 3.2 | Biến đếm theo app/test (closure computed đọc từ state của test). |
| 3.3 | Giới hạn đồng thời theo người dùng (2), lấy **trước** permit toàn cục; dùng chung mọi resource; state có giới hạn và dọn; nhả khi huỷ. Codex xem thiết kế ngắn trước khi code. |
| 3.4 | Guide: timeout không dừng code đồng bộ/CPU, kể cả trong `spawn_blocking`; việc nặng phải tự hợp tác huỷ hoặc tự giới hạn. |
| 3.5 | CHANGELOG + upgrade "Breaking (Rust API)": biến thể enum mới, `DynResource::computed`. Không thêm `non_exhaustive`. |
| 3.6 | Demo kiểm hex trước khi vào CSS; sửa mô tả `size`/`gallery`. Link quan hệ theo quyền từng bản ghi: **hoãn**, ghi guide ("có thể dẫn tới trang không xem được"). |

## Mốc D1 — Repeater JSON, cột đếm, lịch sử

| Mã | Sửa |
|---|---|
| 3.1 | Sửa bộ phân biệt chung `isLayout` (entry có `name` không phải layout); truyền ngữ cảnh resource/bản ghi vào dòng lồng nhau. Test dòng lồng, entry tuỳ biến. |
| 3.15 (JSON) | Test UI cho ánh xạ lỗi dòng JSON khi kéo/xoá/nhân bản (sửa của 612f1a00). |
| 3.16 (count) | `counts()`: nhánh policy không phụ thuộc bản ghi dùng một truy vấn `GROUP BY` giữ scope/distinct; giữ đường dự phòng có giới hạn cho policy theo bản ghi. |

## Mốc D2 — Repeater quan hệ

**Thiết kế khoá (Codex review trước khi code; nếu quá xâm lấn, đề xuất anh hoãn D2 sang bản sau):**
- **Một kế hoạch khoá duy nhất** cho cả lần ghi: gom khoá của mọi repeater, link target, khoá ngoại ngầm định theo resource, khoá theo `Registry::lock_rank`, trong mỗi resource theo thứ tự key; dùng helper khoá có sẵn (`lock_visible`/`write.rs`) vì `ORDER BY … FOR UPDATE` thuần không đủ trên MySQL.
- `check_lock_order`: thêm cạnh cha → con của repeater quan hệ; boot từ chối cấu hình tạo vòng hoặc không hỗ trợ.
- Sắp xếp lại: tìm ứng viên **có giới hạn** (đếm trước, vượt `MAX_ROW_ITEMS` → 422), xét quyền, khoá bằng helper theo thứ tự, kiểm lại dưới khoá. Không khoá dòng người dùng không thấy rồi mới xét quyền.
- Ghi đồng thời từ trang của con: hiện **không** tuần tự hoá — ghi trực tiếp vào con chỉ khoá cha khi con khai báo `lock_parent` (`write.rs:167`), và boot chỉ từ chối `lock_parent` khai báo *sai* (`registry.rs:375`). Sửa: **boot từ chối repeater quan hệ khi resource con không khai báo `lock_parent` của đúng relation này** (thông báo rõ cách thêm); cập nhật demo và resource trong test. Khi đó mọi lần ghi con (trực tiếp, đổi cha, repeater) đều khoá cha trước → tuần tự hoá. Test đồng thời: tạo/đổi cha trực tiếp so với `min/max_items` và cấp vị trí của repeater.

| Mã | Sửa |
|---|---|
| 3.7 | Lịch sử cha (`rows_changed`): mặc định **chỉ hiện số lượng**; hiện khoá dòng con chỉ khi từng khoá vượt qua scope + quyền VIEW của con; dòng đã xoá/không tìm thấy → không hiện khoá (fail closed). |
| 3.8, 3.9 | Như thiết kế khoá trên. Test boot từ chối vòng; test thứ tự khoá trên PostgreSQL và MySQL. |
| 3.10 | `MAX()` (gồm dòng đã xoá mềm) cho cả tạo mới và vị trí tạm khi sắp xếp; đếm trước khi tải/khoá; số học có kiểm tràn → 422. |
| 3.11 | Ô trong dòng **kế thừa luật của form con**; boot từ chối khai báo dòng có options/`min/max` khác với form con. `.addable(false)`/`.deletable(false)` thực thi ở server (sau bước admit). |
| 3.12 | Cột thứ tự i64: mọi chỗ đọc/ghi vị trí (tạo, MAX, vị trí giữ chỗ, hai lượt sắp xếp) theo kiểu cột; test tràn. |
| 3.13 | SPA: mỗi dòng dùng ctx của con (slug con, khoá dòng; dòng mới không có khoá cha); bỏ `optionsPath` kế thừa sai; giữ đường dẫn lỗi dòng. |
| 3.14 | Schema riêng cho dòng mới (Create) và dòng có sẵn (Edit) → **đổi contract** (vocabulary 16, types, fixtures, protocol, renderer cùng commit). Codex xem trước. |
| 3.15 | Test: policy từng dòng từ chối update/delete, field rule ô dòng, media trong dòng, `min_items` khi xoá, panic boot `lock_parent`, panel "xoá dòng giữ thứ tự đã kéo". |
| 3.16 | `find_model_in(...).ok()`: giữ che "không có dòng" nhưng trả lỗi DB thật; ref dòng mới không trùng khoá con bất kỳ (số hay chuỗi) → 422 khi gửi. |
| — | Guide: `order` không có `order_column` bị bỏ qua; đổi vị trí không qua hook/lịch sử của con; migration `order_column` phía app. |

## Kết thúc

- Gate cuối một lần: `just ci`, `just msrv`, `just docs`, E2E Chrome, `just test-cli`, `npm audit --omit=dev`; Codex review toàn nhánh.
- Báo cáo ngắn `docs/core/reports/<ngày>-contrib-0.13.vi.md` (thay đổi, lỗi Codex đã sửa, kết quả gate, việc hoãn).
- Anh quyết tiếp: gộp `main` (CI GitHub phải xanh), đóng 3 PR kèm lời cảm ơn + link commit (em soạn, anh duyệt), phát hành 0.13.0.
- Sau đó: nhánh `serve --watch` (thiết kế v3).

## Việc hoãn (ghi lại để không quên)

- Link quan hệ theo quyền từng bản ghi (3.6); secret settings ẩn không giải mã được; `non_exhaustive`; loạt default view page của tác giả; form action có `min/max`.

## Rủi ro

- D2 là phần nặng nhất (khoá nhiều resource, cả 3 DB). Có điểm dừng: nếu thiết kế khoá sau Codex vẫn xâm lấn, đề xuất anh hoãn D2. Vì code quan hệ đã vào cùng D1, hoãn nghĩa là **gỡ** `.relationship()` (API, route ghi, UI, test) khỏi nhánh — tốn thêm công; em sẽ báo ước lượng trước khi gỡ.
- Code tác giả có thể còn lỗi chưa thấy → Codex review toàn nhánh cuối cùng.
- Đĩa: `df -h` trước gate; `cargo clean` dưới ~15 GB.
