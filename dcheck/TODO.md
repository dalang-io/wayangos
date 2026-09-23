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
