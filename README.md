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
Chưa có: quota tracker, router tự chuyển, handoff (Giai đoạn 3 trở đi).
