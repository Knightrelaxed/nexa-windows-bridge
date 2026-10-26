# Protokol Komunikasi WebSocket — Nexa Bridge

Dokumen ini mendokumentasikan spesifikasi protokol pertukaran pesan antara **Nexa Bridge** dan **N.E.X.A Server**.

---

## 1. Lapisan Transportasi & Handshake HTTP

Komunikasi berjalan di atas protokol **WebSocket (`ws://` atau `wss://`)**.

### Header HTTP Handshake
Saat memulai inisialisasi koneksi, klien mengirimkan header berikut:

```http
GET /ws HTTP/1.1
Host: nexa-server.domain.com
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Key: <base64-key>
Sec-WebSocket-Version: 13
Authorization: Bearer <auth_token>
X-Device-Type: windows
```

* `Authorization`: Menyertakan bearer token perangkat jika diisi pada konfigurasi.
* `X-Device-Type`: Selalu bernilai `windows` untuk mengidentifikasi jenis klien ke router server.
* **Handshake Timeout (Audit H3):** Proses handshake dibatasi waktu maksimal **10 detik** dan dapat dibatalkan seketika jika pengguna menekan "Disconnect" atau saat laptop memasuki mode sleep (*PBT_APMSUSPEND*).
* **Penanganan Penolakan Token (Audit M2):** Jika server merespons dengan status HTTP `401 Unauthorized` atau `403 Forbidden`, klien menghentikan siklus reconnect otomatis untuk mencegah pemborosan bandwidth/log.

---

## 2. Struktur Pesan (Message Payload Schema)

Pesan dikirim dalam format JSON berstruktur dengan tag pembeda tipe `"type"`.

### A. Pesan dari Klien ke Server (`ClientMessage`)

#### 1. `PING` (Heartbeat & Keepalive Watchdog)
Dikirim secara berkala setiap **5 detik** (Audit H2). Klien memonitor waktu kedatangan paket balasan server; jika tidak ada data atau pong yang diterima selama **>15 detik**, koneksi dianggap setengah terbuka (*half-open connection*) dan bridge segera melakukan pemutusan soket serta inisiasi rekoneksi otomatis.
```json
{
  "type": "PING",
  "timestamp": 1787105000000
}
```

#### 2. `COMMAND_RESULT` (Eksekusi Perintah / ACK)
Dikirim oleh bridge setelah menerima dan memproses perintah `EXECUTE_COMMAND` dari server.
```json
{
  "type": "COMMAND_RESULT",
  "command_id": "cmd_win_98231",
  "success": true,
  "message": "Command 'DESKTOP_CLICK' executed successfully",
  "data": {
    "action": "DESKTOP_CLICK",
    "status": "COMPLETED",
    "execution_time_ms": 12
  }
}
```

#### 3. `DESKTOP_TELEMETRY` (Telemetri Sistem)
Melaporkan beban kerja dan status aktif laptop ke server AI.
```json
{
  "type": "DESKTOP_TELEMETRY",
  "cpu_usage": 18.4,
  "ram_mb": 4250,
  "active_window": "Visual Studio Code - nexa-windows-bridge"
}
```

#### 4. `POWER_EVENT` (Event Daya OS)
Memberi tahu server jika laptop ThinkPad sedang ditutup engselnya (*suspend*) atau dibuka kembali (*resume*).
```json
{
  "type": "POWER_EVENT",
  "event": "SUSPEND"
}
```

#### 5. `CALL_VIDEO_FRAME` (Cuplikan Layar Desktop)
Digunakan untuk fitur vision saat server AI meminta tangkapan layar desktop.
```json
{
  "type": "CALL_VIDEO_FRAME",
  "command_id": "cmd_vision_01",
  "image_chunk": "<base64_encoded_jpeg_or_png>"
}
```

#### 6. `CALL_AUDIO_STREAM` (Stream Audio Input)
Digunakan untuk transmisi rekaman mikrofon lokal ke server N.E.X.A.
```json
{
  "type": "CALL_AUDIO_STREAM",
  "command_id": "cmd_audio_01",
  "pcm_chunk": "<base64_encoded_pcm_audio>"
}
```

#### 7. `CALL_AUDIO_INTERRUPTED` (Notifikasi Interupsi Pengguna / Barge-In)
Dikirim ke server saat detektor energi suara mendeteksi bahwa pengguna mulai berbicara menyela keluaran suara AI.
```json
{
  "type": "CALL_AUDIO_INTERRUPTED"
}
```

---

### B. Pesan dari Server ke Klien (`ServerMessage`)

#### 1. `PONG` (Jawaban Heartbeat)
Balasan server terhadap paket `PING`.
```json
{
  "type": "PONG",
  "timestamp": 1787105000050
}
```

#### 2. `EXECUTE_COMMAND` (Perintah Otomatisasi Desktop)
Instruksi dari server AI untuk mengendalikan perangkat Windows. Seluruh nama aksi dinormalisasi secara kanonikal (`canonical_action`) sehingga mendukung gaya penulisan `camelCase`, `kebab-case`, `snake_case`, maupun `UPPERCASE`.

```json
{
  "type": "EXECUTE_COMMAND",
  "command_id": "cmd_win_98231",
  "action": "DESKTOP_CLICK",
  "params": {
    "x": 500,
    "y": 350,
    "button": "LEFT"
  },
  "timestamp": 1787105000100
}
```

##### Daftar Aksi yang Didukung & Parameter:
1. `DESKTOP_CLICK`: `x`, `y` (skala 0..1000), `button` (`"LEFT"`, `"RIGHT"`, `"MIDDLE"`), `double_click` (bool).
2. `DESKTOP_SCROLL`: `direction` (`"UP"`, `"DOWN"`), `amount` / `steps` (default 3 notch).
3. `DESKTOP_TYPE_TEXT`: `text` (string Unicode), `press_enter` (bool opsional). *Catatan: Diblokir jika jendela aktif adalah konsol terminal.*
4. `DESKTOP_PRESS_KEY`: `key` (`"ENTER"`, `"SPACE"`, `"ESCAPE"`, `"TAB"`, `"BACKSPACE"`, `"UP"`, dll.).
5. `HIGHLIGHT_ELEMENT`: `x`, `y`, `width`, `height` (0..1000), `style` (`"BOX"`, `"RING"`, `"POINTER"`, `"SPOTLIGHT"`), `label` (string opsional).
6. `POINT_CURSOR`: `x`, `y` (0..1000).
7. `CLEAR_HIGHLIGHTS`: Membersihkan seluruh sorotan visual overlay.
8. `LAUNCH_APP`: `app` (`"chrome"`, `"notepad"`, `"calc"`, `"explorer"`, `"vscode"`, `"edge"`) atau `url` (`http://`, `https://`).
9. `LOCK_SCREEN`: Mengunci workstation Windows (`LockWorkStation`).
10. `SHOW_DESKTOP`: Menampilkan desktop Windows instan (`Win+D`).
11. `SET_VOLUME`: `volume_level` (0.0..1.0), `direction` (`"UP"`, `"DOWN"`), `steps` (1..25).
12. `MUTE`, `UNMUTE`, `TOGGLEMUTE`: Kendali pembisuan audio laptop.
13. `TAKE_SCREENSHOT`: Menangkap layar desktop.
14. `GET_BATTERY_STATUS`: Meminta laporan status daya baterai laptop.
15. `GET_SYSTEM_METRICS`: Meminta laporan utilisasi CPU, RAM, dan resolusi.
16. `ABORT_AUTOMATION` (atau `CANCEL`): Fast-path abort instan yang melompati antrean FIFO dan membatalkan otomasi seketika.

#### 3. `CALL_AUDIO_PLAY` & Binary Stream (Stream Audio Balasan Suara AI - Audit N6, R4)
Dapat dikirimkan dalam bentuk JSON dengan teks Base64 (`ServerMessage::AudioPlay`) atau dikirim langsung sebagai paket biner WebSocket murni (`Message::Binary`).
* **Isolasi Aliran Data (Dedicated Audio Channel):** Data audio PCM tidak lagi membanjiri antrean perintah kendali desktop, melainkan disalurkan langsung ke kanal audio terpisah (`subscribe_audio`) dengan penanganan buffer yang menyerap hingga 20 detik audio burst tanpa hambatan.
```json
{
  "type": "CALL_AUDIO_PLAY",
  "pcm_chunk": "<base64_encoded_pcm_24k>"
}
```

#### 4. `CALL_STATUS_UPDATE` (Pembaruan Status Kognitif AI)
Mengubah status kognitif avatar dan teks status HUD antarmuka secara dinamis.
```json
{
  "type": "CALL_STATUS_UPDATE",
  "status": "thinking",
  "text": "Sedang menganalisis struktur kode...",
  "user_transcript": "Bisa jelaskan arsitekturnya?"
}
```

#### 5. `CALL_AUDIO_INTERRUPTED` (Interupsi Audio dari Sisi Server)
Instruksi dari server agar klien menghentikan pemutaran audio speaker seketika.
```json
{
  "type": "CALL_AUDIO_INTERRUPTED"
}
```

#### 6. `CALL_REPLY_COMPLETE` (Selesai Berbicara)
Sinyal bahwa seluruh rangkaian audio respons telah selesai dipancarkan server, mengembalikan avatar ke status `idle`.
```json
{
  "type": "CALL_REPLY_COMPLETE"
}
```

#### 7. `Unknown` (Tipe Tak Dikenal)
Jika server di masa mendatang mengirimkan tipe baru yang belum didukung, deserializer JSON Rust menggunakan atribut `#[serde(other)]` sehingga paket tidak menyebabkan aplikasi *crash*, melainkan diabaikan secara aman.

---

## 3. Siklus Hidup Reconnection (Exponential Backoff)

Jika koneksi terputus atau gagal terhubung:
1. Status koneksi beralih ke `ConnectionState::Reconnecting`.
2. Delay awal diatur sebesar **500 ms**.
3. Setiap kegagalan beruntun, delay digandakan ($500 \text{ ms} \to 1000 \text{ ms} \to 2000 \text{ ms} \dots$) hingga batas maksimum **10.000 ms (10 detik)**.
4. Jika pengguna menekan tombol *"Connect"* secara manual atau laptop bangun dari tidur (*OS Resume*), timer backoff langsung di-*bypass* (*preempted*) untuk melakukan koneksi seketika.
