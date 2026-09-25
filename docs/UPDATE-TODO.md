# WayangOS — TODO: `wayang update` / `wayang upgrade`

Tujuan: box yang sudah terpasang bisa **patch** (minor/patch, major sama) dan
**upgrade** (major, mis. v1 → v2) tanpa reinstall, tetap aman (anti-brick,
rollback), dan bisa offline.

Status: **rencana** (belum diimplementasi).

## Semantik

| Perintah | Cakupan | Perilaku |
|---|---|---|
| `wayang update` | minor/patch dalam major yang sama (mis. 1.2 → 1.4.1) | cek, unduh, verifikasi, stage, reboot otomatis |
| `wayang upgrade` | lintas major (mis. 1.x → 2.x) | konfirmasi + langkah migrasi (kalau ada), bisa butuh intervensi |
| `wayang update --check` | — | hanya cek versi, tanpa mengubah |
| `wayang update --from FILE.wup` | — | pasang bundle dari media offline (USB) |
| `wayang update --rollback` | — | paksa kembali ke slot sebelumnya, reboot |
| `wayang status` | — | versi aktif, slot, versi slot lain, channel, /data |

Versi dibaca dari **semver** di manifest; `major` menentukan update vs upgrade.

## Kenapa A/B (dual-slot)

- Rootfs adalah **initramfs yang dibuka ke RAM** tiap boot; "instalasi" = file
  `vmlinuz` + `initramfs.img` di ESP. Update = ganti dua file itu, lalu reboot.
- ESP **FAT32 tanpa journal** → tulis-langsung berisiko jika mati listrik.
  Karena itu tulis ke **slot idle**, verifikasi, baru pindahkan default.
- Keuntungan: atomik, rollback mudah, tak perlu package manager.

## Layout terpasang

Sekarang (installer, `installer/src/install.rs`):
```
p1 512MiB vfat WAYANGBOOT : \EFI\BOOT\BOOTX64.EFI, /boot/grub/grub.cfg,
                            /boot/vmlinuz, /boot/initramfs.img
p2 sisa   ext4 WAYANGDATA : /data (hostname, ssh keys, dropbear, /data)
```
Usulan A/B:
```
ESP /boot/
  A/vmlinuz A/initramfs.img      slot A (versi awal/terpasang)
  B/vmlinuz B/initramfs.img      slot B (cadangan / hasil update)
  grub/grub.cfg                  dua menuentry + pemilihan default
  var/slot          "A" | "B"    slot aktif
  var/attempts      integer      percobaan boot slot aktif
  var/good          "A" | "B"    slot terakhir yang sukses boot
  var/meta-A.json  var/meta-B.json   {version, sha256, arch, time}
```
512 MiB ESP cukup untuk 2 slot (tiap slot 20–35 MB).

## Alur update (online)

1. `wayang update --check` → ambil manifest channel (HTTPS, `curl` sudah ada),
   bandingkan semver & arch.
2. Unduh bundle → verifikasi **sha256** tiap file + **tanda tangan** manifest.
3. Tolak bila arch beda / unsigned / `min_from` tak terpenuhi / anti-rollback.
4. Tulis ke slot idle (`B`), fsync, verifikasi ulang, tulis `meta-B.json`.
5. Set default GRUB → `B`, `attempts=0`, reboot.
6. Setelah boot sukses, rcS menandai `good=B` dan reset `attempts`.

## Rollback / anti-brick

- rcS menaikkan `attempts` tiap boot; setelah "ready" (SSH/DHCP) tandai
  `good` dan reset.
- Jika `attempts` melewati batas (mis. 3) **sebelum** sukses, GRUB otomatis
  memilih `good` (slot lama) → box kembali normal.
- `wayang update --rollback` untuk memaksa kembali kapan saja.
- Konsekuensi: butuh skrip GRUB kecil yang membaca `var/attempts` & `var/good`
  (GRUB `cat`/`regexp`/`if` terbatas — **uji ketat**; alternatif fallback:
  entri kedua + `wayang-mark-ok` menulis `saved_entry`).

## Format bundle `*.wup` (tar.gz + tanda tangan)

```
manifest.json      { product:"wayangos", channel, version, major, arch,
                     kernel:"vmlinuz", initramfs:"initramfs.img",
                     sha256:{vmlinuz,initramfs}, min_from, notes, time }
vmlinuz
initramfs.img
manifest.json.sig  detached signature (ed25519 / minisign)
```
- Verifikasi: sha256 file + verifikasi tanda tangan manifest (ed25519).
- BusyBox tak punya openssl → verifier statis kecil (`wayang` Rust, atau
  `minisign` statis) yang di-bundle ke rootfs; key publik ditanam di
  `/etc/wayang/keys/`.
- Keamanan: **HTTPS + tanda tangan wajib**; tolak unsigned; anti-rollback
  (opsional tapi disarankan).

## Sumber update

- **Online**: release channel (GitHub Releases `dalang-io/wayangos` atau CDN).
  `channel`: `stable` (default), `edge`.
- **Offline**: `wayang update --from /run/media/usb/wayang-1.4.1-x86_64.wup`.
  Penting untuk UMKM tanpa internet.

## Yang perlu diubah (file)

- `installer/` — buat layout A/B, grub.cfg ganda, `var/*`, tanam slot A.
- `scripts/build-rootfs.sh` — tanam `/usr/bin/wayang` (CLI), `/etc/wayang/version`,
  key publik, hook rcS: bump `attempts` + `wayang-mark-ok`.
- `scripts/build-iso.sh` / `build-installer-iso.sh` — emit layout A/B & bundle.
- **Baru** `scripts/build-bundle.sh` — hasilkan `*.wup` (+ tanda tangan).
- **Baru** `scripts/release-wayang.sh` — build matriks edisi, bundle, tanda
  tangan, unggah ke release.
- `scripts/fetch-sources.sh` — tak berubah.
- Landing (`landing-page/`) + `docs/` — halaman/dok update.

## ARM64 (fase 2)

- RPi: `config.txt` (`kernel=`/`initramfs` + `os_prefix=A/`), slot = folder
  prefix di partisi boot FAT; fallback via `autoboot.txt`/tryboot.
- Orange Pi: U-Boot — slot A/B di FAT, pilih via env/script; atau `extlinux`.
- Sama konsepnya, mekanisme boot beda. Kerjakan setelah x86 stabil.

## Milestones

| # | Deliverable | AC |
|---|---|---|
| M0 | Dokumen ini | ✅ |
| M1 | CLI `wayang` (status/version) + `/etc/wayang/version` di rootfs | `wayang status` jalan; versi benar |
| M2 | `scripts/build-bundle.sh` + verifier + penandatanganan | bundle v1 dibuat & terverifikasi; bundle rusak/unsigned ditolak |
| M3 | Layout A/B di installer + GRUB fallback + mark-ok/attempts | install bersih; simulasi boot gagal → otomatis balik ke slot lama (QEMU) |
| M4 | `wayang update` online (check → stage → reboot) + `--from` | v1 → patch → reboot → versi baru, `/data` utuh |
| M5 | `wayang upgrade` (major) + `--rollback` | lintas major terpasang; rollback berhasil |
| M6 | Otomasi rilis + kunci + docs/landing | bundle rilis tersedia & terpasang dari channel |
| M7 | A/B ARM64 | update di RPi/OPi |

## Open questions

1. **Primitif tanda tangan** & manajemen kunci (ed25519 via crate Rust vs
   `minisign` statis). Key publik ditanam di rootfs.
2. **Fallback GRUB**: skrip GRUB terbatas — apakah cukup, atau pakai
   `grubenv`/`saved_entry` + mark-ok? Perlu prototipe di QEMU.
3. **Auto vs manual**: `update` auto-apply patch? atau hanya cek + notifikasi?
4. **Channel & anti-rollback**: seberapa ketat (boleh turun versi?).
5. **Ukuran**: bundle per edisi (intel/amd/nvidia/arm) atau generik per arch?
6. **Tanpa reboot?** tidak mungkin sekarang (rootfs dari RAM) — update selalu
   butuh reboot. Cukupkah?

## Risks

- **FAT tanpa journal**: mati listrik saat tulis → mitigasi tulis ke slot idle +
  verifikasi; slot aktif tak tersentuh.
- **GRUB scripting rapuh** untuk auto-fallback → uji ekstensif, sediakan
  rollback manual.
- **CA/TLS**: `curl` + CA bundle di rootfs dipakai untuk unduh; bila nanti
  dikurangi (lihat `MEMORY-TODO.md`), verifikasi tanda tangan tetap wajib.
- **ESP penuh**: pastikan muat 2 slot + EFI; tolak update bila sisa tak cukup.