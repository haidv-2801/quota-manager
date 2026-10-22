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
Có: thêm/sửa/xoá tài khoản, key lưu trong Credential Manager (UI chỉ thấy 4 ký tự cuối), migration SQLite theo schema trong tech spec.
Chưa có: chat, adapter, quota, router (Giai đoạn 2 trở đi).
