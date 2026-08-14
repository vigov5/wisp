# Phản biện kế hoạch tối ưu hiệu năng truyền file

Đối tượng: `docs/transfer-performance-plan.md`

Tài liệu gồm hai vòng phản biện:

- **Vòng 1 (2026-08-13, commit `afc652f`)** — mục 1–8 bên dưới. Đối chiếu với
  `crates/core/src/blobs/receive.rs`, `crates/core/src/blobs/telemetry.rs`,
  `crates/core/src/transfer/receiver.rs`, `crates/app/src/quic_keepalive.rs`,
  `Cargo.toml`, `flutter/rust/Cargo.toml`.
- **Vòng 2 (2026-08-14, commit `3b6aff0` + working tree)** — phần "Vòng 2" ở cuối,
  sau khi plan được viết lại và A1–A6 được triển khai.

Trạng thái: chỉ là feedback, chưa sửa code hay sửa plan.

## Vòng 1 — 2026-08-13

## 1. Lỗi phương pháp lớn nhất: P0 đã ship trước khi P1 đo

Doc tự viết "P1: đo đúng trước khi chỉnh" nhưng P0 đã sửa 4 thứ và đóng dấu "hoàn tất".
Phần "Xác minh" (plan dòng 95–100) chỉ có `cargo test`, `rustfmt`, `git diff --check` —
**không có một con số throughput nào**. Hệ quả:

- Không biết P0 có tác dụng gì. Baseline đã mất (commit `8a33818` đã vào main), muốn A/B
  phải revert.
- 4 fix chồng lấn nhau nên không attribute được. Cụ thể: checkpoint throttle (P0-2) nằm
  **downstream** của coalescer (P0-1). Sau khi coalesce về 10 Hz thì record write đã tự
  động còn 10/s; throttle 1s/64MiB chỉ hạ tiếp 10 → 1. Hai fix ship cùng lúc ⇒ vĩnh viễn
  không biết cái nào tạo ra kết quả.
- Giả thuyết: gần như toàn bộ win nằm ở việc **thôi serialize + ghi lại `record.json` mỗi
  16 KiB** (một fs write đồng bộ trên mỗi BAO leaf — ~6.400 write/s), chứ không phải
  channel hay progress frame. Chỉ một phép đo là chứng minh được, và điều đó thay đổi cách
  ưu tiên P2/P3.

Doc nên ghi rõ: P0 là *bet chưa kiểm chứng*, không phải *đã xong*.

## 2. Telemetry đang đọc số liệu từ đầu sai của kết nối

Đây là lỗi kỹ thuật nghiêm trọng nhất trong code mới.

Transfer là **pull**: receiver mở stream, dữ liệu chảy sender → receiver. Nhưng
`NetworkSnapshot::capture` (`crates/core/src/blobs/telemetry.rs:246`) đọc `path.stats()` ở
phía receiver, và trong quinn các field này mô tả **hướng gửi của chính local endpoint**:

- `cwnd` — congestion window của receiver, tức cwnd cho luồng ACK. Gần như vô nghĩa với
  transfer.
- `lost_packets` / `lost_bytes` / `congestion_events` — mất mát trên packet **receiver gửi
  đi**, không phải loss của luồng data.

Chỉ `rtt`, `udp_rx.bytes`, `current_mtu` là dùng được. Nghĩa là hai con số mà P3 dựa vào để
quyết định window và CUBIC/BBR (cwnd + loss) đang được lấy từ nhánh sai. Muốn diagnose thật
thì cần stats **phía sender**, hoặc ít nhất phải nói rõ trong doc rằng cwnd/loss ở đây
không phải của luồng bulk.

Kèm theo:

- **Không log giá trị window đang cấu hình.** Cả P3 xoay quanh giả thuyết "window-bound",
  mà log lại không có `stream_receive_window`. Sample hiện tại đã có `app_bytes_per_sec` và
  `rtt_us` — chỉ cần log thêm window là tính được ngay `app_bps × rtt / window`; tỉ số ≈ 1
  là bằng chứng window-bound, không cần stats phía sender. Đây là bổ sung rẻ nhất, giá trị
  cao nhất.
- **`udp_rx_bytes_delta` vs `bytes_delta` là chẩn đoán miễn phí đang bị bỏ**: udp_rx tăng
  mà app offset đứng ⇒ nút thắt nằm *sau* mạng (store write / hash / disk), không phải
  QUIC. Doc list "thời gian chờ ghi blob store" như việc phải làm thêm, trong khi tín hiệu
  này đã có sẵn.

## 3. Metric stall sẽ báo động sai

- `BlobTransferTelemetry::new` được gọi ngay sau connect, trước byte đầu tiên
  (`crates/core/src/blobs/receive.rs:288`), và `last_progress_at = now`. Nên toàn bộ thời
  gian sender mở store/handshake/hash lười bị tính là stall. Tiêu chí nghiệm thu "không có
  stall > 500 ms" sẽ fail ngay lần chạy đầu vì lý do lành tính. Cần tách warm-up khỏi
  mid-transfer stall.
- `Some(Done(_)) | None` gộp chung (`crates/core/src/blobs/receive.rs:100`) ⇒ stream cạn
  **không có `Done`** vẫn được log `outcome = "complete"`. Đúng ra đó là transport failure
  (chính `crates/core/src/transfer/receiver.rs:1014` phân loại như vậy). Vậy thống kê
  failure/stall — thứ mà P1 định dùng để ra quyết định — không đáng tin. Việc gộp này là
  hành vi cũ, nhưng giờ nó làm bẩn dữ liệu đo.
- Stall resolution = 250 ms vì `detect_stall` chỉ chạy trong `sample()`. Với threshold
  500 ms thì tạm ổn, nhưng `stall_total` bị lượng tử hóa; nên nói rõ trong doc để không so
  sánh quá mức tinh.

## 4. Observer effect: đo trên code path khác code path production

Khi telemetry off, loop giữ nguyên. Khi on, `stream.next()` chạy trong `select!` và **bị
cancel mỗi 250 ms** (`crates/core/src/blobs/receive.rs:293`). Doc khen điều này là
"preserve the original hot loop", nhưng mặt kia là: mọi benchmark P1 sẽ chạy loop không
giống loop người dùng chạy.

Thêm nữa cancel-safety của stream từ `store.remote().fetch(...).stream()` chưa được xác
nhận ở đâu cả — nếu nó không cancel-safe thì telemetry-on có thể mất/lệch progress. Cần một
dòng khẳng định (hoặc test) chứ không nên để ngầm định.

## 5. Vài kết luận kỹ thuật trong doc không vững

### Bảng window ceiling (plan dòng 179–183)

Đúng về số học nhưng dẫn tới kết luận sai lệch:

- LAN/Wi-Fi RTT 2–5 ms ⇒ 8 MiB cho ceiling 1,6–4 GB/s. Window **không thể** là nút thắt
  trên LAN/AOA. Đề mục "LAN/AOA: giữ window hiện tại" là đúng nhưng nên nói thẳng là *đã
  loại trừ*, khỏi tốn benchmark.
- Trên relay, cap thực tế là băng thông + rate limit của relay server (thường vài MB/s đến
  vài chục), không phải flow control. Đuổi theo "168 MB/s @100 ms" là mục tiêu ảo. Thử
  32 MiB window trên Android chỉ đổi thêm RAM và bufferbloat, trừ khi self-host relay.

### `send_window = 8 × stream window`

`crates/app/src/quic_keepalive.rs:61` — lý do ghi trong comment ("để bên serve không thành
nút thắt mới") không đúng với kiến trúc hiện tại. Transfer chạy trên **một** stream, nên
`MAX_STREAM_DATA` luôn là ràng buộc chặt hơn; `send_window` connection-level chỉ có ý nghĩa
khi có nhiều stream. Vô hại nhưng lý do là sai — và nó sẽ trở thành đúng nếu P4 làm parallel
children, lúc đó mới nên viết như vậy.

### `opt-level = "z"` → CPU bottleneck (plan dòng 47, 88–93)

Cơ chế trình bày trong doc đáng nghi:

- Trên aarch64, core hot của `blake3` là NEON compile bằng `cc` (build script), **không
  chịu ảnh hưởng opt-level của rustc**. Nếu vậy thì "z tắt loop vectorization làm BLAKE3
  chậm" là sai nguyên nhân; win thực (nếu có) đến từ `ring`/AEAD và code Rust của
  iroh-blobs.
- Con số duy nhất doc dựa vào (28 vs 747 MB/s) là **opt 0 vs 3**, không phải **z vs 3**.
  Không thể ngoại suy.
- `[profile.release.package."*"]` có áp cho `wisp-core`/`wisp-app` (chúng là path dep,
  không phải member của workspace `flutter/rust`) — chỗ này ổn. Nhưng generic/`#[inline]`
  từ core/app được monomorphize **trong** `wisp_bridge`, tức compile ở `z`. Không rõ mức
  ảnh hưởng ⇒ lại cần đo, không nên tuyên bố "hoàn tất".

Cách rẻ nhất: một bench blake3 + AEAD on-device, z vs 3, trước khi ghi công cho fix này.

## 6. Thiếu sót — những thứ ảnh hưởng lớn hơn tất cả P2/P3

- **Thời gian lên direct path và % byte đi qua relay.** Đây là biến 10× thật sự ngoài thực
  địa: một transfer chạy hết đời trên relay vì hole punch fail thì mọi tuning window/CC đều
  vô nghĩa. Telemetry đã log `path` mỗi sample nên derive được, nhưng plan không coi nó là
  metric hạng nhất — cần `time_to_direct_ms` và `relay_bytes_ratio` trong tiêu chí nghiệm
  thu.
- **Không có baseline tuyệt đối.** Tiêu chí "p10 ≥ 70–80% median" là tiêu chí *độ mượt*:
  một transfer chậm đều tay vẫn pass. Không có iperf3 / raw QUIC echo per path thì không
  bao giờ biết mình đang ở 30% hay 90% khả năng của link. Đây là thiếu sót nghiêm trọng của
  ma trận benchmark.
- **Phía sender bị bỏ trống.** Với Android send, kết luận trước đây là nút thắt ở SAF read
  khi pick/copy — plan chỉ có một dòng về import concurrency ở P4 và không có target cho
  phase prepare. Toàn bộ telemetry hiện tại là receiver-side.
- **Finalize/export.** Receiver chỉ ghi file ra ở phase finalize, và trên Android save chạy
  background sau `completed`. Nghĩa là "thời gian người dùng cảm nhận" ≠ transfer time.
  Plan nhắc đo export nhưng không có ngưỡng, không có instrumentation.
- **Nhiều file nhỏ.** Mỗi child HashSeq là một round trip; 1000 file trên relay RTT 200 ms
  là ~200 s thuần RTT, không liên quan gì tới bandwidth. Đây phải là scenario riêng với đơn
  vị **files/s**, không phải MB/s — hiện đang bị nhét chung vào P4.
- **iOS không có trong ma trận** dù repo đang ship IPA và iOS có ràng buộc
  network/background riêng.
- **Issue iroh 4286 (single-stream throughput)** được để ở mục tham khảo rồi bị plan bỏ
  qua: P4 nói "chưa striping trước khi số liệu cho thấy pipeline không giữ đầy một stream",
  trong khi issue được dẫn lại nói bản thân single stream là giới hạn. Nếu upstream đã biết
  vậy thì parallel children là thí nghiệm upside cao nhất và rẻ nhất, không nên xếp cuối.

## 7. Ma trận benchmark hiện tại không chạy được trong thực tế

3 path × 2 device × nhiều biến thể × ≥5 lần × file 4–8 GiB = hàng chục giờ transfer thủ
công trên điện thoại, cộng wear flash và cần 8–16 GiB trống mỗi máy. Thêm nữa:

- Không có yêu cầu cooldown giữa các lần chạy ⇒ thermal throttling sẽ trộn vào kết quả, và
  doc chỉ đo nhiệt độ đầu/cuối.
- Median của n=5 quá yếu để phân biệt hai config chênh 10–15%.
- Không có harness: hiện không tồn tại lệnh benchmark nào, và không có script nào parse log
  telemetry ra p10/CoV. Không có hai thứ đó thì P1 sẽ không bao giờ được thực hiện.

Đề xuất đổi hình dạng: A/B chính chạy **desktop↔desktop qua netem/clumsy** (giả lập RTT +
loss, tái lập được, chạy được hàng chục lần), điện thoại chỉ dùng để *xác nhận* 1–2 config
thắng cuộc; file 1–2 GiB cho A/B, soak dài riêng cho thermal.

## 8. Mâu thuẫn và nit

- "Không dùng unbounded channel cho progress tần suất cao" (plan dòng 218) — code **vẫn**
  dùng `mpsc::unbounded_channel` (`crates/core/src/blobs/receive.rs:358`). Thực tế ổn vì
  coalescer đã chặn ở nguồn, nhưng doc nên ghi đúng: "giữ unbounded, mitigation là coalesce
  tại nguồn", chứ không list như luật đang tuân thủ.
- Mục tiêu "hạn chế burst/pause" vs cơ chế thực: coalescer chỉ đặt **trần** 10 Hz, không có
  **sàn**. Khi stall, không có progress item ⇒ không emit gì ⇒ UI đứng số và speed/ETA
  không giảm. Muốn UI mượt cần heartbeat tick phía consumer — plan không có.
- "Median throughput" và "p50 throughput" (plan dòng 133–135) là cùng một thứ.
- Lệnh kiểm tra (plan dòng 224–230) thiếu `cargo test --workspace` và toàn bộ phía Flutter
  (`flutter analyze`, `flutter test`).
- Ghi `noq 0.17.0` / `noq-proto 0.16.0` trong phần version lock nhưng không nói quan hệ với
  quinn/iroh, người đọc sau sẽ không hiểu tại sao upgrade iroh có thể đảo kết quả
  CUBIC/BBR.

## Tóm lại

Doc mạnh ở phần *kỷ luật* (không tăng window bừa, không chuyển BBR theo cảm tính, giữ
hash/atomic record) và phần telemetry app-level (bytes/s, stall, udp_rx delta) là đúng
hướng. Ba việc cần sửa trước khi đi tiếp:

1. Hạ P0 từ "hoàn tất" xuống "chưa kiểm chứng", và đo `record.json`-per-16KiB để biết win
   thật ở đâu.
2. Sửa cách hiểu telemetry: cwnd/loss phía receiver không mô tả luồng bulk; thêm
   `stream_receive_window` vào sample để tự trả lời câu hỏi window-bound; tách warm-up khỏi
   stall; đừng gán `outcome=complete` cho stream cạn không có `Done`.
3. Thêm baseline tuyệt đối (iperf3/QUIC echo) + metric time-to-direct/relay-ratio, và đổi
   ma trận benchmark sang netem trên desktop — nếu không, P1 sẽ không bao giờ chạy và
   P2/P3 sẽ lại là bet.

# Vòng 2 — 2026-08-14

Cơ sở đối chiếu: plan bản 2026-08-14, commit `3b6aff0` và các thay đổi chưa commit
trong working tree (`tools/analyze_transfer_telemetry.py`, `crates/server/src/lib.rs`,
`flutter/android/app/build.gradle.kts`).

## Đã xử lý tốt

A1–A4 có code thật, không chỉ là chữ trong doc: `local_*` prefix cho stats phía
receiver, provider sampler riêng có cwnd/loss/UDP TX đúng chiều, `blob_config` log
window/CC/build profile, `time_to_first_byte` tách khỏi stall, `None` → `Failed`,
sampler task thay `select!` quanh `stream.next()` (`crates/core/src/blobs/receive.rs:377`).
Analyzer có cửa sổ 1 s trọng số theo `sample_ms`, tách role, `measurement_valid`.
Ba đề xuất chính của vòng 1 (hạ trạng thái P0, baseline tuyệt đối, netem-first) đều
đã vào plan.

## V1. `benchmark_run_id` không phải là ẩn danh

`crates/core/src/blobs/telemetry.rs:35`

`u64::from_str_radix(session_id, 16)` là phép biến đổi 1-1 thuận nghịch của session
ID, chỉ đổi cơ số. Comment biện minh đúng cho một mục tiêu khác (chặn log-injection
chuỗi do peer điều khiển), nhưng plan gọi nó là "anonymous run ID" và smoke check ở
§5 ("`0` match path/peer/session ID") không thể phát hiện điều này vì đang so chuỗi
hex với số thập phân. Kiểm tra đó cho cảm giác an toàn giả.

Repo đã làm đúng cách này ở chỗ khác: `crates/server/src/lib.rs` dùng
`blake3::keyed_hash` với key random per-process cho `client_label`. Hai chỗ đang
không nhất quán. Hoặc dùng cùng kỹ thuật, hoặc bỏ chữ "anonymous" và nói thẳng đây
là session ID dạng số.

## V2. A3 đánh đổi độ phân giải thời gian, và bias là một chiều

`crates/core/src/blobs/telemetry.rs:239`

`observe_progress` giờ chỉ được gọi tại tick 250 ms, nên `first_progress_at` và
`last_progress_at` là **thời điểm tick**, không phải thời điểm byte đến. Hệ quả:

- Stall bị **đo thiếu** tới 250 ms, không phải sai số hai chiều. Một stall thật
  600 ms có thể đo ra 350 ms và không bao giờ vượt ngưỡng 500 ms. Ngưỡng phát hiện
  thực tế là khoảng 750 ms — tức tiêu chí "không có mid-transfer stall trên 500 ms"
  ở §10 đang không đo được thứ nó tuyên bố.
- `time_to_first_byte_ms` bị cộng thêm tới 250 ms. Với transfer ngắn hơn 250 ms,
  TTFB xấp xỉ toàn bộ thời lượng.

A2 có ghi "sai số lượng tử hóa khoảng một sample" nhưng không nói hướng bias và
không quy ra ngưỡng thực.

Fix rẻ, giữ nguyên tinh thần A3: thêm một `AtomicU64` lưu nanos-since-start của lần
tăng byte gần nhất; download loop ghi cùng lúc với `fetch_max`. Một atomic store nữa,
không await, không timer.

## V3. Provider log window không chi phối transfer

`crates/core/src/blobs/telemetry.rs:390`

Receiver là bên advertise `MAX_STREAM_DATA`, nên `stream_receive_window` của
**receiver** mới là thứ giới hạn bulk. Provider record đang log
`stream_receive_window_bytes` của chính sender dưới đúng tên field đó.

Analyzer chỉ tính `bdp_window_ratio` từ role=receiver nên phép tính đúng — nhưng
người đọc thì nhầm ngay: §5 của plan quote "stream receive window 8 MiB" từ run
Android sender, trong khi receiver là desktop CLI = 16 MiB. Số 8 MiB đó không liên
quan tới transfer.

Đề xuất: ở role=provider đổi tên thành `local_stream_receive_window_bytes` (như đã
làm với `local_cwnd_bytes`); giữ `send_window_bytes` không đổi vì đó mới là field có
nghĩa ở phía sender.

## V4. `known: true` cho giá trị hardcode

`crates/core/src/blobs/receive.rs:36`

`NOQ_DEFAULT_STREAM_RECEIVE_WINDOW_BYTES = 1_250_000` là bản chép tay default của
upstream, comment ghi "noq 0.16" trong khi version lock của plan là `noq 0.17.0`. Nó
vẫn được gắn `known: true` và log `config_known=true`. Đây chính là failure mode mà
A1 muốn diệt: một con số đoán được trình bày như đo được. Nếu noq đổi default,
telemetry sẽ nói dối một cách tự tin.

Kèm theo, chỗ này ghi nhận một hành vi thật đáng đưa vào plan: dial AOA tạo
`QuicTransportConfig::builder()` mới nên **không kế thừa** window 8/16 MiB của
endpoint, rơi về 1.25 MB. Ở RTT dưới 1 ms thì vô hại, nhưng E1 nên ghi rõ thay vì để
người đọc suy ra AOA cũng dùng 8 MiB.

Đề xuất: thay `known: bool` bằng `config_source = measured | assumed_upstream_default`.

## V5. Stall đang active lúc fail bị đổi tên thành "finalization"

`crates/core/src/blobs/telemetry.rs:692`

`finish()` trừ `stall_count` và không cộng `stall_total` cho stall đang mở, gộp toàn
bộ vào `finalization_pause`. Với `outcome=complete` thì đúng. Với `outcome=failed`,
stall đang mở chính là stall gây fail — và nó biến mất khỏi mọi metric stall.
Analyzer lại đặt `measurement_valid = outcome == "complete"`, nên failed run gần như
không phân tích được.

Nên phân biệt theo outcome: complete ⇒ finalization pause; failed/cancelled ⇒ giữ
nguyên là stall.

## V6. `None` → `Failed` là thay đổi hành vi production, không chỉ telemetry

`crates/core/src/blobs/receive.rs:185`

Trước đây stream cạn không có `Done` báo `Completed`; giờ báo `Failed`. Đúng ngữ
nghĩa và đúng theo A2, nhưng nếu tồn tại đường nào trong iroh-blobs kết thúc stream
sau khi đã hoàn tất mà không phát `Done`, những transfer đang thành công sẽ thành
fail. Smoke chỉ phủ một kịch bản. Cần test hoặc trích dẫn hợp đồng API upstream
trước khi coi là an toàn.

## V7. `select!` mới chưa được chứng minh cancel-safe

`crates/core/src/blobs/telemetry.rs:223`

A3 tự đặt luật: "nếu chọn giữ `select!`, phải chứng minh cancel-safe bằng tài liệu
API và integration test". Sampler hiện có `select!` với `path_watcher.updated()`.
`&mut oneshot::Receiver` có tài liệu; `Watcher::updated()` thì chưa thấy ghi nhận và
không có test. Nếu nó mất update khi bị cancel thì `application_path` sai lệch ⇒
`relay_bytes_ratio` sai. Luật của chính plan đang không được áp cho code mới.

## V8. Hai điểm nhỏ hơn

- Provider `udp_tx_bytes_total`/`lost_*_total` cộng dồn delta, mà `delta_from` trả
  về 0 khi path đổi ⇒ tổng bị hụt sau migration, không có cờ báo
  (`crates/core/src/blobs/telemetry.rs:409`).
- `bdp_window_ratio` tính trên sample 250 ms trong khi throughput p10/p50 tính trên
  cửa sổ 1 s (`tools/analyze_transfer_telemetry.py:869`) — hai chỉ số không cùng cơ
  sở thống kê.

## V9. Dữ liệu Android sender đang gợi ý window quá lớn, không phải quá nhỏ

Run ở §5 của plan: khoảng 65 MB payload, 4.813 lost packet ở MTU 1452 ⇒ mất mát cỡ
**10%**; RTT cuối 3,057 giây trên đường **direct Wi-Fi**. RTT 3 giây trên LAN là chữ
ký của bufferbloat/queue overflow, không phải của một link chậm.

Phép tính: ở 1,4 MiB/s với RTT 3 s, BDP khoảng 4,2 MiB. Receiver desktop advertise
16 MiB, sender có send window 64 MiB ⇒ cho phép in-flight khoảng 4× BDP. Sender bơm
đầy queue của AP, RTT nở ra, CUBIC thấy loss, sập cwnd, lặp lại.

Nhưng E1 chỉ đặt duy nhất câu hỏi "có nên **tăng** window không", và
`bdp_window_ratio` khoảng 0,26 sẽ được đọc thành "không window-bound", trong khi câu
chuyện thật là over-buffering.

Cần thêm:

- **E1b:** thử **giảm** window / in-flight cap trên path có RTT inflation.
- `rtt_inflation = rtt_current / rtt_min` trong sample. Đây là dấu hiệu trực tiếp và
  rẻ; hiện chưa log `rtt_min`.

## V10. Con số quyết định nhất đang bị chôn trong một bullet

§5 của plan: raw TCP cùng chiều, cùng link đạt p10 1,298 / median 1,591 MiB/s,
**CV 0,186**. Wisp trên cùng link: p10 **0,0** / median 1,2 MiB/s, **CV 1,62**.

Cùng một link, cùng chiều: TCP ổn định, Wisp không. Và utilization khoảng 89% về
average nghĩa là gần như **không còn headroom throughput** trên path đó. Kết luận rút
ra được ngay: vấn đề còn lại thuần túy là stability, và nó không nằm ở link.

Đây nên là kết luận nổi bật của Phase A chứ không phải một gạch đầu dòng, và nó nên
đổi thứ tự ưu tiên: điều tra delivery gap trước, A/B CUBIC/BBR sau. Hiện E2 đang được
đẩy lên vì "provider-side evidence đã có".

## V11. Phân loại stall thiếu lớp quan trọng nhất cho chính dữ liệu đang có

`tools/analyze_transfer_telemetry.py:663`

`_classify_stall_episodes` chỉ có hai lớp: `transport_active_delivery_gap` và
`transport_idle_stall`. A4 hướng dẫn đọc "UDP tăng, app đứng ⇒ nghi ngờ
store/disk/verify".

Với run có 10% loss, cả 3 gap gần như chắc chắn là head-of-line blocking do
retransmit: `GetProgressItem::Progress(offset)` chỉ tiến khi prefix liền mạch được
verify, nên trong lúc chờ packet mất, UDP vẫn chảy còn offset đứng yên — trông y hệt
"nghẽn sau network". Nếu theo A4 mà đi tối ưu store/disk thì đi nhầm hướng hoàn toàn.

Cần lớp thứ ba `transport_active_loss_recovery`, xác định bằng provider
`lost_packets_delta` trong cùng cửa sổ thời gian (đã join được theo run ID). Plan
cũng nên ghi rõ: gap đầu 7,7 giây **không** có sender loss mới là ngoại lệ đáng điều
tra; hai gap sau thì không.

## V12. Mâu thuẫn trạng thái Gate 1

§5 nói "Gate 1 **chưa đóng**". §11 nói "Gate này **đã đạt** cho Android sender +
desktop receiver direct". Cả hai đều có mệnh đề hedge phía sau, nhưng hai câu tiêu đề
nói ngược nhau. Nên chọn một chỗ làm nguồn sự thật, chỗ kia trỏ tới.

## V13. Ngoài phạm vi plan: `client_ip` tin `X-Forwarded-For` vô điều kiện

`crates/server/src/lib.rs` (chưa commit)

Code lấy entry phải nhất parse được từ XFF mà không kiểm tra peer socket có phải
proxy tin cậy hay không. Comment nêu đúng giả định ("một proxy tin cậy") nhưng không
thực thi nó. Nếu ai đó chạm được cổng server trực tiếp (container port lộ ra, hoặc
deployment không có Caddy), client tự đặt XFF là bypass rate limiter bằng cách xoay
IP giả — và `client_label` trong log trở thành giá trị do attacker điều khiển. Test
`forwarded_for_injected_by_the_client_is_ignored` chỉ chứng minh trường hợp *có*
Caddy append, không phủ trường hợp không có proxy.

Fix: chỉ đọc XFF khi `socket_ip` nằm trong danh sách trusted proxy (hoặc subnet của
compose network), ngược lại dùng socket address.

## Tóm lại vòng 2

Plan giờ đã đúng về phương pháp và code đã làm đúng phần lớn A1–A4. Ba việc nên coi
là chặn Gate 1:

1. **V2** — timestamp bị quantize làm ngưỡng stall 500 ms không đo được.
2. **V11** — thiếu lớp loss-recovery nên A4 sẽ chỉ sai hướng cho chính dữ liệu đang có.
3. **V9** — phải bổ sung giả thuyết over-buffering trước khi Phase E chỉ đi theo
   hướng tăng window.

Ngoài ra **V1** và **V5** nên sửa trước khi có ai dựa vào log để ra quyết định.

# Vòng 3 — 2026-08-14

Cơ sở đối chiếu: plan bản 2026-08-14 sau khi V1–V13 được xử lý, commit `b5820c5`
cộng các thay đổi schema v6 chưa commit.

## Trạng thái Vòng 2

Toàn bộ V1–V13 đã được xử lý: token đổi sang BLAKE3 domain-separated, timestamp
progress publish từ hot loop nên hết quantize 250 ms, failed-terminal giữ nguyên
active stall, provider window đổi tên `local_*`, `config_source` thay cho
`known`, cờ discontinuity khi path migration, lớp `transport_active_loss_recovery`,
E1b cho giả thuyết over-buffering, và Gate 1 chỉ còn một nguồn sự thật ở §11.

## V14. Nguyên nhân gốc của "transport counters không bám payload"

Đây là blocker cuối của Gate 1 và nó có nguyên nhân xác định được từ mã nguồn
upstream, không phải hiện tượng ngẫu nhiên trên thiết bị.

`NetworkSnapshot::capture` chỉ đọc `PathStats` qua
`ConnectionInfo::selected_path()`. Trong iroh 0.97
(`src/endpoint/connection.rs:1252`), hàm đó là
`paths().into_iter().find(|p| p.is_selected())`, còn `is_selected` được đặt trong
`src/socket/remote_map/remote_state/path_watcher.rs:111` bằng
`selected_path == p.remote_addr()` và **chỉ khi** selected-path watcher đang giữ
một giá trị. Hệ quả:

- Không path nào được đánh dấu selected ⇒ `selected_path()` trả `None` ⇒
  `path=unknown`, `stats_available=false`, mọi counter bằng 0. Đây đúng là triệu
  chứng `udp_tx_bytes_total=0` của Android provider.
- Byte đi trên path không được chọn không bao giờ được đếm.
- Mỗi migration làm `delta_from` trả về discontinuity và bỏ trọn một khoảng lấy
  mẫu.

Bản sửa: bổ sung counter phạm vi **connection** từ `ConnectionInfo::stats()`
(`udp_tx`/`udp_rx` cùng frame stats). Chúng đơn điệu cho cả connection, không phụ
thuộc path selection, nên là số byte đáng tin; tỉ số counter path trên counter
connection thành `path_counter_coverage`, tức phép kiểm chứng provenance mà Gate 1
đang thiếu.

Smoke CLI↔CLI 64 MiB trên loopback, `path=direct`, `0` discontinuity, không
migration:

| Counter | Provider | Receiver |
|---|---:|---:|
| Path-scoped | 57.591.003 | 57.590.345 |
| Connection-scoped | 68.938.270 | 68.932.812 |
| Coverage | 83,5% | 83,5% |

Payload 67.108.864 byte. Counter connection ứng với overhead 2,7%, đúng như QUIC
trên UDP. Counter path lại thấp hơn cả payload, nên không thể là số byte UDP đúng.

Điểm đáng chú ý cho plan: vấn đề **rộng hơn** báo cáo thiết bị. Nó không chỉ xảy
ra khi `path=unknown` hoặc khi có migration — counter theo path hụt khoảng 16,5%
ngay trong trường hợp sạch nhất. Vì vậy mọi số loss/cwnd theo path đã ghi trong
plan phải đọc là lower bound gắn với coverage của run đó, kể cả các run trước đây
được coi là sạch.

### Cơ chế của khoảng hụt

Ban đầu tôi cho rằng khoảng hụt đến từ những sample không chọn được path. Dữ liệu
per-sample bác bỏ điều đó: `path=direct`, `path_stats_available=true`,
`path_counter_discontinuity=false` ở **mọi** sample, nhưng gap giữa hai counter
xuất hiện xen kẽ — một số sample gap đúng bằng 0, số khác hụt tới 2,6 MB.

Thêm `path_count` vào sample trả lời dứt điểm: connection giữ tới **4 path** cùng
lúc (giá trị quan sát được: 1, 3, 4 ở cả hai đầu), coverage 85,0%. Đối chiếu
`noq-proto 0.16` (`src/connection/mod.rs:1120-1130`), `stats.udp_tx` và
`path_stats[path_id].udp_tx` được tăng tại cùng chỗ với cùng giá trị, nên hai
chuỗi không mâu thuẫn — counter theo path chỉ đơn giản mô tả **một** path trong số
nhiều path đang được dùng.

Kết luận mạnh hơn phát biểu ban đầu: `PathStats` của selected path về bản chất
không phải mẫu số cho "đã gửi/nhận bao nhiêu byte" trên connection multipath. Đây
không phải lỗi biên cần xử lý, mà là ngữ nghĩa của API. Report vì vậy phân biệt
hai chẩn đoán cùng triệu chứng: `path_count > 1` là multipath, `path_count == 1`
với coverage thấp là có sample không chọn được path.

## V15. `stream_data_blocked` trả lời câu hỏi window-bound trực tiếp

`ConnectionStats.frame_tx`/`frame_rx` (noq-proto 0.16,
`src/connection/stats.rs`) có `stream_data_blocked` và `data_blocked`. Frame
`STREAM_DATA_BLOCKED` chỉ được phát khi bên gửi thực sự có dữ liệu sẵn và bị
`MAX_STREAM_DATA` của bên nhận chặn.

Đây là bằng chứng mạnh hơn hẳn `bdp_window_ratio`: ratio là suy đoán từ throughput
và RTT, còn frame là sự kiện giao thức. Plan nên dùng ratio để tầm soát và dùng
frame để kết luận, thay vì để E1/E1b phụ thuộc vào một tỉ số duy nhất.

Trong smoke trên, hai chỉ số đồng thuận: `0` frame blocked và
`bdp_window_ratio_p90 = 0,0033`.

### Xác minh trên thiết bị thật

Hai chiều 128 MiB direct giữa desktop và Pixel 4 release (Wi-Fi 192.168.1.x),
hash round-trip khớp byte-for-byte:

| | desktop → Android | Android → desktop |
|---|---:|---:|
| Provider `path` | `unknown` | `unknown` |
| `path_count` | 6 | 6 |
| Provider coverage | 0,0% | 11,1% |
| Provider avg theo path | 0,00 MiB/s | 1,96 MiB/s |
| Provider avg theo connection | 11,38 MiB/s | 17,66 MiB/s |

Đây là điểm cần nhấn cho plan: ở chiều Android → desktop, cùng một run và cùng
một bộ counter cho hai con số chênh nhau **9 lần**. 1,96 MiB/s là thứ telemetry
cũ sẽ báo cáo; 17,66 MiB/s là thực tế, và nó khớp với app median 16,4 MiB/s phía
receiver. Nếu Gate 2 chạy trên số cũ, toàn bộ đợt A/B sẽ đo một hiện tượng không
tồn tại.

Ở chiều desktop → Android thì còn dứt khoát hơn: provider coverage 0,0%, tức
schema v5 sẽ không có số liệu mạng nào để báo cáo.

Chênh lệch ở tầng connection cũng có ý nghĩa vật lý và trước đây vô hình: sender
gửi 146,55 MB cho payload 134,22 MB, receiver nhận 138,29 MB — khoảng 8,2 MB là
dữ liệu phải truyền lại, nhất quán với loss/congestion event mà provider ghi nhận
cùng một path discontinuity.

## V16. Payload được rải qua nhiều path, và một phần đi relay dù có direct

Truy tiếp câu hỏi "vì sao selected path chỉ thấy một phần traffic" dẫn tới một
phát hiện không nằm trong bảng giả thuyết của plan.

Cộng gộp counter của **tất cả** path (thay vì chỉ path được chọn) cho:

- `active_path_count` = 3–8 path **đang gửi byte cùng lúc**, không phải chỉ tồn
  tại. Trên loopback là 3–4, trên Wi-Fi thiết bị lên 8.
- Selected path mang 42,6% byte trên dây; phần còn lại nằm trên các path khác.
- **25,7% byte đi qua relay path** trong một run mà receiver báo `path=direct`
  suốt và `relay_bytes_ratio` bằng 0.

Điểm cuối là quan trọng nhất cho D1. Plan đang giả định relay là all-or-nothing:
hoặc lên được direct, hoặc rơi về relay. Thực tế đo được là hai đường chạy **song
song**, và chỉ số `relay_bytes_ratio` hiện tại — gán application byte cho path
được chọn — báo 0% cho đúng run có 25,7% relay. D1 cần đổi sang
`wire_relay_bytes_ratio`.

Với D2 thì tiền đề đổi hẳn: ở tầng path đã có song song sẵn. Trước khi thêm song
song ở tầng stream, cần biết song song hiện có giúp hay hại — nhiều path RTT khác
nhau gây reordering, và ở tầng BAO reordering thành head-of-line blocking, tức
đúng dạng "transport-active delivery gap" mà plan đang truy.

### Hai cái bẫy khi cộng gộp per-path

Cả hai đều bị chính dữ liệu bắt được, nên đáng ghi lại:

1. **Không dedup theo `PathId`.** Một path xuất hiện một lần cho mỗi transport
   addr nó reachable, và mọi entry mang **cùng** counter. Cộng thẳng danh sách
   làm tổng lên 1,70× connection.
2. **Xoá entry của path đã rời list.** Path có thể rời rồi quay lại; quên counter
   cũ thì toàn bộ lịch sử của nó bị tính vào interval nó xuất hiện lại. Lệch
   1,07×.

Có cả hai biện pháp thì all-paths khớp connection 1,0001× trên thiết bị thật —
đó là mức đủ để loss/cwnd theo path thôi là lower bound.

## Còn lại của Gate 1

Boundary iOS nếu iOS nằm trong phạm vi phát hành. Cả hai chiều Android đã xác
minh dưới schema v6, gồm một run vừa có coverage thấp vừa có cờ discontinuity.
