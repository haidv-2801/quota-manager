# Quota Manager (Giai đoạn 1)

Khung Tauri 2 + React/TypeScript. Lõi Rust nằm trong `crates/core` (SQLite, kho key, quản lý tài khoản); `src-tauri` chỉ nối lõi với UI.

## Chạy trên Windows
1. Cài Rust, Node 20+, và WebView2 (có sẵn trên Windows 11).
2. `npm install`
3. Tạo icon: `npx tauri icon <ảnh-vuông.png>` (tauri-build cần `src-tauri/icons/icon.ico`).
4. `npm run tauri dev`

## Kiểm thử
- `cargo test -p quota_core` (không cần GTK/WebView, chạy được mọi OS)
- `npm run build` (kiểm kiểu TypeScript + build UI)

## Phạm vi
Giai đoạn 1: thêm/sửa/xoá tài khoản, key lưu trong Credential Manager (UI chỉ thấy 4 ký tự cuối), migration SQLite theo schema trong tech spec.
Giai đoạn 2: chat streaming qua adapter Anthropic + OpenAI, lưu hội thoại và usage vào SQLite, ghim tài khoản cho từng hội thoại.
Giai đoạn 3: theo dõi quota (đọc header rate limit, đếm token cục bộ, trạng thái healthy/low/cooling_down/exhausted/disabled), tab Quota, banner cảnh báo trong app.
Giai đoạn 4: router tự chuyển tài khoản khi 429/hết credit/hội thoại quá dài (healthy trước low, theo độ ưu tiên; hội thoại ghim không bị chuyển), retry lỗi tạm thời 2 lần, thông báo "đã chuyển" trong chat.
Chưa có: handoff tóm tắt (compact), toast Windows và icon khay, áp ngân sách USD.
