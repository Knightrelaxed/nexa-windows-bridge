# Protokol Komunikasi WebSocket — N.E.X.A Windows Bridge

Dokumen ini mendokumentasikan spesifikasi protokol pertukaran pesan antara **N.E.X.A Windows Bridge** dan **N.E.X.A 3.3 Server**.

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

---

## 2. Struktur Pesan (Message Payload Schema)

Pesan dikirim dalam format JSON berstruktur dengan tag pembeda tipe `"type"`.

### A. Pesan dari Klien ke Server (`ClientMessage`)

#### 1. `PING` (Heartbeat)
Dikirim secara berkala setiap **20 detik** untuk menjaga soket tetap aktif dan mendeteksi koneksi terputus (*zombie connection*).
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
Instruksi dari server AI untuk mengendalikan perangkat Windows.
```json
{
  "type": "EXECUTE_COMMAND",
  "command_id": "cmd_win_98231",
  "action": "DESKTOP_CLICK",
  "params": {
    "x": 1280,
    "y": 720,
    "button": "LEFT"
  },
  "timestamp": 1787105000100
}
```

#### 3. `Unknown` (Tipe Tak Dikenal)
Jika server di masa mendatang mengirimkan tipe baru yang belum didukung, deserializer JSON Rust menggunakan atribut `#[serde(other)]` sehingga paket tidak menyebabkan aplikasi *crash*, melainkan diabaikan secara aman.

---

## 3. Siklus Hidup Reconnection (Exponential Backoff)

Jika koneksi terputus atau gagal terhubung:
1. Status koneksi beralih ke `ConnectionState::Reconnecting`.
2. Delay awal diatur sebesar **500 ms**.
3. Setiap kegagalan beruntun, delay digandakan ($500 \text{ ms} \to 1000 \text{ ms} \to 2000 \text{ ms} \dots$) hingga batas maksimum **10.000 ms (10 detik)**.
4. Jika pengguna menekan tombol *"Connect"* secara manual atau laptop bangun dari tidur (*OS Resume*), timer backoff langsung di-*bypass* (*preempted*) untuk melakukan koneksi seketika.
