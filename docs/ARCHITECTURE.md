# Arsitektur Sistem — Nexa Bridge

Dokumen ini menjelaskan rancangan arsitektur, model konkurensi, aliran data (*data flow*), ketahanan jaringan, serta integrasi hardware pada aplikasi `nexa-bridge` (**Nexa Bridge**).

---

## 1. Diagram Arsitektur Tingkat Tinggi

```mermaid
graph TD
    subgraph UI_Thread ["Thread Utama (Slint UI Loop)"]
        UI["MainWindow (Slint GUI: Dual-Mode Windowed / Fullscreen HUD)"]
        Avatar["Cybernetic Avatar (10 Cognitive States)"]
        AudioMeter["Real-time Audio Spectrum Equalizer"]
        SettingsModal["Settings Dialog"]
    end

    subgraph Tokio_Runtime ["Tokio Async Multi-threaded Runtime"]
        StateWatcher["State Watcher Task"]
        CmdDispatcher["Command & Audio Dispatcher Task (Resilient Loop)"]
        Worker["WebSocket Lifecycle Worker (Watchdog + Timeout + Interrupt)"]
        AudioEng["AudioEngine (WASAPI Duplex + WebRTC AEC3 DAC Tap)"]
        CaptureCtrl["ScreenCaptureController (Direct3D 11 WGC)"]
        TelemTask["System Telemetry Sampler (CPU/RAM Win32)"]
        OutQueue["Outbound MPSC Queue"]
        CmdBus["Broadcast Command Bus (Kapasitas 256)"]
    end

    subgraph OS_Threads ["Dedicated Win32 Threads"]
        PowerHook["PowerListener (HWND_MESSAGE / WM_POWERBROADCAST)"]
        TrayHook["SystemTray (Shell_NotifyIconW / Context Menu)"]
        HotkeyHook["HotkeyListener (RegisterHotKey / WM_HOTKEY)"]
    end

    subgraph External ["Eksternal"]
        NexaServer["N.E.X.A Server (WebSocket)"]
        WinKernel["Windows NT Kernel (Power, Times, Memory)"]
        GPU["Intel UHD Graphics 620 (Primary Display Framebuffer)"]
    end

    %% Koneksi OS & Hardware
    WinKernel -->|PBT_APMSUSPEND / PBT_APMRESUME| PowerHook
    PowerHook -->|notify_suspend / notify_resume| Worker
    WinKernel -->|GetSystemTimes / GlobalMemoryStatusEx| TelemTask
    GPU -->|Direct3D 11 Texture Capture| CaptureCtrl

    %% Koneksi Input Pengguna
    TrayHook -->|TrayEvent: Toggle, Mute, Fullscreen, Share| UI
    HotkeyHook -->|HotkeyAction: BossKey, MicMute, Fullscreen, Share| UI

    %% Koneksi WebSocket
    Worker <-->|Tungstenite WSS| NexaServer

    %% Komunikasi Antar Thread / Tasks
    Worker -->|watch::channel| StateWatcher
    StateWatcher -->|slint::invoke_from_event_loop| UI
    Worker -->|broadcast::channel| CmdDispatcher
    CmdDispatcher -->|ACK Outbound| OutQueue
    AudioEng -->|Clean 16kHz PCM Stream| OutQueue
    CaptureCtrl -->|Base64 JPEG VideoFrame (1 FPS)| OutQueue
    OutQueue --> Worker

    %% UI Actions
    UI -->|on_toggle_connect / on_save_settings| Worker
```

---

## 2. Model Konkurensi & Manajemen Thread

Aplikasi ini menggunakan pemisahan tanggung jawab yang ketat antar thread untuk memastikan antarmuka tetap responsif (60 FPS tanpa jeda/stutter) dan tidak terblokir oleh I/O jaringan, pemrosesan audio, atau sinyal OS:

### A. Main Thread (Slint GUI Event Loop)
* Menjalankan fungsi `main_window.run()?` yang menangani render UI dual-mode (Mode Companion ringkas atau Mode Command Center Fullscreen F11), visualisasi avatar cybernetic, dan equalizer audio.
* Komunikasi dari background task ke Slint UI **harus** melalui `slint::invoke_from_event_loop(...)` menggunakan pointer lemah (`Weak<MainWindow>`) untuk mencegah *data race* atau *memory leak*.
* **Penutupan Bersih (Clean Termination):** Tombol close `[X]` pada jendela memicu `slint::quit_event_loop()`, memastikan seluruh background worker, audio duplex, dan runtime Tokio mati total tanpa meninggalkan proses zombi di Task Manager.

### B. Tokio Multi-Threaded Asynchronous Runtime
* Diinisialisasi melalui macro `#[tokio::main]`.
* Mengelola tugas-tugas non-blocking dengan ketahanan tinggi:
  1. **WebSocket Lifecycle Worker (`run_client_worker`)**:
     * **Drain Permit Basi (Audit H1):** Menguras permit tersimpan pada `session_interrupt` dengan `.now_or_never()` sebelum setiap inisiasi koneksi agar koneksi tidak langsung tertutup (*flap*) setelah sleep/resume.
     * **Timeout Handshake (Audit H3):** Membungkus `connect_async` dengan batas waktu 10 detik dan pembatalan instan via `session_interrupt`.
     * **Pencegahan Infinite Retry Auth (Audit M2):** Menangkap status HTTP 401/403 untuk menghentikan reconnect otomatis saat token ditolak server.
     * **Watchdog Aktivitas Keepalive 15s (Audit H2):** Memutus koneksi soket setengah terbuka (*half-open socket*) jika tidak ada lalu lintas atau pong masuk selama >15 detik saat Wi-Fi terputus tanpa FIN TCP.
  2. **State Watcher**: Memantau perubahan `ConnectionState` berbasis `AtomicBool` yang bebas poisoning.
  3. **Command & Audio Dispatcher (Audit H4, N6, R4)**:
     * Menggunakan kapasitas broadcast bus 256 untuk pesan kendali dan status kognitif.
     * **Pemisahan Jalur Audio Dedikasi (`subscribe_audio`)**: Chunk audio PCM dialirkan melalui kanal dedicated `mpsc` terpisah sehingga ledakan audio tidak membanjiri atau menunda antrean eksekusi perintah kendali.
     * Menangani kondisi `RecvError::Lagged` secara mulus tanpa menghentikan loop penerimaan perintah secara permanen.
  4. **Antrean Serial FIFO Ber-Epoch (`fifo_cmd_tx` - Audit H10, K2, K9)**:
     * Antrean eksekusi perintah kapasitas 32 yang menjamin pengetikan, klik, dan pergerakan kursor berjalan serial berurutan.
     * Mengikat stempel generasi sesi (`session_generation`). Saat koneksi terputus atau terjadi pembatalan, nomor generasi dinaikkan sehingga seluruh perintah antrean lama dibatalkan seketika.
     * Masing-masing perintah dibungkus isolasi task (`tokio::spawn`) untuk mencegah *worker termination* saat terjadi panic Windows.
  5. **Fast-Path Abort (Audit N3, R2)**:
     * Perintah pembatalan (`ABORT_AUTOMATION`, `CANCEL`) melompati antrean serial FIFO untuk memberikan respon pembatalan mikrodetik.
  6. **Duplex Audio Forwarder**: Meneruskan frame PCM 16kHz dari mikrofon laptop ke antrean WebSocket dan menghitung energi RMS untuk menganimasikan avatar saat mode listening.
  7. **Barge-In Monitor**: Mendengarkan sinyal interupsi saat pengguna berbicara ketika AI sedang memutar suara, lalu memutus output speaker seketika.
  8. **Telemetry Sampler Task**: Membaca beban CPU & RAM setiap 2 detik dan memperbarui bar HUD antarmuka.

### C. Dedicated Win32 OS Threads (Platform Hooks)
1. **`PowerListener`**: Thread Win32 mandiri dengan *message loop* tersembunyi (`HWND_MESSAGE`) untuk mendengarkan pesan `WM_POWERBROADCAST`.
2. **`SystemTray`**: Thread terisolasi untuk menangani ikon notifikasi taskbar (`Shell_NotifyIconW`), menu klik kanan, dan toggle mode armed (Lease 15 Menit).
3. **`HotkeyListener`**: Thread terisolasi untuk mendengarkan pintasan global (`RegisterHotKey` dan loop `WM_HOTKEY`), termasuk tombol darurat Win+Alt+X dan Boss Key Win+Alt+N.
4. **`SingleInstanceGuard`**: Memastikan hanya ada satu proses Nexa Bridge yang berjalan di sistem dan memulihkan jendela aktif jika pengguna meluncurkan shortcut kedua kali.

---

## 3. Jalur Komunikasi Antar Komponen (Internal Channels)

| Channel | Tipe | Arah | Fungsi |
| :--- | :--- | :--- | :--- |
| `state_tx` | `tokio::sync::watch` | Worker $\to$ UI Watcher | Mengabarkan status koneksi terbaru (`Connected`, `Connecting`, `Reconnecting`, `Disconnected`, `Suspended`). |
| `outbound_tx` | `tokio::sync::mpsc (128)` | UI/Audio/Capture $\to$ Worker | Antrean pesan keluar (JSON string atau paket biner audio). |
| `command_tx` | `tokio::sync::broadcast (256)` | Worker $\to$ Handlers | Bus multi-subscriber kapasitas tinggi untuk pesan status, eksekusi perintah, dan event kognitif. |
| `audio_tx` | `tokio::sync::mpsc (128)` | Worker $\to$ Audio Engine | Kanal khusus streaming PCM audio independen dari antrean kendali (Audit N6). |
| `fifo_cmd_tx` | `tokio::sync::mpsc (32)` | Dispatcher $\to$ Executor | Antrean eksekusi serial perintah desktop dengan generasi epoch isolasi (Audit K2). |
| `reconnect_notify` | `tokio::sync::Notify` | UI/Power $\to$ Worker | Memicu percobaan koneksi instan (mengabaikan timer backoff). |
| `session_interrupt` | `tokio::sync::Notify` | UI/Power $\to$ Worker | Memutus loop sesi aktif secara instan saat user memilih disconnect atau laptop sleep. |
| `barge_in_notify` | `tokio::sync::Notify` | Audio Engine $\to$ Main Runtime | Memberi sinyal bahwa pengguna menyela ucapan AI untuk mematikan audio seketika. |

---

## 4. Penanganan Siklus Daya Laptop (Power State Management)

Dirancang khusus untuk laptop **Lenovo ThinkPad X380 Yoga** yang sering dibuka-tutup engselnya (*laptop/tablet mode transition*):

1. **Lid Close / Sleep (`PBT_APMSUSPEND`)**:
   - `PowerListener` menerima sinyal OS.
   - Memanggil `network_client.notify_suspend()`.
   - Menghentikan subsistem audio duplex dan GPU screen capture.
   - Mengirim WebSocket Close frame dan menunda reconnect hingga sistem terbangun.

2. **Lid Open / Wake (`PBT_APMRESUMEAUTOMATIC` / `PBT_APMRESUMESUSPEND`)**:
   - `PowerListener` menerima sinyal bangun.
   - Memanggil `network_client.notify_resume()`.
   - Menembakkan `reconnect_notify` untuk langsung melakukan koneksi ulang instan tanpa menunggu delay backoff.

---

## 5. Efisiensi Memori (Ultra-Low RAM Mode)

* **Target Memori:** Alokasi RAM dijaga di bawah **25 MB** (aktual ~15 MB pada mode standby).
* **Strategi Efisiensi:**
  * Slint tidak menggunakan browser engine (seperti Electron/Tauri Webview), melainkan renderer native direct software / Skia / Qt yang sangat irit RAM.
  * Buffer pemutaran audio diatur pada 30 detik (menghemat 35 MB dibanding buffer 120 detik).
  * Pembatasan laju tangkapan layar (1 FPS) dan ring buffer audio *lock-free* mencegah memory leak atau lonjakan alokasi heap.
