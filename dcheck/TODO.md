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

- [ ] Pecah `src/tui.rs` → `src/tui/{mod,theme,widgets,views}.rs`
- [ ] Tema neon truecolor (auto via `COLORTERM`, override `DCHECK_COLOR`) + fallback ANSI
- [ ] Widget HUD: panel bracket, segmented gauge, badge, keycap footer, spinner braille
- [ ] Splash boot ≤ 0,5 dtk (skip: tombol apa pun, `--plain`, `DCHECK_NO_SPLASH`, `"splash": false`)
- [ ] Header `◢◤ DCHECK // DEVICE HEALTH SYSTEM` + hostname + status global
- [ ] Menu "command deck" dengan kartu ringkas (storage / RAM / CPU, prefetch di background)
- [ ] Storage: badge health berwarna + mini gauge life%
- [ ] Report: dashboard VITALS (verdict, gauge life/suhu/TBW, issues) + TELEMETRY LOG
- [ ] RAM: gauge used/swap, peta slot DIMM, ECC, suhu
- [ ] CPU: gauge load/suhu, grid core, clock, cache
- [ ] Help sebagai overlay modal
- [ ] Layout kompak untuk terminal kecil (< 60×16)
- [ ] Test snapshot `TestBackend` (80×24, 140×40, plain ASCII-only, NO_COLOR)
- [ ] Update README, `dcheck.1`, config `splash`
