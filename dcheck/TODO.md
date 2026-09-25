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
- [x] Rilis 0.2.2 (+ tag `dcheck-v0.2.2`) — build aarch64 sempat gagal
      (rustc 1.98 + zig: `--fix-cortex-a53-843419`), lalu tarball x86_64 yang
      diupload ulang tertahan cache Cloudflare → checksum mismatch (update
      menolak dengan aman). Diterbitkan ulang sebagai **0.2.3**; rilis kini
      immutable (deploy menolak menimpa versi yang sudah terbit).

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

## F. Detail storage SSD: tanggal produksi & umur

Pertanyaan: kenapa SSD (Samsung SM863a, SATA, 10.0.0.177) tidak menampilkan
tanggal produksi dan umur?

Temuan (`smartctl -x -j`, fixture `testdata/smart-sata-sm863a.json`):
- **Tanggal produksi tidak disimpan** di SSD/HDD SATA maupun NVMe — ATA dan
  NVMe tidak punya field-nya. Hanya SCSI/SAS yang punya (log page 0x0E), karena
  itu HDD Toshiba bisa menampilkan "2012 week 12". Tidak bisa dibaca oleh tool
  apa pun; yang bisa ditampilkan adalah umur **pakai** dari power-on hours.
- Laporan sebelumnya diam saja soal ini (baris "Manufactured" tidak muncul),
  sehingga terlihat seperti data yang hilang.
- Banyak data SATA yang tersedia tapi belum dipakai — **ATA Device Statistics**
  (GP/SMART log 0x04):
  - p1: power-on resets, logical sectors written/read (SM863a ~2 TB ditulis,
    sebelumnya "Written" tidak muncul sama sekali)
  - p4: reported uncorrectable errors
  - p5: suhu seumur hidup min/max (22–39°C) + rating max operasi (70°C)
  - p6: hardware resets, interface CRC errors
  - p7: **Percentage Used Endurance Indicator** (standar ACS) = 2%, lebih
    tepat dari atribut vendor 233 (1%)
  - juga: jumlah entri SMART error log, TRIM, write cache

Tugas:
- [x] Baris umur untuk semua drive: "In service" dari power-on hours (tahun
      @24/7 + power-on resets); bila tanggal produksi ada → umur kalender +
      persentase waktu menyala
- [x] "Manufactured: not reported" + alasannya untuk SATA/NVMe (report & TUI)
- [x] Parser Device Statistics (smartctl `ata_device_statistics`) dan native
      (SMART READ LOG 0x04 via HDIO_DRIVE_CMD) → written/read, wear standar,
      suhu min/max/rating, resets, CRC, uncorrectable
- [x] Error log count, self-test terakhir (ATA), TRIM, write cache
- [x] Tampilkan di report, JSON, TUI VITALS; verifikasi di 10.0.0.177
- [x] SATA di belakang RAID/SAS controller (bus terlihat SCSI) dikenali sebagai
      ATA dari data SMART-nya → pesan & label yang benar
- [x] Native SATA di belakang MegaRAID: HDIO tidak menembus controller, jadi
      tanpa smartctl hanya suhu yang terbaca. Perlu ATA PASS-THROUGH (SAT,
      SG_IO ATA 16) untuk SMART READ DATA / READ LOG — cek apakah megaraid_sas
      JBOD meneruskannya

## G. Riset: disk palsu / kloningan, RAM health, CPU health (belum dikerjakan)

Pertanyaan: bisakah dcheck menunjukkan disk palsu atau "sampah" kloningan
unbranded, dan apakah ada cek kesehatan RAM dan CPU?

Probe read-only di 10.0.0.251 dan 10.0.0.177 (Dell R630, Xeon Broadwell):
- EDAC aktif (`sb_edac`, mc0–mc3), `ce_count`/`ue_count` = 0 di keduanya.
- `/proc/meminfo HardwareCorrupted` = 0 kB.
- Intel `thermal_throttle/{core,package}_throttle_count` tersedia (0).
- `/sys/devices/system/cpu/vulnerabilities/*` ada 17 entri; microcode 0xb000040.
- Tidak ada driver SPD EEPROM (ee1004/spd5118) yang ter-load.
- Tidak ada rasdaemon/mcelog. Kernel log bersih (hanya init EDAC).
- 10.0.0.177 punya `ipmitool`. Sensor ECC Corr/Uncorr ada, dan **SEL berisi
  event nyata**: "Power Supply AC lost — Asserted" (2026-06-09 dan
  2026-06-18), chassis intrusion, dan drive bay dicabut (07-06/07-07).
  Informasi seperti ini penting untuk teknisi, tapi sekarang tidak terlihat
  di dcheck.

### 1. Disk palsu / kloningan / unbranded

Tidak ada satu tanda yang membuktikan palsu. Yang realistis adalah skor
"authenticity" dari beberapa sinyal, tiap sinyal disertai alasannya, dan
bahasanya "mencurigakan", bukan "palsu":

- **Identitas generik**: model seperti `SSD 512GB` / `NVMe SSD` / `Generic`,
  vendor kosong, serial kosong / nol / pola placeholder
  (`0123456789ABCDEF`, `AA000000000000000001`), firmware dari controller
  generik (mis. string SBFM / T0909A0 / SN…).
- **WWN / OUI tidak cocok**: ATA `wwn` (NAA 5 + IEEE OUI) dan SCSI LU WWN
  menyandi pabrikan, mis. Samsung = `002538` (SM863a: `5 002538 e10249cb0`).
  Brand di nama model yang tidak cocok dengan OUI → rebrand/klon. WWN kosong
  pada SSD "bermerek" juga mencurigakan.
- **NVMe PCI vendor ID vs model**: mis. "Samsung 980" dengan VID controller
  Maxio/Phison/Realtek (`/sys/class/nvme/*/device/vendor`, subsystem VID,
  IEEE OUI di Identify Controller).
- **Kapasitas palsu** (flash drive / SSD murah): kapasitas yang dilaporkan >
  flash fisik; firmware memutar (wrap) alamat tulis. Tidak bisa dibuktikan
  read-only. Perlu tes tulis-baca-verifikasi seperti f3/H2testw:
  - disk kosong/tak ter-mount: probe destruktif opsional
    `dcheck storage <dev> --verify-capacity` (butuh konfirmasi eksplisit)
  - disk ter-mount: tulis file uji di free space lalu verifikasi (aman
    untuk data)
  - heuristik ringan: kapasitas bukan ukuran standar, rasio harga/merek
    tidak bisa dicek
- **SMART tidak masuk akal**: tidak ada di database smartctl
  (`in_smartctl_database: false` — SM863a pun false, jadi sinyal lemah),
  semua atribut 0, POH 0 padahal data tertulis besar, wear 0% setelah
  ratusan TB, suhu konstan persis, atribut hilang untuk kelasnya.
- **HDD "baru" yang sebenarnya bekas / SMART di-reset**: Seagate FARM log
  (`seagate_farm_log`, smartctl ≥ 7.4) menyimpan jam sebenarnya. Selisih
  POH FARM vs SMART = SMART di-reset (kasus drive "recertified" dijual
  sebagai baru). Juga: tanggal produksi SAS jauh lebih tua dari yang dijual,
  load-unload / start-stop tinggi pada disk "baru".

Tugas:
- [x] Modul `authenticity`: level CONSISTENT / UNVERIFIED / UNBRANDED /
      SUSPICIOUS / LIKELY FAKE + alasan per sinyal; tampil di report
      (bagian AUTHENTICITY), TUI (badge ORIGIN di VITALS, hanya kalau ≥
      UNBRANDED supaya ALERTS tetap kelihatan di layar kecil) dan JSON
      (`authenticity`). Tidak mengubah verdict kesehatan / exit code.
- [x] Tabel OUI → pabrikan: **semua** OUI dari registry IEEE untuk 8 grup
      pabrikan (1662 entri, `src/oui_table.rs`, dibuat oleh
      `scripts/gen-dcheck-oui.sh` dari hwdata/ieee-data). Awalnya tabel
      ditulis dari ingatan (15 OUI); semuanya sudah dicek cocok dengan
      registry, lalu diganti ekstrak lengkap karena Samsung punya 904 OUI
      dan Intel 684. NVMe PCI VID dicek terhadap pci.ids (hwdata).
- [x] Baca WWN: smartctl (`wwn` / `logical_unit_id`), native ATA IDENTIFY
      word 108–111 dan SCSI VPD 0x83, sysfs `wwid`, udev
      `/dev/disk/by-id/wwn-*` (sda Seagate di 10.0.0.251: kernel `wwid`
      ENXIO, udev punya WWN)
- [x] Terverifikasi: lab-243 `SSD 1TB` → UNBRANDED (juga tanpa root);
      10.0.0.251 Seagate×2 + Toshiba, 10.0.0.177 Samsung SM863a + Toshiba×4
      → CONSISTENT
- [ ] Seagate FARM: bandingkan POH FARM vs SMART
- [ ] `--verify-capacity` (non-destruktif di free space; destruktif hanya
      dengan flag + konfirmasi, tidak pernah default)

### 2. RAM health

Sudah ada: usage, swap, ECC total dari EDAC, modul SMBIOS, suhu DDR5.

Bisa ditambah (read-only):
- **Per-DIMM ECC** (`edac/mc*/dimm*/dimm_{ce,ue}_count`, label slot) →
  tunjuk DIMM mana yang bermasalah, bukan cuma total.
- **`HardwareCorrupted`** di `/proc/meminfo` (halaman memori yang di-poison
  oleh kernel) → > 0 = REPLACE.
- **Riwayat error**: rasdaemon DB (`/var/lib/rasdaemon/ras-mc_ctx.db`),
  mcelog, kernel log (`EDAC ... CE/UE`, `Hardware Error`). Counter EDAC
  hilang saat reboot, riwayat tidak.
- **IPMI / iDRAC SEL**: event memori (ECC, "Correctable memory error
  logging disabled", DIMM failed), juga PSU/kipas/suhu → bagian "Platform".
- **Konfigurasi**: kecepatan terkonfigurasi < rating modul, campuran part
  number / rank / ukuran, populasi channel tidak seimbang, modul non-ECC di
  server → catatan (bukan error).
- **SPD EEPROM** (driver `ee1004`/`spd5118`, perlu modprobe): pabrikan,
  **tanggal produksi modul**, part number → juga deteksi RAM rebrand
  (SPD ≠ SMBIOS/label).
- **Tes aktif opsional**: `dcheck ram --test SIZE` (pola tulis/baca di
  memori bebas, seperti memtester; tidak bisa menguji memori yang dipakai
  kernel). Tes penuh butuh boot memtest86+ — kandidat menu WayangOS.

Tugas:
- [ ] Per-DIMM ECC + slot label, `HardwareCorrupted`, riwayat
      rasdaemon/kernel log
- [ ] Deteksi konfigurasi (speed / mixed / channel balance / non-ECC)
- [ ] SPD (opsional, bila driver ada) + tanggal produksi DIMM
- [x] IPMI SEL / sensor (native via `/dev/ipmi0`, tanpa ipmitool) —
      dikerjakan di modul board (TODO S, 0.5.0)
- [ ] `dcheck ram --test` (opsional, eksplisit)

### 3. CPU health

Sudah ada: model, topologi, clock, cache, suhu (hwmon), load 1m.

Bisa ditambah (read-only):
- **Machine Check Exceptions**: kernel log `mce: [Hardware Error]`,
  rasdaemon/mcelog, `/sys/devices/system/machinecheck` → CPU/cache/bus
  error → MONITOR/REPLACE.
- **Thermal throttling**: `thermal_throttle/{core,package}_throttle_count`
  (+ `_max_time_ms`) per core → pendingin/pasta/kipas bermasalah.
- **Suhu vs batas**: `coretemp` `temp*_crit` (Tjmax), selisih antar-core
  dan antar-socket.
- **Core offline / hilang**: `present` vs `online`, jumlah core tidak sesuai
  model.
- **Clock**: frekuensi sekarang vs max (governor powersave / throttling
  platform), BIOS power profile.
- **Microcode & kerentanan**: versi microcode, isi `vulnerabilities/*`
  ("Vulnerable" → catatan keamanan).
- **IPMI**: sensor Processor (IERR, thermal trip, config error) dan SEL.
- **Stress test singkat opsional** (`dcheck cpu --stress 60s`): lihat suhu
  puncak + throttle + stabilitas.

Tugas:
- [ ] MCE (kernel log / rasdaemon), throttle counter, Tjmax, core offline
- [ ] Microcode + vulnerabilities (catatan keamanan)
- [ ] IPMI sensor Processor
- [ ] `--stress` opsional

### Temuan kecil lain (dari pengambilan screenshot)
- [ ] `dcheck storage` (daftar teks) menampilkan HEALTH "?" untuk semua
      disk. Daftar tidak membaca SMART; TUI dan `check` sudah benar.
      Seharusnya daftar ikut membaca health (atau diberi keterangan).
- [ ] `dcheck prometheus` belum mengekspor metrik baru (design life, overdue,
      grown defects, uncorrected, phy errors, suhu lifetime).

## H. CPU "MONITOR" tanpa alasan (10.0.0.177)

Laporan: di Dell (10.0.0.177) status CPU "MONITOR" tapi tidak dijelaskan
kenapa.

Temuan:
- `CpuInfo::verdict` membandingkan suhu CPU dengan `temp_warn_c` = 60°C —
  itu batas suhu **disk**. CPU server 64°C normal. Sensor `coretemp` sendiri
  melaporkan batasnya: di E5-2682 v4 (10.0.0.177) high 77°C, crit 87°C; di
  E5-2673 v4 (10.0.0.251) high 93°C, crit 103°C.
- Hanya sensor pertama yang dibaca (socket 0 = 64°C). Socket 1 = **73°C**,
  4°C di bawah batas high dan 9°C lebih panas dari socket 0 — ini yang
  sebenarnya layak diperhatikan (pendingin/kipas sisi socket 1), tapi tidak
  terlihat.
- Alasan verdict tidak ditampilkan di report, TUI, maupun JSON.

Tugas:
- [x] Baca semua sensor package (per socket); pakai yang terpanas
- [x] Batas dari sensor: MONITOR ≥ `temp*_max` (high), REPLACE-level
      catatan ≥ `temp*_crit`; tanpa batas dari sensor → 85°C; config
      `cpu_temp_warn_c` untuk override (terpisah dari `temp_warn_c` disk)
- [x] Catatan bila ≤ 5°C dari batas high, dan bila selisih antar-socket
      ≥ 10°C
- [x] Alasan ditampilkan: report ("Issues"/"Notes"), TUI (ALERTS di panel
      PROCESSOR), JSON
- [x] Verifikasi: 10.0.0.177 → OK + catatan "Package id 1 at 73°C, only 4°C
      below its 77°C limit"; 10.0.0.251 → OK + catatan "Package id 0 runs 16°C
      hotter than Package id 1"

## I. Tanpa dependency: SAT native + opsi installer (0.2.6)

- SAT (ATA PASS-THROUGH 16 via SG_IO) native: SMART READ DATA/THRESHOLDS,
  RETURN STATUS (CK_COND, sense descriptor/fixed), READ LOG 0x04. Dipakai
  otomatis untuk drive "ATA" di bus SCSI (PERC/MegaRAID JBOD) dan USB, dan
  sebagai fallback HDIO. `DCHECK_SAT=1` memaksa jalur ini (uji).
- Verifikasi: 10.0.0.177 SM863a di belakang PERC 3108 dengan
  `DCHECK_NATIVE=1` → status, atribut + threshold, device statistics
  lengkap (sebelumnya hanya suhu); lab-243 (Fedora 44, AHCI) HDIO = SAT =
  smartctl; 10.0.0.251 SAS tidak berubah.
- Installer: `--with-smartmontools` (apt/dnf/yum/zypper/apk/pacman/brew,
  lewat sudo bila perlu, dilewati bila sudah ada), `--dry-run`. Default
  tetap tidak memasang paket.
- Contoh nyata untuk bagian G (disk kloningan): lab-243 punya SSD model
  "SSD 1TB", firmware VE0R6304, **WWN 0 000000 000000000** (tanpa OUI
  pabrikan) → kandidat sinyal "unbranded".
- [x] Tampilkan sumber Rated TBW ("override dari tbw.json" vs tabel) —
      lab-243 punya override `{"ssd 1tb":600}` yang tidak terlihat di report

## J. RAM: 4 modul terpasang, terbaca 2 (lab-243, Fedora 44, X99/C610)

Laporan: di lab-243 terpasang 4 modul RAM, dcheck hanya menampilkan 2.

Temuan:
- dcheck mengambil daftar modul dari **SMBIOS** (dmidecode / DMI sysfs). Di
  board ini BIOS hanya mencatat 2 modul (DIMM_B1, DIMM_D1 — 32 GiB DDR3
  Hynix, rank 4), dua slot lain "NO DIMM". Padahal `MemTotal` = 125.6 GiB
  → **4 × 32 GiB** terpasang. Tabel DIMM firmware tidak lengkap (umum di
  board X99/aftermarket); dcheck mempercayainya tanpa cek silang.
- EDAC tidak aktif di mesin ini (sbridge tidak menemukan controller), jadi
  tidak ada sumber per-DIMM lain; SPD via SMBus (i2c_i801) ada tapi perlu
  modul i2c-dev.
- 10.0.0.251 (R630): SMBIOS 1 modul, **EDAC 2 DIMM** (32 GB per socket),
  OS ~32 GB → kemungkinan memory mirroring atau tabel firmware tidak
  lengkap. Juga tidak terlihat di dcheck.
- Ukuran RAM ditampilkan desimal ("34.4 GB") — seharusnya biner (32 GiB).

Tugas:
- [x] Cek silang: jumlah ukuran modul SMBIOS vs MemTotal. Bila OS melihat
      lebih banyak → catatan "firmware DIMM table incomplete" + estimasi
      jumlah modul (ukuran seragam: ceil(MemTotal / ukuran modul))
- [x] Baca DIMM dari EDAC (label, ukuran, CE/UE per DIMM) bila ada;
      tampilkan dan bandingkan dengan SMBIOS (EDAC > SMBIOS → catatan
      mirroring/sparing atau tabel tidak lengkap)
- [x] Satuan GiB untuk RAM (report, TUI, JSON tetap bytes)
- [x] Peta slot TUI: tampilkan estimasi bila SMBIOS tidak lengkap
- [x] Verifikasi: lab-243 → "4 used / 4 (firmware lists 2)" + catatan
      ~4 × 32 GiB; 10.0.0.251 → "2 used / 24 (firmware lists 1)" + catatan
      EDAC + daftar DIMM memory controller

## K. SSD SATA baru tidak terdeteksi (lab-243, dicurigai palsu/faulty)

Laporan: SSD SATA baru dipasang di lab-243, dicurigai palsu atau rusak.

Temuan (kernel log, read-only):
- Port `ata1` (AHCI C610): perangkat terdeteksi di link, tapi **tidak pernah
  siap** — "link is slow to respond (ready=0)" ~55 detik, turun ke 3.0 Gbps,
  lalu "hardreset failed / reset failed, giving up". IDENTIFY tidak pernah
  dijawab → tidak ada `/dev/sdX`; boot tertahan ~60 detik.
- Tidak ada tool (dcheck, smartctl, lsblk) yang bisa membaca drive dalam
  kondisi ini. Kemungkinan: controller SSD mati/firmware rusak (umum pada
  SSD palsu/rebrand), kabel data/daya, atau port. Cek: ganti kabel/port,
  coba di mesin lain atau adapter USB.
- Saat ini dcheck **diam** soal ini — disk yang gagal di level link tidak
  muncul di daftar sama sekali.

Tugas:
- [x] "Unresponsive devices": deteksi port SATA dengan perangkat yang gagal
      (kernel log `ata*: reset failed`, `link is slow to respond`,
      `/sys/class/ata_link/*` / `ata_port`) dan tampilkan di storage array
      sebagai baris FAILED + alasan; `dcheck check` → exit 3
- [ ] Setelah drive terbaca: jalankan sinyal authenticity (TODO G) dan
      tawarkan `--verify-capacity` (butuh izin eksplisit; destruktif hanya
      pada drive kosong)
- [x] Probe read-only SSD pengganti di lab-243 (`SSD 1TB`, fw VE0R6304),
      contoh nyata "unbranded": model generik tanpa merek, WWN semua nol
      (NAA 0, OUI 000000), tidak ada di database smartctl, atribut vendor
      161–169 tanpa nama. SMART sehat (277 jam, wear 1%), jadi "generik"
      tidak berarti "rusak". "Rated TBW: 600 TB" di report ternyata dari
      override `/root/.config/dcheck/tbw.json` (`{"ssd 1tb":600}`, dibuat
      saat testing 2026-09-23), bukan tabel. Report sekarang menulis
      sumbernya: "(tbw.json override)" / "(built-in table)" (TODO I).
- [x] Verifikasi lab-243: `check` → "ata1 REPLACE … never became ready; 3 ×
      link is slow to respond; link speed was reduced", exit 3; report per
      port dengan langkah selanjutnya. User konfirmasi: kabel/port diganti
      tetap gagal → SSD mati.

## L. Cache pembacaan SMART (sda di 10.0.0.251 lambat tiap dibuka)

Profil (strace) `dcheck storage /dev/sda` di .251: ~3 dtk; 13 perintah
SG_IO, masing-masing 0.3–0.75 dtk lewat PERC (LOG SENSE per page, READ
DEFECT, INQUIRY). Tidak ada timeout — disk SAS + controller memang lambat
per perintah. Masalahnya pengulangan: TUI membaca semua disk saat scan, lalu
membaca ulang disk yang sama setiap kali detail dibuka; `check` membaca 3
disk berurutan (~5 dtk).

Tugas:
- [x] Cache di memori (per proses): hasil scan TUI dipakai saat membuka
      detail → instan; `r` = baca ulang
- [x] Cache di disk dengan TTL (default 10 menit, config `cache_ttl_secs`):
      root → /var/cache/dcheck, user → ~/.cache/dcheck; kunci = device +
      model + serial + ukuran (disk diganti = cache tidak dipakai)
- [x] Monitoring selalu segar: `check`, `watch`, `prometheus`, `--json`
      tidak memakai cache; `--fresh` / `DCHECK_NO_CACHE=1` untuk memaksa
- [x] Tampilkan umur data ("cached 3 min ago") di report / TUI
- [x] Baca disk paralel (scan TUI dan `check`)
- [x] Terukur di .251: `storage /dev/sda` 3.4 dtk → 0.01 dtk (cache);
      `check` 5.1 → 3.9 dtk (paralel); `prometheus` ~18 dtk (6 baca/disk)
      → 3.6 dtk (sekali baca, paralel). Identitas (INQUIRY 0.75 dtk) ikut
      disimpan bersama SMART. Kunci + WWID sysfs bila ada.

## M. macOS: vendor RAM tidak terbaca, swap "none" (Mac M2)

Temuan: `dcheck ram` di MacBook M2 tidak menampilkan modul/vendor sama
sekali, dan Swap tertulis "none" padahal `vm.swapusage` = 8 GiB (terpakai
7.2 GiB).

Penyebab:
- Parser `system_profiler SPMemoryDataType -json` hanya mengenal format Mac
  Intel (daftar DIMM di `_items`). Di Apple Silicon, RAM on-package, dan
  JSON-nya flat: `{"dimm_manufacturer":"Hynix","dimm_type":"LPDDR5",
  "SPMemoryDataType":"16 GB"}`. Tidak ada `_items`, jadi hasilnya kosong.
- Swap macOS tidak dibaca sama sekali (field swap dibiarkan 0).

Tugas:
- [x] Parser: item tanpa `_items` → satu modul "on-package" (size dari
      `SPMemoryDataType`), plus unit test untuk format Intel dan Apple Silicon
- [x] Swap dari `sysctl vm.swapusage` (+ unit test parser)
- [x] Ikut ketemu: unit test TUI (disk demo) menulis ke cache SMART asli
      (`~/.cache/dcheck`) dengan hasil `null`; run berikutnya membaca itu
      dan 2 test gagal. Cache disk dimatikan saat `cfg(test)`.
- Terverifikasi di Mac M2: modul `on-package 16 GiB LPDDR5 Hynix`,
  Layout "on-package (unified memory, not replaceable)", Swap 7.1/8 GiB.

## N. `dcheck verify`: uji kapasitas asli (fake capacity)

Permintaan: verifikasi kapasitas, supaya disk palsu yang mengaku 1 TB padahal
flash-nya kecil bisa ketahuan. Ini tidak bisa dilihat dari identitas (TODO
G/M); satu-satunya cara adalah menulis data lalu membacanya kembali (seperti
f3 / H2testw).

Desain:
- Setiap blok 4 KiB diberi header: magic, seed run, dan nomor blok global,
  lalu diisi pola pseudo-random dari (seed, nomor blok). Saat dibaca ulang
  bisa dibedakan: blok berisi data blok lain (**wrap-around**, tanda khas
  kapasitas palsu), nol, atau acak (rusak).
- Cache OS di-bypass: fsync + `posix_fadvise(DONTNEED)` (Linux) /
  `F_NOCACHE` (macOS). Tanpa ini, yang terbaca hanya RAM.
- **Mode aman (default)**: tulis file uji ke free space filesystem yang
  ter-mount di disk itu (`.dcheck-verify-<pid>/`), lalu baca semua kembali,
  lalu hapus. Data yang ada tidak disentuh. Default mengisi free space
  dikurangi cadangan 1% (min 256 MiB); `--size` untuk tes cepat. Tiap file
  selesai, beberapa file lama dicek ulang supaya wrap-around ketahuan lebih
  awal tanpa menunggu disk penuh. Ctrl-C → berhenti dan file uji dihapus.
  Konfirmasi dulu (disk jadi hampir penuh sementara); `--yes` untuk skrip.
- **Mode destruktif** (`--destructive`): untuk disk kosong/tidak ter-mount
  (flashdisk/SSD baru). Menulis chunk di posisi tersebar di seluruh kapasitas
  (termasuk ujung akhir), lalu membaca semuanya. Cepat (~1 GiB ditulis),
  tapi **menimpa data**. Ditolak kalau ada partisi ter-mount atau device
  sedang dipakai (O_EXCL); wajib mengetik nama device, tidak bisa dari
  non-TTY.
- Exit: 0 = semua data kembali utuh, 3 = gagal (kapasitas palsu/rusak),
  1 = error/dibatalkan.

Tugas:
- [x] Modul `verify.rs` + perintah `dcheck verify DEV` (alias
      `storage DEV --verify-capacity`)
- [x] Unit test: pola blok, diagnosis wrap/nol/acak, posisi sampel
- [x] Uji nyata: lab-243 mode aman (`--size`), device-mapper palsu
      (1 GiB yang dipetakan berulang ke 256 MiB) untuk mode destruktif:
      harus FAIL + wrap-around; loop device asli harus OK
- [x] Docs: README, man page, HANDOVER
- Catatan desain: mode destruktif awalnya direncanakan "sampling" (chunk
  tersebar, cepat). Dibatalkan: pada fake yang memetakan alamat modulo
  kapasitas asli, sampel tersebar jarang bertabrakan sehingga semua terbaca
  benar (lolos palsu). Kedua mode menulis berurutan + cek ulang sampel
  region awal setelah tiap region → fake gagal begitu tulisan melewati
  kapasitas aslinya.
- Hasil lab-243: mode aman `--size 4G` di SSD `SSD 1TB` PASS (76 MB/s
  tulis, 112 MB/s baca; disk sendiri cuma ~150–180 MB/s baca), tidak ada
  sisa file. device-mapper 1 GiB → 256 MiB: FAIL setelah 272 MiB ditulis,
  "Real size: about 256 MiB", exit 3. Loop 512 MiB asli: PASS. Ditolak:
  loop yang dipegang dm, /dev/sda (ter-mount), nama salah ketik.
- [x] Pengaman tambahan (permintaan: jangan sampai teknisi tidak sengaja
      merusak isi disk): `--destructive` menampilkan isi drive (partisi +
      tanda filesystem/partition table) dan wajib mengetik `ERASE <nama>`
      kalau ada data; mode free space di disk sistem (/, /boot, /var,
      /home, …) ditolak tanpa `--size` atau `--full`. Diuji di lab-243:
      loop ext4 → mengetik nama saja ditolak, file tetap utuh; `ERASE
      loop0` → jalan, PASS; `/dev/sda` tanpa `--size` ditolak.
- Belum: macOS (`F_NOCACHE`, /dev/rdiskN), baca/cek paralel (sekarang I/O
  dan cek pola bergantian, ±470 MB/s batas CPU per thread).

## O. File terhapus: bisa di-restore? (riset, belum dikerjakan)

Pertanyaan: kalau teknisi tidak sengaja menghapus file, datanya sebenarnya
masih ada (hanya "alamat"-nya yang dihapus)? Bisa ditambah fitur restore, dan
melihat peta disk?

Jawaban singkat: **tergantung jenis disk dan filesystem**.
- **HDD**: isi file tetap ada di piringan sampai tertimpa tulisan baru.
  Bisa dipulihkan kalau disk segera berhenti ditulisi.
- **SSD dengan TRIM**: filesystem memberi tahu SSD blok mana yang kosong;
  controller menghapusnya, dan blok itu terbaca nol (lab-243: "TRIM,
  deterministic"). Dengan `discard` di mount option, ini terjadi dalam
  hitungan detik/menit → **praktis tidak bisa dipulihkan**. Tanpa
  `discard`, data bertahan sampai `fstrim.timer` jalan (mingguan, aktif di
  Fedora & Ubuntu).
- Per filesystem:
  - ext4: saat delete, extent/pointer blok di inode dikosongkan → nama
    dan lokasi hilang. Pemulihan lewat jurnal (ext4magic/extundelete,
    hanya kalau jurnal belum berputar) atau *carving* (photorec: mencari
    tanda tipe file di blok kosong; nama file hilang).
  - XFS: mirip ext4 (xfs_undelete, carving).
  - btrfs: copy-on-write, root tree lama kadang masih ada (`btrfs
    restore -t`), tapi dengan discard=async cepat hilang.
  - NTFS/FAT/exFAT: record hanya ditandai "tidak dipakai" → nama +
    lokasi sering masih ada (ntfsundelete, testdisk) → paling mudah.
- Probe read-only:
  - lab-243: `/` btrfs `ssd,discard=async`, fstrim.timer enabled → file
    terhapus di SSD ini praktis langsung hilang.
  - 10.0.0.251: `/` ext4 di HDD (ROTA=1, tanpa discard) → isi masih ada,
    tapi butuh carving/jurnal.

Aturan emas (harus jadi pesan pertama fitur ini): **berhenti menulis ke
disk itu segera**, unmount / remount read-only, buat image (`ddrescue`),
lalu pulihkan **dari image** ke disk lain. Jangan install tool recovery ke
disk yang sama.

Usulan bertahap:
1. `dcheck recover <dev|mount>` **penilaian peluang** (read-only, kecil):
   jenis media, TRIM/discard/fstrim.timer, filesystem, free space
   (semakin penuh semakin cepat tertimpa) → peluang (tinggi / rendah /
   hampir nol) + langkah konkret dan tool yang cocok untuk FS itu +
   perintah image. Juga tawarkan `fstrim.timer`/discard sebagai penyebab.
2. **Peta disk** (usage map): gambar area terpakai / kosong / di-TRIM per
   region (dari bitmap filesystem, atau sampling baca blok kosong: nol =
   sudah di-TRIM). Menunjukkan apakah sisa data masih ada.
3. **Undelete sungguhan**: besar dan berisiko salah; untuk NTFS/FAT/exFAT
   realistis (record masih ada), ext4/XFS hanya carving. Lebih baik
   mengintegrasikan tool yang sudah matang (photorec/testdisk,
   ext4magic) daripada menulis ulang; dcheck menuntun dan menjalankan
   ke image, bukan ke disk asli.

Sudah dikerjakan sekarang: `dcheck verify` (mode free space) memperingatkan
bahwa tes itu menimpa sisa file yang terhapus.

Dikerjakan (poin 1 + 2), `dcheck recover <disk|partisi|path>`, read-only:
- [x] Resolusi target: disk → semua partisi; partisi; path → mount
      terpanjang yang memuatnya; device-mapper (LUKS/LVM) → slave sampai
      disk fisik
- [x] Fakta: media (rotational), TRIM (`queue/discard_max_bytes` di device
      filesystem, jadi ikut stack dm/RAID/USB), mount option `discard`,
      `fstrim.timer` (enabled, last, next), filesystem, used%, disk sistem
      (/, /var, /home …), read-only
- [x] Penilaian peluang HIGH / MEDIUM / LOW / ALMOST NONE + alasan, dan
      langkah konkret dengan perintah yang sudah terisi device/mount:
      cek Trash/backup/snapshot → hentikan fstrim → berhenti menulis
      (remount ro / umount / boot live USB untuk disk sistem) → image
      dengan ddrescue ke disk LAIN → tool per filesystem ke image
- [x] Peta disk (root): sampling baca di N sel (default 512 × 4 blok),
      tiap sel data / kosong (nol atau 0xFF: belum pernah ditulis atau
      sudah di-TRIM) / campuran; per partisi: % sel berisi data vs used%
      filesystem → perkiraan "sisa data di free space"
- [x] Unit test + uji nyata lab-243 (SSD btrfs discard=async, harus
      ALMOST NONE) dan 10.0.0.251 (HDD ext4, MEDIUM, read-only)
- Hasil: lab-243 sda3 (btrfs, SSD, discard=async) → ALMOST NONE; peta: data
  di 4% sampel = 4% used → ~0% free space berisi data lama (efek TRIM).
  sda2 (/boot ext4, SSD tanpa discard, fstrim.timer aktif) → LOW. 10.0.0.251
  sda2 (/ ext4, HDD) → MEDIUM; peta: data di 97% sampel, 63% used → ~92%
  free space masih berisi data lama. Peta 512 sel: 0.2 dtk di SSD, 19 dtk
  di HDD SAS lewat PERC.
- Ikut diperbaiki: `dcheck … | head` panic "Broken pipe" (SIGPIPE
  dikembalikan ke default di main); partisi BIOS-boot kecil tanpa
  filesystem dilewati.
- Catatan: tbw.json override di lab-243 sudah dihapus (2026-09-24).
- Belum: poin 3 (undelete sungguhan), TUI, macOS.

## P. TUI: layar RECOVERY dan VERIFY

Permintaan: verify dan recover juga ada di UI (sebelumnya CLI saja).

Desain:
- Dari report disk atau daftar storage: `u` → RECOVERY, `v` → VERIFY.
- RECOVERY (read-only): panel atas = peluang per filesystem (badge) + peta
  disk berwarna per partisi + % sisa data di free space; bawah = log
  alasan + langkah (scroll, `c` salin). Penilaian tampil dulu, peta
  menyusul (HDD bisa ~20 dtk). Non-root: peta dilewati + keterangan.
- VERIFY: hanya mode free space. Mode destruktif **sengaja tidak ada di
  TUI** (satu salah tekan terlalu mudah); tetap di CLI dengan ERASE <nama>.
  Alur: rencana (target mount, free, cadangan, peringatan backup & file
  terhapus) → pilih ukuran (cepat 8 GiB / penuh; penuh tidak tersedia di
  disk sistem) → `y` untuk mulai → progress tulis/baca live, Esc batal
  (file uji tetap dihapus) → hasil PASS/FAIL + ukuran asli.
- Refactor: verify/recover mengembalikan data (rencana, progress, hasil
  sebagai baris) alih-alih mencetak; CLI dan TUI memakai kode yang sama.
- Demo: recover dan verify tersimulasi (drive palsu di memori) untuk
  screenshot dan test.

Tugas:
- [x] Refactor verify (progress callback, stop flag, plan/execute,
      outcome_lines, simulasi) dan recover (gather → data, lines)
- [x] Layar RECOVERY + VERIFY, tombol, footer, help, copy
- [x] Test TUI (render demo, alur verify simulasi sampai hasil) + snapshot
- [x] Uji nyata lab-243 (TUI lewat pty), rilis 0.3.0 + landing page
- Hasil: 125 test (render layar, alur verify simulasi sampai PASS/FAIL,
  Esc membatalkan → ABORTED, q ditolak selama tes, port mati ditolak).
  lab-243 lewat pty: `u` → ALMOST NONE + peta; `v` → FULL tidak
  ditawarkan (disk sistem); `y` → file uji tertulis; Esc → ABORTED,
  folder uji terhapus, keluar bersih. Snapshot asli: R630 HDD ~92% sisa
  data di free space, SSD lab ~0%.
- Ditemukan & diperbaiki: race tombol stop (flag STOP direset di dalam
  thread; sekarang sebelum thread dibuat); screenshot demo memuat nama
  host Mac (sekarang `--host`).

## Q. Undelete sungguhan (`dcheck undelete`)

Permintaan: bukan cuma saran, tapi benar-benar mengembalikan file terhapus.

Realistisnya per filesystem:
- **NTFS**: record MFT file terhapus hanya ditandai "tidak dipakai"; nama,
  ukuran, timestamp dan data runs (lokasi cluster, termasuk yang
  terfragmentasi) masih utuh sampai record dipakai ulang → pulih lengkap
  dengan nama dan folder. File kecil (< ~700 B) tersimpan di dalam record
  (resident).
- **FAT32**: byte pertama nama diganti 0xE5, rantai cluster di FAT dinolkan
  → nama (LFN) dan ukuran + cluster awal masih ada; isi diasumsikan
  berurutan (benar untuk sebagian besar file yang tidak terfragmentasi).
- **exFAT**: bit InUse di entry dimatikan; nama, ukuran, cluster awal dan
  flag NoFatChain (file berurutan) masih ada → sering pulih utuh.
- **ext4 / XFS / btrfs / lainnya**: alamat data dihapus → **carving**:
  cari tanda awal/akhir file (JPEG, PNG, PDF, ZIP/DOCX/XLSX) di seluruh
  device. Nama file hilang.
- SSD yang sudah di-TRIM: isi sudah nol, tidak ada yang bisa dilakukan.

Keamanan (wajib):
- Sumber hanya dibuka read-only (device atau file image hasil ddrescue).
- Folder tujuan harus di disk LAIN: ditolak kalau berada di disk yang
  sama (partisi mana pun) dengan sumber. Tidak pernah menimpa file yang
  sudah ada di tujuan.
- Sumber yang ter-mount read-write: peringatan keras (OS terus menulis),
  sarankan image dulu.
- Tiap file diberi status: utuh (cluster masih bebas) / kemungkinan
  tertimpa (cluster sudah dipakai file lain) — dicek dari FAT / $Bitmap.

Perintah:
- `dcheck undelete <dev|image>` → daftar file terhapus (read-only)
- `dcheck undelete <dev|image> --to DIR [--match POLA]` → pulihkan
- `dcheck undelete <dev|image> --carve --to DIR` → carving
- TUI: dari layar RECOVERY, daftar file terhapus + pulihkan ke folder.

Tugas:
- [x] Reader sumber (device/image, offset partisi), cek tujuan beda disk
- [x] FAT32 (LFN), exFAT, NTFS (MFT, fixup, $FILE_NAME, $DATA
      resident/non-resident, runlist, path dari parent, $Bitmap)
- [x] Carver JPEG/PNG/PDF/ZIP
- [x] Fixture image asli (dibuat di lab-243: mkfs, tulis, hapus) dalam
      format sparse teks + unit test
- [x] Uji nyata lab-243 (loop image), TUI, docs, landing page, rilis
- Hasil fixture (dibuat di lab-243: mkfs → tulis → hapus → tulis file
  baru): FAT32 3/3 INTACT (LFN + folder), exFAT 2 INTACT + note.txt
  OVERWRITTEN (cluster dipakai after.txt), NTFS (ntfs-3g) 4/4 INTACT dengan
  nama + folder (2 resident), NTFS (ntfs3) 3/3 tanpa nama. **SHA-256 semua
  file hasil pulihan = file asli.** Loop device + `--match` OK; tujuan di
  disk yang sama ditolak; ext4 → saran `--carve`, carving menemukan JPEG.
- Temuan: driver **ntfs3 (Linux) membuang $FILE_NAME** saat delete, $DATA
  tetap → dipulihkan sebagai `$NoName/record-N.<tipe dari isi>`.
- Fixture NTFS dipangkas (hanya MFT terpakai, $Bitmap, data file) supaya
  ~120 KB, bukan 2.4 MB.
- TUI: RECOVERY → `d` = DELETED FILES + **peta blok** (permintaan: "block
  array memory bisa di-show visual"): tiap sel = bagian volume, warna =
  terpakai / kosong / terhapus utuh / terhapus tertimpa / ditandai /
  dipilih; lokasi file terpilih ditampilkan. `space`/`a` tandai, `w` →
  prompt folder tujuan (dicek beda disk), hasil dalam popup.
- Belum: exFAT/FAT32 terfragmentasi (diasumsikan berurutan), NTFS
  $ATTRIBUTE_LIST (file sangat terfragmentasi), nama file NTFS dari index
  slack direktori (untuk kasus ntfs3), carving di free space saja, macOS.

## R. Kecepatan uji kapasitas dalam Mbps

Permintaan: kecepatan di `verify` jangan MB/s, tapi Mbps (megabit/detik)
seperti kecepatan internet.
- [x] Progress, hasil (Written / Read back) dan layar TUI (kecepatan live,
      sebelumnya tidak ada) memakai Mbps: bytes × 8 / 10⁶ per detik.
      Contoh lab-243: 76 MB/s → 608 Mbps.
- [x] Landing page: contoh output ikut diganti.
- Catatan: `storage --bench` (benchmark baca) masih MB/s — belum diminta.

## S. Modul MOTHERBOARD (identitas, BIOS, perangkat, kesehatan)

Permintaan: selain storage/RAM/processor, tambah motherboard: pabrikan,
versi BIOS, perangkat (peripheral), dan kesehatannya.

Probe read-only:
- 10.0.0.251 (R630): DMI Dell PowerEdge R630, board 02C2CP A04, BIOS Dell
  2.19.0 (2023-12-12), boot legacy. Tidak ada sensor board di hwmon (hanya
  coretemp + i350), tapi **/dev/ipmi0** ada (BMC/iDRAC) → kipas, PSU,
  tegangan, suhu, dan SEL lewat IPMI. PCIe: igb x4/x4, PERC x8/x8, AER 0.
- lab-243: DMI "INTEL X99 / Default string" → board generik (DMI tidak
  diisi), BIOS AMI 5.11 (2024-03-05). **GPU AMD 02:00.0 tanpa driver.**
  Tidak ada sensor board (driver nct/it87 tidak ter-load), tidak ada IPMI.
- Bridge PCIe dengan slot kosong melaporkan lebar x0 → abaikan bridge dan
  link x0; cek penurunan hanya pada perangkat ujung.

Desain:
- Identitas: sistem, board, chassis (DMI sysfs; serial hanya root).
- BIOS: vendor, versi, tanggal + umur, mode UEFI/legacy, Secure Boot.
- PCIe: kelas, nama (pci.ids bila ada, else tabel vendor + nama kelas),
  driver, link sekarang vs maks, AER (correctable / nonfatal / fatal).
- USB: vendor/produk/kecepatan (tanpa root hub).
- Sensor: hwmon chip board (bukan coretemp/nvme/drivetemp/RAM) dengan alarm;
  IPMI native via /dev/ipmi0 (SDR full/compact, pembacaan + status
  threshold) + SEL (event terakhir, hitung kritis) — tanpa ipmitool.
- Kesehatan → OK / MONITOR / REPLACE + alasan: sensor kritis, alarm kipas/
  tegangan, AER fatal/nonfatal, link turun, SEL kritis baru-baru ini
  (PSU, memori, prosesor); catatan: BIOS tua, board generik, perangkat
  tanpa driver, intrusi chassis.
- CLI `dcheck board` (alias motherboard/mobo, `--json`), menu TUI item 4
  "Motherboard", kartu di command deck, snapshot.
- macOS: model, chip, firmware (system_profiler).

Tugas:
- [x] board.rs (DMI, BIOS, PCI, USB, hwmon, notes/verdict) + test fixture sysfs
- [x] IPMI native (SDR + reading + SEL) + test parser; banding dengan
      ipmitool di 10.0.0.177
- [x] Report teks/JSON, CLI, TUI (menu, kartu, layar), snapshot, docs
- [x] Uji nyata R630 .251/.177 dan lab-243; rilis 0.5.0 (2026-09-25),
      `dcheck update` 0.4.1 → 0.5.0 diuji di Mac dan .251
- Hasil nyata:
  - 10.0.0.177: IPMI native cocok dengan ipmitool (Temp CPU 66/75 °C,
    Voltage 2 220 V, Current 2 1 A, CPU Usage 25 %, "Status 10.1: AC lost").
    **PSU 1 tanpa AC** → MONITOR "redundancy lost"; SEL: AC lost + chassis
    dibuka 31 Mei, 9 Juni, 18 Juni (temuan lama TODO G kini terlihat).
    BCM5720 onboard x1 dari x2 → catatan (desain Dell), bukan masalah.
  - 10.0.0.251: **PSU 2 tanpa AC** → MONITOR; 14 kipas, suhu, daya 112 W;
    SEL 130 entri. Pembacaan ~2 dtk.
  - lab-243: board generik ("Default string"), BIOS AMI 5.11, GPU AMD
    tanpa driver → catatan; tidak ada sensor board (tanpa BMC / driver
    hwmon).
  - macOS: model, chip, versi firmware.
- Keputusan: PSU tanpa AC = CRITICAL kalau tidak ada PSU lain yang hidup,
  MONITOR kalau ada (redundansi hilang). Nama sensor IPMI yang sama (Dell
  "Status", "Temp") diberi entity: "Status (PSU 1)", "Temp (CPU 2)".
