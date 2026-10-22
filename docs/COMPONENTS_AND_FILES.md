# Katalog Berkas & Komponen — N.E.X.A Windows Bridge

Dokumen ini menyediakan referensi teknis mendetail dari setiap berkas kode yang ada di dalam repositori `nexa-windows-bridge`.

---

## 1. Berkas Konfigurasi & Root Proyek

### 📄 `Cargo.toml`
* **Fungsi:** Manifest paket Rust dan deklarasi dependensi pihak ketiga.
* **Dependensi Utama:**
  * `slint`: Framework GUI desktop native ultra-cepat dan irit RAM.
  * `tokio` (fitur `full`): Asynchronous runtime untuk konkurensi dan I/O jaringan.
  * `tokio-tungstenite`: Klien WebSocket asinkron dengan native TLS.
  * `windows-capture`: Windows Graphics Capture API berbasis Direct3D 11 untuk pengambilan layar monitor.
  * `cpal`: Antarmuka audio lintas platform tingkat rendah (WASAPI di Windows).
  * `sonora`: Mesin Digital Signal Processing (DSP) WebRTC AEC3 untuk pembatalan gema.
  * `ringbuf`: Struktur data *lock-free SPSC circular ring buffer*.
  * `windows-sys`: FFI langsung ke Win32 API (`Foundation`, `UI_WindowsAndMessaging`, `UI_Shell`, `SystemInformation`, `Threading`, `Power`).
  * `serde` & `serde_json`: Serialisasi protokol JSON.
  * `tracing` & `tracing-subscriber`: Sistem logging terstruktur.

### 📄 `build.rs`
* **Fungsi:** Skrip pra-kompilasi (*custom build script*).
* **Operasi:** Menjalankan `slint_build::compile("ui/main_window.slint")` sebelum kompilasi Rust dimulai untuk menerjemahkan file markup `.slint` menjadi kode Rust native di dalam folder `target/`.

### 📄 `Cargo.lock`
* **Fungsi:** Mengunci versi eksak (*exact version lockfile*) seluruh *dependency tree* demi *deterministic & reproducible builds*.

---

## 2. Logika Utama Root `src/`

### 📄 `src/main.rs`
* **Fungsi:** Titik masuk utama aplikasi (*entry point / orchestrator*).
* **Tanggung Jawab Teknis:**
  1. Bootstrapping logging runtime `tracing_subscriber`.
  2. Memuat dan mengikat konfigurasi `BridgeConfig`.
  3. Membuka jendela Slint (`MainWindow`) dan menyinkronkan event loop UI dengan Tokio runtime.
  4. Menginisialisasi `NetworkClient`, `AudioEngine`, dan `ScreenCaptureController`.
  5. Menjalankan *worker loop* audio mic streaming ke WebSocket dan pemutaran audio balasan server ke speaker.
  6. Mengelola event interupsi suara pengguna (*barge-in notification*).
  7. Mendaftarkan pendengar daya Win32 (`PowerListener`), System Tray (`SystemTray`), dan Hotkeys global (`HotkeyListener`).
  8. Menjalankan sampler telemetri sistem berkala (CPU & RAM) untuk HUD Command Center.

### 📄 `src/config.rs`
* **Fungsi:** Model data konfigurasi dan utilitas persistensi.
* **Komponen:**
  * `BridgeConfig`: Menyimpan URL endpoint, token otentikasi, mode low RAM, FPS tangkapan layar, dan preferensi audio.
  * `load()` & `save()`: Membaca/menulis file `~/.nexa-bridge.json` dan migrasi otomatis dari format lama `~/.nexa-config.json`.
  * `normalize_ws_url()`: Sanitasi alamat HTTP/HTTPS/IP ke format WebSocket sah (`ws://` atau `wss://`).

---

## 3. Submodul Audio & WASAPI (`src/audio/`)

* **`src/audio/mod.rs` (`AudioEngine`):**
  Orkestrator subsistem audio duplex. Menginisialisasi speaker, mikrofon, loop referensi AEC, dan menyalurkan notifikasi interupsi suara (*barge-in*).
* **`src/audio/speaker.rs` (`SpeakerPlayer`):**
  Memutar audio respons server (24 kHz PCM) ke speaker fisik laptop via WASAPI dan mengirimkan salinan render frame ke AEC.
* **`src/audio/microphone.rs` (`MicrophoneCapture`):**
  Merekam suara dari mic fisik laptop, memprosesnya melalui filter WebRTC AEC3, dan mengirimkan chunk PCM 16 kHz bersih ke jaringan.
* **`src/audio/echo_canceller.rs` (`EchoCancellerEngine`):**
  Mesin peredam gema WebRTC AEC3, peredam bising (*noise suppression*), dan *dynamic ducking* (-18 dB) saat AI berbicara.
* **`src/audio/resampler.rs`:**
  Konverter sample rate audio berlatensi rendah (konversi antara 16 kHz, 24 kHz, dan 48 kHz).
* **`src/audio/audio_buffer.rs`:**
  Struktur data *lock-free SPSC circular ring buffer* (`ringbuf`) untuk pertukaran audio real-time tanpa alokasi heap.

---

## 4. Submodul Penangkapan Layar & Vision (`src/capture/`)

* **`src/capture/mod.rs`:**
  Mengekspor `ScreenCaptureController`, `FrameEncoder`, dan `FrameRateLimiter`.
* **`src/capture/screen_capture.rs` (`ScreenCaptureController` & `ScreenCaptureHandler`):**
  Menggunakan Windows Graphics Capture API (WGC) modern berbasis Direct3D 11 untuk menangkap frame desktop monitor utama langsung dari buffer GPU.
* **`src/capture/encoder.rs` (`FrameEncoder` & `FrameRateLimiter`):**
  Membatasi laju frame (misal 1 FPS untuk Gemini Live Vision) dan mengompresi buffer RGBA mentah menjadi byte JPEG lalu mengubahnya ke string Base64.

---

## 5. Submodul Jaringan WebSocket (`src/network/`)

* **`src/network/mod.rs`:**
  Pintu masuk modul jaringan, mengekspor tipe publik: `ClientMessage`, `ServerMessage`, `NetworkClient`, dan `ConnectionState`.
* **`src/network/protocol.rs`:**
  Definisi kontrak pesan JSON:
  * `ClientMessage`: `Ping`, `CommandResult`, `Telemetry`, `PowerEvent`, `VideoFrame`, `AudioStream`, `AudioInterrupted`.
  * `ServerMessage`: `Pong`, `ExecuteCommand`, `AudioPlay`, `AudioInterrupted`, `StatusUpdate`, `ReplyComplete`, `Unknown`.
* **`src/network/client.rs` (`NetworkClient`):**
  Klien WebSocket asinkron dengan *exponential backoff reconnection* otomatis (500 ms – 10 detik), *heartbeat keepalive* (20 detik), serta antrean pesan keluar berbasis channel MPSC.

---

## 6. Submodul Integrasi Platform Windows (`src/platform/`)

* **`src/platform/mod.rs`:**
  Mengekspor komponen platform: `PowerListener`, `HotkeyListener`, `SystemTray`, dan `SystemTelemetry`.
* **`src/platform/power.rs` (`PowerListener`):**
  Mendengarkan event daya kernel Windows `WM_POWERBROADCAST` pada `HWND_MESSAGE` (`PBT_APMSUSPEND` saat layar ditutup & `PBT_APMRESUMEAUTOMATIC` saat dibuka kembali).
* **`src/platform/system_tray.rs` (`SystemTray`):**
  Mengelola ikon di area notifikasi Windows via `Shell_NotifyIconW`, klik kiri untuk toggle jendela, dan menu klik kanan (Buka, Mute, Layar Penuh, Berbagi Layar, Pengaturan, Keluar).
* **`src/platform/hotkeys.rs` (`HotkeyListener`):**
  Mendaftarkan pintasan keyboard global via `RegisterHotKey`: Boss Key (`Win+Alt+N`), Mic Mute (`Win+Alt+M`), Fullscreen (`F11` / `Win+Alt+F`), dan Berbagi Layar (`Win+Alt+S`).
* **`src/platform/telemetry.rs` (`SystemTelemetry`):**
  Mengukur persentase beban CPU (`GetSystemTimes`) dan memori RAM fisik (`GlobalMemoryStatusEx`) dengan alokasi memori nol.

---

## 7. Direktori Antarmuka Slint (`ui/`) & Dokumentasi Spesifikasi UI

> 📘 **Dokumentasi Lengkap Elemen UI:** Rincian mendalam setiap elemen visual, label teks, fungsi kode Rust, dan properti antarmuka tersedia di [UI_COMPONENTS_AND_SPECIFICATION.md](file:///c:/workspace/nexa-windows-bridge/docs/UI_COMPONENTS_AND_SPECIFICATION.md).

* **`ui/main_window.slint`:**
  Jendela antarmuka utama yang mendukung **Dual-Mode**:
  1. *Compact Companion Mode* (~380x250px): Jendela ringkas di sudut layar dengan visualisasi avatar mini 52px, status kognitif, audio meter 18-bar, pil indikator periferal, dan baris tombol aksi 36px yang terkunci rapi.
  2. *Fullscreen Cybernetic Command Center* (Resolusi Penuh / F11): HUD bergaya J.A.R.V.I.S dengan avatar 120px berputar, visualisator spektrum audio 48-bar, telemetri CPU/RAM, dan floating action dock.
* **`ui/settings_dialog.slint`:**
  Modal dialog pengaturan endpoint server, token otentikasi, profil low RAM, toggle autostart Windows, serta uji coba handshake WebSocket secara live. Dibatasi pada ukuran maksimal 500x460px agar rapi.
* **`ui/overlay_window.slint`:**
  Jendela transparan *click-through* tingkat atas DirectComposition Win32 untuk visualisasi otomasi AI: Bounding Box (`BOX`), Target Ring (`RING`), Pointer Reticle (`POINTER`), Spotlight Dimmer (`SPOTLIGHT`), dan konfirmasi klik 300ms (`RIPPLE`).
* **`ui/components/avatar.slint`:**
  Avatar cybernetic responsif dengan visualisasi dual-ocular ring dan rotasi giroskopik yang mendukung 9 status kognitif AI (`idle`, `listening`, `thinking`, `speaking`, `barge_in`, `screen_sharing`, `tool_executing`, `connecting`, `offline`).
* **`ui/components/audio_meter.slint`:**
  Visualisator spektrum audio real-time dengan batang frekuensi parametrik teranimasi dinamis yang bereaksi terhadap suara pengguna dan suara AI (18 bilah di Mode 1, 48 bilah di Mode 2).
