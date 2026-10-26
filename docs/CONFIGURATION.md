# Spesifikasi Konfigurasi Sistem — N.E.X.A Windows Bridge

Dokumen ini mendokumentasikan skema serialisasi konfigurasi, lokasi persistensi disk, strategi migrasi legacy, serta fungsi normalisasi endpoint pada `nexa-windows-bridge`.

---

## 1. Lokasi Berkas Konfigurasi

Persistensi konfigurasi disimpan pada direktori *User Profile Home* menggunakan pustaka `dirs`:

* **Berkas Utama:** `C:\Users\<NamaUser>\.nexa-bridge.json`
* **Berkas Cadangan (Auto-Backup):** `C:\Users\<NamaUser>\.nexa-bridge.json.bak`
* **Berkas Legacy (Migrasi Otomatis):** `C:\Users\<NamaUser>\.nexa-config.json`

### Logika Resolusi Booting & Penyelamatan Data (Audit N5, R7)
1. `BridgeConfig::load()` memeriksa keberadaan file utama `~/.nexa-bridge.json`.
2. **Penyelamatan File Rusak (`.bak`):** Jika file ada tetapi gagal di-parse (misal JSON korup akibat mati listrik), sistem otomatis menyalin berkas tersebut ke `~/.nexa-bridge.json.bak` sebelum memuat default, menjamin data konfigurasi pengguna tidak hilang atau tertimpa tanpa jejak.
3. Jika tidak ditemukan, fungsi memeriksa fallback ke `~/.nexa-config.json`, mengekstrak kunci camelCase `serverUrl` dan `secret`, lalu memicu `save()` untuk mengonversinya ke format standar baru.
4. Jika berkas tidak ada, sistem menggunakan implementasi `Default::default()`.

---

## 2. Struktur Schema Konfigurasi

Format data berbasis JSON terstruktur:

```json
{
  "server_url": "wss://nexa.my-server.com/ws",
  "auth_token": "nexa_sec_ab892019382103",
  "audio_input_device": "Default",
  "audio_output_device": "Default",
  "capture_fps": 1,
  "low_ram_mode": true,
  "auto_connect": false,
  "autostart": false,
  "show_card_vision": true,
  "show_card_automation": true,
  "show_card_transcript": true,
  "show_card_telemetry": true,
  "show_card_hotkeys": true
}
```

### Spesifikasi Tipe & Nilai Bawaan

| Bidang | Tipe Rust | Nilai Default | Keterangan Teknis |
| :--- | :--- | :--- | :--- |
| `server_url` | `String` | `"ws://127.0.0.1:3000/ws"` | Endpoint WebSocket server N.E.X.A. |
| `auth_token` | `String` | `""` | Kredensial otentikasi Bearer token pada HTTP handshake. |
| `audio_input_device` | `String` | `"Default"` | Device identifier input WASAPI untuk mikrofon. |
| `audio_output_device` | `String` | `"Default"` | Device identifier output WASAPI untuk speaker. |
| `capture_fps` | `u32` | `1` | Interval laju penangkapan layar (1 frame/detik pada mode hemat daya). |
| `low_ram_mode` | `bool` | `true` | Batasan buffer ketat untuk menjaga footprint RAM di bawah 25 MB. |
| `auto_connect` | `bool` | `false` | Menentukan apakah worker langsung memulai koneksi saat startup. |
| `autostart` | `bool` | `false` | Sinkronisasi startup Windows via Registry `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. |
| `show_card_vision` | `bool` | `true` | Menampilkan HUD kartu Vision Screen Share pada Command Center. |
| `show_card_automation` | `bool` | `true` | Menampilkan HUD kartu log otomasi Win32. |
| `show_card_transcript` | `bool` | `true` | Menampilkan HUD kartu transkrip percakapan real-time. |
| `show_card_telemetry` | `bool` | `true` | Menampilkan HUD kartu utilisasi CPU & RAM. |
| `show_card_hotkeys` | `bool` | `true` | Menampilkan HUD kartu panduan pintasan tombol global. |

---

## 3. Logika Sanitasi URL & Penegakan Enkripsi TLS (Audit N5, R1)

### A. Normalisasi Format (`normalize_ws_url`)
* Protokol `https://` otomatis ditranslasikan menjadi `wss://`.
* Protokol `http://` otomatis ditranslasikan menjadi `ws://`.
* Input tanpa skema protokol pada host lokal diasumsikan `ws://`, sedangkan host remote diasumsikan `wss://`.
* Path otomatis dipastikan memiliki akhiran `/ws`.

### B. Validasi Transportasi Ketat (`validate_ws_url` & `is_local_host`)
Untuk melindungi token otentikasi dan data audio dari intersepsi jaringan (*Man-in-the-Middle*):
* **Hanya Host Loopback yang Boleh Menggunakan `ws://`:**
  Hanya alamat IP loopback lokal (`localhost`, `127.0.0.1`, `::1`, `[::1]`) yang diizinkan menggunakan protokol tanpa enkripsi `ws://`.
* **Kewajiban TLS (`wss://`):**
  Semua koneksi ke domain publik, internet, atau IP jaringan lokal LAN (`192.168.x.x`, `10.x.x.x`) wajib menggunakan `wss://`.
* **Anti-Spoofing Authority:**
  Parsing host memisahkan komponen `userinfo` (karakter `@`) dan port, serta memeriksa pembatas host secara ketat sehingga trik spoofing seperti `ws://localhost.evil.com`, `ws://127.0.0.1.nip.io`, dan `ws://127.0.0.1@evil.com` langsung ditolak keras dengan galat validasi.

---

## 4. Persistensi Atomik & Binding State UI

* **Penyimpanan Berkas Atomik (Atomic File Write):**
  Fungsi `BridgeConfig::save()` menulis seluruh string JSON ke berkas sementara `.nexa-bridge.json.tmp`, memastikan seluruh byte ter-flush ke disk fisik, lalu melakukan *atomic rename* ke `.nexa-bridge.json`. Operasi ini mencegah berkas konfigurasi menjadi korup atau 0-byte jika terjadi crash atau pemadaman listrik di tengah penulisan.
* **UI Property Binding:** Jendela Slint mengikat properti konfigurasi (`cfg_server_url`, `cfg_auth_token`, `cfg_low_ram_mode`, kartu HUD, dan autostart).
* **Sinkronisasi Thread-Safe:** Disimpan dalam `Arc<Mutex<BridgeConfig>>`.
* **Callback `on_save_settings`:** Memvalidasi URL via `validate_ws_url`, memperbarui nilai mutex di memori, dan melakukan serialisasi atomik ke disk.
* **Callback `on_test_connection`:** Menjalankan task asinkron terisolasi untuk melakukan handshake TCP + TLS + HTTP Upgrade selama 4 detik tanpa mengganggu koneksi aktif yang sedang berjalan.
