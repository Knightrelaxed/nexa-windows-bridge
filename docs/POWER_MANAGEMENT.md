# Spesifikasi Manajemen Daya OS — N.E.X.A Windows Bridge

Dokumen ini menjelaskan implementasi teknis pemantauan siklus daya Windows OS (*Windows Power Broadcast Listener*) untuk optimalisasi baterai dan stabilitas jaringan pada perangkat portabel (khususnya *Lenovo ThinkPad X380 Yoga*).

---

## 1. Latar Belakang Masalah

Laptop convertible seperti ThinkPad X380 Yoga sering mengalami pergantian mode operasional secara dinamis:
1. **Lid Close / Standby / Modern Standby**: Pengguna menutup layar saat berpindah ruangan.
2. **Lid Open / Resume**: Pengguna membuka kembali layar laptop.

Jika aplikasi desktop mempertahankan koneksi WebSocket tanpa mempedulikan event OS Sleep:
* Socket akan terputus paksa dengan timeout TCP atau RST frame.
* Klien akan melakukan perulangan reconnect berulang-ulang (*connection spam*) saat laptop sedang sleep, menghabiskan baterai secara cepat (*battery drain*).
* State sinkronisasi AI di server menjadi korup.

---

## 2. Arsitektur `PowerListener` (Win32 HWND_MESSAGE)

Windows OS mendistribusikan notifikasi perubahan daya melalui pesan jendela `WM_POWERBROADCAST`. Untuk menangkap event ini tanpa memerlukan jendela GUI fisik tambahan, `PowerListener` menggunakan **Message-Only Window**:

```mermaid
graph TD
    WinKernel["Windows NT Kernel / Power Manager"] -->|Broadcast WM_POWERBROADCAST| WinProc["Power Window Procedure (power_wnd_proc)"]
    
    subgraph Win32_Listener_Thread ["Dedicated Win32 Thread"]
        MsgWindow["Message-Only Window (HWND_MESSAGE)"]
        MsgPump["GetMessageW / DispatchMessageW Loop"]
        WinProc --> MsgPump
    end

    subgraph Nexa_Runtime ["Tokio Async Runtime"]
        Callback["Power Event Callback"]
        NetClient["NetworkClient State Machine"]
    end

    MsgPump -->|Trigger Fn(PowerStateEvent)| Callback
    Callback -->|PowerStateEvent::Suspend| NetClient
    Callback -->|PowerStateEvent::Resume| NetClient
```

### Win32 API yang Digunakan
* `RegisterClassW`: Mendaftarkan class window internal bernilai `NexaPowerListenerClass`.
* `CreateWindowExW`: Membuat jendela tersembunyi dengan parent `HWND_MESSAGE`. Jendela ini tidak memiliki tampilan visual, tidak memiliki icon di taskbar, dan tidak menangani input keyboard/mouse.
* `GetMessageW` & `DispatchMessageW`: Message loop standar yang berjalan di thread terisolasi.

---

## 3. Penanganan Kode Event Windows

| Event Win32 | Nilai Hex | Klasifikasi | Respons Aplikasi |
| :--- | :--- | :--- | :--- |
| `PBT_APMSUSPEND` | `0x0004` | OS entering Sleep / Hibernation / Lid Closed | Memanggil `notify_suspend()`. Mengirim WebSocket Close frame, menonaktifkan loop reconnect, dan mengatur UI state ke `Suspended`. |
| `PBT_APMRESUMEAUTOMATIC` | `0x0012` | OS resuming automatically (timer/event) | Memanggil `notify_resume()`. Menandai flag `is_suspended = false`, membangunkan worker via `reconnect_notify`, dan memulai handshake instan. |
| `PBT_APMRESUMESUSPEND` | `0x0007` | OS resuming by user interaction (lid open/button) | Memanggil `notify_resume()`. Penanganan serupa dengan resume automatic. |

---

## 4. Siklus Hidup Penutupan Rapi (Graceful Shutdown)

Ketika aplikasi utama ditutup oleh pengguna:
1. Instans `PowerListener` di-drop.
2. Destructor mengirimkan pesan custom `WM_STOP_LISTENER` (`WM_USER + 101`) ke jendela via `PostMessageW`.
3. Loop `GetMessageW` keluar dari perulangan, memanggil `DestroyWindow(hwnd)`, dan melakukan `join()` pada thread worker OS secara bersih tanpa *resource leak*.
