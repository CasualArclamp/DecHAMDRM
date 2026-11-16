# DecHAMDRM — design

Rust HamDRM modem: send and receive pictures, digital voice and waterfall/FSK ID
over a 2.5 kHz SSB channel, interoperating on air with EasyPal, EasyDRF (WinDRM) and
QSSTV, plus our own stronger-FEC modes. Sister project of DecDRM (DRM30).

## Decisions (Rory, 2026-11-16)

| Question | Answer |
|---|---|
| Where | New repo `F:\codebases\DecHAMDRM`, GitHub `CasualArclamp/DecHAMDRM` (private to start). Reuse DecDRM: git dependency on `decdrm-core` for the generic FEC/DSP pieces. |
| Scope | RX **and** TX, full on-air interop with EasyPal / EasyDRF / QSSTV, **plus better FEC modes** of our own (signalled so that legacy receivers ignore them). |
| Payloads | Pictures (JPEG / JP2 / WebP files over MOT with RS erasure coding and BSR), digital voice, waterfall text ID and FSK ID. |
| App form | egui GUI in the DecSAT look (portable exe, no console window), CLI for WAV rx/tx and tests, rig CAT/PTT (off by default, VOX option; never keyed in tests). |

## References (never committed)

- `..\easydrf-ref` — EasyDRF by Daz Man (github.com/DazDSP/EasyDRF, GPL-3), WinDRM
  updated; the main authority for the on-air format (EasyPal descends from WinDRM).
- `..\qsstv-ref` — QSSTV (github.com/ON4QZ/QSSTV, GPL-3), independent HamDRM rx/tx.
- `..\DecDRM` — our DRM30 implementation; `reference/etsi/` there has ES 201 980.

Format specs written from those sources: `docs/spec/phy.md`, `docs/spec/app.md`.

## Layout (planned)

- `crates/hamdrm-core` — HamDRM numerology (modes A/B/E, 2.3/2.5 kHz), cell map,
  FAC, MSC MLC, transmitter and receiver chains. Ported from DecDRM's `decdrm-core`
  (same conventions: f64 DSP, 48 kHz, one bit per byte), generic FEC/DSP used from it.
- `crates/hamdrm-data` — packet mode, MOT data groups, file RS erasure coding, BSR.
- `crates/hamdrm-voice` — digital voice codecs and framing.
- `crates/hamdrm-id` — waterfall text and FSK ID.
- `crates/hamdrm-engine` — worker thread, sound card, PTT, snapshots for the UI.
- `apps/hamdrm-cli` (`dechamdrm`), `apps/hamdrm-gui`.

## Milestones

1. [ ] Specs from the references (phy, app).
2. [ ] Core TX: file → MOT → MSC → OFDM WAV that EasyDRF/QSSTV decode.
3. [x] Core RX: our TX loopback through the channel simulator (AWGN, HF fading).
   Measured AWGN thresholds (≥ 99 % of frames, SNR in 2.5 kHz, 0.5 dB steps, 2 × 60
   frames, `awgn_threshold_sweep`): mode B 2.5 kHz 4-QAM 6.0 dB, 16-QAM p0 10.0 dB,
   16-QAM p1 11.5 dB, 64-QAM p0 15.0 dB; 16-QAM p0 in mode A 9.5 dB, mode E 9.5 dB.
4. [ ] RS erasure coding + BSR, picture gallery.
5. [ ] CLI rx/tx on WAV and sound card.
6. [ ] GUI.
7. [ ] Waterfall text / FSK ID.
8. [ ] Digital voice.
9. [ ] Better-FEC modes.
10. [ ] CAT/PTT, portable exe, CI, release.
