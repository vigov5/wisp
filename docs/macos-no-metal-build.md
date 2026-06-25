# Chạy / build Wisp trên macOS không có Metal

Hướng dẫn này dành cho **máy macOS test không có Metal khả dụng** (máy ảo
UTM/QEMU/VMware, hoặc GPU quá cũ). Trên các máy này, build mặc định mở ra **cửa
sổ trắng / app fail loading**, vì Flutter (>= 3.29) render macOS bằng **Impeller**
— renderer cần Metal.

Cách xử lý: **tắt Impeller** để Flutter quay về renderer cũ (Skia). Quan trọng:
chỉ tắt ở **local để test**, **không động vào CI** (release/CI vẫn dùng Impeller).

---

## Nguyên nhân

- Flutter bản hiện tại (`flutter --version` → 3.41.x) bật Impeller mặc định trên macOS.
- Impeller yêu cầu Metal hoạt động. VM/GPU cũ không đáp ứng → màn hình trắng,
  hoặc log dạng `Failed to create Metal device` / `Unable to create a Metal device`.
- Repo **không commit** key tắt Impeller → mọi build mặc định (kể cả CI) đều dùng Impeller.
  Vì vậy ta chỉ tắt Impeller **cục bộ khi test**.

---

## Cách 1 — Script một lệnh (khuyến nghị)

Đã có sẵn `flutter/tool/run-macos-no-metal.sh`. Script tự sửa
`macos/Runner/Info.plist` (thêm `FLTEnableImpeller=false`), chạy app, rồi **khôi
phục lại Info.plist gốc khi thoát** (kể cả Ctrl-C / lỗi). CI không bị ảnh hưởng.

Chạy **trên máy macOS** (đã cài Flutter + Xcode), từ thư mục repo:

```bash
chmod +x flutter/tool/run-macos-no-metal.sh   # lần đầu

# Debug — flutter run -d macos (hot reload):
flutter/tool/run-macos-no-metal.sh

# Build release .app để test bản đóng gói:
flutter/tool/run-macos-no-metal.sh build

# Build profile:
flutter/tool/run-macos-no-metal.sh profile
```

Bản `build` xuất ra:
`flutter/build/macos/Build/Products/Release/app.app`
(mở bằng `open flutter/build/macos/Build/Products/Release/app.app`).

> Nếu Gatekeeper chặn app chưa ký:
> `xattr -dr com.apple.quarantine flutter/build/macos/Build/Products/Release/app.app`

---

## Cách 2 — Sửa tay rồi revert

Nếu không muốn dùng script:

```bash
cd flutter

# Tắt Impeller
/usr/libexec/PlistBuddy -c "Add :FLTEnableImpeller bool false" macos/Runner/Info.plist

# Chạy / build như bình thường
flutter run -d macos
# hoặc: flutter build macos --release

# QUAN TRỌNG: trả lại Info.plist gốc để không lọt vào commit / CI
git checkout -- macos/Runner/Info.plist
```

Hoặc sửa trực tiếp `macos/Runner/Info.plist`, thêm vào trong `<dict>`:

```xml
<key>FLTEnableImpeller</key>
<false/>
```

…rồi `git checkout -- macos/Runner/Info.plist` sau khi test xong.

---

## Trường hợp KHÔNG có Metal thật sự (vd: VMware trên Windows)

⚠️ Quan trọng — **Cách 1/2 chỉ cứu được khi máy vẫn có Metal** (chỉ là Impeller
lỗi). Nếu máy **hoàn toàn không có Metal**, cả hai cách trên đều vô tác dụng vì:

- Trên macOS, renderer cũ (Skia) **cũng vẽ qua Metal** → không Metal thì tắt
  Impeller cũng fail.
- Embedder macOS của Flutter là **Metal-only**, không có đường render bằng CPU.
  `--enable-software-rendering` là **no-op trên macOS desktop** → không dùng được.

### Kiểm tra máy có Metal không

```bash
system_profiler SPDisplaysDataType | grep -i metal
```

- `Metal: Supported` / `Metal Family: ...` → có Metal, dùng **Cách 1**.
- Không có dòng nào / `Not Supported` → **không có Metal**, không chạy được Flutter
  desktop trên máy này.

### VMware Workstation/Player trên Windows (macOS guest)

GPU ảo là **VMware SVGA II**, **không cung cấp Metal**. VMware **không** có "3D
acceleration" cho guest macOS (toggle đó chỉ cho guest Windows/Linux). ⇒ **Không
có cách nào** chạy được build Flutter macOS desktop trong môi trường này — đây là
giới hạn của VMware + macOS embedder, không phải lỗi cấu hình app.

### Phương án test macOS thực tế

1. **Mac thật / Cloud Mac** (MacStadium, AWS EC2 Mac, hoặc máy Mac vật lý).
2. **CI macOS runner** (GitHub Actions `macos-latest` — máy thật, có Metal) cho
   build + widget/integration test tự động.
3. **Apple Silicon host + Virtualization.framework** (UTM/Tart/Anka): cung cấp
   Metal paravirtualized cho guest macOS 13+. (VMware-on-Windows thì không.)
4. Test phần **logic chung** (Rust bridge, state, UI) bằng build Windows ngay trên
   máy dev; chỉ để dành phần macOS-specific cho máy thật.

---

## Kiểm chứng đã tắt Impeller

Khi chạy debug, log khởi động sẽ **không** còn dòng khởi tạo Impeller; app mở ra
giao diện thay vì màn hình trắng. Có thể xác nhận key đang áp dụng:

```bash
/usr/libexec/PlistBuddy -c "Print :FLTEnableImpeller" flutter/macos/Runner/Info.plist
# -> false  (trong lúc đang chạy; sau khi thoát script sẽ không còn key này)
```

---

## Lưu ý về CI

- **Không commit** `FLTEnableImpeller` vào `macos/Runner/Info.plist`.
- CI và bản release chính thức **giữ Impeller** (hiệu năng tốt hơn, máy CI có Metal).
- Cách 1 (script) tự khôi phục file → `git status` phải **sạch** sau khi chạy xong.
  Nếu thấy `Info.plist` bị thay đổi nghĩa là script bị kill bất thường, chạy
  `git checkout -- flutter/macos/Runner/Info.plist`.
