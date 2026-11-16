# DecHAMDRM — notes for Claude

Rust HamDRM modem (EasyPal / EasyDRF / QSSTV compatible pictures, voice, waterfall ID,
plus our own stronger-FEC modes). Read `docs/DESIGN.md` first (decisions, milestones),
then the format specs `docs/spec/phy.md` and `docs/spec/app.md`.

## Layout
- `crates/hamdrm-core` — params (modes A/B/E, 2.3/2.5 kHz), tables, cell map, FAC,
  MLC, `tx` (Transmitter) and `rx` (push-driven Receiver: DC search near 350 Hz, mode
  detection, sync, channel estimation, FAC-configured MSC; port of DecDRM's `rx`).
  Loopback tests: `tests/loopback.rs`; SNR sweeps with
  `cargo test --release -p hamdrm-core --test loopback -- --ignored --nocapture`.
  Ported from DecDRM's `decdrm-core` (git dependency, tag pinned in the workspace
  `Cargo.toml`) and uses its generic FEC/DSP modules.

## References (outside the repo, never committed)
- `..\easydrf-ref` — EasyDRF (DazDSP/EasyDRF, GPL-3): the on-air authority.
  `..\easydrf-build\Release\EasyDRF.exe` is our own MSVC build of it for interop tests.
- `..\qsstv-ref` — QSSTV (ON4QZ/QSSTV, GPL-3).
- `..\DecDRM` — our DRM30 code; `reference/etsi/` there has ES 201 980 as text.

## Rules
- GPL-3.0-or-later. Credit EasyDRF/Dream/QSSTV/DecDRM at the function or module a
  port comes from; cite spec sections / reference file:line next to tables.
- Never commit recordings, received pictures, decoded audio, reference clones, exes.
- Never key a transmitter in tests: fake rigs / null sinks; PTT off by default.
- DSP in f64 (`Real`, `Cplx`), 48 kHz, bit streams one bit per byte (DecDRM conventions).
- `cargo test -p hamdrm-core`; DSP is slow unoptimised (dev profile is opt-level 1).
- Commit identity is repo-local (Arclamp no-reply). Push to origin per milestone.
