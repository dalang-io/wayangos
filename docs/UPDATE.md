# WayangOS — panduan update & channel rilis

Dokumen ini untuk **operator** dan pengguna lanjutan: cara kerja update A/B,
perintah `wayang`, manajemen kunci, dan cara menyiapkan **channel rilis** yang
dipakai `wayang update`.

Antarmuka yang dibekukan ada di [`UPDATE-DESIGN.md`](UPDATE-DESIGN.md). Rencana
kerja ada di [`UPDATE-TODO.md`](UPDATE-TODO.md).

## Cara kerja update

- Rootfs adalah **initramfs yang dibuka ke RAM** tiap boot. "Instalasi" =
  file `vmlinuz` + `initramfs.img` di ESP (`WAYANGBOOT`), bukan paket.
- ESP berisi **dua slot**: `A` dan `B`. Sistem yang berjalan menempati satu
  slot; slot lainnya kosong/cadangan.
- Update menulis kernel + initramfs baru ke **slot idle**, memverifikasi
  sha256 & tanda tangan, menulis metadata slot, lalu menunjuk GRUB agar boot
  dari slot baru.
- Update **selalu butuh reboot** (rootfs di RAM). Tanpa `--reboot`, `wayang`
  hanya menyiapkan dan mencetak pesan; reboot manual untuk menerapkan.
- Anti-brick: `rcS` menjalankan `wayang mark-ok` setelah sistem siap. Kalau
  slot baru gagal boot beberapa kali (`attempts`), GRUB otomatis kembali ke
  slot terakhir yang sukses (`good`). Lihat *Rollback*.

Perbedaan `update` vs `upgrade`:

| Perintah | Cakupan |
|---|---|
| `wayang update` | minor/patch **dalam major yang sama** (mis. 1.2.0 → 1.4.1) |
| `wayang upgrade` | lintas major (mis. 1.x → 2.x), konfirmasi + langkah migrasi |

`major` diambil dari manifest bundle.

## Perintah `wayang`

```
wayang version
wayang status [--json]
wayang update  [--check] [--from FILE.wup] [--channel C] [--esp DEV] [--reboot]
wayang upgrade [--check] [--from FILE.wup] [--esp DEV] [--reboot]
wayang update --rollback [--esp DEV] [--reboot]
wayang keygen --out DIR [--keyid NAME]
wayang sign   --key FILE [--keyid NAME] MANIFEST.json
wayang verify FILE.wup [--esp DEV]
wayang mark-ok [--esp DEV]
```

- `--check` — hanya melaporkan ketersediaan update, tidak mengubah apa pun.
- `--from FILE.wup` — pasang dari bundle offline (USB), bukan dari channel.
- `--channel C` — pilih channel (`stable`, `edge`); default dari
  `/etc/wayang/channel` atau `stable`.
- `--esp DEV` — override partisi ESP (default: label `WAYANGBOOT`).
- `--reboot` — reboot otomatis setelah berhasil stage.
- `--rollback` — paksa boot ke slot sebelumnya, lalu (opsional) reboot.
- `verify` — periksa sha256 + tanda tangan bundle tanpa memasang.
- `mark-ok` — tandai slot aktif sebagai `good` (dipanggil otomatis oleh `rcS`).

### Environment

- `WAYANG_REPO_URL` — base URL channel (default: GitHub Releases
  `https://github.com/dalang-io/wayangos/releases/latest/download`).
- `WAYANG_ESP` — device ESP.
- `WAYANG_ROOT` — relokasi `/etc/wayang` + `/boot` untuk pengujian.

### Kode keluar (exit codes)

| Kode | Arti |
|---|---|
| `0` | sukses |
| `1` | error umum |
| `2` | tidak ada update yang tersedia |
| `3` | verifikasi / tanda tangan gagal |
| `4` | tidak kompatibel (arch atau `min_from`) |

## Kunci: membuat & menandatangani

Bundle **wajib ditandatangani** untuk diterima updater. Verifikasi memakai
ed25519.

1. Buat pasangan kunci di mesin rilis (jangan di box produksi):

   ```sh
   wayang keygen --out ~/wayang-keys --keyid release
   # menulis ~/wayang-keys/release.key (rahasia, 64-hex seed)
   #          ~/wayang-keys/release.pub (64-hex publik)
   ```

2. Tanamkan **hanya kunci publik** ke rootfs/box pada
   `/etc/wayang/trusted_keys`, satu baris per kunci:

   ```
   # keyid  <64-hex-ed25519-pub>
   release  <isi release.pub>
   ```

   Baris kosong & `#` komentar diabaikan. `keyid` di manifest menentukan kunci
   publik mana yang dipakai.

3. `scripts/build-bundle.sh` (dipanggil oleh `scripts/release-wayang.sh`)
   menandatangani manifest saat diberi `--key`:

   ```sh
   WAYANG_VERSION=1.4.1 WAYANG_KEY=~/wayang-keys/release.key \
       WAYANG_KEYID=release ./scripts/release-wayang.sh
   ```

   Tanpa `--key`/`WAYANG_KEY`, bundle dibuat **unsigned** (hanya untuk uji;
   updater produksi menolaknya).

Rahasia: `*.key` jangan pernah masuk repo; simpan di secret store/HSM.
Rotasi kunci = tambah baris baru di `trusted_keys` dengan `keyid` baru, lalu
pakai keyid itu saat menandatangani.

## Menyediakan channel (hosting)

`wayang update` mengambil dua URL dari base `WAYANG_REPO_URL`:

```
<base>/<channel>/<arch>/manifest.json
<base>/<channel>/<arch>/wayang-<version>-<arch>.wup
```

`arch` ∈ `x86_64`, `arm64`; `channel` ∈ `stable`, `edge`.

`scripts/release-wayang.sh` menghasilkan pohon ini di bawah
`dist/`:

```
dist/channel/<channel>/<arch>/manifest.json
dist/channel/<channel>/<arch>/wayang-<version>-<arch>.wup
dist/SHA256SUMS
```

Place `manifest.json` = schema manifest bundle (lihat `UPDATE-DESIGN.md`).
Hanya `manifest.json` dan `.wup` yang perlu dilayani lewat HTTP(S); tanda
tangan ada di dalam `.wup` (`manifest.json.sig`).

Contoh hosting statis:

- **CDN / S3 / object storage**: unggah isi `dist/channel/` apa adanya,
  pertahankan struktur direktori.
- **nginx** di LAN (mis. untuk UMKM tanpa internet), root ke direktori
  channel:
  ```
  root /srv/wayangos;
  # menyajikan /stable/x86_64/manifest.json dst.
  ```
  Set `WAYANG_REPO_URL=http://host/wayangos` di box.

`<base>` boleh URL HTTPS apa pun. Bila memakai GitHub Releases (bagian
berikut), biarkan default.

## Publikasi lewat GitHub Releases

`scripts/release-wayang.sh` **tidak mengunggah** apa pun; ia mencetak
perintah `gh`. Setelah bundle & pohon channel diperiksa:

```sh
# rilisan baru
gh release create "v1.4.1" --title "WayangOS v1.4.1" --notes "..." \
    dist/qemu/wayang-1.4.1-x86_64.wup \
    dist/intel/wayang-1.4.1-x86_64.wup \
    dist/channel/stable/x86_64/wayang-1.4.1-x86_64.wup \
    dist/channel/stable/x86_64/manifest.json \
    dist/SHA256SUMS

# bila rilis sudah ada, cukup unggah/ganti pohon channel
gh release upload "v1.4.1" \
    dist/channel/stable/x86_64/wayang-1.4.1-x86_64.wup \
    dist/channel/stable/x86_64/manifest.json \
    dist/SHA256SUMS --clobber
```

Dengan default `WAYANG_REPO_URL`, URL `.../releases/latest/download/...`
otomatis menunjuk aset dari rilis terbaru. Jadi channel "bergerak" setiap kali
rilis baru dipublikasikan.

Alur CI: `.github/workflows/release.yml` dijalankan saat tag `v*`. Karena
runner CI **tidak punya source kernel**, workflow ini hanya **dry-run/validasi**
(shellcheck, build crate `wayang`, bundel dummy, cetak perintah publish) dan
tidak mengunggah. Kernel asli dibangun lokal lalu diunggah manual/oleh job
terpisah.

## Update offline (`--from`)

Untuk box tanpa internet, salin `*.wup` ke USB lalu:

```sh
wayang update --from /run/media/usb/wayang-1.4.1-x86_64.wup --reboot
```

Bila file berada di direktori root USB, cukup sebutkan path-nya. Cara ini
melewati channel tetapi **tetap memverifikasi** tanda tangan terhadap
`trusted_keys`.

## Rollback

- Otomatis: bila slot baru gagal boot ≥3 kali, GRUB memilih slot `good`
  terakhir. Tidak perlu tindakan.
- Manual:
  ```sh
  wayang update --rollback            # siapkan, reboot manual
  wayang update --rollback --reboot   # langsung pindah + reboot
  ```
- Cek kondisi slot: `wayang status`.

## Catatan keamanan

- **HTTPS + tanda tangan wajib.** Enkripsi saja tidak cukup; tolak bundle
  unsigned atau sha256 yang tidak cocok. Update tanpa TTY pun harus aman.
- **Kunci privat** hanya di mesin rilis; box hanya menyimpan publik di
  `trusted_keys`. Jangan pernah menaruh `*.key` di image/rootfs/channel.
- **SSH hanya public-key** di box produksi (`PasswordAuthentication no`,
  `PermitRootLogin prohibit-password`). Update berjalan sebagai root.
- `curl` + CA bundle dipakai untuk unduh; bila CA dihapus kelak, verifikasi
  tanda tangan tetap wajib.
- Anti-rollback versi (opsional) dapat ditambahkan untuk mencegah downgrade.

## Caveat: satu edisi per arsitektur (v0)

Nama bundle di channel bersifat **arch-only**:
`wayang-<version>-<arch>.wup`. Karena itu **hanya satu edisi yang bisa
menempati satu `<arch>`**. `scripts/release-wayang.sh` memilih edisi lewat
`CHANNEL_EDITION` (default `generic`; `generic` memakai kernel `qemu`) untuk
x86_64 dan `CHANNEL_EDITION_ARM` (default `rpi3`) untuk arm64, lalu mencetak
peringatan.

**TODO (perubahan mendatang):** channel per-edisi
(`<channel>/<edition>/<arch>/…`) agar intel/amd/nvidia/rpi3/orangepi bisa
disajikan bersamaan. Layout v0 yang dibekukan tetap arch-only sampai
`UPDATE-DESIGN.md` diperbarui.

## CI/CD

Rilis otomatis via GitHub Actions:

| Workflow | Trigger | Hasil |
|---|---|---|
| `.github/workflows/ci.yml` | push `master`, PR | lint (shellcheck), validasi config kernel, build `wayang`, build POS |
| `.github/workflows/release.yml` | tag `v*` | dry-run: validasi pipeline channel + cetak perintah publish (tanpa build kernel) |
| `.github/workflows/installer-iso.yml` | tag `v*` atau manual | build kernel→rootfs→**installer ISO** + **bundle `.wup`** + channel tree, upload & attach ke Release |

`installer-iso.yml` memakai secret berikut (opsional tapi disarankan):

- `WAYANG_SIGNING_KEY` — seed ed25519 (64 hex) dari `wayang keygen --out keys`.
- `WAYANG_TRUSTED_KEYS` — isi `trusted_keys` (mis. `release <64hex>`), di-*bake* ke rootfs agar box terpasang mempercayai update.
- `WAYANG_KEYID` — id kunci (default `release`).

Tanpa `WAYANG_SIGNING_KEY`, bundle dibuat **tanpa tanda tangan** (peringatan) — tetap berguna untuk uji, tapi jangan dipakai di produksi. Bunding lokal juga bisa: `WAYANG_VERSION=1.4.1 WAYANG_KEY=keys/release.key ./scripts/release-wayang.sh`.

Aset yang diunggah: `wayangos-installer.iso`, `wayang-<ver>-x86_64.wup`, `channel/stable/x86_64/manifest.json`, `SHA256SUMS`.
