# CFGFIX — cfg-coverage legs red on the Tuesday fold

Finding: `./arroyo check` ran 86 kernel cfg legs, 38 red (cargo E0433), from five root errors.

| Error | Legs | Fix |
|---|---|---|
| `main.rs:910` `net_fetch` (module is `smolnet`+x86) | 7 x86 legs (bin) | call gated `#[cfg(all(feature="smolnet", target_arch="x86_64"))]`, same line |
| `main.rs:7043` `net_tick` (module gated smolnet+x86 or sntp6+net6+aarch64) | 7 x86 legs (bin) | call gated with the module's own cfg, same line |
| `arch/aarch64/syscall.rs:23158` `fs::users` (needs `login`) | 26 aarch64 legs | `#[cfg(feature="login")]` on the call statement inside the NOTICE arm |
| `video/fileview.rs:34` `quarry::live` (needs `quarry`) | 4 legs | assert gated `#[cfg(feature="quarry")]`, same line |
| `video/brightkeys.rs:96` `gpu::igpu` (needs `intel-ivb`) | 2 legs | real `write_gmux` under `all(gmux_igd, intel-ivb)`, no-op twin under the negation |

All edits line-neutral (except brightkeys, same line count). No feature removed.
Result: `kernel cfg coverage OK (86 legs)` — 86/86 green, 0 red.
