# dcheck — Handover

Status per 2026-09-23 · versi rilis **0.2.8** (tag `dcheck-v0.2.8`, master
`9ea72fd`).
Catatan kerja rinci per topik ada di [`TODO.md`](TODO.md) (bagian A–L), dan
desain lengkapnya di [`../docs/DCHECK.md`](../docs/DCHECK.md).

## 1. Apa itu dcheck

Tool CLI/TUI untuk mengecek kesehatan **storage** (SAS, SATA, NVMe), **RAM**,
dan **CPU**. Satu binary tanpa dependency (Linux: musl statis; macOS). Target
utamanya teknisi server:

- SMART dibaca **native**: ATA HDIO, SCSI/SAS log pages, NVMe admin, dan
  ATA pass-through (SAT) untuk SATA di belakang RAID/USB. smartctl opsional
  sebagai sumber kedua, lalu kedua sumber digabung.
- Estimasi sisa umur: SSD dari wear/TBW, HDD dari design life (asumsi 5
  tahun) dan rating siklus drive.
- Verdict: OK / UNKNOWN / MONITOR / BACK UP NOW / REPLACE. `dcheck check`
  keluar dengan exit 0/1/2/3 sesuai verdict terburuk.
- Monitoring: `watch` + webhook, `prometheus`, `--json`.
- Drive mati yang tidak pernah muncul sebagai `/dev/sdX` dideteksi dari log
  kernel (tampil sebagai `ataN REPLACE`).
- TUI bertema "sci-fi HUD", dengan fallback ANSI / `--plain` / `NO_COLOR`.
- Self-update: `dcheck update` (verifikasi SHA-256).

Landing page: <https://wayang.dalang.io/apps/dcheck.html> (juga ada bagian di
`download.html`).

## 2. Peta kode (`dcheck/src`, ±11.8k baris)

| File | Isi |
|---|---|
| `main.rs` | Routing CLI (`storage`, `check`, `watch`, `prometheus`, `ram`, `cpu`, `tui`, `update`, `snapshot`), flag global `--fresh` |
| `enumerate.rs` | Daftar disk dari `/sys/block`, port ATA yang gagal (dari `kernlog`), macOS `diskutil`, FreeBSD, data demo (`demo_devices` / `demo_smart`) |
| `native.rs` | ioctl: HDIO (ATA), SG_IO (SCSI log pages, READ DEFECT, SAT ATA-16), NVMe admin; parser murni yang bisa dites di semua OS |
| `smartctl.rs` | `SmartData` (model data SMART), parser `smartctl -x -j`, `fill_from` (menggabungkan sumber), codec JSON untuk cache, ATA Device Statistics |
| `health.rs` | Verdict, TBW (tabel + override `tbw.json`), model umur HDD (`hdd_design_hours`) |
| `report.rs` | Report teks, JSON, Prometheus, `read_smart` (cache → smartctl + native), `metrics_all` (baca paralel) |
| `authenticity.rs` | Cek keaslian: WWN/OUI vs merek, NVMe PCI VID, model/serial generik; `oui_table.rs` (generated, jangan diedit) |
| `cache.rs` | Cache SMART: di memori + di disk dengan TTL; mode fresh |
| `kernlog.rs` | Membaca `/dev/kmsg` / `$DCHECK_KMSG` → status port ATA |
| `ram.rs` | meminfo, EDAC (total + per-DIMM), SMBIOS (dmidecode → sysfs DMI → lshw), cek silang firmware vs OS vs EDAC |
| `cpu.rs` | Topologi, clock, sensor suhu per socket beserta batas dari sensor |
| `monitor.rs` | `check`, `watch`, webhook (lewat curl) |
| `recover.rs` | `dcheck recover`: peluang pemulihan file terhapus (media, TRIM, discard, fstrim, filesystem), langkah + tool, peta disk sampling (read-only) |
| `verify.rs` | `dcheck verify`: tulis data berlabel alamat lalu baca ulang (kapasitas palsu); mode free space dan `--destructive` |
| `update.rs` | Self-update dari `https://wayang.dalang.io/dcheck` |
| `tui/` | `mod.rs` (state/event; RECOVERY `u` dan CAPACITY TEST `v`), `views.rs` (layar), `widgets.rs`, `theme.rs`, `snapshot.rs` (render layar ke SVG), `tests.rs` |
| `config.rs` | `~/.config/dcheck/config.json` (dibaca sekali per proses) |

Fixture test: `testdata/*.json` (output smartctl asli: SAS Toshiba, SATA
SM863a, sampel SATA).

## 3. Build, test, rilis

Toolchain dikunci di **Rust 1.98.1** (`rust-toolchain.toml`). Cross-link ke
Linux dari macOS memakai `zig cc`.

```bash
cd dcheck
cargo test                                  # ±96 unit test
cargo clippy --all-targets                  # harus bersih
cargo clippy --target x86_64-unknown-linux-musl --all-targets   # modul linux
../scripts/test-dcheck.sh                   # e2e fixture (40 assertion)
```

Rilis (jalankan dari Mac, supaya build macOS ikut):

1. Naikkan `version` di `dcheck/Cargo.toml`. Samakan juga string versi di
   `landing-page/apps/dcheck.html` (`sed -i '' 's/0\.2\.8/0.2.9/g' …`).
2. `./scripts/deploy-site.sh`: build 4 target (Linux x86_64/aarch64, macOS
   arm64/x86_64), upload `v<ver>/`, set `LATEST`, sinkronkan landing page.
   Hanya landing page: `SKIP_DCHECK=1 ./scripts/deploy-site.sh`.
3. Verifikasi: `curl -s https://wayang.dalang.io/dcheck/LATEST`, lalu tes
   `dcheck update` dari versi sebelumnya di mesin uji (pakai direktori
   sementara: `DCHECK_INSTALL_DIR=$(mktemp -d)`).
4. `git tag -a dcheck-vX.Y.Z` lalu push master dan tag. Tag `v0.x` di repo
   ini milik **WayangOS**; tag dcheck selalu berawalan `dcheck-`.

## 4. Jebakan yang sudah pernah kena

- **Rilis immutable.** Cloudflare meng-cache tarball (4 jam per PoP). Jangan
  pernah menimpa versi yang sudah terbit, karena user akan kena checksum
  mismatch (kasus 0.2.2). Selalu naikkan versi. `deploy-site.sh` menolak
  menimpa versi lama (`FORCE_REPUBLISH=1` hanya untuk darurat).
- **Build aarch64 dengan rustc ≥ 1.98** mengirim flag
  `-Wl,--fix-cortex-a53-843419` yang ditolak zig. Flag ini dibuang di wrapper
  `build-dcheck.sh`. Deploy menolak jalan kalau ada target yang gagal build.
- **FFI hanya 64-bit.** Struct ioctl/statvfs mengasumsikan LP64 (Linux 32-bit
  sengaja gagal compile). Pakai `std::ffi::c_char`, bukan `i8` (dulu
  merusak build aarch64).
- **CDB SCSI.** Page control LOG SENSE ada di byte 2, bukan byte 1 (bug lama
  yang membuat semua SAS "unavailable"). Ada unit test untuk susunan CDB;
  jangan diubah tanpa test.
- **`dcheck/target/debug/dcheck` di Mac dev adalah file lama milik root**
  (sisa build dengan sudo) dan tidak ditimpa cargo. Untuk tes manual, build
  ke `CARGO_TARGET_DIR` lain.
- **Tailwind di landing page itu prebuilt** (`assets/tailwind.css`, tanpa
  build script). Class baru harus sudah ada di CSS itu; kalau tidak, pakai
  `<style>` lokal. i18n memakai atribut `data-en` + `data-id`.
- **Test e2e** memakai `DCHECK_NO_CACHE=1`. Tanpa itu cache mengacaukan
  assertion.
- **`dcheck verify` menulis ke disk.** Jangan dijalankan di server
  produksi (mis. host landing page); uji di lab-243. Untuk simulasi drive
  palsu pakai device-mapper (lihat TODO N), bukan disk asli.
- **Screenshot landing page** dibuat dengan
  `dcheck snapshot DIR --host dell-r630 --mask-serials` di server asli.
  Output CLI **tidak** otomatis dimask; samarkan serial secara manual sebelum
  ditempel ke halaman publik.

## 5. Infrastruktur rilis (ringkas)

- Situs + kanal rilis: static site `wayang.dalang.io` di host rilis (lihat
  default `HOST`/`REMOTE_DIR` di `scripts/deploy-site.sh`), disajikan oleh
  unit systemd `wayang.dalang.io.service` (python `http.server` di
  localhost) di belakang proxy Pingora milik Dalang, lalu Cloudflare.
- Layout kanal: `/dcheck/install.sh`, `/dcheck/LATEST`,
  `/dcheck/v<ver>/{dcheck-<ver>-<target>.tar.gz, SHA256SUMS}`.
- Route proxy untuk host ini sudah ada. Proxy **tidak** hot-reload file
  konfigurasinya (restart memengaruhi semua situs Dalang). Detail proxy
  sengaja tidak ditulis di sini; tanyakan ke pemilik infra.

## 6. Mesin uji yang dipakai

| Mesin | Hardware | Dipakai untuk |
|---|---|---|
| Dell R630 "a" | PERC H330 (MegaRAID 3008), 3 HDD SAS (2 sudah lewat design life → MONITOR), Xeon E5-2673 v4 ×2 | SAS native, umur HDD, cache (sda lambat ~3 dtk/baca), ketidakcocokan SMBIOS/EDAC |
| Dell R630 "b" | PERC H730 (MegaRAID 3108), SSD SATA SM863a + 4 HDD SAS Toshiba, Ceph RBD, ipmitool | SAT di belakang RAID, device statistics, penyaringan RBD, suhu CPU per socket |
| lab-243 | Fedora 44, board X99/C610 AHCI, SSD generik "SSD 1TB" (WWN nol), 4×32 GiB DDR3 (BIOS hanya mencatat 2) | Jalur HDIO/SAT, cek silang RAM, **SSD mati di ata1** (deteksi via kernel log) |

Akses: SSH key milik pemilik. Kredensial tidak dicatat di repo publik ini;
minta langsung ke pemilik.

## 7. Pekerjaan terbuka (prioritas)

1. **Keaslian disk (TODO G/K).** Modul `authenticity.rs` sudah jadi (OUI
   WWN vs merek, NVMe PCI VID, model/serial generik) dan `dcheck verify`
   (uji kapasitas, TODO N). Sisa: SMART yang tidak masuk akal, jam FARM
   Seagate vs SMART, `verify` di macOS. Tabel OUI diperbarui dengan
   `scripts/gen-dcheck-oui.sh`.
2. **Kesehatan RAM (TODO G):** ECC per DIMM + label slot (EDAC sudah
   terbaca), `HardwareCorrupted`, riwayat rasdaemon, SPD, IPMI SEL. Di R630
   "b", SEL berisi event "Power Supply AC lost" yang belum pernah
   ditindaklanjuti.
3. **Kesehatan CPU (TODO G):** MCE, counter throttling, core offline,
   microcode/vulnerabilities.
4. **Kecil:**
   - `dcheck storage` (daftar teks) masih menampilkan HEALTH "?".
   - `prometheus` belum mengekspor metrik baru (design life, overdue, grown
     defects, phy errors, suhu lifetime, port gagal).
   - Error ATA runtime per port (sudah dihitung di `kernlog::PortState.errors`)
     belum ditampilkan untuk disk yang masih hidup.
5. **Verifikasi hardware yang belum pernah dilakukan:** NVMe native di mesin
   Linux asli, aarch64 di hardware asli (baru dicek dengan `file`), USB
   bridge lewat SAT, FreeBSD.

## 8. Konvensi kerja dengan pemilik

- Kalau ada temuan atau pertanyaan, **tulis catatan di `TODO.md` dulu**
  (bagian baru berhuruf urut), baru dikerjakan.
- Setiap rilis: test + clippy (host dan target Linux) + e2e harus bersih,
  lalu uji `dcheck update` di mesin nyata sebelum melapor selesai.
- Commit berbahasa Inggris dengan co-author; catatan (`TODO.md`) berbahasa
  Indonesia.
