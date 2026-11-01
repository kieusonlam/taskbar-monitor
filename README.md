# Taskbar Monitor

Hiển thị CPU, GPU, VRAM và RAM ngay trên taskbar Windows. Bấm vào để xem chi tiết.

![Thanh số liệu trên taskbar](docs/taskbar.png)

![Bảng chi tiết khi bấm vào](docs/popup.png)

## Tính năng

- **Nhẹ**: một file exe khoảng 350 KB, dùng khoảng 6 MB RAM. Không cần cài đặt, không cần quyền admin, không cài driver.
- **Số liệu giống Task Manager**: CPU và GPU lấy qua Performance Counters (PDH), chạy với card NVIDIA, AMD, Intel và máy có nhiều GPU.
- **Bảng chi tiết**: tải từng luồng CPU, xung nhịp, RAM và bộ nhớ cam kết, tải 3D / Copy / Video của từng GPU, VRAM, đồ thị 60 giây.
- Số liệu ≥ 60% tô vàng, ≥ 85% tô đỏ.
- Tự ẩn khi chơi game hoặc xem video toàn màn hình.
- Tự cập nhật từ GitHub Releases.

## Cài đặt

1. Tải `taskbar-monitor.exe` ở trang [Releases](https://github.com/kieusonlam/taskbar-monitor/releases/latest).
2. Chép vào một thư mục cố định, ví dụ `%LOCALAPPDATA%\Programs\TaskbarMonitor\`, rồi chạy.
3. Chuột phải vào thanh, chọn **Khởi động cùng Windows**.

File exe chưa được ký số nên Windows SmartScreen có thể cảnh báo ở lần chạy đầu: bấm **More info** rồi **Run anyway**.

## Sử dụng

| Thao tác | Kết quả |
|---|---|
| Chuột trái vào thanh | Mở / đóng bảng chi tiết |
| Esc hoặc bấm ra ngoài | Đóng bảng chi tiết |
| Chuột phải vào thanh | Menu: cập nhật, khởi động cùng Windows, thoát |

## Cập nhật

App kiểm tra bản mới 15 giây sau khi khởi động và mỗi ngày một lần. Khi có bản mới, góc thanh hiện một chấm xanh; chuột phải chọn **Cập nhật lên vX.Y.Z**, app tự tải, thay file và khởi động lại.

Cập nhật từ dòng lệnh (cho script hoặc triển khai nhiều máy):

```
taskbar-monitor.exe --update
```

Mã thoát: `0` đã là bản mới nhất, `10` đã cập nhật, `1` lỗi.

## Build từ mã nguồn

Cần Rust (toolchain MSVC):

```
cargo build --release
```

File ra ở `target\release\taskbar-monitor.exe`.

## Phát hành bản mới

1. Tăng `version` trong `Cargo.toml`, commit.
2. Đẩy tag cùng số: `git tag v0.2.0 && git push origin v0.2.0`.
3. GitHub Actions build và đăng exe lên Releases. Máy người dùng nhận bản mới trong vòng một ngày.

## Giới hạn

- Windows 11 không còn cho nhúng vào taskbar, nên thanh là một cửa sổ nằm đè lên taskbar, đặt sát khay hệ thống. Chỉ hiện trên taskbar của màn hình chính.
- Chưa có nhiệt độ: đọc nhiệt độ CPU cần kernel driver.

## Giấy phép

[MIT](LICENSE)
