//! UI copy in English and Vietnamese.
//!
//! Every string is a field on [`Strings`], and each language is one `const`
//! literal of that struct. That is the point of the shape: adding a field
//! fails to compile until *both* languages define it, so a half-translated
//! screen cannot ship. Strings that take a value keep a `{}` placeholder and
//! are filled by [`fill`] / [`fill2`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    /// The default: this firmware ships worldwide and English is the safer
    /// first impression. Vietnamese is one press away in Settings.
    #[default]
    En,
    Vi,
}

impl Lang {
    /// Shown in the settings row, each in its own language — a user looking
    /// for their language should recognise it without reading the other one.
    pub fn label(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Vi => "Tiếng Việt",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Lang::En => Lang::Vi,
            Lang::Vi => Lang::En,
        }
    }

    pub fn strings(self) -> &'static Strings {
        match self {
            Lang::En => &EN,
            Lang::Vi => &VI,
        }
    }
}

/// Substitutes the single `{}` placeholder in a localised template.
///
/// `format!` needs a literal, and these templates are chosen at runtime, so
/// the substitution is done by hand. A template with no placeholder is
/// returned unchanged rather than treated as an error — a translator dropping
/// the `{}` should lose the value, not panic the UI.
pub fn fill(template: &str, value: &str) -> String {
    template.replacen("{}", value, 1)
}

/// Same, for a template with two placeholders, filled left to right.
pub fn fill2(template: &str, first: &str, second: &str) -> String {
    fill(&fill(template, first), second)
}

/// Picks the singular or plural template for `count`.
///
/// Only English needs the distinction, but the choice is made here for both
/// languages so a call site never has to ask which language it is in.
pub fn plural(count: u64, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

pub struct Strings {
    // Home
    pub status_ready: &'static str,
    pub status_starting: &'static str,
    pub pairing_code: &'static str,
    pub save_to: &'static str,
    pub device_name: &'static str,
    /// Label for this device's public key.
    pub identity: &'static str,
    pub lan_address: &'static str,
    pub no_wifi: &'static str,
    pub scan_to_pair: &'static str,
    pub building_qr: &'static str,
    pub code_offline_hint: &'static str,
    pub code_stale_hint: &'static str,
    pub code_expired: &'static str,
    /// `{}` = whole minutes remaining.
    pub minutes_left: &'static str,
    /// `{}` = whole seconds remaining.
    pub seconds_left: &'static str,

    // Footer hints
    pub hint_new_code: &'static str,
    pub hint_settings: &'static str,
    pub hint_exit: &'static str,
    pub hint_select: &'static str,
    pub hint_back: &'static str,
    pub hint_accept: &'static str,
    pub hint_decline: &'static str,
    pub hint_always_trust: &'static str,
    pub hint_cancel: &'static str,
    pub hint_done: &'static str,
    pub hint_trust: &'static str,
    pub hint_untrust: &'static str,
    pub hint_remove: &'static str,

    // Offer
    pub offer_title: &'static str,
    /// `{}` = item count, `{}` = human size. English needs a separate
    /// singular; Vietnamese does not inflect, so both forms are identical
    /// there — kept as two fields anyway so the choice lives in the table
    /// rather than in a language check at the call site.
    pub offer_summary: &'static str,
    pub offer_summary_one: &'static str,
    pub pill_browser: &'static str,
    pub text_content: &'static str,

    // Transfer
    pub transfer_title: &'static str,
    pub connecting: &'static str,
    pub transferring: &'static str,
    /// `{}` = completed, `{}` = total.
    pub files_progress: &'static str,
    /// `{}` = formatted duration.
    pub eta_left: &'static str,

    // Result
    pub result_completed: &'static str,
    pub result_declined: &'static str,
    pub result_cancelled: &'static str,
    pub result_failed: &'static str,
    /// `{}` = item count, `{}` = human size.
    pub received_summary: &'static str,
    pub received_summary_one: &'static str,
    pub declined_body: &'static str,
    pub cancelled_body: &'static str,
    pub failed_body: &'static str,

    // Settings
    pub settings_title: &'static str,
    pub row_save_folder: &'static str,
    pub row_name_clash: &'static str,
    pub row_trusted: &'static str,
    pub row_button_test: &'static str,
    pub row_device_name: &'static str,
    pub row_device_name_hint: &'static str,
    pub row_language: &'static str,
    pub row_about: &'static str,
    pub conflict_rename: &'static str,
    pub conflict_reject: &'static str,

    // About
    pub about_title: &'static str,
    pub about_version: &'static str,
    pub about_identity: &'static str,
    pub about_identity_hint: &'static str,
    pub about_save_folder: &'static str,
    pub about_project: &'static str,

    // Folder picker
    pub folder_title: &'static str,
    /// Heading above folders the user has chosen before.
    pub folder_recent: &'static str,
    pub folder_suggested: &'static str,
    /// Marks the folder currently in use. A word, not a tick: the bundled
    /// Noto Sans has no U+2713 and renders it as a blank box.
    pub folder_in_use: &'static str,
    pub folder_browse: &'static str,
    pub folder_use_this: &'static str,
    pub folder_changed: &'static str,

    // Trusted devices
    pub trusted_title: &'static str,

    // Button test
    pub button_test_title: &'static str,
    pub button_test_hint: &'static str,
    pub button_unmapped: &'static str,
    pub button_none_yet: &'static str,

    // Toasts
    pub toast_new_code: &'static str,
    pub toast_cancelling: &'static str,
    pub toast_no_identity: &'static str,
    /// `{}` = device name.
    pub toast_trusted: &'static str,
    /// `{}` = device name.
    pub toast_untrusted: &'static str,
    /// `{}` = device name.
    pub toast_removed: &'static str,
    /// `{}` = sender name.
    pub toast_auto_accept: &'static str,
    /// `{}` = policy label.
    pub toast_clash_policy: &'static str,
    /// `{}` = language label.
    pub toast_language: &'static str,

    // Sending
    pub send_title: &'static str,
    pub send_to_title: &'static str,
    pub send_code_title: &'static str,
    pub send_progress_title: &'static str,
    pub send_picked: &'static str,
    /// `{}` = number of items queued.
    pub send_selected_count: &'static str,
    pub send_nothing_picked: &'static str,
    pub send_via_nearby: &'static str,
    pub send_via_recent: &'static str,
    pub send_scanning: &'static str,
    pub send_scan_failed: &'static str,
    pub send_enter_code: &'static str,
    pub send_code_hint: &'static str,
    pub send_completed: &'static str,
    pub send_declined: &'static str,
    pub send_cancelled: &'static str,
    pub send_failed: &'static str,
    /// `{}` = item count, `{}` = human size.
    pub send_sent_summary: &'static str,
    pub send_sent_summary_one: &'static str,
    pub send_declined_body: &'static str,
    pub hint_send: &'static str,
    pub hint_pick: &'static str,
    pub hint_pick_folder: &'static str,
    pub hint_continue: &'static str,
    pub hint_type: &'static str,
    pub hint_delete: &'static str,
    pub hint_send_now: &'static str,
    pub hint_rescan: &'static str,
    pub hint_other_device: &'static str,

    // Misc
    pub fatal_title: &'static str,
    pub list_empty: &'static str,
}

pub static EN: Strings = Strings {
    status_ready: "Ready to receive",
    status_starting: "Starting…",
    pairing_code: "Pairing code",
    save_to: "Save to",
    device_name: "Device name",
    identity: "Identity",
    lan_address: "LAN address",
    no_wifi: "Not connected to Wi-Fi",
    scan_to_pair: "Scan to pair offline",
    building_qr: "Building QR code…",
    code_offline_hint: "No internet — use the QR or Nearby",
    code_stale_hint: "Code may already be used — press A for a new one",
    code_expired: "Code expired — press A",
    minutes_left: "{} min left",
    seconds_left: "{} s left",

    hint_new_code: "New code",
    hint_settings: "Settings",
    hint_exit: "Exit",
    hint_select: "Select",
    hint_back: "Back",
    hint_accept: "Accept",
    hint_decline: "Decline",
    hint_always_trust: "Always trust",
    hint_cancel: "Cancel",
    hint_done: "Done",
    hint_trust: "Trust sender",
    hint_untrust: "Untrust sender",
    hint_remove: "Remove",

    offer_title: "Incoming transfer",
    offer_summary: "{} items · {}",
    offer_summary_one: "{} item · {}",
    pill_browser: "Browser",
    text_content: "Text",

    transfer_title: "Receiving",
    connecting: "Connecting…",
    transferring: "Transferring…",
    files_progress: "{}/{} files",
    eta_left: "{} left",

    result_completed: "Complete",
    result_declined: "Declined",
    result_cancelled: "Cancelled",
    result_failed: "Failed",
    received_summary: "Received {} items · {}",
    received_summary_one: "Received {} item · {}",
    declined_body: "You declined this transfer",
    cancelled_body: "The transfer was cancelled",
    failed_body: "Nothing was received",

    settings_title: "Settings",
    row_save_folder: "Save folder",
    row_name_clash: "On name clash",
    row_trusted: "Trusted devices",
    row_button_test: "Button test",
    row_device_name: "Device name",
    row_device_name_hint: "Edit settings.json over SSH",
    row_language: "Language",
    row_about: "About",
    conflict_rename: "Rename (keep both)",
    conflict_reject: "Reject",

    about_title: "About Wisp",
    about_version: "Version",
    about_identity: "Device identity",
    about_identity_hint: "Senders remember this handheld by this key",
    about_save_folder: "Save folder",
    about_project: "github.com/vigov5/wisp",

    folder_title: "Save folder",
    folder_recent: "Recently used",
    folder_suggested: "Suggested",
    folder_in_use: "in use",
    folder_browse: "Browse…",
    folder_use_this: "[ Use this folder ]",
    folder_changed: "Save folder changed",

    trusted_title: "Trusted devices",

    button_test_title: "Button test",
    button_test_hint: "Press each button to see its raw code. If the name is \
                       wrong, add the code to \"button_overrides\" in \
                       settings.json.",
    button_unmapped: "unmapped",
    button_none_yet: "No button pressed yet",

    toast_new_code: "Requesting a new code…",
    toast_cancelling: "Cancelling…",
    toast_no_identity: "This sender has no stable identity",
    toast_trusted: "Trusted {}",
    toast_untrusted: "No longer trusting {}",
    toast_removed: "Removed {}",
    toast_auto_accept: "Auto-accepted from {}",
    toast_clash_policy: "On name clash: {}",
    toast_language: "Language: {}",

    send_title: "Choose what to send",
    send_to_title: "Send to",
    send_code_title: "Enter pairing code",
    send_progress_title: "Sending",
    send_picked: "picked",
    send_selected_count: "{} selected",
    send_nothing_picked: "Pick a file or folder first",
    send_via_nearby: "On this network",
    send_via_recent: "Sent to before",
    send_scanning: "Looking for nearby devices…",
    send_scan_failed: "Could not search the network",
    send_enter_code: "Enter a pairing code",
    send_code_hint: "Type the six characters shown on the other device",
    send_completed: "Sent",
    send_declined: "Declined",
    send_cancelled: "Cancelled",
    send_failed: "Send failed",
    send_sent_summary: "Sent {} items · {}",
    send_sent_summary_one: "Sent {} item · {}",
    send_declined_body: "The other device declined",
    hint_send: "Send",
    hint_pick: "Open / pick",
    hint_pick_folder: "Add folder",
    hint_continue: "Continue",
    hint_type: "Type",
    hint_delete: "Delete",
    hint_send_now: "Send",
    hint_rescan: "Search again",
    hint_other_device: "Another device",

    fatal_title: "Could not start",
    list_empty: "Empty",
};

pub static VI: Strings = Strings {
    status_ready: "Sẵn sàng nhận",
    status_starting: "Đang khởi động…",
    pairing_code: "Mã ghép nối",
    save_to: "Lưu vào",
    device_name: "Tên máy",
    identity: "Danh tính",
    lan_address: "Địa chỉ LAN",
    no_wifi: "Chưa kết nối Wi-Fi",
    scan_to_pair: "Quét để ghép offline",
    building_qr: "Đang tạo mã QR…",
    code_offline_hint: "Không có mạng — dùng QR hoặc Nearby",
    code_stale_hint: "Mã có thể đã dùng — bấm A để lấy mã mới",
    code_expired: "Mã đã hết hạn — bấm A",
    minutes_left: "Còn {} phút",
    seconds_left: "Còn {} giây",

    hint_new_code: "Mã mới",
    hint_settings: "Cài đặt",
    hint_exit: "Thoát",
    hint_select: "Chọn",
    hint_back: "Quay lại",
    hint_accept: "Nhận",
    hint_decline: "Từ chối",
    hint_always_trust: "Luôn tin máy này",
    hint_cancel: "Huỷ",
    hint_done: "Xong",
    hint_trust: "Tin máy này",
    hint_untrust: "Bỏ tin máy này",
    hint_remove: "Bỏ tin",

    offer_title: "Yêu cầu gửi",
    offer_summary: "{} mục · {}",
    offer_summary_one: "{} mục · {}",
    pill_browser: "Trình duyệt",
    text_content: "Nội dung văn bản",

    transfer_title: "Đang nhận",
    connecting: "Đang kết nối…",
    transferring: "Đang truyền…",
    files_progress: "{}/{} tệp",
    eta_left: "còn {}",

    result_completed: "Hoàn tất",
    result_declined: "Đã từ chối",
    result_cancelled: "Đã huỷ",
    result_failed: "Thất bại",
    received_summary: "Đã nhận {} mục · {}",
    received_summary_one: "Đã nhận {} mục · {}",
    declined_body: "Bạn đã từ chối yêu cầu này",
    cancelled_body: "Phiên truyền đã bị huỷ",
    failed_body: "Không nhận được tệp nào",

    settings_title: "Cài đặt",
    row_save_folder: "Thư mục lưu",
    row_name_clash: "Khi trùng tên",
    row_trusted: "Thiết bị tin cậy",
    row_button_test: "Kiểm tra nút",
    row_device_name: "Tên thiết bị",
    row_device_name_hint: "Sửa trong settings.json qua SSH",
    row_language: "Ngôn ngữ",
    row_about: "Thông tin",
    conflict_rename: "Đổi tên (giữ cả hai)",
    conflict_reject: "Từ chối",

    about_title: "Thông tin Wisp",
    about_version: "Phiên bản",
    about_identity: "Danh tính máy",
    about_identity_hint: "Máy gửi nhớ máy này bằng khoá đó",
    about_save_folder: "Thư mục lưu",
    about_project: "github.com/vigov5/wisp",

    folder_title: "Thư mục lưu",
    folder_recent: "Đã dùng gần đây",
    folder_suggested: "Gợi ý",
    folder_in_use: "đang dùng",
    folder_browse: "Duyệt thư mục…",
    folder_use_this: "[ Chọn thư mục này ]",
    folder_changed: "Đã đổi thư mục lưu",

    trusted_title: "Thiết bị tin cậy",

    button_test_title: "Kiểm tra nút",
    button_test_hint: "Bấm từng nút để xem mã thô. Nếu tên nút không khớp, \
                       thêm mã đó vào \"button_overrides\" trong \
                       settings.json.",
    button_unmapped: "chưa gán",
    button_none_yet: "Chưa bấm nút nào",

    toast_new_code: "Đang lấy mã mới…",
    toast_cancelling: "Đang huỷ…",
    toast_no_identity: "Máy gửi không có danh tính cố định",
    toast_trusted: "Đã tin {}",
    toast_untrusted: "Đã bỏ tin {}",
    toast_removed: "Đã bỏ {}",
    toast_auto_accept: "Tự động nhận từ {}",
    toast_clash_policy: "Khi trùng tên: {}",
    toast_language: "Ngôn ngữ: {}",

    send_title: "Chọn thứ cần gửi",
    send_to_title: "Gửi tới",
    send_code_title: "Nhập mã ghép nối",
    send_progress_title: "Đang gửi",
    send_picked: "đã chọn",
    send_selected_count: "đã chọn {}",
    send_nothing_picked: "Hãy chọn tệp hoặc thư mục trước",
    send_via_nearby: "Trong mạng này",
    send_via_recent: "Đã gửi trước đây",
    send_scanning: "Đang tìm thiết bị gần đây…",
    send_scan_failed: "Không tìm được trong mạng",
    send_enter_code: "Nhập mã ghép nối",
    send_code_hint: "Gõ sáu ký tự hiện trên máy kia",
    send_completed: "Đã gửi",
    send_declined: "Bị từ chối",
    send_cancelled: "Đã huỷ",
    send_failed: "Gửi thất bại",
    send_sent_summary: "Đã gửi {} mục · {}",
    send_sent_summary_one: "Đã gửi {} mục · {}",
    send_declined_body: "Máy kia đã từ chối",
    hint_send: "Gửi",
    hint_pick: "Mở / chọn",
    hint_pick_folder: "Thêm thư mục",
    hint_continue: "Tiếp tục",
    hint_type: "Gõ",
    hint_delete: "Xoá",
    hint_send_now: "Gửi",
    hint_rescan: "Tìm lại",
    hint_other_device: "Máy khác",

    fatal_title: "Không khởi động được",
    list_empty: "Trống",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_the_default() {
        assert_eq!(Lang::default(), Lang::En);
    }

    #[test]
    fn toggling_round_trips() {
        assert_eq!(Lang::En.toggled(), Lang::Vi);
        assert_eq!(Lang::Vi.toggled().toggled(), Lang::Vi);
    }

    #[test]
    fn each_language_names_itself() {
        assert_eq!(Lang::En.label(), "English");
        assert_eq!(Lang::Vi.label(), "Tiếng Việt");
    }

    #[test]
    fn fill_substitutes_once() {
        assert_eq!(fill("{} min left", "4"), "4 min left");
        assert_eq!(fill2("{}/{} files", "2", "5"), "2/5 files");
    }

    #[test]
    fn plural_picks_the_singular_only_for_one() {
        assert_eq!(plural(1, "one", "many"), "one");
        assert_eq!(plural(0, "one", "many"), "many");
        assert_eq!(plural(2, "one", "many"), "many");
    }

    #[test]
    fn english_inflects_the_item_count_but_vietnamese_does_not() {
        // "1 items" on the device is what prompted this.
        assert_eq!(fill2(EN.offer_summary_one, "1", "74 B"), "1 item · 74 B");
        assert_eq!(fill2(EN.offer_summary, "4", "1.0 MB"), "4 items · 1.0 MB");
        assert_eq!(VI.offer_summary_one, VI.offer_summary);
    }

    #[test]
    fn a_template_without_a_placeholder_survives() {
        // A translation that drops the `{}` should lose the value, not panic.
        assert_eq!(fill("no placeholder", "x"), "no placeholder");
    }

    #[test]
    fn templates_carry_the_same_placeholder_count_in_both_languages() {
        // A missing `{}` in one language silently drops a filename or a
        // countdown, which is the failure this catches.
        let pairs: [(&str, &str); 13] = [
            (EN.minutes_left, VI.minutes_left),
            (EN.seconds_left, VI.seconds_left),
            (EN.offer_summary, VI.offer_summary),
            (EN.offer_summary_one, VI.offer_summary_one),
            (EN.received_summary_one, VI.received_summary_one),
            (EN.files_progress, VI.files_progress),
            (EN.eta_left, VI.eta_left),
            (EN.received_summary, VI.received_summary),
            (EN.toast_trusted, VI.toast_trusted),
            (EN.toast_untrusted, VI.toast_untrusted),
            (EN.toast_removed, VI.toast_removed),
            (EN.toast_auto_accept, VI.toast_auto_accept),
            (EN.toast_clash_policy, VI.toast_clash_policy),
        ];
        for (en, vi) in pairs {
            assert_eq!(
                en.matches("{}").count(),
                vi.matches("{}").count(),
                "placeholder mismatch between {en:?} and {vi:?}"
            );
        }
    }

    /// The bundled Noto Sans renders a missing glyph as a blank box, which
    /// looked like a rendering fault when a tick was used as the "in use"
    /// marker. Keep the UI to characters the font actually has.
    #[test]
    fn ui_strings_avoid_glyphs_the_bundled_font_lacks() {
        let missing = ['\u{2713}', '\u{2714}', '\u{2717}', '\u{2718}'];
        for lang in [Lang::En, Lang::Vi] {
            let s = lang.strings();
            for value in [
                s.folder_in_use,
                s.folder_recent,
                s.folder_suggested,
                s.hint_accept,
                s.hint_done,
                s.row_about,
            ] {
                for ch in missing {
                    assert!(
                        !value.contains(ch),
                        "{value:?} uses {ch:?}, which the font has no glyph for"
                    );
                }
            }
        }
    }

    #[test]
    fn no_string_is_empty_in_either_language() {
        for lang in [Lang::En, Lang::Vi] {
            let s = lang.strings();
            // A representative sweep; an empty label renders as a blank row.
            for value in [
                s.status_ready,
                s.pairing_code,
                s.settings_title,
                s.hint_accept,
                s.result_failed,
                s.list_empty,
                s.row_language,
            ] {
                assert!(!value.is_empty(), "empty string for {lang:?}");
            }
        }
    }
}
