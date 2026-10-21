# Arsitektur Sistem — N.E.X.A Windows Bridge

Dokumen ini menjelaskan rancangan arsitektur, model konkurensi, aliran data (*data flow*), serta integrasi hardware pada aplikasi `nexa-windows-bridge`.

---

## 1. Diagram Arsitektur Tingkat Tinggi

```mermaid
graph TD
    subgraph UI_Thread ["Thread Utama (Slint UI Loop)"]
        UI["MainWindow (Slint GUI: Dual-Mode Windowed / Fullscreen HUD)"]
        Avatar["Cybernetic Avatar (9 Cognitive States)"]
        AudioMeter["Real-time Audio Spectrum Equalizer"]
        SettingsModal["Settings Dialog"]
    end

    subgraph Tokio_Runtime ["Tokio Async Multi-threaded Runtime"]
        StateWatcher["State Watcher Task"]
        CmdDispatcher["Command & Audio Dispatcher Task"]
        Worker["WebSocket Lifecycle Worker (run_client_worker)"]
        AudioEng["AudioEngine (WASAPI Duplex + WebRTC AEC3)"]
        CaptureCtrl["ScreenCaptureController (Direct3D 11 WGC)"]
        TelemTask["System Telemetry Sampler (CPU/RAM Win32)"]
        OutQueue["Outbound MPSC Queue"]
        CmdBus["Broadcast Command Bus"]
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
* Menjalankan fungsi `main_window.run()?` yang menangani render UI dual-mode (Mode Companion 380x520 atau Mode Command Center Fullscreen F11), visualisasi avatar cybernetic, dan equalizer audio.
* Komunikasi dari background task ke Slint UI **harus** melalui `slint::invoke_from_event_loop(...)` menggunakan pointer lemah (`Weak<MainWindow>`) untuk mencegah *data race* atau *memory leak*.

### B. Tokio Multi-Threaded Asynchronous Runtime
* Diinisialisasi melalui macro `#[tokio::main]`.
* Mengelola tugas-tugas non-blocking:
  1. **WebSocket Lifecycle Worker (`run_client_worker`)**: Loop koneksi persisten yang menangani *handshake*, antrean pengiriman pesan keluar, pembacaan paket server, dan *exponential backoff*.
  2. **State Watcher**: Memantau perubahan `ConnectionState` dan menyinkronkan status visual ke jendela utama.
  3. **Command & Audio Dispatcher**: Mendistribusikan instruksi dari server ke modul handler lokal, memicu respons suara audio ke speaker, dan mengirimkan *ACK (CommandResult)* kembali ke server.
  4. **Duplex Audio Forwarder**: Meneruskan frame PCM 16kHz dari mikrofon laptop ke antrean WebSocket dan menghitung energi RMS untuk menganimasikan avatar saat mode listening.
  5. **Barge-In Monitor**: Mendengarkan sinyal interupsi saat pengguna berbicara ketika AI sedang memutar suara, lalu memutus output speaker seketika.
  6. **Telemetry Sampler Task**: Membaca beban CPU & RAM setiap 2 detik dan memperbarui bar HUD antarmuka.

### C. Dedicated Win32 OS Threads (Platform Hooks)
1. **`PowerListener`**: Thread Win32 mandiri dengan *message loop* tersembunyi (`HWND_MESSAGE`) untuk mendengarkan pesan `WM_POWERBROADCAST`.
2. **`SystemTray`**: Thread terisolasi untuk menangani ikon notifikasi taskbar (`Shell_NotifyIconW`) dan menu klik kanan.
3. **`HotkeyListener`**: Thread terisolasi untuk mendengarkan pintasan global (`RegisterHotKey` dan loop `WM_HOTKEY`).

---

## 3. Jalur Komunikasi Antar Komponen (Internal Channels)

| Channel | Tipe | Arah | Fungsi |
| :--- | :--- | :--- | :--- |
| `state_tx` | `tokio::sync::watch` | Worker $\to$ UI Watcher | Mengabarkan status koneksi terbaru (`Connected`, `Connecting`, `Reconnecting`, `Disconnected`, `Suspended`). |
| `outbound_tx` | `tokio::sync::mpsc (128)` | UI/Audio/Capture $\to$ Worker | Antrean pesan keluar (JSON string atau paket biner audio). |
| `command_tx` | `tokio::sync::broadcast (64)` | Worker $\to$ Handlers | Bus multi-subscriber untuk pesan `EXECUTE_COMMAND`, `AudioPlay`, dan `StatusUpdate`. |
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

* **Target Memori:** Alokasi RAM dijaga di bawah **25 MB**.
* **Strategi Efisiensi:**
  * Slint tidak menggunakan browser engine (seperti Electron/Tauri Webview), melainkan renderer native direct software / Skia / Qt yang sangat irit RAM.
  * Buffer channel dibatasi ketat: MPSC 128 items, broadcast 64 items.
  * Pembatasan laju tangkapan layar (1 FPS) dan ring buffer audio *lock-free* mencegah memory leak atau lonjakan alokasi heap.
