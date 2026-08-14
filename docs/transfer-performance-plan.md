# Kế hoạch tối ưu hiệu năng truyền file

Cập nhật: 2026-08-14
Điều chỉnh theo: Vòng 1 và Vòng 2 của `docs/transfer-performance-plan-review.md`

## 1. Mục tiêu và cách hiểu trạng thái

Mục tiêu của dự án là tận dụng phần lớn năng lực thực của từng đường truyền, giữ
tốc độ ổn định và giảm thời gian người dùng phải chờ từ lúc chọn file đến lúc file
sẵn sàng ở đích. Không đánh đổi resume, xác minh BLAKE3, an toàn đường dẫn hoặc độ
ổn định bộ nhớ để lấy throughput.

Tài liệu dùng ba trạng thái riêng biệt:

- **Đã triển khai:** code đã được merge hoặc có trong working tree.
- **Đã xác minh chức năng:** build/test đúng, chưa chứng minh nhanh hơn.
- **Đã xác minh hiệu năng:** có baseline, A/B tái lập được và số liệu đạt ngưỡng.

Các thay đổi tại commit `8a33818` mới ở trạng thái **đã triển khai và đã xác minh
chức năng**. Chúng là các giả thuyết tối ưu chưa được A/B; không gọi P0 là “hoàn
tất” cho đến khi Phase B quy được tác dụng cho từng thay đổi.

Không thực hiện thêm QUIC window, congestion-control hoặc AOA tuning trước khi
Gate 1 và Gate 2 ở cuối tài liệu đạt.

## 2. Kiến trúc và ràng buộc hiện tại

- Sender import file vào `iroh_blobs::FsStore` bằng `ImportMode::TryReference`,
  tạo collection và blob ticket.
- Transfer là mô hình **pull**: receiver mở kết nối ALPN `iroh-blobs`, gọi
  `store.remote().fetch(connection, ticket).stream()`, dữ liệu bulk chảy từ
  sender sang receiver.
- Một HashSeq và các child hiện được xử lý tuần tự trên một bidirectional QUIC
  stream. Workload nhiều file nhỏ phải được đo riêng vì latency/round-trip có thể
  quan trọng hơn bandwidth.
- Receiver tải vào `.wisp/transfers/<hash>/store`, sau đó export bằng
  `ExportMode::TryReference` khi filesystem hỗ trợ.
- Control và progress dùng stream riêng với payload file.
- App dùng một `iroh::Endpoint` chung để tránh nhiều endpoint cùng identity tranh
  relay slot.
- Progress channel hiện vẫn là `mpsc::unbounded_channel`. Rủi ro backlog được
  giảm bằng coalesce tại nguồn xuống tối đa 10 Hz; plan không tuyên bố đã thay nó
  bằng bounded channel.

Phiên bản đang khóa:

- `iroh 0.97.0`
- `iroh-blobs 0.99.0`
- `noq 0.17.0`
- `noq-proto 0.16.0`
- `tokio 1.50.0`

`noq`/`noq-proto` là QUIC stack bên dưới `iroh`; nâng `iroh` có thể kéo theo thay
đổi congestion control, multipath, path stats và kết quả benchmark dù code Wisp
không đổi.

## 3. Giả thuyết hiện tại, chưa phải kết luận

| Giả thuyết | Bằng chứng hiện có | Phép đo quyết định |
|---|---|---|
| Ghi `record.json` theo mỗi progress item làm nghẽn receiver | Trước P0 có thể đạt hàng nghìn write/s; I/O chạy trong async path | Microbench record write và A/B legacy/checkpoint-only |
| Progress/event storm tạo scheduler/UI pressure | BAO progress có thể phát theo block nhỏ | A/B coalescer-only, đo CPU, queue lag và app throughput |
| `opt-level = "z"` làm chậm runtime transfer trên mobile | Chưa có z-vs-3; số cũ chỉ so opt 0-vs-3 | Bench BLAKE3, AEAD và E2E z-vs-3 trên thiết bị |
| AOA allocation/copy gây GC pause | Có allocation trên hot path nhưng chưa có profile | CPU/GC trace, USB throughput và A/B buffer reuse |
| Không lên được direct path làm throughput giảm lớn nhất ngoài thực địa | Direct/relay chênh năng lực lớn; telemetry đã thấy path | `time_to_direct_ms`, `wire_relay_bytes_ratio`, direct-success rate |
| Connection rải payload qua nhiều path đồng thời, gây reordering/HOL và gửi một phần qua relay dù có direct | Đo được 3–8 path active cùng lúc; selected path chỉ mang 42,6% byte; 25,7% byte đi qua relay path trong một run direct | A/B giới hạn path, đối chiếu CV/delivery gap và `wire_relay_bytes_ratio` |
| Window giới hạn relay | Chỉ có phép tính lý thuyết `window / RTT` | BDP/window ratio, raw relay baseline và A/B tăng window |
| HashSeq tuần tự giới hạn nhiều file nhỏ hoặc một stream | Kiến trúc hiện tại và upstream issue #4286 | files/s, RTT sweep, concurrency/stream A/B |

## 4. P0 đã triển khai nhưng chưa xác minh hiệu năng

### P0.1 Coalesce blob progress

- Giới hạn update từ blob layer sang transfer/application layer ở tối đa 10 Hz.
- Flush byte count cuối trước trạng thái terminal.
- Đã có unit test về rate limit/final flush.
- Chưa biết riêng thay đổi này tăng throughput bao nhiêu.

### P0.2 Throttle và tách record checkpoint khỏi Tokio worker

- Checkpoint tối đa mỗi 1 giây hoặc sau 64 MiB.
- Serialize/write bằng `spawn_blocking`.
- Các checkpoint trạng thái quan trọng vẫn ghi ngay.
- Nằm downstream của coalescer, vì vậy phải A/B riêng để tránh quy công trùng.

### P0.3 Ghi record atomically

- JSON compact, temp file ngẫu nhiên với `create_new`, rồi rename cùng thư mục.
- Đây trước hết là thay đổi tính đúng đắn/an toàn. Đo overhead riêng nhưng không
  bỏ atomic replace để đổi lấy benchmark đẹp hơn.

### P0.4 Release dependency ở `opt-level = 3`

- Bridge crate vẫn ưu tiên kích thước; dependency transfer/crypto được cấu hình
  `opt-level = 3`.
- Không tuyên bố BLAKE3 là nguyên nhân cho đến khi có z-vs-3 trên aarch64; BLAKE3
  NEON có thể được build qua `cc`, còn win thực có thể nằm ở AEAD hoặc code Rust.

## 5. Phase A — Sửa độ tin cậy của telemetry

Phase này chặn mọi quyết định tuning tiếp theo.

Telemetry nền tảng tại commit `afc652f` đã xác minh chức năng nhưng chưa được coi
là measurement-valid cho đến khi A1–A4 hoàn tất.

**Cập nhật 2026-08-14 — commit `7036d47`:** A1–A4 và phần analyzer của A6 đã
được triển khai, format/test/analyze cục bộ đều đạt. Receiver/provider dùng cùng
benchmark correlation token; provider có đúng sender-side cwnd/loss/UDP TX;
window/config được log; warm-up/finalization được tách khỏi mid-transfer stall;
EOF thiếu `Done` là failed; sampler task không cancel download future; path change
force sample và analyzer dùng cửa sổ 1 giây có trọng số. Commit `bb1bb93` bổ sung
A5 phase timing cho core sender/receiver và analyzer schema v3. Commit `ced0e5e`
đo Android SAF URI → app-cache và background save app-cache → SAF/MediaStore,
correlate bằng cùng token; telemetry mobile bị tắt mặc định. Commit
`1fe1c25` nối cùng opt-in flag vào Rust provider telemetry trên mobile, commit
`3b6aff0` xuất riêng target này thành JSON typed không có span context, và commit
`dca51e3` phân loại stall thành transport-active delivery gap, transport-idle hoặc
unknown mà không nới gate ổn định.
Commit `bcbde90` đã thay session base-conversion bằng BLAKE3 pseudonymous token,
publish first/latest progress timestamp từ hot loop và giữ active stall khi failed.
Commit `f978003` ghép provider loss theo token/timeline để tách loss-recovery, tính
RTT inflation và BDP trên cùng cửa sổ 1 giây, đồng thời sửa mẫu số coverage. Commit
`26b4f7c` đổi provider receive window sang tên `local_*`, thêm `config_source`,
không coi default chép từ noq-proto là measured/configured, và đánh dấu path-counter
discontinuity để totals bị hụt không còn bị hiểu là đầy đủ. Analyzer hiện là schema
v5 và vẫn đọc được log schema cũ. Recheck trước device smoke phát hiện Dart phase
emitter còn dùng session hex đổi cơ số trong khi core đã dùng BLAKE3; commit
`b5820c5` đưa token do chính core tạo vào `TransferPlanData`, nên mobile phase và
Rust events không còn có thể lệch thuật toán correlation.

**Cập nhật — transport counter provenance đã có nguyên nhân gốc và bản sửa
(schema v6, chưa commit):** counter mạng trước đây chỉ đọc `PathStats` qua
`ConnectionInfo::selected_path()`. Trong iroh 0.97, `selected_path()` là
`paths().find(|p| p.is_selected())`, và `is_selected` chỉ được đặt khi
selected-path watcher giữ một địa chỉ khớp path đang sống. Vì vậy byte đi qua lúc
không path nào được chọn không được đếm, và mỗi migration làm mất trọn một khoảng
lấy mẫu. Đó là cơ chế đằng sau `udp_tx_bytes_total=0` và `4.261 byte` trên payload
hàng trăm MiB.

Bản sửa thêm counter phạm vi **connection** từ `ConnectionInfo::stats()`
(`udp_tx`/`udp_rx`, cộng frame counter). Chúng đơn điệu cho cả connection, không
phụ thuộc path selection, nên là số byte đáng tin; tỉ số giữa counter path và
counter connection trở thành `path_counter_coverage` — chính là phép kiểm chứng
provenance mà Gate 1 đang thiếu.

Smoke CLI↔CLI 64 MiB (loopback, direct, `0` discontinuity, không migration) cho
thấy vấn đề rộng hơn báo cáo trên thiết bị:

| Counter | Provider | Receiver |
|---|---:|---:|
| Path-scoped | 57.591.003 | 57.590.345 |
| Connection-scoped | 68.938.270 | 68.932.812 |
| `path_counter_coverage` | 83,5% | 83,5% |

Payload là 67.108.864 byte. Counter connection cho overhead khoảng 2,7% — đúng với
QUIC/UDP. Counter path lại **thấp hơn cả payload**, tức không thể là số byte UDP
đúng. Nói cách khác counter path hụt khoảng 16,5% ngay cả trong trường hợp sạch
nhất, chứ không chỉ khi migration hoặc khi `path=unknown`. Mọi con số loss/cwnd
theo path trong tài liệu này phải đọc là lower bound tương ứng với coverage của
run đó.

**Cơ chế của khoảng hụt đã được xác định.** Sample giờ log `path_count`, và smoke
lặp lại cho thấy connection giữ tới **4 path** cùng lúc (`path_count` nhận giá trị
1, 3, 4 ở cả hai đầu), coverage 85,0%. `noq-proto` tăng `stats.udp_tx` và
`path_stats[path_id].udp_tx` tại cùng chỗ với cùng giá trị, nên hai chuỗi không
mâu thuẫn — chỉ là counter theo path mô tả **một** path trong nhiều path đang
được dùng. Vì vậy `PathStats` của selected path về bản chất không phải mẫu số cho
"đã gửi/nhận bao nhiêu", và đây không phải trường hợp biên: nó đúng với mọi
connection multipath, kể cả LAN direct ổn định.

Do đó report phân biệt hai chẩn đoán có cùng triệu chứng: `path_count > 1` nghĩa
là selected path chỉ mang một phần traffic; `path_count == 1` với coverage thấp
nghĩa là có sample không chọn được path nào.

**Smoke thiết bị thật (desktop → Pixel 4 release, Wi-Fi 192.168.1.x, 128 MiB
direct) tái hiện đúng ca xấu nhất của Gate 1 và cho thấy bản sửa xử lý được nó:**

| | Provider (desktop) | Receiver (Android) |
|---|---:|---:|
| `path` báo cáo | `unknown` | direct |
| `path_count` | 6 | 6 |
| Path-scoped total | **0** | 6.278.860 |
| Connection-scoped total | **137.971.390** | 137.969.068 |
| Coverage | **0,0%** | 4,6% |

Payload 134.217.728 byte. Provider chính là triệu chứng đã báo cáo trước đây —
`path=unknown` và path counter bằng 0 — nên với schema v5 run này sẽ không có số
liệu mạng nào dùng được. Với schema v6, counter connection ghi 137,97 MB ở cả hai
đầu (lệch nhau 2.322 byte, overhead 2,8% so với payload), trung bình 11,93 MB/s
khớp với app median 11,7 MiB/s phía receiver. Receiver `outcome=complete`, tức
xác minh BLAKE3 đạt. `stream_data_blocked` bằng 0 ở cả hai đầu, nên run này
**không** window-bound — đo được chứ không phải giả định.

Kết luận: byte accounting không còn phụ thuộc vào việc iroh có chọn được path hay
không. Số theo path vẫn giữ nguyên nhưng luôn đi kèm coverage để không bị đọc
nhầm là toàn bộ traffic.

**Chiều ngược lại (Android release provider → desktop receiver, 128 MiB direct)
cho thấy hậu quả định lượng của lỗi cũ.** File dùng chính payload nhận được ở
chiều trước, và hash round-trip desktop → Android → desktop khớp byte-for-byte.

| | Android provider | Desktop receiver |
|---|---:|---:|
| `path` báo cáo | `unknown` | direct |
| `path_count` | 6 | 6 |
| Path-scoped total | 16.298.505 | — |
| Connection-scoped total | 146.552.381 | 138.288.810 |
| Coverage | 11,1% | 21,0% |
| Throughput trung bình | **1,96 MiB/s** theo path | — |
| | **17,66 MiB/s** theo connection | |

Cùng một run, cùng một counter, hai cách đọc chênh nhau **9 lần**. Con số theo
path (1,96 MiB/s) là thứ telemetry cũ sẽ báo cáo, và nó sẽ dẫn cả đợt điều tra
hiệu năng đi truy một hiện tượng không tồn tại. Con số theo connection
(17,66 MiB/s) khớp với app median 16,4 MiB/s phía receiver.

Chênh lệch sender/receiver ở tầng connection cũng có ý nghĩa vật lý: sender gửi
146,55 MB cho payload 134,22 MB (overhead 9,2%), receiver nhận 138,29 MB
(overhead 3,0%). Khoảng 8,2 MB chênh là dữ liệu mất/phải truyền lại — nhất quán
với loss và congestion event mà provider ghi nhận, cộng một path discontinuity.
Trước đây toàn bộ bức tranh này vô hình. `stream_data_blocked` bằng 0 ở cả hai
đầu nên run này cũng không window-bound.

Bản sửa cũng log `stream_data_blocked` (tx ở provider, rx ở receiver). Frame này
có nghĩa "bên gửi có dữ liệu sẵn và bị `MAX_STREAM_DATA` của bên nhận chặn", nên
nó trả lời câu hỏi window-bound một cách trực tiếp thay vì suy đoán từ
`bdp_window_ratio`. Trong smoke trên cả hai chỉ số đồng thuận: `0` frame blocked và
`bdp_window_ratio_p90 = 0,0033`.

Gate 1 **đã đóng cho desktop và Android**. Emission/schema v6, correlation token,
phase timing, provenance counter và cờ discontinuity đều đã xác minh trên release
ở cả hai chiều. iOS được tách thành hạng mục riêng, không còn chặn Gate 1; xem
§11.

**Smoke thực tế ngày 2026-08-14:**

- CLI debug ↔ CLI debug qua rendezvous/loopback truyền 128 MiB, SHA-256 khớp;
  analyzer schema v3 ghép receiver/provider/13 phase, `0` malformed. Kết quả này
  chỉ xác nhận schema, không phải baseline hiệu năng vì là debug + loopback.
- Desktop CLI debug → Pixel 4 Android release truyền direct 64 MiB, SHA-256 khớp;
  provider ghi 60 sample trong 14.481 giây, 2 packet loss/congestion event lúc
  đầu và mobile `background_save=171 ms`; analyzer ghép 9 phase theo pseudonymous
  run ID, `0` malformed. UI báo khoảng 4,4 MB/s, provider UDP TX trung bình khoảng
  4,2 MiB/s. Không dùng số này làm baseline vì sender là debug, file synthetic,
  chỉ có một run và chưa có iperf3 cùng phiên.
- Android release sender → desktop CLI debug receiver đã truyền direct 64 MiB hai
  lần với SHA-256 khớp. `saf_read_copy` chỉ 71–106 ms, trong khi `fetch_store`
  mất 46,854–59,955 giây; SAF/cache không phải bottleneck của workload file lớn
  hiện tại. Run có đầy đủ provider JSON ghi 190 sample/46,417 giây, UDP TX trung
  bình khoảng 1,4 MiB/s, 4.813 packet/6.988.224 byte loss, 71 congestion event,
  RTT cuối khoảng 3,057 giây, CUBIC, MTU 1452, provider-local receive window 8 MiB
  và send window 64 MiB; local receive window này không chi phối bulk gửi đi.
  Receiver có p10 0,0 MiB/s, median 1,2 MiB/s, CV 1,62 và 3 stall;
  analyzer ghép 14 phase, `0` malformed và `--fail-on-unstable` fail đúng kỳ vọng.
  Trong từng gap app bytes đứng nhưng receiver vẫn nhận khoảng 4,6–12,1 MB UDP.
  Đối chiếu timeline
  provider/receiver (terminal lệch chỉ 35 ms) cho thấy gap đầu dài khoảng 7,7 giây
  không có sender loss/congestion, còn gap 2/3 lần lượt trùng 2.877/1.532 packet loss
  và 37/28 congestion event. Re-analysis bằng `f978003`/schema v4 đổi đúng hai gap
  sau thành `transport_active_loss_recovery`; chỉ gap đầu còn là delivery gap chưa
  giải thích. Schema v5 đọc lại cùng log vẫn cho `0` malformed, hai loss-recovery,
  một delivery-gap, receiver/provider terminal lệch 35 ms và coverage 1,0.
- Smoke schema v5 Android release sender → desktop CLI debug receiver truyền direct
  85.540.428 byte trong 152,7 giây và SHA-256 file đầu ra khớp. Android
  provider phát `config_source=configured`, tên window local đúng semantics,
  `path_counter_discontinuity=false` trên sample và count `0` trong summary.
  Dart `saf_read_copy=126 ms`, Rust provider và desktop receiver cùng token
  `benchmark_run_id`; analyzer ghép đúng 14 phase, 614 provider sample và `0`
  malformed. Run này có 5 stall loss-recovery, 8 delivery gap, 5.513 packet loss,
  104 congestion event, p10 `0`, p50 khoảng 0,3 MiB/s và 80,1 giây stall.
  RTT QUIC cuối ở provider/receiver lên khoảng 7,5/8,1 giây, nên chỉ dùng
  để nghiệm thu schema/correlation, không dùng làm baseline hay tuning input.
- Sau khi chuyển Pixel sang `TienNA 5G`, hai smoke 256 MiB random mới đều hoàn
  tất direct và SHA-256 `0994D6F7...DC84BA9` khớp. Desktop → Android mất
  12,903 giây, trung bình 19,84 MiB/s, p10/p50 17,7/21,1 MiB/s, CV 0,187,
  p10/p50 84,2% và không stall. Android → desktop mất 12,252 giây, trung bình
  20,89 MiB/s, p10/p50 15,5/21,6 MiB/s, CV 0,232, p10/p50 71,6% và không
  stall. `saf_read_copy=250 ms`, Android `background_save=471 ms`; control
  handshake chỉ 24–37 ms thay vì timeout 30 giây ở topology cũ. Đây là bằng
  chứng mạnh rằng batch Wi-Fi 4/cross-band trước bị topology chi phối, nhưng vẫn
  chưa là baseline tuyệt đối vì thiếu iperf3 bracket, host BSSID và release build
  ở cả hai đầu.
- Receiver core trên Android release đã phát schema v5 đầy đủ: config thực,
  sample/summary, correlation token, phase Rust/Dart và `0` malformed. Tuy nhiên
  hai run 256 MiB phát hiện provider telemetry chưa measurement-valid cho bulk:
  Android provider báo `path=unknown`, `udp_tx_bytes_total=0`; desktop provider
  chỉ đếm 4.261 byte dù payload 256 MiB. Receiver selected-path counters cũng
  under-account payload và run desktop → Android có hai counter discontinuity.
  Vì vậy app throughput, phase và stall của hai run vẫn dùng được, còn provider
  loss/cwnd/UDP, loss attribution và network totals phải coi là unavailable/lower
  bound cho tới khi chứng minh sampler đang giữ đúng payload connection hoặc
  analyzer hạ validity theo coverage.
- Follow-up có raw bracket hợp lệ cho chiều desktop → Android: `.NET TcpClient`
  buffer 1 MiB gửi 256 MiB vào Android `toybox nc >/dev/null`. Ba run trước đạt
  29,97/30,19/30,99 MiB/s, ba run sau đạt 26,86/31,44/28,97 MiB/s; median
  trước/sau 30,19/28,97 MiB/s, drift 4,0%. Wisp ở giữa dùng một file random mới,
  hash khớp, nhưng path chuyển direct → relay → direct → relay và 95,0% payload
  bị tính vào relay. Run chỉ đạt trung bình 11,79 MiB/s, p10/p50 gần
  0/14,22 MiB/s, CV 0,460, một transport-idle stall 3.529 ms; utilization theo
  mean-of-medians raw 29,58 MiB/s chỉ 39,9% average và 48,1% ở p50. Provider
  lần này bám được bulk, đếm 251,2 MiB UDP TX, 16 packet loss, 4 congestion event
  và 3 discontinuity; receiver thấy 5 discontinuity. Đây là path-migration smoke
  thật chứng minh lower-bound handling hoạt động, đồng thời đưa D1 lên trước mọi
  window/CC A/B.
- Opt-in Rust telemetry trên Android đã được nối từ cùng Dart define. Target này
  được tách khỏi formatter `tracing_android` dạng text và xuất JSON typed không có
  span context; smoke có 201 event, `0` dòng text legacy và `0` match path/peer/
  session ID. Không dùng parser regex cho text dính field vì vừa dễ vỡ vừa có thể
  kéo metadata nhạy cảm từ span.
- Hai sender run đều tái hiện stall dù RSSI/thermal tốt. Dữ liệu khoanh vùng vào
  network/QUIC path sau prepare, nhưng chưa tách được nhiễu/retry Wi-Fi 4 khỏi
  hành vi loss recovery/CUBIC vì chưa có raw baseline ổn định bao quanh batch.
- Baseline tạm thời không cài thêm binary dùng Android `toybox dd | nc` gửi raw
  TCP 128 MiB cùng chiều tới Windows. Hai run đạt 1,526 và 1,556 MiB/s, lệch
  khoảng 2%; run có timer 1 giây đạt p10 1,298, median 1,591 MiB/s, CV 0,186 và
  không có zero-window. Wisp đạt khoảng 89% raw TCP ceiling về average nhưng kém
  ổn định rõ rệt ở application delivery trong phiên đó. Hai raw run mới ngay trước
  batch kế tiếp chỉ đạt 1,119/1,087 MiB/s, CV 0,553/0,444 và có 8/10 cửa sổ 1 giây
  rỗng dù RSSI khoảng -34 dBm. Vì vậy không còn được kết luận “vấn đề không nằm ở
  link”; Wi-Fi 4/2.4 GHz thay đổi mạnh theo thời gian và batch này bị loại khỏi A/B
  tuning. Đây là denominator tạm, chưa thay thế iperf3 vì không có retransmit/cwnd
  và multi-stream report chuẩn. ICMP cùng phiên dao động 7–713 ms, nhưng chỉ dùng
  làm dấu hiệu queue/jitter, không làm throughput baseline vì có thể bị deprioritize.
- Android provider/receiver schema v5 và boundary Dart đã được xác nhận trên
  release; boundary iOS thì chưa. Gate 1 vẫn mở vì transport-counter provenance
  mới phát hiện ở run nhanh, path migration thật và iOS. Hỗ trợ provider-only vẫn
  được giữ cho log chỉ có phía sender và đã được sửa ở commit `7e7b3c2`.

### Kết quả đối chiếu Vòng 2

- V1, V2 và V5 đã sửa ở `bcbde90`, sau đó boundary Dart được đồng bộ ở `b5820c5`:
  token không còn là base-conversion thuận nghịch ở bất kỳ emitter nào, timestamp
  lấy từ hot loop, failed-terminal không làm mất active stall.
- V3, V4 và phần path-counter của V8 đã sửa ở `26b4f7c`: provider window mang
  nghĩa local, nguồn config tách configured/assumed/unknown, migration có cờ
  discontinuity và analyzer coi network totals khi đó là lower bound.
- Phần BDP/coverage của V8 và V11 đã sửa ở `f978003`: throughput/RTT dùng cùng
  cửa sổ 1 giây; provider loss chỉ join khi token duy nhất và terminal timeline
  đủ gần. Discontinuity giao stall làm attribution hạ về `unknown`.
- V6 đã đối chiếu contract `iroh-blobs 0.99.0`; V7 đã đối chiếu tài liệu
  cancel-safety của `n0-watcher 0.6.1` và lưu bằng chứng cạnh `select!`.
- V9 đã thành E1b nhưng chỉ chạy sau raw bracket ổn định; V10 không còn đủ cơ sở
  kết luận “không phải link” vì raw Wi-Fi 4 mới cũng burst/zero-window.
- V12 dùng duy nhất trạng thái Gate 1 ở §11. V13 là vấn đề trust-proxy/security
  ngoài phạm vi performance plan và phải được xử lý bằng commit riêng.

### A1. Đọc đúng chiều của QUIC stats

Receiver là đầu nhận bulk. Tại receiver:

- Dùng được: application bytes, `udp_rx.bytes`, RTT, selected path và current MTU.
- `cwnd`, sent loss và congestion events mô tả chiều gửi cục bộ của receiver,
  chủ yếu ACK/control; chỉ giữ dưới tên `local_*`, không dùng để kết luận về bulk.
- Muốn đánh giá CUBIC/BBR hoặc payload loss phải thu path stats tại blob provider
  phía sender và gắn log bằng correlation token dùng chung. Từ `bcbde90`, token
  là BLAKE3 domain-separated pseudonym chứ không còn là session ID đổi cơ số.

Bổ sung vào mỗi sample/config record:

- `stream_receive_window_bytes`, `connection_receive_window_bytes` và
  `send_window_bytes` thực tế ở receiver. Provider phải prefix receive-side field
  bằng `local_` để không bị đọc nhầm là flow-control credit của receiver.
- Congestion controller và build profile đang dùng.
- `config_source=measured|configured|assumed_upstream_default|unknown`; giá trị
  chép tay từ noq-proto không được gắn `known=true`.
- `role=receiver|provider` để parser không trộn semantics hai phía.
- Counter phạm vi connection từ `ConnectionInfo::stats()`:
  `connection_udp_tx_bytes_delta`/`connection_udp_rx_bytes_delta` cùng
  `connection_stats_available`. Đây là số byte đáng tin; counter theo path chỉ là
  lower bound và phải luôn đọc kèm `path_counter_coverage`.
- `path_count` và `active_path_count`: số path connection đang giữ, và số path
  thực sự có byte trong sample đó. Lớn hơn 1 nghĩa là counter theo selected path
  chỉ mô tả một phần traffic — khác hẳn với trường hợp không chọn được path nào.
- Aggregate theo **tất cả** path: `all_paths_udp_tx/rx_bytes_delta`,
  `all_paths_lost_packets_delta`. Cộng gộp per-`PathId` nên loss không còn là
  lower bound. Hai điều kiện bắt buộc để con số này đúng: dedup theo `PathId`
  trong từng snapshot (một path có thể xuất hiện một lần cho mỗi transport addr,
  cùng counter), và giữ lại entry của path đã rời list (path có thể quay lại).
  Bỏ một trong hai làm tổng lệch 1,07× và 1,70× so với connection; có cả hai thì
  khớp 1,0001×.
- `direct_path_udp_bytes_delta` / `relay_path_udp_bytes_delta` /
  `aoa_path_udp_bytes_delta`: byte trên dây theo loại path đã mang chúng. Đây là
  cách duy nhất trả lời "bao nhiêu đi qua relay" trên connection multipath.
- `stream_data_blocked_tx/rx` và `data_blocked_tx`. Đây là bằng chứng trực tiếp về
  flow control: bên gửi có dữ liệu sẵn và bị window của bên nhận chặn.

Tính chỉ số chẩn đoán:

```text
bdp_window_ratio = app_bytes_per_sec * rtt_seconds / stream_receive_window_bytes
```

`bdp_window_ratio >= 0,8` kéo dài chỉ là tín hiệu **có thể window-bound**. Tính ratio
từ cùng cửa sổ throughput 1 giây, không trộn app rate 250 ms với percentile 1 giây.
Từ schema v6, `stream_data_blocked` là bằng chứng mạnh hơn hẳn: khác với một tỉ số
suy đoán, frame này chỉ được phát khi bên gửi thực sự bị window chặn. Ratio dùng để
tầm soát; frame dùng để kết luận.
Chỉ xác nhận khi đổi window làm throughput thay đổi tái lập được và raw path còn
headroom. Đồng thời tính `rtt_inflation = current_rtt / min_rtt` để phát hiện queue.

### A2. Sửa định nghĩa warm-up, stall và terminal outcome

- Đo `time_to_first_byte_ms` riêng; thời gian từ connect đến byte đầu không phải
  mid-transfer stall.
- Chỉ kích hoạt stall detector sau lần tăng byte đầu tiên và kết thúc khi nhận
  `GetProgressItem::Done` tường minh.
- Stream kết thúc bằng `None` mà chưa có `Done` phải là `failed/incomplete`, không
  ghi `outcome=complete`.
- Byte counter và timestamp lần tăng đầu/cuối phải được publish từ download loop.
  Nếu chỉ gắn thời điểm ở sampler 250 ms thì TTFB bị đo thừa và stall bị đo thiếu
  một chiều tới 250 ms; stall thật khoảng 600 ms có thể lọt ngưỡng 500 ms.
- Phân biệt `stall_count`, `stall_total_ms`, `longest_stall_ms` với warm-up và
  finalization pause. Stall đang mở khi `complete` là finalization; khi `failed`
  phải giữ là failure stall, không trừ khỏi summary.
- Hợp đồng `iroh-blobs 0.99.0` xác nhận `Done` là completed, `Error` là closed but
  incomplete, và `GetProgress::complete()` đổi stream đóng không có result thành
  `LocalFailure`; vì vậy `None -> Failed` là đúng semantics upstream và có unit test.

### A3. Loại observer effect khỏi download loop

Telemetry-on không được liên tục cancel `stream.next()` mỗi 250 ms.

Thiết kế ưu tiên:

- Download loop ở cả production và benchmark tiếp tục dùng `stream.next().await`.
- Khi telemetry bật, loop chỉ cập nhật byte counter nguyên tử.
- Một sampler task tùy chọn đọc byte counter và `ConnectionInfo` mỗi 250 ms; task
  có stop/final flush rõ ràng và không có channel dữ liệu không giới hạn.
- `n0-watcher 0.6.1::Watcher::updated()` được upstream ghi rõ cancel-safe; lưu bằng
  chứng version/source này cạnh `select!` và test path-change accounting.
- Nếu chọn giữ `select!`, phải chứng minh `next()` cancel-safe bằng tài liệu API và
  integration test; nếu không có bằng chứng thì không dùng thiết kế đó.

### A4. Metric path và phân loại nghẽn

- `time_to_direct_ms`: từ lúc bắt đầu dial/transfer đến khi selected path lần đầu
  là direct; ghi `never_direct=true` nếu không đạt.
- `relay_bytes_ratio`: tổng application byte delta khi selected path là relay chia
  tổng transferred bytes.
- `direct_bytes_ratio` và số lần path migration.
- So sánh `udp_rx_bytes_delta` với `app_bytes_delta`:
  - UDP vẫn tăng nhưng app offset đứng nhiều sample liên tiếp và provider có loss
    cùng timeline ⇒ `transport_active_loss_recovery` (HOL/retransmit).
  - UDP vẫn tăng, app đứng nhưng provider không có loss ⇒
    `transport_active_delivery_gap`; mới lúc này điều tra reorder/verify/store/disk.
  - Cả UDP và app cùng đứng ⇒ nghi ngờ sender, path, flow/congestion hoặc upstream
    store read.
- Chỉ join loss khi có đúng một provider match và timeline terminal đủ gần; nếu
  không thì giữ `unknown`/delivery-gap rộng, không khẳng định recovery.
- Đây là heuristic; `GetProgressItem::Progress` là payload prefix đã aggregate nên
  packet ngoài thứ tự có thể làm offset đứng dù UDP vẫn tăng. Luôn kết hợp provider
  stats và phase timing trước khi kết luận.

### A5. Timing đầy đủ theo phase

Instrument cả sender lẫn receiver:

- Walk/metadata, SAF read/copy, import/hash và thời gian đến lúc blob ticket sẵn sàng.
- Handshake, time-to-first-byte và network transfer.
- Receiver store/verify và final export.
- Trên Android/iOS, đo cả thời gian background save đến khi file thật sự sẵn sàng,
  không chỉ thời điểm protocol báo completed.

### A6. Analyzer/harness

- Parser JSONL theo kiểu streaming, nhóm bằng `(source_log, transfer_id)` để nhiều
  process cùng bắt đầu counter từ 1 không bị gộp.
- Giới hạn kích thước file, dòng và số sample; chỉ dùng JSON primitive, không
  `pickle`/`eval`.
- Xuất cả JSON machine-readable và bảng người đọc được.
- Gộp sample 250 ms thành cửa sổ 1 giây không chồng lấn trước khi tính throughput
  p10/p50/p90 và coefficient of variation; dùng `sample_ms` làm trọng số khi tick
  bị trễ.
- Tính low-speed episode, stall, path byte ratio và phase timing; khi path đổi thì
  force sample để giảm lượng byte bị quy nhầm cho path mới.
- Cho phép report provider-only khi mobile release chỉ có boundary phase; vẫn ghép
  phase theo pseudonymous correlation token nhưng không tạo giả receiver
  throughput/stability.
  `--fail-on-unstable` phải fail nếu không có receiver sample hợp lệ.
- Báo path-counter discontinuity khi selected path đổi thay vì im lặng hụt totals;
  `network_stats_coverage` phải chia cho sample count, không chia cho throughput
  window count.
- `tools/analyze_transfer_telemetry.py` hiện phát report schema v5; chỉ coi schema
  đã nghiệm thu sau smoke emission trên các target còn thiếu ở Gate 1.

## 6. Phase B — Dựng baseline và quy công P0

### B1. Baseline tuyệt đối

Mỗi path cần hai mốc:

1. **Link baseline:** iperf3 cho LAN/AOA để biết năng lực TCP/UDP thực tế.
2. **Transport baseline:** raw QUIC echo/source-sink dùng cùng iroh/noq, encryption
   và path với Wisp nhưng không có blob store/hash/export.

Đo thêm baseline local:

- Sequential file read/write trên filesystem thật.
- Hash throughput và AEAD throughput theo đúng release profile/architecture.
- SAF/provider read trên Android; file-provider access trên iOS nếu áp dụng.

Chỉ số hạng nhất:

```text
link_utilization      = app_payload_throughput / link_baseline_throughput
transport_utilization = app_payload_throughput / raw_quic_throughput
```

Ngưỡng ban đầu để điều tra là dưới 70% baseline phù hợp. Ngưỡng release cuối sẽ
được khóa sau khi có dữ liệu theo từng class thiết bị/path; không dùng một con số
chung để che giấu giới hạn disk/CPU của mobile.

### B2. A/B quy công các thay đổi đã land

Tái dựng baseline từ commit trước `8a33818`, rồi tạo các build chỉ khác một biến.
Historical build chỉ chạy trong thư mục benchmark dùng một lần; không dùng nó với
dữ liệu người dùng vì record write cũ không atomic.

| Build | Record format/write | Coalescer | Record checkpoint | Release dependency |
|---|---|---:|---:|---:|
| H — exact historical | pretty/direct write | off | per-progress/blocking in async | z |
| A — controlled baseline | compact/atomic | off | per-progress via blocking pool | z |
| B — record-only | compact/atomic | off | 1 s/64 MiB + blocking pool | z |
| C — coalescer-only | compact/atomic | 10 Hz | per-progress via blocking pool | z |
| D — P0 runtime | compact/atomic | 10 Hz | 1 s/64 MiB + blocking pool | z |
| E — current release | compact/atomic | 10 Hz | 1 s/64 MiB + blocking pool | 3 |

- So H-vs-A chỉ cho biết tổng chênh lệch lịch sử/correctness rewrite; attribution
  coalescer và checkpoint dùng A–D, nơi atomic write được giữ cố định.
- Có microbench direct-write-vs-atomic trong scratch directory để tách overhead
  record format/write mà không tạo production build thiếu an toàn.
- Có microbench feed progress ở 10, 100, 1.000 và khoảng 6.400 event/s để đo CPU,
  số write và scheduler delay.
- Báo cả effect size và confidence interval, không chỉ chọn run nhanh nhất.

Kết quả bắt buộc: bảng attribution cho coalescer, checkpoint và z-vs-3. Nếu một
thay đổi không có win đo được, giữ/bỏ dựa trên lợi ích correctness/complexity chứ
không tiếp tục ghi công hiệu năng cho nó.

## 7. Phase C — Benchmark tái lập được

### C1. Desktop A/B trước

Ưu tiên desktop↔desktop với Linux `netem`; trên Windows dùng công cụ impairment
tương đương như clumsy khi cần. Desktop cho phép chạy nhiều lần, kiểm soát RTT/loss
và tránh thermal/mobile flash làm nhiễu phép đo.

Ma trận tối thiểu:

| Scenario | RTT | Loss | Rate cap | Mục đích |
|---|---:|---:|---:|---|
| Direct LAN | 2–5 ms | 0% | 1 Gbit/s hoặc link thật | CPU/store ceiling |
| Direct WAN giả lập | 50/100/200 ms | 0% | 100 Mbit/s | flow-control/RTT scaling |
| Loss sweep | 50/100 ms | 0,1% và 1% | 100 Mbit/s | CC/recovery stability |
| Relay thật/self-host | RTT đo thực | đo thực | relay thực | relay/path overhead |

- File random 1–2 GiB là mặc định; chọn kích thước để mỗi run đo được ít nhất
  30–60 giây. File lớn hơn chỉ dùng khi link quá nhanh hoặc cho soak.
- Tách warm-cache network benchmark và cold-cache E2E benchmark; không trộn hai
  loại trong cùng thống kê.
- Mỗi variant có ít nhất 1 warm-up và 10 measured runs.
- Chạy A/B xen kẽ hoặc randomized order, không chạy toàn bộ A rồi toàn bộ B.

### C2. Mobile chỉ xác nhận cấu hình thắng

- Android↔Android, desktop↔Android và iOS↔desktop/iOS theo khả năng lab.
- Chỉ xác nhận 1–2 cấu hình đã thắng trên desktop, tối thiểu 5 measured runs mỗi
  class thiết bị.
- Chờ nhiệt độ/clock trở về vùng định trước trước run tiếp theo; ghi thermal state
  theo thời gian, không chỉ đầu/cuối.
- Soak riêng 10–20 phút hoặc file lớn để phát hiện thermal throttling, memory tăng
  tuyến tính và background restriction. Không dùng soak làm run A/B thường ngày.

### C3. Phân tầng Wi-Fi 4/5/6

Có, benchmark phải tính đến thế hệ Wi-Fi, nhưng không dùng nhãn Wi-Fi 4/5/6 hoặc
PHY rate làm baseline throughput. Thế hệ, band và channel width là **stratum**;
iperf3 đo ngay trên cùng cặp thiết bị, cùng chiều và cùng thời điểm mới là mẫu số.

Với mesh, cùng SSID và cùng subnet chưa đủ để coi là cùng môi trường.
`same-node`, `cross-node wired-backhaul` và `cross-node wireless-backhaul` là ba
stratum khác nhau. Run chính để tối ưu phải pin cùng AP node/BSSID và band
nếu lab cho phép; cross-node mesh là track robustness riêng.

Ma trận lab tối thiểu, chỉ chạy hàng mà phần cứng/AP thực sự hỗ trợ:

| Class | Chuẩn | Band | Channel width mục tiêu | Vai trò |
|---|---|---:|---:|---|
| Wi-Fi 4 / 2.4 | 802.11n | 2.4 GHz | 20 MHz; 40 MHz tách riêng | Môi trường phổ biến, dễ nhiễu |
| Wi-Fi 4 / 5 | 802.11n | 5 GHz | 40 MHz | Tách ảnh hưởng band khỏi generation |
| Wi-Fi 5 | 802.11ac | 5 GHz | 80 MHz; 160 MHz tách riêng nếu có | Baseline mobile/desktop hiện đại |
| Wi-Fi 6 | 802.11ax | 5 GHz | 80 MHz | OFDMA/ax nhưng không giả định tải đơn sẽ nhanh hơn |
| Wi-Fi 6E | 802.11ax | 6 GHz | 80/160 MHz, mỗi width một stratum | Tùy chọn khi lab có AP/client 6 GHz |

Mỗi run ghi, nếu OS cho phép:

- Chuẩn 802.11, band/frequency, channel width, negotiated TX/RX PHY, RSSI,
  NSS/MCS, retry/channel utilization; trường không lấy được phải là `unavailable`,
  không suy đoán.
- SSID và BSSID của cả hai endpoint, AP/mesh node ID, kiểu backhaul, client
  isolation/multicast filtering và Windows network profile/firewall. Không pool run
  có BSSID/node khác nhau; BSSID không đọc được phải ghi `unavailable`.
- Model/firmware AP, model/OS client, khoảng cách/vị trí cố định, nguồn điện,
  thermal status và nhiệt độ theo thời gian.
- Selected Wisp path, direct/relay ratio, hướng truyền và vai trò từng thiết bị.

Preflight topology trước mỗi batch:

1. Xác nhận IP/subnet, SSID/BSSID/node và band của cả hai endpoint; invalid run
   nếu roam BSSID/node giữa chừng, trừ khi đó chính là path-migration test.
2. Kiểm tra unicast hai chiều và mDNS/broadcast discovery. mDNS fail nhưng
   unicast pass phải gắn cờ `multicast_blocked`; không quy lỗi cho transfer core.
3. Ping hai chiều chỉ là cảnh báo queue/asymmetry, không là throughput baseline.
   Khi RTT/loss bất đối xứng rõ, phải xác nhận bằng iperf3/raw test hai
   chiều và không chạy A/B QUIC tuning trước khi topology ổn định.
4. Xác nhận Windows network profile/firewall và AP `client isolation`/multicast
   snooping theo cấu hình lab; không tắt security control trong production để che lỗi.

Protocol đo baseline cho từng chiều:

1. Chạy iperf3 TCP một stream cùng chiều Wisp ngay trước batch; tùy chọn thêm 4
   stream để biết link ceiling khác single-flow ceiling bao nhiêu.
2. Chạy lại ngay sau batch. Dùng median before/after làm `link_baseline`; nếu hai
   mốc lệch trên 10% thì coi môi trường thay đổi và chạy lại batch.
3. Tính `link_utilization = app payload throughput / iperf3 throughput`. PHY chỉ
   là metadata/upper bound; không dùng `payload / PHY` để pass/fail.
4. Không pool kết quả khác generation/band/channel width. So sánh p10/p50, CV,
   stall, direct ratio và utilization bên trong từng stratum.

Smoke Pixel 4 hiện tại là một quan sát **không phải baseline**: Wi-Fi 4/2.4 GHz,
2442 MHz, RSSI khoảng -35 đến -41 dBm, PHY TX dao động khoảng 52–117 Mbps và RX
khoảng 130–173 Mbps, thermal status 0; cuối các run pin khoảng 37,9–38,7 °C.
Host-side PHY không lấy được do quyền Windows nên được ghi `unavailable`. Cần
lặp lại bằng release ở cả hai đầu, file đại diện, iperf3 bracket và ít nhất 5
measured runs trong cùng Wi-Fi stratum.

Schema-v5 smoke mới ghi Pixel trên BSSID `c2:49:43:1f:a6:77`, Wi-Fi 4/2.4 GHz
2442 MHz, RSSI -44 dBm, PHY TX/RX 52/117 Mbps. Laptop dùng AX211 trên
Windows profile `TienNA 5G 2`, category `Public`; Pixel dùng SSID `TienNA`, nên
hai endpoint khác SSID/band dù cùng `192.168.1.0/24`. BSSID/PHY host không đọc
được do Windows location permission. Trong lúc transfer, ping Pixel → laptop chỉ
3,1–3,5 ms trong khi laptop → Pixel lên 119–209 ms; mDNS không thấy peer,
Windows → Android rớt control handshake hai lần, còn Android → Windows đi
direct. Đây là batch `cross_band_suspect_mesh_or_ap_asymmetry` và bị loại
khỏi mọi kết luận window/CC.

Controlled retry chuyển Pixel sang SSID `TienNA 5G`, BSSID
`c2:49:43:3f:a6:78`, Wi-Fi 5/5805 MHz, RSSI -44 dBm, negotiated TX/RX
866/780 Mbps và IP `192.168.1.83`. Laptop vẫn ở profile `TienNA 5G 2`; hậu tố
profile Windows không chứng minh SSID/BSSID khác, còn host BSSID vẫn phải ghi
`unavailable`. Ping hai chiều không mất gói; sau warm-up laptop → Pixel giảm từ
178 xuống 2 ms và Pixel → laptop giảm từ 21 xuống khoảng 3 ms. Hai chiều Wisp
256 MiB random đều direct, khoảng 19,84–20,89 MiB/s trung bình, không stall và
hash khớp. Đây là controlled topology smoke xác nhận giả thuyết cross-band/mesh,
nhưng chưa thay iperf3 before/after hay ma trận ít nhất 5 measured runs.

Raw bracket kế tiếp cho chiều desktop → Android giữ median trước/sau trong 4,0%,
nhưng Wisp ở giữa chỉ có direct ratio 5,0% và relay ratio 95,0%, dù BSSID/band
không đổi và RSSI cuối -47 dBm. Do đó topology Wi-Fi cũ giải thích batch rất chậm
ban đầu, nhưng “cùng 5 GHz” chưa đủ bảo đảm Wisp giữ direct path. Phải stratify
thêm selected path và loại run relay khỏi A/B transport tuning cho direct LAN.

mDNS/broadcast vẫn fail hai chiều trên topology mới: desktop `--nearby` tìm 0
receiver, Android cũng báo không có nearby device khi desktop receiver đang sống,
trong khi short-code unicast handshake/transfer thành công. Gắn cờ batch
`multicast_blocked`; điều tra Windows `Public` firewall và mesh multicast
forwarding riêng, không trộn với direct-transfer throughput.

### C4. Workload tách riêng

- **Một file lớn:** MB/s, utilization, p10/p50/p90, CV và stall.
- **Nhiều file nhỏ:** ví dụ 1.000 file ở các bucket 4/64/1.024 KiB; đo files/s,
  total completion time và round trips. Không đánh giá scenario này chỉ bằng MB/s.
- **Prepare/SAF:** time-to-ticket và effective source-read MB/s.
- **Finalize/export:** time-to-file-ready và effective export/write MB/s.
- **Path establishment:** direct-success rate, time-to-direct và relay byte ratio.

## 8. Phase D — Thứ tự thử nghiệm sau khi có dữ liệu

### D1. Direct-path reliability trước

Nếu `relay_bytes_ratio` cao hoặc `never_direct` thường xuyên, ưu tiên discovery,
hole punching, address freshness và path migration. Một transfer đi sai path có
thể chậm hơn mọi micro-tuning cộng lại.

Smoke 2026-08-14 trên cùng subnet cho thấy Windows browse không thấy mDNS/broadcast,
nhưng gói WSPD unicast trực tiếp tới Pixel nhận reply hợp lệ và transfer qua code
đi direct. Receiver/responder vẫn sống; nhánh cần điều tra là multicast/broadcast,
Windows firewall/AP isolation và cách lấy địa chỉ peer an toàn để targeted unicast.
Không quét mù toàn bộ `/24` trong production.

Smoke schema v5 ban đầu cho thấy Windows → Android timeout
`control_handshake`/`LastOpenPath`, còn Android → Windows hoàn tất direct. Sau
khi Pixel chuyển từ Wi-Fi 4/2.4 GHz sang `TienNA 5G`, handshake hai chiều còn
24–37 ms và hai transfer 256 MiB đạt khoảng 20 MiB/s không stall. Vì vậy cố định
band/node/BSSID và kiểm topology vẫn đứng trước D2/E1/E2. mDNS vẫn fail dù
short-code direct pass, nên discovery/firewall là nhánh D1 độc lập chứ không phải
bằng chứng transport core chậm.

Bracket raw/Wisp/raw sau đó cô lập rõ hơn: raw TCP vẫn khoảng 29,58 MiB/s theo
mean-of-medians, còn Wisp migration ba lần rồi gửi 95,0% payload qua relay, đạt
11,79 MiB/s average và có stall 3.529 ms. Đây là quyết định D1: tìm vì sao direct
path rơi về relay trên cùng BSSID trước khi thử parallel stream, receive window,
MTU hoặc congestion controller.

**Cập nhật quan trọng — relay không phải hiện tượng all-or-nothing.** Trước đây
D1 chỉ nhìn `relay_bytes_ratio`, tức application byte gán cho path **được chọn**.
Schema v6 đo byte trên dây theo từng loại path và cho kết quả khác hẳn: một run
desktop → Android mà receiver báo `path=direct` suốt, `relay_bytes_ratio` bằng 0,
vẫn có **25,7% byte đi qua relay path**, còn selected path chỉ mang 42,6%. Nói
cách khác đường direct và đường relay chạy **song song**, không phải cái này thay
cái kia.

Hệ quả cho D1: câu hỏi không còn là "có lên được direct không" mà là "bao nhiêu
phần trăm thực sự đi direct". Dùng `wire_relay_bytes_ratio` làm chỉ số, không
dùng `relay_bytes_ratio` — chỉ số cũ báo 0% cho đúng run có 25,7% relay.

### D2. Parallel child/stream experiment

Đưa thí nghiệm này lên trước AOA và QUIC tuning tổng quát vì upstream issue #4286
đã chỉ ra rủi ro single-stream throughput.

**Tiền đề đã đổi:** đo được 3–8 path gửi byte đồng thời, nên ở tầng path đã có
song song sẵn, không chủ ý. Trước khi thêm song song ở tầng stream/child, phải
biết song song hiện có đang giúp hay đang hại: nhiều path với RTT/loss khác nhau
gây reordering, và ở tầng BAO thì reordering thành head-of-line blocking — đúng
dạng "transport-active delivery gap" đang săn. Thí nghiệm rẻ nhất là giới hạn số
path và so CV/delivery gap/`wire_relay_bytes_ratio`, chạy trước mọi thay đổi
protocol.

- Với nhiều file nhỏ: A/B concurrency 1/2/4/8, có bounded queue và giới hạn memory.
- Với một file lớn: spike riêng 1-vs-2/4 stream chỉ trên benchmark branch để kiểm
  chứng single-stream ceiling; chưa thay protocol production trước khi có win rõ.
- Đánh giá files/s, throughput, CPU, memory, fairness, cancel/resume và hash verify.
- Khi có nhiều stream, mới đánh giá lại connection-level `send_window`; với một
  stream, `MAX_STREAM_DATA` là ràng buộc chặt hơn và lý do “8× để serve không nghẽn”
  chưa đủ cơ sở.

### D3. Sender prepare và mobile provider

- Đưa walk/metadata blocking I/O ra blocking pool nếu profile xác nhận chặn runtime.
- Import/hash với bounded concurrency, bắt đầu 2–4 mobile và 4–8 desktop.
- Profile SAF read/copy trên Android và provider access trên iOS trước khi tăng
  network concurrency.
- Duy trì `TryReference` và consistency của source trong thời gian serve.

Với file Android 64 MiB hiện tại, SAF copy 71–106 ms tương đương khoảng
604–901 MiB/s và prepare tổng khoảng 131–148 ms, nhỏ hơn rất nhiều so với
46–60 giây network fetch. Không ưu tiên tối ưu SAF cho large-file path này; chỉ
mở lại D3 khi workload nhiều file nhỏ, provider khác hoặc iOS cho kết quả khác.

### D4. Finalize/export

- Tối ưu chỉ khi time-to-file-ready hoặc export baseline cho thấy nghẽn.
- Giữ atomic record và conflict/path validation.
- Tách protocol-completed khỏi user-visible file-ready trong telemetry/UI.

### D5. AOA copy/GC

Chỉ thực hiện khi USB/GC profile xác nhận:

- Reusable batch buffer thay `toByteArray`/`copyOf`.
- Double buffer hoặc bounded ownership pool.
- Ring/compacting reassembler thay `buffer + chunk`/`copyOfRange`.
- A/B 16/32/64 KiB theo controller; không tăng đồng loạt.
- Không retry partial write bằng cả batch và luôn giữ bounded memory.
- Duy trì `7900 + IPv4/UDP overhead <= TUN MTU 8000`.

### D6. UI heartbeat, không che stall thật

Coalescer chỉ đặt trần 10 Hz, không tạo update khi transfer đứng. Nếu UI giữ tốc độ
cũ hoặc giật:

- Consumer/UI dùng heartbeat trên latest byte counter để speed/ETA giảm về 0 khi
  không có byte mới.
- Heartbeat không ghi record hoặc gửi thêm progress frame bulk.
- Vẫn hiển thị/ghi nhận stall thật; không dùng smoothing để che pipeline pause.

## 9. Phase E — QUIC tuning chỉ khi có bằng chứng

### E1. Window

Cấu hình hiện tại:

- Android stream receive window: 8 MiB.
- Desktop: 16 MiB.
- Connection send window: 8 lần stream window.

Các con số phải đọc theo endpoint: trong run Android **sender** → desktop receiver,
8 MiB là local receive window không chi phối bulk; receiver desktop advertise
16 MiB mới là `MAX_STREAM_DATA` có nghĩa. `send_window` là memory cap phía gửi khi
peer cấp nhiều flow-control credit, không phải bằng chứng rằng từng ấy byte đang nằm
trên Wi-Fi.

Với LAN/AOA RTT khoảng 2–5 ms, 8 MiB cho flow-control ceiling lý thuyết khoảng
1,6–4 GiB/s. Xem window là **đã loại trừ theo toán học** trên các path này trừ khi
telemetry chỉ ra cấu hình thực khác hoặc BDP ratio mâu thuẫn; không tốn ma trận
8/16/32 MiB cho LAN/AOA.

Relay không được so với mục tiêu ảo `window / RTT` nếu relay server/rate limit thấp
hơn. Chỉ thử 16/32 MiB trên Android khi đồng thời:

- `bdp_window_ratio` thường xuyên gần 1;
- raw QUIC/relay baseline còn headroom;
- sender/provider không CPU/disk-bound;
- memory budget cho window lớn hơn đã đo.

**E1b — giảm window/in-flight cap khi RTT inflation cao:** run direct hiện có RTT
provider p50 khoảng 1,67 giây, p90 4,16 giây và cwnd p50 khoảng 4,87 MB trên link chỉ
1,1–1,6 MiB/s. Đây là giả thuyết queue/bufferbloat, chưa phải kết luận nguyên nhân vì
raw TCP mới cũng giật. Sau khi raw baseline ổn định, A/B receiver window 4/8/16 MiB
(và send-memory cap tương ứng nếu cần) theo randomized order. Chỉ nhận cấu hình thấp
hơn khi giảm RTT inflation/loss/stall mà không làm giảm utilization/average quá ngưỡng.

### E2. Congestion control

- Giữ CUBIC làm mặc định cho đến khi có provider-side cwnd/loss.
- A/B CUBIC/BBR trên cùng iroh/noq version, cùng impairment và randomized order.
- Quyết định bằng utilization, p10/p50, CV, stall và loss recovery; không chỉ average.
- Không tự chọn controller chỉ từ nhãn direct/relay nếu chưa có dữ liệu thiết bị/path.

Provider-side evidence hiện đã có trên Android sender: một run direct 64 MiB ghi
4.813 lost packet, khoảng 6,99 MB lost bytes, 71 congestion event và RTT cuối hơn
3 giây. Điều này đủ để ưu tiên phân loại recovery và E1b sau khi có raw bracket,
nhưng chưa đủ để đổi mặc định: chạy randomized CUBIC/BBR trong cùng Wi-Fi stratum,
ít nhất 5 measured run mỗi cell, và loại batch khi baseline before/after lệch >10%.
BBR từng được bật ở commit `d386240` rồi revert ở `05c9e4d` do stutter release
phone-to-phone; dữ liệu cũ chỉ định tính. Nếu retest, dùng benchmark-only override,
không đổi production default, và yêu cầu thắng cả p10/CV/delivery-gap lẫn average.

### E3. MTU

- LAN giữ PMTUD mặc định.
- AOA chỉ tăng/tinh chỉnh sau khi MTU sample, probe loss và USB profile cho thấy lợi
  ích; kiểm tra fragmentation và controller-specific regression.

### E4. Nâng iroh

- Spike riêng cho iroh/iroh-blobs tương thích mới hơn.
- Chạy lại baseline và benchmark do noq/noq-proto có thể đổi hành vi CC/multipath.
- Không trộn dependency upgrade với AOA rewrite hoặc parallel-stream protocol change.

## 10. Chỉ số và tiêu chí nghiệm thu

Mỗi report phải có cả tốc độ tuyệt đối lẫn độ ổn định:

- Link/transport utilization.
- p10, p50, p90 throughput trên cửa sổ 1 giây; không ghi “median” và “p50” thành
  hai metric khác nhau.
- Coefficient of variation và low-speed episodes dưới 10% p50.
- Mid-transfer stall count/total/longest, tách warm-up/finalize.
- Time-to-first-byte, time-to-direct, direct/relay byte ratio.
- Prepare, transfer, finalize và file-ready duration.
- CPU, RSS, thermal state; sender và receiver riêng.
- Nhiều file nhỏ: files/s và completion latency.

Ngưỡng ban đầu:

- p10 ít nhất 70% p50; 80% là target sau khi loại warm-up và phase boundary.
- Không có mid-transfer stall trên 500 ms ở LAN/AOA ổn định.
- Không có memory tăng tuyến tính trong soak.
- Không regression resume, cancel, reconnect, hash verification hoặc path safety.
- Throughput chậm nhưng đều không được coi là đạt nếu utilization thấp; mọi run
  phải báo baseline ratio.

## 11. Decision gates

### Gate 1 — Telemetry đáng tin

- Đúng semantics receiver/provider.
- Window/config được log.
- Warm-up không tính stall; `None` không giả complete.
- Benchmark loop không cancel `stream.next()` theo tick.
- Có phase timing, path byte ratio và parser test.
- Progress timestamp không bị sampler bias; failed-terminal stall không biến mất.
- Active gap được tách loss-recovery bằng provider stats; correlation token không
  phải session ID đổi cơ số; config source/endpoint semantics không gây hiểu nhầm.
- Số byte mạng đến từ counter phạm vi connection, không phải counter theo path;
  mọi report có `path_counter_coverage` để nói rõ counter theo path phủ được bao
  nhiêu phần traffic.

**Nguồn sự thật: Gate 1 hiện đang mở.** Android ở cả role sender lẫn receiver đã
xác minh emission schema v5, receiver samples, SAF/background-save phase join và
JSON boundary với `0` malformed. Các
blocker code của Vòng 2 đã được sửa qua `bcbde90`, `f978003`, `26b4f7c` và
`b5820c5`. Path-migration smoke direct → relay → direct → relay đã xác nhận cờ
discontinuity và lower-bound reporting.

Blocker provider counter provenance **đã đóng** (schema v6, xem §5). Nguyên nhân
gốc là `selected_path()` trả `None` hoặc bỏ qua path không được chọn; bản sửa bổ
sung counter phạm vi connection, `path_count` và `path_counter_coverage`. Smoke
CLI↔CLI cho coverage 83,5% ngay trên đường direct sạch (4 path), và smoke thiết
bị thật desktop → Pixel 4 release tái hiện đúng ca `path=unknown`/counter 0 với
coverage 0,0% (6 path) — trong cả hai trường hợp counter connection vẫn ghi đúng
số byte, lệch hai đầu chỉ 2.322 byte trên 128 MiB.

Cả hai chiều đã xác minh dưới schema v6 trên release Android: desktop → Android
(provider coverage 0,0%) và Android → desktop (provider coverage 11,1%, và
`path_counter_discontinuity_count=1` ở receiver, tức cờ migration cũng hoạt động
cùng lúc với coverage). Hash round-trip khớp byte-for-byte.

**Gate 1 đóng cho desktop và Android.** iOS được tách thành hạng mục riêng chạy
sau, không chặn Gate 2 hay Phase C/D trên hai platform đã nghiệm thu. Khi làm
iOS, phạm vi là: boundary file-provider, background-save đến lúc file thật sự sẵn
sàng, và một run schema v6 hai chiều để đối chiếu với bảng ở §5. Cho tới lúc đó,
mọi kết luận trong tài liệu này chỉ áp dụng cho desktop và Android.

Provider-only smoke không đủ để nghiệm thu p10/p50/CV/stall.

### Gate 2 — Baseline và attribution hoàn tất

- Có iperf3/raw QUIC/disk/hash baseline.
- Có historical H và A/B A–E cho P0 với ít nhất 10 measured runs trên desktop.
- Biết thay đổi nào tạo win và effect size.

### Gate 3 — Chọn đúng nhánh tối ưu

- Relay ratio cao ⇒ D1.
- Small-file RTT-bound/single-stream-bound ⇒ D2.
- Prepare/SAF-bound ⇒ D3.
- Export-bound ⇒ D4.
- AOA CPU/GC-bound ⇒ D5.
- Window/CC-bound với provider stats ⇒ Phase E.

Không đạt gate thì không merge tuning của phase sau.

## 12. Lệnh kiểm tra

```powershell
cargo fmt --all -- --check
cargo test --workspace --exclude wisp-web-receiver
cargo check --workspace --exclude wisp-web-receiver
cargo check -p wisp-web-receiver --target wasm32-unknown-unknown
cargo metadata --manifest-path flutter/rust/Cargo.toml --no-deps --format-version 1

Push-Location flutter/rust
cargo fmt --all -- --check
Pop-Location

Push-Location flutter
flutter analyze
flutter test
Pop-Location

python -B -m unittest tools/test_analyze_transfer_telemetry.py
git diff --check
```

Các test cần socket/relay/thiết bị thật chạy thành suite riêng và ghi rõ khi bị skip.
Benchmark luôn ghi commit, release profile, device/OS, CPU governor/thermal state,
filesystem, Wi-Fi/USB, selected path, impairment config và baseline cùng phiên.

## 13. Những việc chưa làm

- Không tăng window/MTU đồng loạt khi chưa có BDP/baseline.
- Không chuyển sang BBR theo benchmark môi trường khác.
- Không lấy receiver-local cwnd/loss để kết luận payload congestion.
- Không dùng smoothing UI làm bằng chứng pipeline đã ổn định.
- Không chạy ma trận mobile hàng chục giờ trước khi desktop A/B chọn ứng viên.
- Không bỏ hash verification, atomic record hoặc path validation để đổi throughput.
- Không gọi thay đổi “tối ưu thành công” chỉ vì test chức năng xanh.

## 14. Tài liệu tham khảo

- Iroh QUIC transport configuration: <https://docs.rs/iroh/latest/iroh/endpoint/struct.QuicTransportConfigBuilder.html>
- Iroh path statistics: <https://docs.rs/iroh/latest/iroh/endpoint/struct.PathStats.html>
- Tokio filesystem tuning: <https://docs.rs/tokio/latest/tokio/fs/>
- Cargo profile overrides: <https://doc.rust-lang.org/cargo/reference/profiles.html#overrides>
- Iroh single-stream throughput investigation: <https://github.com/n0-computer/iroh/issues/4286>
