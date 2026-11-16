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

## Format choices (from the specs)

- **Two incompatible RS file schemes exist** (app.md §3). We send the EasyPal/QSSTV one
  (`name.rsN`, field 0x11D) because EasyPal is the main audience, and receive both
  (EasyDRF's: field 0x187, `EasyDRFHeader/|`, level in the data-group Rfa bits).
- Packet lengths ≥ 257 (mode A 64-QAM) hit the 8-bit PPI count wrap that every
  HamDRM receiver has; we keep the on-air behaviour (same as EasyPal).
- TX defaults: mode B, 2.5 kHz, 16-QAM, protection 0, short interleaver, carrier 0 at
  350 Hz (EasyDRF's receiver only searches 350 ± 5 Hz), lead-in 9 segments.

## Better-FEC modes (plan, milestone 9)

Signalled in FAC bits 32–39, which EasyDRF and QSSTV never write but cover with the
CRC (phy.md §4.1): legacy receivers still decode the FAC, fail the packet CRCs and
ignore the MSC. Candidates, to be measured on the channel simulator:
1. PHY: LDPC + BICM in the MSC instead of the K=7 convolutional MLC (≈2–3 dB).
2. Application: a fountain code over MOT segments instead of RS + BSR, so any
   K(1+ε) received segments rebuild the file whatever was lost.

## Interop testing

`..\easydrf-build` is EasyDRF built from source with a local patch to `sound/Sound.cpp`:
`EASYDRF_WAV=file.wav EASYDRF_SPEED=2 EasyDRF.exe` (run in `..\easydrf-build\run`) feeds
a 48 kHz mono WAV in place of the sound card and exits after 30 s of silence; received
files land in `run\Rx Files`. Above about 2× real time it stalls. Script:
`out/interop.sh NAME tx-args…` (git-ignored).

## Milestones

1. [x] Specs from the references (phy, app).
2. [x] Core TX: file → MOT → MSC → OFDM WAV, byte-exact through EasyDRF's receiver.
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
