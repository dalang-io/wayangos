# WayangOS — TODO: kurangi pemakaian memori

Titik awal (lihat [`MINIMUM-SPEC.md`](MINIMUM-SPEC.md)): minimum **72 MiB**
(`defconfig-intel`) / **70 MiB** (`defconfig-qemu`), 1 vCPU. Yang membatasi
adalah **boot**, bukan sistem yang sudah jalan (~12 MB terpakai):

- kernel memakan ~41 MB RAM yang tidak pernah kembali;
- initramfs harus muat dua kali saat boot: 4,7 MB terkompresi + 10,3 MB
  terbongkar (curl 6,2 MB, BusyBox 2,4 MB, Dropbear 1,5 MB, CA 0,2 MB).

Target: **≤ 48 MiB** untuk edisi minimal, tanpa mengorbankan fitur core
(SSH pubkey-only, DHCP, `/data`, installer).

Setiap langkah: ukur ulang dengan metode di `MINIMUM-SPEC.md` (bisect RAM,
empat syarat "jalan") dan catat angkanya di sini.

## A. Kernel (porsi terbesar, ~41 MB)

- [ ] Fragment baru `configs/defconfig-mini` (base `defconfig-qemu`): tanpa
      DRM/i915, SAS/RAID, NIC 10G, sound, wireless — hanya virtio/e1000/
      e1000e/r8169, AHCI, NVMe, USB storage/HID
- [ ] Audit `defconfig-qemu` juga: subsystem yang tidak dipakai core
      (mis. `DRM_I915`, `IP_MROUTE`, debug) — tiap MB kode kernel = 1 MB RAM
- [ ] `CONFIG_CC_OPTIMIZE_FOR_SIZE=y` — ukur selisih ukuran & performa
- [ ] Matikan `KALLSYMS_ALL`, debug/tracing yang tidak perlu (`FTRACE`,
      `DEBUG_KERNEL` bila aktif), `IKCONFIG`
- [ ] Catat baris `Memory: ... reserved` dari log boot sebelum/sesudah
- [ ] Pertahankan `defconfig-intel` lengkap untuk hardware nyata; edisi mini
      untuk VM/embedded kecil

## B. Initramfs (puncak saat boot: 4,7 + 10,3 MB)

- [ ] **curl (6,2 MB, separuh rootfs):** opsi —
      (a) build curl sendiri, statis, hanya HTTPS + mbedTLS/BearSSL;
      (b) keluarkan curl dari core, sediakan hanya di image installer;
      (c) `wget` BusyBox — **jangan** untuk ambil SSH key: TLS-nya tidak
      memverifikasi sertifikat. Pilihan memengaruhi `wayang-addkey`
      dan fetch key di installer
- [ ] Strip Dropbear (build sekarang "not stripped") — `scripts/build-rootfs.sh`
- [ ] Pangkas applet BusyBox yang tidak dipakai (config minimal, bukan
      `defconfig`) — cek dulu yang dipakai rcS, installer, `wayang-addkey`
- [ ] Kompres initramfs dengan xz/zstd (`CONFIG_RD_XZ`/`RD_ZSTD` di kernel)
      — memperkecil bagian terkompresi yang ikut dimuat saat boot
- [ ] CA bundle: pertimbangkan hanya CA yang dibutuhkan (GitHub/GitLab)
      — hemat kecil, risiko pemeliharaan; mungkin tidak sepadan

## C. Saat boot

- [ ] Uji `initramfs_async=0/1` dan urutan unpack terhadap puncak memori
- [ ] Pastikan initrd dibebaskan setelah dibongkar (log `Freeing initrd memory`)
- [ ] Ukur 2 vCPU: selisih +8 MiB (per-CPU area) — cek `NR_CPUS` yang
      terlalu besar di config

## D. Otomasi & dokumentasi

- [ ] `scripts/test-minmem.sh`: bisect RAM otomatis di QEMU (per kernel),
      cetak minimum — metode yang sekarang dilakukan manual
- [ ] Jalankan di CI (opsional, lambat di TCG) atau sebelum rilis
- [ ] Perbarui klaim **"64MB minimum RAM"** di hero landing page
      (`landing-page/index.html`) dengan angka yang terukur
- [ ] Perbarui `MINIMUM-SPEC.md` & tabel memori di `ARCHITECTURE.md`
      setiap kali angka berubah

## Urutan yang disarankan

1. **B — strip Dropbear + kompresi xz** (murah, tanpa efek fitur).
2. **A — `defconfig-mini`** (hemat terbesar untuk VM/embedded).
3. **B — keputusan curl** (hemat besar, perlu keputusan soal HTTPS).
4. **D — `test-minmem.sh`** supaya angka tidak basi lagi, lalu perbarui
   website.
