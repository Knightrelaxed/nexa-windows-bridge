# N.E.X.A Windows Bridge Desktop Companion

> Native Slint Desktop Companion for Lenovo ThinkPad X380 Yoga connecting to N.E.X.A Server.

## 📚 Dokumentasi Proyek

Dokumentasi lengkap dan arsitektur sistem telah disusun di dalam folder [`docs/`](./docs/):

* [**Arsitektur Sistem & Model Konkurensi**](./docs/ARCHITECTURE.md)
* [**Subsistem Audio WASAPI & WebRTC AEC3**](./docs/AUDIO_SUBSYSTEM.md)
* [**Subsistem Screen Capture GPU Direct3D 11 & Vision**](./docs/CAPTURE_SUBSYSTEM.md)
* [**Subsistem Otomasi Input & Panduan Layar**](./docs/AUTOMATION_SUBSYSTEM.md)
* [**Integrasi System Tray, Global Hotkeys & Telemetri**](./docs/SYSTEM_TRAY_AND_HOTKEYS.md)
* [**Manajemen Daya OS & Win32 Hook**](./docs/POWER_MANAGEMENT.md)
* [**Spesifikasi Protokol WebSocket & Skema JSON**](./docs/PROTOCOL.md)
* [**Spesifikasi Konfigurasi Sistem**](./docs/CONFIGURATION.md)
* [**Katalog Komponen & Referensi Berkas Lengkap**](./docs/COMPONENTS_AND_FILES.md)

---

## 🚀 Menjalankan Aplikasi

```powershell
# Jalankan mode pengembangan
cargo run

# Jalankan pengujian unit
cargo test

# Build binary mandiri (.exe)
cargo build --release
```