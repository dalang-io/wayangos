# dcheck — TODO

## A. Perbaikan dari analisa

- [x] Webhook: `devices` dikirim sebagai object JSON, bukan string (double-encoded) — `monitor.rs`
- [x] Satukan severity: `Verdict::severity()` vs `monitor::severity`, perbaiki doc 0–3 vs 4 — `health.rs`
- [x] Cache `config::load()` (sekarang baca disk per `evaluate()`) — `config.rs`
- [x] Estimasi umur berbasis wear: confidence turun bila wear used ≤ 2% — `health.rs`
- [x] Heuristik "host writes implausibly low" hanya setelah ≥ 720 jam — `health.rs`
- [x] Guard FFI 64-bit (`compile_error!` untuk target 32-bit) — `native.rs`, `mount.rs`
- [x] Menu teks non-TTY masih "RAM/CPU (coming soon)" — `main.rs`
- [x] Bersihkan warning `cargo clippy --all-targets`
- [x] Sinkronkan `docs/DCHECK.md` (status RAM/CPU/macOS, istilah Go, diagram arsitektur)

## B. Rework TUI — tema sci-fi

- [x] Pecah `src/tui.rs` → `src/tui/{mod,theme,widgets,views}.rs`
- [x] Tema neon truecolor (auto via `COLORTERM`, override `DCHECK_COLOR`) + fallback ANSI
- [x] Widget HUD: panel bracket, segmented gauge, badge, keycap footer, spinner braille
- [x] Splash boot ≤ 0,5 dtk (skip: tombol apa pun, `--plain`, `DCHECK_NO_SPLASH`, `"splash": false`)
- [x] Header `◢◤ DCHECK // DEVICE HEALTH SYSTEM` + hostname + status global
- [x] Menu "command deck" dengan kartu ringkas (storage / RAM / CPU, prefetch di background)
- [x] Storage: badge health berwarna + mini gauge life%
- [x] Report: dashboard VITALS (verdict, gauge life/suhu/TBW, issues) + TELEMETRY LOG
- [x] RAM: gauge used/swap, peta slot DIMM, ECC, suhu
- [x] CPU: gauge load/suhu, grid core, clock, cache
- [x] Help sebagai overlay modal
- [x] Layout kompak untuk terminal kecil (< 60×16)
- [x] Test snapshot `TestBackend` (80×24, 140×40, plain ASCII-only, NO_COLOR)
- [x] Update README, `dcheck.1`, config `splash`

## C. SAS/SCSI health di Dell R630 (PERC H330 / MegaRAID SAS-3008)

Laporan: `sudo dcheck` di Dell server menampilkan "SMART telemetry unavailable —
run as root or install smartmontools", padahal sudah root dan health seharusnya
dibaca native (tanpa smartctl).

Temuan (probe SG_IO langsung di 10.0.0.251, disk SEAGATE ST900MM0006 /
TOSHIBA AL13SEB900 / SEAGATE ST2000NX0273 via `megaraid_sas`, JBOD):
- INQUIRY dan LOG SENSE page 0x00 (daftar page) **berhasil** → controller
  tidak memblokir pass-through.
- LOG SENSE page 0x0D/0x02/0x03 **gagal**: CHECK CONDITION, ILLEGAL REQUEST,
  ASC 0x24 "invalid field in CDB", field pointer → **byte 1**.
- Akar masalah: `native.rs::log_page` menaruh PC (page control) di byte 1 CDB.
  Di LOG SENSE, PC ada di **byte 2 bit 7–6** bersama page code
  (`[0x4D, 0x00, pc | page, ...]`). Byte 1 hanya SP/PPC. Jadi semua LOG SENSE
  selalu ditolak di disk SAS mana pun → `scsi_read` → `None` → fallback
  "unavailable". Note lama "controller may block LOG SENSE" salah diagnosa.
- Pesan UI/report juga menyesatkan: menyarankan "run as root" walau sudah root,
  dan "install smartmontools" walau jalur native seharusnya cukup.

Tugas:
- [x] Perbaiki CDB LOG SENSE (PC di byte 2) + unit test untuk susunan CDB
- [x] Baca page 0x2F (Informational Exceptions) → status SMART SAS (ASC/ASCQ)
- [x] Tambah power-on hours / start-stop cycles (page 0x0E) dan uncorrected
      errors (0x02/0x03), grown defects tetap dari READ DEFECT DATA
- [x] Pesan "unavailable" kontekstual: sebut root hanya bila bukan root,
      smartmontools hanya sebagai opsi tambahan, dan tampilkan alasan nyata
- [x] Bug serupa di READ DEFECT DATA(10): REQ_GLIST juga di byte 1 → pindah ke
      byte 2 (+ format 4). Counter "uncorrected" memakai param 0x0005 (bytes
      processed) → seharusnya 0x0006. Page 0x0E param 0x0003 = rating siklus,
      yang aktual 0x0004.
- [x] Verifikasi di R630: 3 disk SAS → OK / passed, suhu 36–42°C, power-on
      7.7k–92k jam, start-stop 47–104, host writes/reads terbaca
- [x] Rilis 0.2.1 ke wayang.dalang.io supaya bisa di-`dcheck update`

## D. HDD SAS minim info + tanpa estimasi umur (10.0.0.177, PERC 3108)

Laporan: HDD TOSHIBA MBF2300RC (SAS 10k rpm, 300 GB) cuma menampilkan verdict,
suhu, power-on — tanpa estimasi umur.

Temuan:
- Server ini punya `smartctl`, jadi sumbernya smartctl. Parser SCSI kita di
  `smartctl.rs` hanya mengambil suhu + power-on; field yang tersedia di JSON
  diabaikan: `scsi_start_stop_cycle_counter` (tanggal produksi, start-stop
  40/50000, load-unload 1785/200000), `scsi_grown_defect_list`,
  `scsi_error_counter_log` (458 TB dibaca, 58.6 TB ditulis, uncorrected 0),
  `scsi_self_test_0`, `rotation_rate`, `form_factor`, `temperature.drive_trip`,
  `scsi_sas_port_*` (link 6 Gbps, invalid dword 724, loss of sync 181).
- Jalur native (0.2.1) sudah lebih lengkap dari smartctl untuk SAS, tapi tidak
  dipakai karena smartctl menang duluan. Tidak ada penggabungan sumber.
- `health.rs` hanya punya model umur berbasis TBW/wear (SSD). HDD tidak punya
  TBW → "Life left: unknown". Rencana di `docs/DCHECK.md` §10 (design life HDD)
  belum pernah diimplementasikan.
- Estimasi SSD dari wear 1% terlalu liar (SM863a: "~31.5y @24/7").

Tugas:
- [x] Gabungkan sumber: native + smartctl saling melengkapi (field kosong diisi
      dari sumber lain), bukan salah satu
- [x] Parser smartctl SCSI: tanggal produksi, start-stop & load-unload (aktual +
      rating), grown defects, error counter log, self-test terakhir, rotation,
      form factor, trip temperature, SAS phy (link rate + error counter)
- [x] Native SCSI: page 0x0E lengkap (tanggal produksi, rating), 0x06
      non-medium errors, 0x10 self-test terakhir
- [x] Model umur HDD: design life (5 th 24/7 = 43.800 jam) + rasio start-stop
      dan load-unload terhadap rating → wear %, sisa jam, catatan bila sudah
      melewati design life; umur sejak tanggal produksi
- [x] ~~Batasi estimasi "> 10 th"~~ → keputusan: **tanpa cap**. Estimasi
      ditampilkan apa adanya (keandalan dibaca dari baris Confidence); HDD yang
      lewat design life menampilkan "0 — past its rated life by ~X y"
- [x] Tampilkan semua field baru di report teks, JSON, dan VITALS TUI
- [x] Ceph RBD / DRBD / bcache tidak lagi terdaftar sebagai disk (sebelumnya
      ~60 baris UNKNOWN di 10.0.0.177 dan `dcheck check` exit 1)
- [x] Toolchain dinaikkan ke Rust 1.98.1 (`rust-toolchain.toml`, rust-version)
- [x] Verifikasi: 10.0.0.177 → 5 disk OK, Toshiba design life 57–59%
      (~2.1–2.2 th @24/7), exit 0; 10.0.0.251 → Toshiba 92k jam dan Seagate
      77k jam jadi MONITOR (lewat design life ~5.5 / ~3.8 th), exit 2
- [x] Rilis 0.2.2 (+ tag `dcheck-v0.2.2`)

## E. Design life HDD: asumsi, bukan data drive

Pertanyaan: design life HDD berapa lama, dan apakah ditampilkan?

- Nilainya **5 tahun @24/7 = 43.800 jam** (`health::HDD_DESIGN_HOURS`). Ini
  **asumsi** dcheck: umumnya service life / garansi HDD enterprise, dan HDD
  consumer jarang di-rating lebih tinggi. SAS/SATA tidak melaporkan design
  life, jadi tidak bisa dibaca dari drive.
- Yang **dibaca dari drive**: rating start-stop dan load-unload (SCSI page
  0x0E / smartctl `scsi_start_stop_cycle_counter`), dibandingkan dengan nilai
  aktualnya.
- Sebelumnya angka 5 tahun hanya muncul samar di "Estimate from" (tanpa jam),
  dan tidak ada di TUI/JSON.

Tugas:
- [x] Report: baris "Design life" menyebut 43.800 h (5 y @24/7, diasumsikan)
      beserta limit yang dominan dan rating siklus dari drive
- [x] TUI VITALS: field DESIGN dengan info yang sama
- [x] JSON: `design_life_hours` + `design_life_assumed: true`
- [x] Bisa di-override lewat config (`hdd_design_years`) untuk drive dengan
      rating berbeda (mis. consumer / NAS)
