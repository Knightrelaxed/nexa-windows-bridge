# Spesifikasi Subsistem Otomasi Input & Panduan Layar — N.E.X.A Windows Bridge

Dokumen ini mendokumentasikan arsitektur teknis modul otomasi desktop ([`src/automation/`](../src/automation)), meliputi simulasi input fisik (*SendInput* mouse & keyboard), denormalisasi koordinat, integrasi *Windows UI Automation*, dan sistem jendela overlay siluman (*Stealth Windowing*).

---

## 1. Arsitektur Otomasi Input Desktop (Milestone 6.1)

Modul [`src/automation/input.rs`](../src/automation/input.rs) bertanggung jawab melakukan injeksi event mouse dan keyboard tingkat rendah menggunakan Win32 API `SendInput`.

```mermaid
graph TD
    subgraph Server_Instruction ["Instruksi dari Server / Gemini Live"]
        NormCoord["Koordinat Ternormalisasi (0 - 1000)"]
        Action["Aksi: Click, Scroll, TypeText, PressKey"]
    end

    subgraph Denormalization ["src/automation/input.rs"]
        ScreenMetrics["GetSystemMetrics(SM_CXSCREEN / SM_CYSCREEN)"]
        PixelCalc["Denormalisasi ke Resolusi Fisik (Px, Py)"]
        AbsCalc["Denormalisasi ke Win32 Absolute (0 - 65535)"]
        NormCoord --> PixelCalc
        ScreenMetrics --> PixelCalc
        NormCoord --> AbsCalc
    end

    subgraph Win32_Injection ["Win32 SendInput API"]
        MouseMove["MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE"]
        MouseClick["MOUSEEVENTF_LEFTDOWN / UP, RIGHTDOWN / UP"]
        MouseScroll["MOUSEEVENTF_WHEEL (WHEEL_DELTA 120)"]
        KeyUnicode["KEYEVENTF_UNICODE (UTF-16 wScan)"]
        KeyVirtual["Virtual Key (VK_RETURN, VK_SPACE, etc.)"]
        
        AbsCalc --> MouseMove
        Action --> MouseClick
        Action --> MouseScroll
        Action --> KeyUnicode
        Action --> KeyVirtual
    end

    subgraph Target_OS ["Windows Desktop Applications"]
        Desktop["Browser, Office, Terminal, Windows Explorer"]
        MouseMove --> Desktop
        MouseClick --> Desktop
        MouseScroll --> Desktop
        KeyUnicode --> Desktop
        KeyVirtual --> Desktop
    end
```

---

## 2. Fitur & Komponen Utama

### A. Denormalisasi Koordinat Bebas Masalah Display Scaling
* Model AI Gemini Live bekerja dengan sistem koordinat ternormalisasi `x: [0..1000]`, `y: [0..1000]`.
* Fungsi `denormalize_to_pixels()` menghitung posisi piksel fisik pada layar monitor dengan memperhitungkan resolusi asli ThinkPad X380 Yoga.
* Fungsi `denormalize_to_mouse_absolute()` mengonversi koordinat secara langsung ke rentang koordinat absolut Windows `0..65535` untuk bendera `MOUSEEVENTF_ABSOLUTE`.

### B. Otomasi Mouse (`desktop_click` & `desktop_scroll`)
* **Klik Tombol Mouse:** Mendukung tombol kiri (`LEFT`), kanan (`RIGHT`), dan tengah (`MIDDLE`).
* **Klik Ganda (*Double-Click*):** Menjalankan siklus klik pertama, jeda mikro 70 ms, lalu siklus klik kedua.
* **Proteksi HUD Nexa Bridge (`is_point_inside_own_interactive_window`):**
  - Mencegah klik AI mengenai antarmuka floating capsule/pill HUD Nexa Bridge sendiri secara tidak sengaja.
  - Setiap titik koordinat diverifikasi terhadap bounding box jendela interaktif non-transparan milik proses sendiri. Jika titik target mengenai HUD, aksi ditolak dengan pesan keamanan ramah yang mengarahkan model untuk memanggil `closeWindow()` alih-alih mengeklik.
* **Pengguliran Layar Pintar (*Smart Scroll*):**
  - Mendukung target koordinat opsional `(x, y)` ternormalisasi (0–1000) agar AI dapat mengarahkan pengguliran ke panel atau area dokumen tertentu.
  - **Deadzone Auto-Centering Recovery:** Pada Win32, event `MOUSEEVENTF_WHEEL` dikirimkan ke elemen tepat di bawah kursor fisik. Jika kursor tertinggal di area tab bar, title bar, atau toolbar atas (`y < 220`) atau di taskbar (`y > screen_h - 60`), browser tidak akan menggulir halaman. Fungsi `desktop_scroll` secara otomatis mendeteksi deadzone via `GetCursorPos` dan memindahkan kursor ke tengah area konten (`norm_x: 500, norm_y: 500`) sebelum memutar roda mouse, menjamin konten selalu tergulir mulus.
  - Menggunakan kalkulasi kelipatan `WHEEL_DELTA` (120 unit per notch) untuk arah `"UP"` dan `"DOWN"`.

### C. Otomasi Keyboard Bebas Layout (`desktop_type_text` & `desktop_press_key`)
* **Pengetikan Teks Unicode:** Menggunakan bendera `KEYEVENTF_UNICODE` dengan nilai `wScan` berisi kode unit UTF-16. Hal ini menjamin pengetikan teks bahasa Indonesia, simbol khusus, maupun aksara non-Latin dapat dilakukan dengan presisi 100% tanpa bergantung pada layout keyboard aktif di Windows.
* **Penekanan Tombol Virtual:** Mendukung tombol-tombol fungsional seperti `"ENTER"`, `"SPACE"`, `"ESCAPE"`, `"TAB"`, `"BACKSPACE"`, `"UP"`, `"DOWN"`, `"LEFT"`, `"RIGHT"`, tombol fungsi `"F1"` - `"F12"`, serta karakter alfanumerik tunggal.

### D. Magnetic Smart-Snap via Windows UI Automation & Timeout Guard (Audit H8)
* Modul [`src/automation/element_finder.rs`](../src/automation/element_finder.rs) mengintegrasikan COM API resmi Microsoft Windows (`IUIAutomation::ElementFromPoint`).
* **Koreksi Presisi Sub-Piksel:** Koordinat target yang dikirim Gemini Live (yang rentan meleset beberapa piksel akibat kompresi visual JPEG) otomatis ditarik secara magnetis (*magnetic snap*) ke batas fisik kontrol OS (`CurrentBoundingRectangle`).
* **Timeout Guard 150ms (`smart_snap_bounding_box_timeout`):** Query COM UI Automation bersifat lintas-proses (*inter-process communication*). Jika aplikasi pihak ketiga target sedang hang (*not responding*), query COM standar dapat memblokir thread hingga timeout RPC OS beberapa detik. Dengan `smart_snap_bounding_box_timeout`, thread pemanggil dibatasi maksimal 150 ms; jika terlampaui, sistem otomatis *fallback* instan ke koordinat AI tanpa mem-freeze dispatcher maupun UI.
* **Filtering & Rasionalitas:** Mengeliminasi elemen yang terlalu kecil (<4px) atau terlalu besar (>95% layar seperti desktop root).
* **AI Vision Fallback:** Jika elemen berada pada kanvas tak berstruktur (diagram/game), sistem otomatis *fallback* memakai *Vision Bounding Box* dari Gemini Live tanpa error (`is_snapped: false`).

### E. Native Win32 GDI Stealth Overlay Window (Bebas Layar Hitam 100%)
* Menggunakan native Win32 GDI Stealth Windowing (`src/automation/highlighter.rs`) menggantikan swapchain OpenGL/DirectComposition Slint layar penuh.
* **Arsitektur Kekebalan Layar Hitam (Black Screen Immunity):**
  - Jendela overlay Slint layar penuh sebelumnya mengalokasikan swapchain DirectX/OpenGL yang saat di-hide/show memicu restrukturisasi DWM dan menyebabkan kedipan layar hitam (*black screen flickers*) pada hardware Intel UHD / ThinkPad Yoga.
  - Native Win32 GDI Stealth Window mengeliminasi masalah ini secara total: menggunakan `CreateWindowExW` native dengan gaya `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` dan `SetLayeredWindowAttributes(hwnd, 0x00000000, 0, LWA_COLORKEY)`.
  - Warna hitam murni RGB(0, 0, 0) 100% tembus pandang hardware tanpa alokasi swapchain grafis baru, tanpa kedipan DWM, dan 100% tembus klik (*click-through*).
* **4 Variasi Gaya Sorotan Visual:**
  - `BOX`: Bounding box neon cyan dengan sudut futuristik berketebalan 3px.
  - `RING`: Cincin konsentris berdenyut melingkari target.
  - `POINTER`: Reticle dan titik penunjuk laser kursor.
  - `SPOTLIGHT`: Efek peredupan layar di sekitar elemen untuk memfokuskan pandangan.
* **Click Ripple Indicator & Auto-Dissolve:**
  - Animasi pembubaran otomatis (*auto-dissolve*) berbasis epoch token atomik. Saat target berhasil diklik, sorotan otomatis dibersihkan tanpa mengganggu alur visual pengguna.

### F. Manajemen Jendela Berkeamanan Tinggi (`close_active_window`)
* **Pengiriman `WM_CLOSE` Resmi:** Menutup aplikasi aktif secara anggun (*graceful*) menggunakan Win32 `PostMessageW(hwnd, WM_CLOSE, 0, 0)`.
* **Proteksi Proses Sistem & Nexa Bridge:** Memblokir penutupan proses kritis Windows (`csrss.exe`, `dwm.exe`, `lsass.exe`, `services.exe`, `smss.exe`, `explorer.exe`).
* **Resolusi Cerdas Z-Order saat Nexa Bridge Terfokus:**
  - Jika jendela aktif kebetulan adalah Nexa Bridge sendiri (misalnya akibat klik tidak sengaja atau kombinasi `ALT+TAB`), sistem tidak melempar error, melainkan otomatis mengenumerasi Z-order jendela top-level dan menutup jendela aplikasi pengguna teratas di bawahnya. Hal ini mencegah error proteksi diri sendiri saat pengguna bermaksud menutup aplikasi kerja di belakang HUD.

### F. Command Dispatcher & Antrean Serial FIFO Ber-Epoch (Milestone 6.4 & 7.1)
* Mengorkestrasi seluruh subsistem otomasi melalui fungsi utama [`handle_execute_command`](../src/automation/mod.rs).
* **Antrean Serial FIFO dengan Isolasi Panic (Audit H10, K2, K9):**
  - Perintah masuk disalurkan ke antrean non-blocking `mpsc::channel(32)` yang diproses secara berurutan (*serial execution*) agar klik dan pengetikan tidak tumpang-tindih.
  - Setiap perintah diikat dengan nomor generasi sesi (`session_generation`). Jika sesi di-reset atau koneksi terputus, perintah basi dibatalkan instan (*stale command dropped*).
  - Setiap eksekusi dibungkus dalam `tokio::spawn` terisolasi sehingga panic tak terduga tidak membunuh loop worker.

### G. Perintah Ekstensi Desktop & Kontrol Sistem OS (Milestone 6.6)
Selain 7 perintah interaksi dasar, subsistem otomasi mendukung perintah sistem terverifikasi:
1. `CLOSE_WINDOW` (atau alias `CLOSE_APP`, `CLOSE_TAB`): Menutup jendela aplikasi yang sedang aktif menggunakan sinyal Win32 `WM_CLOSE`. Mendukung *safe Z-order fallback* jika jendela aktif adalah Nexa Bridge sendiri. Mengabaikan/menolak jika target adalah proses OS terlindungi (`csrss.exe`, `dwm.exe`, `explorer.exe`).
2. `GET_OPEN_WINDOWS`: Mengenumerasi seluruh jendela aplikasi pengguna yang sedang aktif dan terlihat di desktop, lengkap dengan nama proses dan status foreground, tanpa mengekspos proses internal Nexa Bridge.
3. `TASK_VIEW`: Membuka tampilan Windows Task View secara persisten menggunakan kombinasi `Win + Tab` native.
4. `LAUNCH_APP`: Membuka aplikasi resmi (Chrome, Notepad, Calculator, Explorer, VS Code, Edge) atau tautan URL `http(s)://` yang telah disanitasi dari karakter injeksi shell (`&`, `|`, `;`, dll.) menggunakan Win32 `ShellExecuteW`. Menolak keras eksekusi `.exe` bebas, skrip `.bat/.cmd/.ps1`, maupun jalur UNC.
5. `LOCK_SCREEN`: Mengunci workstation Windows seketika melalui native Win32 API `LockWorkStation` (`windows_sys::Win32::System::Shutdown`).
6. `SHOW_DESKTOP`: Menampilkan desktop Windows seketika menggunakan kombinasi native Win32 `SendInput` tombol `Win + D`, bebas dari *console flashing* tanpa memanggil PowerShell eksternal.
7. `SET_VOLUME`: Menyesuaikan volume suara laptop secara proporsional berdasarkan `volume_level` (0.0 - 1.0) atau jumlah langkah (`steps` / `amount` dibatasi 1 - 25 langkah) dengan penekanan tombol `VOLUME_UP` / `VOLUME_DOWN`.
8. `MUTE`, `UNMUTE`, `TOGGLEMUTE`: Mengendalikan mute speaker laptop. `UNMUTE` dipisahkan secara aman dari tombol toggle dengan mengirimkan penekanan tombol `VOLUME_UP` (standar OS Windows untuk memulihkan audio tanpa risiko membisukan kembali).
9. `TAKE_SCREENSHOT`: Menangkap tangkapan layar perangkat.
10. `GET_BATTERY_STATUS`: Membaca telemetri daya real-time melalui Win32 `GetSystemPowerStatus` (level baterai %, status pengisian AC/DC).
11. `GET_SYSTEM_METRICS`: Membaca metrik perangkat keras sistem (CPU, RAM, dimensi layar fisik).
12. `ABORT_AUTOMATION`: Pembatalan otomasi jalur cepat (*fast-path*) yang melompati antrean FIFO, menaikkan epoch generasi sesi, meminta pembatalan atomik, dan menghapus seluruh sorotan layar seketika.

### H. Arsitektur Keamanan Armed Guard & Proteksi Terminal (Audit N2, R2, R3)
* **Default-Deny:** Semua otomasi yang melakukan interaksi fisik (`CLICK`, `SCROLL`, `TYPE_TEXT`, `PRESS_KEY`), meluncurkan aplikasi, atau mengubah sistem wajib memperoleh izin (**Armed**).
* **Lease 15 Menit:** Pengaktifan status armed via System Tray otomatis kedaluwarsa setelah 15 menit menggunakan epoch Unix timestamp, mencegah izin aktif selamanya tanpa pengawasan.
* **Normalisasi Kanonikal (`canonical_action`):** Seluruh aksi diverifikasi konsisten terlepas dari gaya penamaan (`camelCase`, `kebab-case`, `snake_case`, atau spasi).
* **Proteksi Terminal Konsol Latar Depan:** Memeriksa jendela aktif sebelum mengetik; otomatis memblokir `DESKTOP_TYPE_TEXT` dan `DESKTOP_PRESS_KEY` jika jendela aktif adalah `cmd.exe`, `powershell.exe`, `pwsh.exe`, `WindowsTerminal.exe`, `conhost.exe`, atau `regedit.exe`.
* Untuk dokumentasi keamanan lengkap, lihat [Spesifikasi Keamanan & Kebijakan Otomasi](SECURITY.md).

---

## 3. Status Rencana Milestone Subsistem Otomasi

| Milestone | Deskripsi | Status |
| :--- | :--- | :--- |
| **Milestone 6.1** | Desktop Input Automation (`src/automation/input.rs`) — SendInput mouse & keyboard | **SELESAI ✅** |
| **Milestone 6.2** | Magnetic Smart-Snap via Windows UI Automation & Timeout Guard 150ms | **SELESAI ✅** |
| **Milestone 6.3** | Stealth Overlay, Click Ripple, & High-DPI Scaling 150% ThinkPad Yoga | **SELESAI ✅** |
| **Milestone 6.4** | Router EXECUTE_COMMAND & WebSocket Binding di `src/main.rs` | **SELESAI ✅** |
| **Milestone 6.5** | Verifikasi Komprehensif & Audit Review (C1, H1-H8, M1-M8) | **SELESAI ✅** |
| **Milestone 6.6** | Ekstensi Aksi Desktop: Safe App Launch, Lock Screen, Show Desktop, Honest Volume | **SELESAI ✅** |
| **Milestone 7.1** | Antrean Serial FIFO Ber-Epoch, Isolasi Panic, dan Fast-Path Abort | **SELESAI ✅** |
| **Milestone 7.2** | Audit Keamanan Ronde 1 (N1-N11): Eliminasi Injeksi cmd, Armed Guard Tray, Graceful Drop | **SELESAI ✅** |
| **Milestone 7.3** | Audit Keamanan Ronde 2 (R1-R9): Strict TLS Loopback, Terminal Guard, 15-Min Lease | **SELESAI ✅** |
| **Milestone 7.4** | Native Win32 GDI Stealth Overlay Window (`WS_EX_LAYERED`) & Black Screen Immunity | **SELESAI ✅** |
| **Milestone 7.5** | Smart Scroll Deadzone Centering, HUD Click Guard, & Z-Order Close Fallback | **SELESAI ✅** |
