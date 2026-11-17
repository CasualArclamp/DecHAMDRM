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
  350 Hz (EasyDRF's receiver only searches 350 ± 5 Hz), lead-in 24 segments.

## Better-FEC modes (plan, milestone 9)

Signalled in FAC bits 32–39, which EasyDRF and QSSTV never write but cover with the
CRC (phy.md §4.1): legacy receivers still decode the FAC, fail the packet CRCs and
ignore the MSC. Candidates, to be measured on the channel simulator:
1. PHY: LDPC + BICM in the MSC instead of the K=7 convolutional MLC (≈2–3 dB).
2. Application: a fountain code over MOT segments instead of RS + BSR, so any
   K(1+ε) received segments rebuild the file whatever was lost.

## Digital voice (2026-11-16)

Two codecs behind `hamdrm_voice::VoiceCodec` (400 ms of 48 kHz speech ↔ one MSC frame):

- **LPC-10 (EasyDRF)** — `hamdrm-voice`: the HawkVoice LPC-10 C code as EasyDRF
  modified it, vendored in `crates/hamdrm-voice/c` (LGPL-2+), with EasyDRF's framing
  (app.md §7.1: 54-bit frames, 27-bit XOR check, frame length 180–540 samples).
  FAC: audio, coding 0. Verified: EasyDRF's receiver passes the check on our frames
  (18 blocks in mode B 2.5 kHz 16-QAM).
- **Neural (Mimi)** — `hamdrm-neural`: Kyutai's Mimi (24 kHz, 12.5 frames/s, residual
  codebooks of 11 bits = 550 bit/s each) on candle, CPU. Rory asked for a neural codec
  (2026-11-16). Vendored from candle-transformers 0.9.2 with one change: the decoder
  accepts fewer codebooks than the model has. Frame format "NV1" (`hamdrm_voice::nv`):
  5 Mimi frames per MSC frame, codebooks in groups 1,1,2,4,4,4,8,8, each group with a
  CRC-8, so damage to later groups only lowers the quality. Codebooks per mode: 8 in
  B 2.3 kHz 4-QAM (4.4 kbit/s), 16 in B 2.5 kHz 16-QAM (8.8 kbit/s), 32 in A
  2.5 kHz 64-QAM p1. FAC: audio, coding 1 ("CELP", which EasyDRF never decodes) plus
  extension byte `0xA1` in FAC bits 32–39 (which EasyDRF/QSSTV never write).
  Measured on the 9800X3D (release): encode 93 ms + decode 58 ms per 400 ms frame.
  Weights: `kyutai/mimi` at a pinned commit (385 MB, CC-BY-4.0, SHA-256 checked),
  not in the repo: `dechamdrm models download` or Settings → Download.

CLI: `dechamdrm voice speech.wav --out tx.wav --codec neural|lpc10`; `dechamdrm rx`
writes received voice to `voice.wav`. GUI: Talk button, microphone / speaker / gain in
Settings. Not done: EasyDRF's Speex mode, the text message in voice frames, and voice
quality has only been checked by a spectral-distance measure, not by listening.

## GUI additions (2026-11-16, asked by Rory)

- **Picture → JPEG 2000 transcoder** (`apps/hamdrm-gui/src/transcode.rs`): quality
  slider (compression ratio 2:1 … 400:1, log scale) and longest-side slider; a worker
  thread re-encodes on every change (newest request wins) and decodes the result, so
  the right-hand preview is what the other station sees; size and air time shown;
  "Add to send queue" writes `NAME.jp2` into `%APPDATA%/DecHAMDRM/Tx Files`. Received
  `.jp2` pictures display too. Codec: `jpeg2k`/`openjp2` (pure Rust).
  Unattended look: `--jp2 FILE --jp2-quality N --screenshot …`.
- **FAC and MSC constellations** side by side, 1.6 px dots (DecDVB-T style), redrawn
  every display frame from the receiver's rolling cell window; engine blocks 40 ms.
- **Channel tab** (DecDRM style, `plots.rs`): |H|², group delay, impulse response with
  guard interval and estimated spread, SNR per carrier, fading map of 64 symbols.
  `--tab channel` for screenshots.
- **Spectrum and waterfall** from a 16384-point FFT (2.9 Hz bins, a row every 50 ms),
  asked 2026-11-16 ("way higher"): single carriers and pilots are resolved.
- Live voice loop test (silent, cables A and B): `--example voice_loop`.

## Interop testing

`..\easydrf-build` is EasyDRF built from source with a local patch to `sound/Sound.cpp`:
`EASYDRF_WAV=file.wav EASYDRF_SPEED=2 EasyDRF.exe` (run in `..\easydrf-build\run`) feeds
a 48 kHz mono WAV in place of the sound card and exits after 30 s of silence; received
files land in `run\Rx Files`. Above about 2× real time it stalls. Script:
`scripts/interop/interop.sh NAME tx-args…`.

The same build also transmits into a file (2026-11-16): with `EASYDRF_TXWAV=out.wav` it
starts a transmission by itself and writes the transmit audio (after its PAPR clipper)
to the WAV, then exits. `EASYDRF_TXFILES=a;b` sends files (mode, bandwidth, QAM,
interleave and instances/RS level come from `settings.txt`, `EASYDRF_PROT` sets the
protection level), `EASYDRF_TXBSR=file` sends a BSR request, and without either it
sends voice from `EASYDRF_MIC=speech.wav` (`EASYDRF_CODEC=speex` for Speex). No sound
device is opened and PTT is disabled in harness runs. Script:
`scripts/interop/edtx.sh` (run in `..\easydrf-build\runtx`), all 30 combinations:
`scripts/interop/edmatrix.sh`, noise burst into a recording: `scripts/interop/gap.py`.
All our changes to EasyDRF are in `docs/interop/easydrf-harness.patch`.

## Milestones

1. [x] Specs from the references (phy, app).
2. [x] Core TX: file → MOT → MSC → OFDM WAV, byte-exact through EasyDRF's receiver.
3. [x] Core RX: our TX loopback through the channel simulator (AWGN, HF fading).
   Measured AWGN thresholds (≥ 99 % of frames, SNR in 2.5 kHz, 0.5 dB steps, 2 × 60
   frames, `awgn_threshold_sweep`): mode B 2.5 kHz 4-QAM 6.0 dB, 16-QAM p0 10.0 dB,
   16-QAM p1 11.5 dB, 64-QAM p0 15.0 dB; 16-QAM p0 in mode A 9.5 dB, mode E 9.5 dB.
4. [x] RS erasure coding (EasyPal/QSSTV `.rsN` send+receive, EasyDRF receive) + BSR
   (request, parse, repair), LZMA `.lz`, file receiver with sanitised names.
5. [x] CLI: `dechamdrm tx FILES --out x.wav`, `dechamdrm rx x.wav --out-dir DIR`.
6. [x] GUI (`dechamdrm-gui`) + engine: sound card / WAV input, TX queue, PTT via
   decdsss-rig (VOX default), waterfall, gallery. Live VB-cable loop verified
   (`--example cable_loop`).
7. [~] Waterfall text (EasyDRF style) done; QSSTV FSK ID not done (QSSTV only sends it
   after analogue SSTV, so low priority).
8. [~] Digital voice: neural (Mimi) and EasyDRF LPC-10 done, both directions against
   EasyDRF for LPC-10; Speex not done.
9. [ ] Better-FEC modes (see above).
10. [~] CI, release workflow, portable exes done; release not cut.

## Interop results

- Our TX → EasyDRF RX: all 30 mode/bandwidth/QAM/protection combinations byte-exact
  (2026-11-16). With a 9-segment lead-in, B 2.3 kHz 64-QAM p0 sometimes lost both MOT
  headers because EasyDRF locks slowly in that mode; default lead-in is now 24 segments
  (EasyDRF's own startup value) and the leader repeats for small files.
- EasyDRF TX → our RX (2026-11-16, EasyDRF's own transmitter into a WAV, see above):
  - pictures: all 30 mode/bandwidth/QAM/protection combinations byte-exact; long
    interleave; three files in one transmission (one of them 19 bytes);
  - EasyDRF's RS levels 1–4 byte-exact, and still byte-exact with a noise burst of
    2 / 4 / 8 / 14 s cut into the RS1 / RS2 / RS3 / RS4 recording (RS3: file rebuilt
    from 71 of 82 segments), so the erasure decoder agrees with EasyDRF's encoder;
  - BSR request as EasyDRF sends it (run-compressed, twice) parsed;
  - LPC-10 voice: 34–36 good frames, 0 bad, in six mode combinations (not listened to);
  - Speex voice: recognised and reported as not supported.
  EasyDRF's clipper limits the MER we measure on its signal to about 14 dB (4-QAM),
  22 dB (16-QAM) and 28 dB (64-QAM); that is its design, not a receiver fault.
- EasyPal itself and QSSTV → our RX: **not tested** (no transmitter of theirs here;
  needs a recording). EasyPal's `.rsN` files are only verified against our own encoder
  and through EasyDRF's receiver.

## Receiver robustness (2026-11-17)

Seen live by Rory: after audio dropouts the receiver locked off centre (DC 344 Hz,
clock correction at its ±200 Hz limit, FACs failing) and never recovered. Cause: the
clock (sample-rate) loop kept integrating while FACs failed, the wrong value survived
restarts, and a wrong sample rate moves the three frequency pilots apart, so each new
acquisition landed on a neighbouring bin. Now:

- the clock loop is applied only while FACs decode; a value is trusted after 5 good
  FACs in lock, and resync/restart return to the trusted value; limit ±96 Hz (2000 ppm);
- acquisition interpolates the DC frequency from the three pilot lines (was ±3.9 Hz,
  now within about 0.1 Hz on clean signals);
- a receiver with one good FAC and then only bad ones resyncs after 3 (it used to stay).

Test `dropouts_keep_frequency_and_clock_centred`; trace tool
`cargo run --release -p hamdrm-core --example rxtrace -- FILE.wav`; live:
`--example voice_loop -- SPEECH.wav neural a64long`.

## Open items

- A recording of a real EasyPal (or QSSTV) transmission for RX interop.
- EasyDRF's Speex voice mode and the text message in voice frames.
- A BSR request that is repeated on air is reported once per copy.
- Receiver status after the signal ends keeps the last figures (MER from noise-only
  symbols); show "signal lost" instead.
