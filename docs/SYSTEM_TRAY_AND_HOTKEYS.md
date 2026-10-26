# Spesifikasi Integrasi Desktop: System Tray, Global Hotkeys & Telemetri

Dokumen ini menjelaskan implementasi fitur integrasi desktop native Windows pada `nexa-windows-bridge`: System Tray (ikon area notifikasi), Global Hotkeys (pintasan keyboard global), dan Pemantau Telemetri Sistem CPU/RAM.

---

## 1. System Tray Controller (`src/platform/system_tray.rs`)

Aplikasi berjalan sebagai background companion yang dapat disembunyikan ke area notifikasi Windows (*System Tray*) tanpa mengonsumsi memori ekstra:

* **Mekanisme Win32:** Menggunakan fungsi `Shell_NotifyIconW` dengan struktur `NOTIFYICONDATAW` yang dikaitkan pada jendela tersembunyi `HWND_MESSAGE`.
* **Interaksi Mouse:**
  * **Klik Kiri (`WM_LBUTTONUP`):** Melakukan *toggle* instan visibilitas jendela (Sembunyikan / Tampilkan di atas jendela lain).
  * **Klik Kanan (`WM_RBUTTONUP`):** Menampilkan menu konteks native (`CreatePopupMenu` dan `TrackPopupMenu`).

### Menu Konteks Tray:
1. **Buka N.E.X.A** (ID `1001`): Memulihkan jendela utama dari posisi tersembunyi/diminimalkan.
2. **Mute / Unmute** (ID `1002`): Menyalakan/mematikan mikrofon secara global.
3. **Toggle Screen Share (Win+Alt+S)** (ID `1006`): Mengaktifkan/menonaktifkan streaming penangkapan layar Direct3D 11.
4. **Toggle Arm/Disarm Automation** (ID `1007`): Memberikan izin eksekusi otomasi desktop secara berbatas waktu (**Lease 15 Menit**) atau menonaktifkannya seketika (Audit N2, R3).
5. **Layar Penuh (F11)** (ID `1003`): Beralih antara mode *Dynamic Capsule Pill* dan *Cybernetic Fullscreen Command Center*.
6. **Pengaturan** (ID `1004`): Membuka dialog pengaturan endpoint, token, dan preferensi kartu HUD.
7. **Keluar** (ID `1005`): Menutup aplikasi secara bersih (*graceful exit*).

---

## 2. Global Hotkey Listener (`src/platform/hotkeys.rs`)

Pengguna dapat mengendalikan N.E.X.A kapan saja bahkan saat sedang bermain game atau membuka aplikasi layar penuh lain menggunakan pintasan keyboard global Win32:

* **Mekanisme Win32:** Menggunakan API `RegisterHotKey` dan `UnregisterHotKey` pada thread tersendiri yang memompa pesan `WM_HOTKEY`.

| Pintasan Keyboard | Aksi Teknis (`HotkeyAction`) | Fungsi Operasional & Fitur Keamanan |
| :--- | :--- | :--- |
| **Win + Alt + X** | `EmergencyStop` | **Tombol Darurat Otomasi (Audit N3, N4):** Menghentikan otomasi yang sedang berjalan seketika (*instant abort*), menaikkan epoch generasi sesi untuk membuang antrean perintah, mendisarm sistem (`armed = 0`), membersihkan sorotan layar, mematikan screen share, dan membisukan mikrofon. |
| **Win + Alt + N** | `BossKey` | **Boss Key / Quick Summon (Audit N4):** Tanpa memandang jendela terlihat atau tersembunyi, Boss Key tanpa syarat selalu menghentikan otomasi dan mendisarm sistem, lalu menyembunyikan/menampilkan jendela instan. |
| **Win + Alt + M** | `ToggleMute` | **Mute Mikrofon Cepat:** Mematikan stream WASAPI tanpa perlu menyentuh mouse. |
| **F11** atau **Win + Alt + F** | `ToggleFullscreen` | **Toggle Layar Penuh:** Beralih ke HUD J.A.R.V.I.S beresolusi penuh. |
| **Win + Alt + S** | `ToggleScreenShare` | **Toggle Vision AI:** Memulai atau menghentikan penangkapan layar GPU Direct3D 11. |

---

## 3. Pemantau Telemetri Sistem Native (`src/platform/telemetry.rs`)

Untuk melengkapi tampilan HUD *Command Center*, modul ini mengumpulkan metrik perangkat keras Windows secara berkala (tiap 2 detik) dengan **alokasi memori nol (*zero RAM allocation*)**:

* **Pengukuran Beban CPU (`sample_cpu_percent`):**
  * Memanggil kernel Win32 `GetSystemTimes` untuk membaca `FILETIME` dari *Idle Time*, *Kernel Time*, dan *User Time*.
  * Menghitung selisih waktu relatif (*delta*) antar-sampel:
    $$\text{CPU \%} = \frac{(\Delta \text{Kernel} + \Delta \text{User}) - \Delta \text{Idle}}{\Delta \text{Kernel} + \Delta \text{User}} \times 100$$
* **Pengukuran Beban RAM (`sample_ram_percent`):**
  * Memanggil `GlobalMemoryStatusEx` dengan struct `MEMORYSTATUSEX`.
  * Membaca nilai `dwMemoryLoad` yang merefleksikan persentase penggunaan memori fisik sistem secara instan.
* **Integrasi UI:** Nilai CPU dan RAM dikirim ke properti Slint `win.set_cpu_percent(cpu)` dan `win.set_ram_percent(ram)` untuk divisualisasikan dalam bentuk bar grafik HUD.
