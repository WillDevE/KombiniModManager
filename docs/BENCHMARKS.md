# Benchmarks

Measured 2026-11-17 on the development machine (Windows 11, AMD Radeon RX 9060 XT, NVMe SSD),
Rust 1.98, `--release` (thin LTO, 1 codegen unit, stripped). Budgets are the project's targets.
Automated budget tests assert only in optimized builds and print timings in all builds:

```
cargo test --release -p kombini-vpk --test budgets -- --nocapture
cargo test --release -p kombini-engine budget_ -- --nocapture
```

| Area | Budget | Measured | How |
|---|---|---|---|
| Cold start to usable shell | < 1.5 s | first frame 153 ms; library loaded 196 ms | `startup:` lines in `logs/app.log`, sandbox game with 5 mods |
| VPK inspector first page (synthetic, 158,000 entries) | < 1 s | 69 ms (open + index + first 200 rows) | `kombini-vpk/tests/budgets.rs` |
| VPK inspector first page (live `pak01_dir.vpk`, 158,667 entries) | < 1 s | 191 ms warm, 446 ms cold, including process start | `kombini vpk <pak01_dir.vpk> --limit 200`, read-only |
| Filter 158k paths | < 100 ms | 2.2 ms | `budgets.rs` |
| Last page sorted by size | — | 0.7 ms | `budgets.rs` |
| Apply load order, 100 mods (enable all) | < 2 s | 197 ms | `kombini-engine` `budget_apply_load_order_100_mods` |
| Apply load order, 100 mods (reverse order) | < 2 s | 197 ms | same test |
| Memory, normal use | < 400 MB | ≈ 93 MB working set after startup | Task Manager / `Get-Process`, release build |
| Library with 1,000 mods | no frame drops | virtualized (`uniform_list`); not yet profiled with 1,000 real mods | — |
| Toggle/reorder acknowledgement | < 100 ms | immediate: optimistic switch + pending spinner on the same frame | by construction |

Binary sizes (release, stripped): `kombini-mod-manager.exe` 33.5 MB, `kombini.exe` (CLI) 10.9 MB.

Not measured yet: installing a 100 MB VPK and snapshotting 10 GB (both stream with progress and
cancel; timing depends on disk), scrolling a 1,000-mod library, 125/150/200 % DPI.
