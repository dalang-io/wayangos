# WayangOS sebagai router / firewall / BGP — TODO

Target: box x86 (ThinkStation P320/P330 Tiny + kartu PCIe 4×RJ45) menjalankan
WayangOS sebagai router + firewall ala RouterOS / FortiOS, termasuk BGP.

Prinsip yang dipertahankan: tetap jalan dari RAM, userspace = binary statis,
konfigurasi & state di `/data` (partisi `WAYANGDATA`), tanpa package manager.

## Keputusan: edisi router terpisah, core tetap bersih

WayangOS **core** (kernel `defconfig-qemu`/`defconfig-intel`, rootfs dari
`scripts/build-rootfs.sh`: BusyBox + Dropbear + curl) **tidak berubah** dan
tidak mendapat dependensi router apa pun. Router adalah lapisan di atasnya:

| Lapisan | Core | Edisi router |
|---------|------|--------------|
| Kernel | `defconfig-intel` | `configs/defconfig-router` (fragment, base `defconfig-intel`) |
| Rootfs | `wayangos-initramfs.img` | core **+** overlay `router.img` |
| Build | `build-kernel.sh`, `build-rootfs.sh` | `build-kernel.sh defconfig-router` + `scripts/build-router.sh` (baru) |
| Boot | `initrd /boot/initramfs.img` | `initrd /boot/initramfs.img /boot/router.img` |

Overlay = initramfs kedua; kernel membongkar keduanya berurutan sehingga
file overlay menimpa core (mis. `/etc/init.d/network`). Tidak ada satu baris
pun kode router di `build-rootfs.sh`.

## Status sekarang (diaudit dari `configs/defconfig-qemu` + `defconfig-intel`)

| Area | Ada | Belum |
|------|-----|-------|
| Netfilter | core, conntrack, NAT core, iptables (tabel `filter`), `xt_conntrack`, helper FTP | nftables, tabel `nat` iptables + MASQUERADE, ipset, flowtable |
| Routing | `IP_ADVANCED_ROUTER`, multiple tables, multipath, multicast routing, IPv6 | VRF, policy routing IPv6, MPLS |
| L2 | — | bridge, VLAN 802.1Q, bonding/LACP, macvlan, veth |
| VPN | XFRM | WireGuard, TUN (OpenVPN), ESP (IPsec) |
| QoS | `NET_SCHED` | CAKE, HTB, fq_codel, IFB, classifier u32 |
| WAN | DHCP client (udhcpc) | PPPoE |
| NIC | e1000e, r8169, igb, igc, ixgbe, i40e, USB-Ethernet | — |
| Userspace | BusyBox (`ip` terbatas, `udhcpd`), Dropbear, curl | `nft`, iproute2 penuh + `tc`, dnsmasq, wg, pppd, BIRD/FRR, ethtool, tcpdump, conntrack-tools |
| Sistem | installer, `/data` persisten, SSH pubkey-only, `wayang-addkey` | supervisor service, config terpusat, rollback, upgrade A/B, UI admin |

Catatan: `tc` BusyBox sengaja dimatikan (qdisc CBQ hilang dari header kernel
≥ 6.8) — QoS butuh `tc` dari iproute2.

---

## A. Kernel — fondasi data-plane

Semua di fragment baru `configs/defconfig-router` (base `defconfig-intel`);
`defconfig-qemu` / `defconfig-intel` tidak ditambah.

- [ ] Buat `configs/defconfig-router` + daftarkan di `configs/README.md`

- [ ] nftables: `NF_TABLES`, `NF_TABLES_INET`, `NF_TABLES_NETDEV`, `NFT_CT`,
      `NFT_NAT`, `NFT_MASQ`, `NFT_REDIR`, `NFT_REJECT`, `NFT_LIMIT`, `NFT_LOG`,
      `NFT_COUNTER`/`NFT_QUOTA`, `NFT_FIB_INET`, `NFT_FLOW_OFFLOAD`
- [ ] Fast path: `NF_FLOW_TABLE`, `NF_FLOW_TABLE_INET`
- [ ] Set: `IP_SET` (+ `hash:ip`, `hash:net`) — atau cukup set nftables
- [ ] Kompatibilitas iptables (opsional): `IP_NF_NAT`, `IP_NF_TARGET_MASQUERADE`
- [ ] Conntrack helper yang umum: SIP, PPTP, TFTP (hati-hati: default off)
- [ ] L2: `BRIDGE`, `BRIDGE_VLAN_FILTERING`, `BRIDGE_NETFILTER`, `VLAN_8021Q`,
      `BONDING`, `MACVLAN`, `VETH`
- [ ] VRF: `NET_VRF`, `NET_L3_MASTER_DEV`, `IPV6_MULTIPLE_TABLES`
- [ ] VPN: `WIREGUARD`, `TUN`, `INET_ESP`, `INET6_ESP`, `XFRM_INTERFACE`
- [ ] QoS: `NET_SCH_CAKE`, `NET_SCH_HTB`, `NET_SCH_FQ_CODEL`, `NET_SCH_INGRESS`,
      `IFB`, `NET_CLS_U32`, `NET_CLS_FW`, `NET_ACT_MIRRED`
- [ ] WAN: `PPP`, `PPPOE`, `PPP_ASYNC` (opsional LTE modem: `USB_NET_QMI_WWAN`,
      `USB_NET_CDC_MBIM`)
- [ ] MPLS (opsional): `MPLS`, `MPLS_ROUTING`, `MPLS_IPTUNNEL`
- [ ] Tunnel: `NET_IPGRE`, `IPV6_SIT`, `NET_IPIP`, `VXLAN`
- [ ] Ukur ulang ukuran bzImage & waktu boot setelah semua di atas

**Selesai bila:** kernel boot di QEMU dengan 4 NIC, `nft list ruleset`,
bridge+VLAN, WireGuard dan CAKE bisa dikonfigurasi manual.

## B. Userspace data-plane (binary statis, cross-compile)

Pola sama dengan Dropbear / `sfdisk`: sumber di-fetch, build statis — tetapi
masuk overlay `router.img`, **bukan** rootfs core.

- [ ] `scripts/build-router.sh`: fetch + build statis semua tool router,
      susun overlay (`/usr/sbin/*`, `/etc/init.d/*`, config default), keluarkan
      `$BUILD/router.img`; sumber router tidak masuk `fetch-sources.sh`
- [ ] Overlay hanya menambah/menimpa file, tidak menghapus apa pun dari core

- [ ] `nft` (nftables + libnftnl + libmnl, statis)
- [ ] iproute2 statis (`ip`, `tc`, `bridge`, `ss`) — menggantikan `ip` BusyBox
      untuk VRF, bond, bridge VLAN
- [ ] `dnsmasq` — DHCP server + DNS cache + DHCPv6/RA (ganti `udhcpd`)
- [ ] `wg` (wireguard-tools)
- [ ] `pppd` + plugin `pppoe.so` (atau `rp-pppoe`) untuk WAN ISP
- [ ] `ethtool`, `tcpdump`, `conntrack` (conntrack-tools)
- [ ] Opsional: strongSwan (IPsec), `iperf3`, `mtr`
- [ ] Pastikan semua binary x86_64 statis, cek ukuran initramfs total

**Selesai bila:** setiap tool jalan di WayangOS tanpa library tambahan.

## C. Routing dinamis — BGP

- [ ] Pilih daemon: **BIRD 2/3** vs **FRRouting**

      | | BIRD | FRRouting |
      |---|---|---|
      | Bentuk | 1 daemon + `birdc` | banyak daemon + `vtysh` |
      | Protokol | BGP, OSPF, RIP, Babel, BFD, RPKI, MPLS/L3VPN dasar | + IS-IS, LDP, EVPN/VXLAN, PIM, VRRP, SR, PBR |
      | Config | file deklaratif + bahasa filter kuat, reload atomik | CLI gaya Cisco; reload via `frr-reload.py` (Python) |
      | Efisiensi | sangat hemat, standar route server IXP | lebih berat (multi-daemon) |
      | Build statis | mudah (C, dependensi minim) | repot (libyang, json-c, Python) |

      Condong ke BIRD: config mudah di-render dari config terpusat (bagian E),
      satu binary statis. Pilih FRR bila butuh EVPN/IS-IS/MPLS/multicast atau
      CLI routing ala Cisco/RouterOS.
- [ ] Build statis + init service
- [ ] BGP: sesi eBGP/iBGP, filter prefix, communities, BFD
- [ ] OSPF (opsional), static route dengan health check
- [ ] Ukur RAM untuk full table (~1 juta prefix IPv4+IPv6) → minimal RAM edisi
- [ ] Lab QEMU: dua WayangOS saling peering + satu upstream simulasi

**Selesai bila:** dua box saling BGP, route masuk ke kernel, failover saat
link diputus.

## D. Jaringan dasar yang harus berubah

Di core, `/etc/init.d/network` menjalankan DHCP client di **semua** port —
cocok untuk box biasa dan tetap begitu. Overlay router mengganti script ini.

- [ ] Penamaan interface stabil berdasar MAC / slot PCI (`wan`, `lan1`…),
      bukan urutan `eth0`/`eth1` — di overlay (`/etc/init.d/network` router)
- [ ] Mode per interface: DHCP client / statis / PPPoE / anggota bridge / VLAN
- [ ] `sysctl` router: `ip_forward`, `rp_filter`, `accept_redirects=0`,
      SYN cookies, `nf_conntrack_max` sesuai RAM
- [ ] Default firewall aman saat boot pertama: drop dari WAN, SSH hanya dari
      LAN, allow established/related, rate-limit ICMP
- [ ] Tanpa config router di `/data`, overlay jatuh ke perilaku core (DHCP
      di semua port) supaya box tidak terkunci setelah install

## E. Control-plane — pembeda utama dari RouterOS / FortiOS

- [ ] **Config terpusat** di `/data/etc/wayang.conf` (format: TOML / YAML /
      teks gaya RouterOS — **keputusan**) sebagai satu-satunya sumber
- [ ] **Renderer**: config → ruleset nftables, `ip`/bridge/VLAN, dnsmasq,
      BIRD, WireGuard, tc — idempoten, bisa dites tanpa hardware
- [ ] **Apply + rollback**: `commit confirmed` ala Junos / "safe mode" RouterOS
      — auto-revert bila tidak dikonfirmasi dalam N detik (anti terkunci)
- [ ] Validasi sebelum apply (syntax + referensi interface/zone)
- [ ] **Supervisor service** (restart daemon yang mati, dependensi urutan) —
      BusyBox `runsv`/`runit` sudah tersedia, atau supervisor kecil sendiri
- [ ] Riwayat config (versi di `/data`, diff antar versi)
- [ ] Bahasa implementasi: Rust (sama seperti dcheck + installer) —
      **keputusan**

## F. Antarmuka admin

- [ ] **TUI admin** bergaya HUD dcheck (pakai ulang `installer/src/hud.rs`):
      interface & traffic live, rule firewall + hit counter, NAT, lease DHCP,
      sesi BGP, WireGuard peer, log
- [ ] CLI non-interaktif untuk scripting (`wayang set …`, `wayang show …`)
- [ ] Opsional: WebUI + REST API (pertimbangkan permukaan serangan)
- [ ] Opsional: kompatibilitas impor config (RouterOS `/export` sebagian)

## G. Keamanan & operasional

- [ ] Upgrade image **A/B** di ESP (`vmlinuz` + `initramfs` slot A/B,
      GRUB `fallback`), rollback otomatis bila boot gagal
- [ ] Backup / restore config (`/data`) — lokal & remote
- [ ] Remote syslog, NTP yang andal (sekarang `ntpd` sekali jalan)
- [ ] Monitoring: SNMP atau Prometheus exporter (dcheck sudah punya mode
      `prometheus` untuk hardware)
- [ ] Hardening: SSH hanya dari zone manajemen, rate-limit login, audit log
- [ ] Secure Boot (opsional): tanda tangan GRUB + kernel sendiri

## G2. Installer & ISO

- [ ] `scripts/build-installer-iso.sh`: opsi sertakan `router.img` + kernel
      `defconfig-router` (ISO router terpisah, atau satu ISO dua edisi)
- [ ] `wayang-installer`: pilih edisi (core / router) di layar ACCESS atau
      layar baru; `grub.cfg` di disk memuat `router.img` hanya untuk router
- [ ] Upgrade: core dan overlay router bisa diganti terpisah

## H. Pengujian

- [ ] Lab QEMU multi-NIC (script seperti `~/wayangos-build/p320/run.sh`):
      WAN + 3 LAN, dua router BGP, klien di belakang NAT
- [ ] Test otomatis renderer config (unit test, tanpa hardware)
- [ ] Uji throughput di P330 Tiny + kartu 4 port (iperf3, NAT, dengan dan
      tanpa flowtable)
- [ ] Uji PPPoE ke server PPPoE lab

## I. Keputusan yang perlu diambil dulu

1. ~~Edisi router terpisah atau semua edisi x86 jadi router?~~ **Diputuskan:**
   terpisah — core bersih, router = `defconfig-router` + overlay `router.img`.
2. BIRD atau FRRouting?
3. Format config: TOML/YAML, atau sintaks gaya RouterOS?
4. Admin utama: TUI + CLI saja, atau juga WebUI?
5. Lisensi: nftables, iproute2, dnsmasq, BIRD, FRR, pppd semuanya GPL —
   boleh dikirim bersama produk closed-source asal source komponen itu
   disediakan; kode WayangOS sendiri (config, renderer, UI) tetap bisa tertutup.

## Urutan yang disarankan

1. **MVP router/firewall** — `defconfig-router` + `build-router.sh` (A: nftables,
   bridge, VLAN, sysctl; B: `nft`, iproute2, dnsmasq) + D. Box sudah bisa
   menggantikan router kantor kecil; core tidak berubah.
2. **E** — config terpusat, rollback, supervisor. Syarat aman dikelola remote.
3. **VPN + WAN + QoS** — WireGuard, PPPoE, CAKE.
4. **C** — BGP dengan lab QEMU, lalu **F** TUI admin.
5. **G** — upgrade A/B, monitoring, hardening.
