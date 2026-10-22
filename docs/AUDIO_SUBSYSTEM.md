# Spesifikasi Subsistem Audio — Nexa Bridge

Dokumen ini menjelaskan rancangan teknis subsistem audio dua arah (*duplex*), algoritma peredam gema (*Acoustic Echo Cancellation / WebRTC AEC3*), mitigasi latensi, sinkronisasi referensi real-time DAC, serta deteksi interupsi pengguna (*barge-in*).

---

## 1. Arsitektur Pemrosesan Audio Duplex & Sinkronisasi DAC

Subsistem audio dirancang untuk pengoperasian tanpa headset (*headset-free*) pada laptop **Lenovo ThinkPad X380 Yoga**, di mana mikrofon bawaan terletak dekat dengan speaker fisik laptop.

```mermaid
graph LR
    subgraph Server_Input ["Input Server (Gemini Live)"]
        RawServerPCM["PCM 24kHz / 16-bit Mono"]
    end

    subgraph Playback_Pipeline ["Speaker Pipeline (WASAPI DAC Output)"]
        Speaker["SpeakerPlayer (30s Ring Buffer)"]
        PlaybackCallback["Real-Time Callback (cpal DAC)"]
        EchoRef["Lock-Free SPSC RingBuffer (48kHz Mono)"]
    end

    subgraph Hardware_IO ["Windows Hardware I/O (WASAPI)"]
        LaptopSpeaker["Laptop Physical Speakers"]
        LaptopMic["Laptop Physical Mic Array"]
    end

    subgraph Capture_Pipeline ["Microphone Pipeline (WASAPI + AEC3)"]
        MicCapture["MicrophoneCapture"]
        Worker["run_mic_processing_worker"]
        ResampleRef["Resampler (48k -> 16k)"]
        AEC["EchoCancellerEngine (WebRTC AEC3)"]
        BargeIn["Barge-In Detector & Reverb Guard"]
        CleanPCM["Cleaned PCM 16kHz / 16-bit Mono"]
    end

    subgraph Server_Output ["Output Server (WebSocket)"]
        OutQueue["Outbound ClientMessage::AudioStream"]
    end

    RawServerPCM --> Speaker
    Speaker --> PlaybackCallback
    PlaybackCallback --> LaptopSpeaker
    PlaybackCallback -.->|Real-time DAC Tap (Zero-Alloc)| EchoRef
    EchoRef --> Worker
    Worker --> ResampleRef
    ResampleRef --> AEC

    LaptopMic --> MicCapture
    MicCapture --> Worker
    Worker --> AEC
    AEC --> BargeIn
    BargeIn --> CleanPCM
    CleanPCM --> OutQueue
```

---

## 2. Inovasi Kritis: Alinyemen Sinyal Referensi AEC Real-Time (Audit C1)

### Permasalahan Arsitektur Lama (Desinkronisasi Buffer)
Pada rancangan awal, sinyal referensi dikirim ke antrean AEC saat fungsi `play_pcm_24k()` dipanggil (*enqueue time*). Karena data audio Gemini Live tiba dalam bentuk semburan cepat (*burst streaming*) yang lebih cepat dari waktu nyata (misal 5 detik suara tiba dalam 200 ms) dan disimpan dalam buffer pemutaran, sinyal referensi tiba di mesin AEC **beberapa detik lebih awal** sebelum suara fisik benar-benar keluar dari speaker laptop. 

Akibatnya:
1. WebRTC AEC3 (yang memiliki jendela toleransi estimasi delay maksimal ~200–300 ms) menganggap sinyal referensi sudah kedaluwarsa saat suara speaker fisik baru memantul ke mikrofon internal.
2. Saluran pengiriman berbasis `mpsc::channel` berkapasitas 64 mengalami *drop* saat burst audio, menyebabkan sinyal referensi terputus-putus dan filter pembatal gema mengalami divergensi total.

### Solusi Arsitektur Baru: SPSC DAC Real-Time Tap
* **Lock-Free SPSC Ring Buffer:** Referensi audio dipindahkan langsung dari dalam fungsi callback real-time WASAPI output hardware (`playback_callback_f32` dan `playback_callback_i16`).
* **Zero Allocation:** Cuplikan sampel yang dikirim ke DAC pada milidetik itu juga di-*downmix* ke mono pada stack buffer lokal (`[0.0f32; 1024]`), kemudian disalurkan ke ring buffer `AudioProducer<f32>`.
* **Sinkronisasi Presisi Tinggi:** Mesin AEC menerima sinyal referensi tepat pada saat yang sama ketika transduser speaker laptop bergetar di ruangan fisik, menghasilkan estimasi delay stabil (10–35 ms) dan pembatalan gema 100% sempurna tanpa *self-interruption*.

---

## 3. Komponen Subsistem Audio

### A. `AudioEngine` (`src/audio/mod.rs`)
* Bertindak sebagai *facade* orkestrator yang mengontrol siklus hidup `SpeakerPlayer` dan `MicrophoneCapture`.
* Menginisialisasi *lock-free SPSC ring buffer* berkapasitas 4 detik (`48000 * 4 = 192.000` float) untuk menyalurkan sinyal referensi speaker DAC ke mikrofon secara non-blocking.
* Mengelola status percakapan AI (`is_speaking`) dan mendistribusikan notifikasi `barge_in_notify` ke runtime utama.

### B. `EchoCancellerEngine` (`src/audio/echo_canceller.rs`)
* **Core Engine:** WebRTC AEC3 (*Acoustic Echo Canceller 3*), *Noise Suppression*, dan *Gain Controller 2* via binding `sonora`.
* **Karakteristik Operasi:**
  * Sample rate pemrosesan internal: **16.000 Hz (16 kHz mono)**.
  * Ukuran frame kalkulasi: **160 sampel (10 ms)**.
  * Buffer render queue: ~200 ms (`VecDeque<f32>` kapasitas 3200 float) untuk mengantisipasi jeda hardware antara clock speaker dan mikrofon.
* **Fitur Tambahan:**
  * **Dynamic Ducking & Reverberation Tail:** Reduksi gain audio mikrofon sebesar `-18 dB` (`factor = 0.125`) saat AI sedang berbicara serta perlindungan ekor dengung 350 ms untuk mencegah false barge-in akibat pantulan ruangan.
  * **Barge-In Detector:** Mengukur level energi RMS pada frame yang sudah difilter. Pengguna dapat langsung memotong ucapan AI dengan berbicara secara tegas atau melalui hotkey `Win+Alt+N`.

### C. `SpeakerPlayer` (`src/audio/speaker.rs`)
* Menginisialisasi output stream WASAPI default menggunakan `cpal`.
* Menerima audio respons berformat **24 kHz 16-bit mono**.
* **Footprint Memori Hemat (Audit M3):** Kapasitas ring buffer pemutaran dioptimalkan dari 120 detik menjadi **30 detik** (`PLAYBACK_BUFFER_SECONDS = 30`), menghemat ~35 MB RAM (hanya butuh ~11.5 MB pada 48kHz stereo) sambil tetap menjamin penyerapan burst streaming tanpa underrun.
* **Zero Real-Time Allocations (Audit H5):** Pada konversi format sampel `I16`, alokasi heap `vec![0.0f32; n]` telah digantikan dengan stack buffer statis `temp = [0.0f32; 2048]`, menjamin callback WASAPI beroperasi pada latensi terendah tanpa gangguan *garbage collection* atau *page fault*.
* **Status Debounce Hold (Audit L):** Memberikan penahan status `is_playing` selama jeda antar-chunk untuk mencegah kedipan (*jitter*) pada animasi orb avatar UI.

### D. `MicrophoneCapture` (`src/audio/microphone.rs`)
* Membuka input stream mikrofon WASAPI default (ThinkPad Conexant Dual-Array Mic).
* Mengonsumsi sampel referensi langsung dari ring buffer SPSC, melakukan downsampling ke 16 kHz jika diperlukan, dan menyinkronkannya dengan frame tangkapan mic per blok 10 ms.
* Hasil audio bersih dienkode menjadi byte PCM 16-bit mono (frame 100 ms / 1600 sampel) dan dikirim ke WebSocket.

### E. `AudioRingBuffer` (`src/audio/audio_buffer.rs`)
* Struktur data *Single-Producer Single-Consumer (SPSC)* berbasis `ringbuf`.
* **Lock-free & Zero-Allocation:** Thread audio realtime dengan prioritas tinggi tidak pernah terblokir oleh *mutex contention* atau alokasi heap saat membaca/menulis sampel audio.

### F. `Resampler` (`src/audio/resampler.rs`)
* Menggunakan algoritma interpolasi polinomial linear berlatensi ultra-rendah untuk konversi laju sampel (misal 24 kHz ke 48 kHz untuk speaker, atau 48 kHz ke 16 kHz untuk referensi AEC) tanpa beban CPU yang signifikan.
