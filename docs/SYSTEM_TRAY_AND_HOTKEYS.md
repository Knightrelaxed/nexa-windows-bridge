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
3. **Layar Penuh (F11)** (ID `1003`): Beralih antara mode *Companion Window* (380x520) dan *Cybernetic Fullscreen Command Center*.
4. **Berbagi Layar** (ID `1004`): Mengaktifkan/menonaktifkan streaming penangkapan layar Direct3D 11.
5. **Pengaturan** (ID `1005`): Membuka dialog pengaturan endpoint dan token.
6. **Keluar** (ID `1006`): Menutup aplikasi secara bersih (*graceful exit*).

---

## 2. Global Hotkey Listener (`src/platform/hotkeys.rs`)

Pengguna dapat mengendalikan N.E.X.A kapan saja bahkan saat sedang bermain game atau membuka aplikasi layar penuh lain menggunakan pintasan keyboard global Win32:

* **Mekanisme Win32:** Menggunakan API `RegisterHotKey` dan `UnregisterHotKey` pada thread tersendiri yang memompa pesan `WM_HOTKEY`.

| Pintasan Keyboard | Aksi Teknis (`HotkeyAction`) | Fungsi Operasional |
| :--- | :--- | :--- |
| **Win + Alt + N** | `BossKey` | **Boss Key / Quick Summon:** Menyembunyikan aplikasi seketika jika ingin privasi, atau memunculkannya kembali ke layar. |
| **Win + Alt + M** | `ToggleMute` | **Mute Mikrofon Cepat:** Mematikan stream WASAPI tanpa perlu menyentuh mouse. |
| **F11** atau **Win + Alt + F** | `ToggleFullscreen` | **Toggle Layar Penuh:** Beralih ke HUD J.A.R.V.I.S beresolusi penuh. |
| **Win + Alt + S** | `ToggleScreenShare` | **Toggle Vision AI:** Memulai atau menghentikan penangkapan layar GPU. |

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
