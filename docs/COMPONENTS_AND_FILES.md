# Katalog Berkas & Komponen — N.E.X.A Windows Bridge

Dokumen ini menyediakan referensi mendetail dari setiap berkas kode yang ada di dalam repositori `nexa-windows-bridge`.

---

## 1. Berkas Konfigurasi & Root Proyek

### 📄 `Cargo.toml`
* **Fungsi:** Manifest paket Rust dan deklarasi dependensi.
* **Dependensi Utama:**
  * `slint` (v1.18.1): Framework GUI desktop native.
  * `tokio` (v1, fitur `full`): Asynchronous runtime untuk I/O dan konkurensi.
  * `tokio-tungstenite` (v0.26, fitur `native-tls`): Klien WebSocket asinkron dengan enkripsi TLS.
  * `serde` & `serde_json`: Serialisasi/deserialisasi objek dan protokol JSON.
  * `windows-sys` (v0.59): FFI langsung ke Windows Win32 API (`Win32_Foundation`, `Win32_UI_WindowsAndMessaging`, `Win32_System_Power`, dll.).
  * `tracing` & `tracing-subscriber`: Sistem logging terstruktur ke stdout/stderr.
  * `dirs`: Helper lintas platform untuk menemukan direktori profil user (`~`).

### 📄 `build.rs`
* **Fungsi:** Skrip pra-kompilasi (*custom build script*).
* **Operasi:** Memanggil `slint_build::compile("ui/main_window.slint")` sebelum kompilasi Rust dimulai untuk mengubah DSL markup Slint menjadi kode Rust native di dalam folder `target/`.

### 📄 `Cargo.lock`
* **Fungsi:** Catatan versi eksak (*exact lockfile*) untuk menjamin *reproducible builds* dan verifikasi integritas hash (*checksum*).

---

## 2. Direktori `src/` (Logika Inti Rust)

### 📄 `src/main.rs`
* **Fungsi:** Titik masuk utama aplikasi (*entry point*).
* **Tanggung Jawab:**
  1. Menginisialisasi logging `tracing_subscriber` dengan filter level `debug`/`info`.
  2. Membaca konfigurasi `BridgeConfig::load()` ke dalam pointer bersama `Arc<Mutex<BridgeConfig>>`.
  3. Membuka jendela Slint GUI (`MainWindow::new()`).
  4. Menjalankan instance [`NetworkClient`](../src/network/client.rs).
  5. Menghubungkan perubahan state jaringan ke UI secara thread-safe (`invoke_from_event_loop`).
  6. Mengatur dispatcher perintah `EXECUTE_COMMAND` yang dikirim dari server N.E.X.A.
  7. Menjalankan thread pendengar daya laptop [`PowerListener`](../src/platform/power.rs).
  8. Mengikat callback tombol UI (Simpan pengaturan, Uji koneksi WebSocket, Hubungkan/Putuskan).

### 📄 `src/config.rs`
* **Fungsi:** Struktur data dan manipulasi konfigurasi.
* **Komponen:**
  * Struct `BridgeConfig`: Menyimpan properti `server_url`, `auth_token`, `audio_input_device`, `audio_output_device`, `capture_fps`, `low_ram_mode`, `auto_connect`.
  * `load()` & `save()`: Manajemen berkas `~/.nexa-bridge.json` dan konversi otomatis dari konfigurasi versi lama (`~/.nexa-config.json`).
  * `normalize_ws_url()`: Helper untuk memformat URL HTTP/HTTPS/IP menjadi endpoint WebSocket yang valid.
  * Unit test untuk pengujian deserialisasi dan normalisasi URL.

---

## 3. Submodul Audio & WASAPI (`src/audio/`)

Modul ini bertanggung jawab atas pemrosesan audio dua arah (*duplex*) bersuara jernih tanpa headset (*headset-free*) yang disetel khusus untuk speaker & microphone array Lenovo ThinkPad X380 Yoga.

* **`src/audio/mod.rs` (`AudioEngine`):**
  Orkestrator subsistem audio. Menginisialisasi `SpeakerPlayer`, `MicrophoneCapture`, dan menghubungkan loop referensi gema ke `EchoCancellerEngine`.
* **`src/audio/speaker.rs` (`SpeakerPlayer`):**
  Menangani pemutaran audio balasan (misal respon Gemini Live PCM 24kHz mono) ke output speaker Windows melalui driver WASAPI / cpal.
* **`src/audio/microphone.rs` (`MicrophoneCapture`):**
  Menangkap suara pengguna dari mikrofon, menjalankan filtering pembatalan gema, dan mengirimkan chunk PCM 16kHz bersih ke antrean pengiriman WebSocket.
* **`src/audio/echo_canceller.rs` (`EchoCancellerEngine`):**
  Mesin peredam gema berbasis WebRTC AEC3 (Acoustic Echo Cancellation) dengan deteksi *double-talk* dan *barge-in notification*.
* **`src/audio/resampler.rs`:**
  Konverter sample rate audio berkualitas tinggi (misalnya konversi antara 16kHz, 24kHz, dan 48kHz).
* **`src/audio/audio_buffer.rs`:**
  Struktur data *lock-free circular ring buffer* untuk pertukaran frame audio berlatensi rendah antar thread audio real-time.

---

## 4. Submodul Jaringan (`src/network/`)

### 📄 `src/network/mod.rs`
* **Fungsi:** Root modul jaringan, mengekspor tipe-tipe publik: `ClientMessage`, `ServerMessage`, `NetworkClient`, dan `ConnectionState`.

### 📄 `src/network/protocol.rs`
* **Fungsi:** Kontrak skema protokol pesan WebSocket.
* **Tipe:**
  * Enum `ClientMessage`: Berisi varian `Ping`, `CommandResult`, `Telemetry`, `PowerEvent`, `VideoFrame`, `AudioStream`.
  * Enum `ServerMessage`: Berisi varian `Pong`, `ExecuteCommand`, dan `Unknown` (fallback toleran pembaruan server).
  * Dilengkapi pengujian unit serialisasi JSON.

### 📄 `src/network/client.rs`
* **Fungsi:** Klien WebSocket asinkron tangguh.
* **Fitur Utama:**
  * `run_client_worker`: Loop koneksi mandiri dengan *exponential backoff* (500 ms hingga 10 detik).
  * Watchdog keepalive: Mengirim paket `PING` setiap 20 detik untuk mencegah koneksi terputus diam-diam oleh router/NAT.
  * Antrean pengiriman keluar (`mpsc::channel` kapasitas 128 pesan).
  * Event interrupt seketika (`tokio::sync::Notify`) saat pengguna memilih diskonek atau saat laptop sleep.
  * Handshake builder yang menyertakan header `Authorization: Bearer` dan `X-Device-Type: windows`.

---

## 4. Submodul Platform OS (`src/platform/`)

### 📄 `src/platform/mod.rs`
* **Fungsi:** Mengekspor modul platform Windows, mendefinisikan `PowerListener` dan enum `PowerStateEvent`.

### 📄 `src/platform/power.rs`
* **Fungsi:** Pemantau status daya laptop ThinkPad.
* **Mekanisme Kerja:**
  * Membuka thread dedicated yang memanggil Win32 `CreateWindowExW` terhubung ke `HWND_MESSAGE`.
  * Mendengarkan event pesan window `WM_POWERBROADCAST`:
    * `PBT_APMSUSPEND` $\to$ Laptop masuk mode tidur / engsel ditutup.
    * `PBT_APMRESUMESUSPEND` & `PBT_APMRESUMEAUTOMATIC` $\to$ Laptop dibuka / dibangunkan.
  * Mengirimkan callback ke klien jaringan agar tidak melakukan spam reconnect saat laptop dalam keadaan sleep.

---

## 5. Direktori Antarmuka (`ui/`)

### 📄 `ui/main_window.slint`
* **Fungsi:** Tata letak antarmuka utama bridge (lebar 380px, tinggi 520px).
* **Fitur UI:**
  * Header judul dengan tombol pintasan `⚙ Settings`.
  * Penampil animasi avatar N.E.X.A di bagian tengah.
  * Teks status kontekstual ("Listening to Tuan Faqih...", "Cognitive Extended Thinking...", "Online & Ready", dll.).
  * Tiga indikator status hardware: *WASAPI Audio*, *D3D11 Vision*, dan *AR Guidance*.
  * Tombol aksi koneksi utama (*Connect to N.E.X.A / Disconnect / Connecting...*).
  * Lapisan overlay modal untuk dialog pengaturan.

### 📄 `ui/settings_dialog.slint`
* **Fungsi:** Modal pengaturan konfigurasi.
* **Fitur:**
  * Input Server Endpoint WebSocket.
  * Input Device Authorization Token (tipe password terlindungi).
  * Checkbox *Ultra-Low RAM Mode (<25MB, Lean Buffer)*.
  * Tombol *Test Connection* interaktif dengan teks respon hasil handshake langsung.
  * Tombol *Save & Apply* dan *Close*.

### 📄 `ui/components/avatar.slint`
* **Fungsi:** Komponen grafis avatar responsif.
* **Fitur:**
  * Lingkaran cincin luar bersinar (*outer glow ring*) dan inti dalam (*pulsing core*).
  * Transisi warna dan animasi berbasis status:
    * `listening`: Sian (#00E5FF)
    * `speaking`: Hijau (#00FF88)
    * `thinking`: Amber (#FFB300)
    * `idle`: Biru kehijauan (#4DD0E1)
    * `offline`: Abu-abu (#546E7A)
