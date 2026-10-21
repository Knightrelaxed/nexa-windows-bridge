# Spesifikasi Konfigurasi Sistem — N.E.X.A Windows Bridge

Dokumen ini mendokumentasikan skema serialisasi konfigurasi, lokasi persistensi disk, strategi migrasi legacy, serta fungsi normalisasi endpoint pada `nexa-windows-bridge`.

---

## 1. Lokasi Berkas Konfigurasi

Persistensi konfigurasi disimpan pada direktori *User Profile Home* menggunakan pustaka `dirs`:

* **Berkas Utama:** `C:\Users\<NamaUser>\.nexa-bridge.json`
* **Berkas Legacy (Migrasi Otomatis):** `C:\Users\<NamaUser>\.nexa-config.json`

### Logika Resolusi Booting
1. `BridgeConfig::load()` memeriksa keberadaan file utama `~/.nexa-bridge.json`.
2. Jika tidak ditemukan, fungsi memeriksa fallback ke `~/.nexa-config.json`, mengekstrak kunci camelCase `serverUrl` dan `secret`, lalu memicu `save()` untuk mengonversinya ke format standar baru.
3. Jika kedua berkas tidak ada atau korup, fallback ke implementasi `Default::default()`.

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
  "auto_connect": false
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

---

## 3. Logika Normalisasi URL WebSocket (`normalize_ws_url`)

Fungsi statis `BridgeConfig::normalize_ws_url` memastikan sanitasi format input:

* Protokol `https://` otomatis ditranslasikan menjadi `wss://`.
* Protokol `http://` otomatis ditranslasikan menjadi `ws://`.
* Input tanpa skema protokol otomatis diasumsikan `ws://`.
* Path otomatis dipastikan memiliki akhiran `/ws`.

---

## 4. Binding State Antara UI Slint & Storage

* **UI Property Binding:** Jendela Slint mengikat properti `cfg_server_url`, `cfg_auth_token`, dan `cfg_low_ram_mode`.
* **Sinkronisasi Thread-Safe:** Disimpan dalam `Arc<Mutex<BridgeConfig>>`.
* **Callback `on_save_settings`:** Memperbarui nilai mutex di memori sekaligus melakukan serialisasi sinkron via `BridgeConfig::save()` ke disk.
* **Callback `on_test_connection`:** Menjalankan task asinkron terisolasi untuk melakukan handshake TCP + TLS + HTTP Upgrade selama 4 detik tanpa mengganggu koneksi aktif yang sedang berjalan.
