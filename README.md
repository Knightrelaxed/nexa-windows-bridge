# Nexa Bridge Desktop Companion

> Native Slint Desktop Companion for Lenovo ThinkPad X380 Yoga connecting to N.E.X.A Server.

## 📚 Dokumentasi Proyek

Dokumentasi lengkap dan arsitektur sistem telah disusun di dalam folder [`docs/`](./docs/):

* [**Arsitektur Sistem & Model Konkurensi**](./docs/ARCHITECTURE.md)
* [**Spesifikasi Keamanan & Kebijakan Otomasi (Armed Guard & Anti-Injection)**](./docs/SECURITY.md)
* [**Subsistem Audio WASAPI & WebRTC AEC3 (DAC Reference Tap)**](./docs/AUDIO_SUBSYSTEM.md)
* [**Subsistem Screen Capture GPU Direct3D 11 & Vision**](./docs/CAPTURE_SUBSYSTEM.md)
* [**Subsistem Otomasi Input & Panduan Layar (UIA Timeout & High-DPI)**](./docs/AUTOMATION_SUBSYSTEM.md)
* [**Integrasi System Tray, Global Hotkeys & Telemetri**](./docs/SYSTEM_TRAY_AND_HOTKEYS.md)
* [**Manajemen Daya OS & Win32 Hook**](./docs/POWER_MANAGEMENT.md)
* [**Spesifikasi Protokol WebSocket & Skema JSON**](./docs/PROTOCOL.md)
* [**Spesifikasi Konfigurasi Sistem**](./docs/CONFIGURATION.md)
* [**Spesifikasi Komponen & Antarmuka UI (Dual-Mode & HUD)**](./docs/UI_COMPONENTS_AND_SPECIFICATION.md)
* [**Katalog Komponen & Referensi Berkas Lengkap**](./docs/COMPONENTS_AND_FILES.md)

### 🌟 Fitur Unggulan Terbaru:
* **Native Win32 GDI Stealth Overlay Window:** Bebas kedipan layar hitam (*black screen flickers*) 100% pada DWM Windows saat penyorotan target kursor & bounding box.
* **Smart Scroll Deadzone Auto-Centering:** Pemulihan kursor otomatis dari area header/tab bar (<220px) ke tengah konten dokumen sebelum putaran mouse wheel diinjeksikan.
* **Interactive HUD Click Protection:** Perlindungan titik klik otomatis agar agen otomasi tidak dapat mengeklik jendela floating pill HUD miliknya sendiri.
* **Graceful Window Close with Z-Order Fallback:** Penutupan jendela aplikasi aktif via sinyal resmi Win32 `WM_CLOSE`, lengkap dengan fallback pencarian jendela di Z-order jika jendela HUD sedang terfokus.

---

## 🚀 Menjalankan Aplikasi

```powershell
# Jalankan mode pengembangan
cargo run

# Jalankan pengujian unit (118 tests)
cargo test

# Build binary mandiri (.exe) -> menghasilkan target/release/nexa-bridge.exe
cargo build --release
```