# HamDRM physical layer — bit-level specification

Target: a Rust implementation that interoperates on air with EasyPal, EasyDRF,
WinDRM and QSSTV.

## Sources and how to read this

| Tag | Path | Role |
|---|---|---|
| `E:` | `F:\codebases\easydrf-ref\` (EasyDRF, Daz Man's update of WinDRM) | main authority; paths below are relative to `common\` unless they start with `..\` |
| `QT:` | `F:\codebases\qsstv-ref\src\drmtx\common\` | QSSTV transmitter. A fork of the same WinDRM code. Its tables are identical to EasyDRF's (checked with `diff`) |
| `QR:` | `F:\codebases\qsstv-ref\src\drmrx\` | QSSTV receiver (PA0MBO's RXAMADRM). An independent implementation running at 12 kHz |
| `D:` | `F:\codebases\DecDRM\crates\decdrm-core\src\` | our DRM30 code (a Dream port) |

How each number is marked:
- **[c file:line]**: copied from that file and line.
- **[d …]**: derived, with the arithmetic shown.
- **[py]**: computed by a Python port of `E:ofdmcellmapping/CellMappingTable.cpp`
  `MakeTable` that keeps C truncation semantics. It reads the tables straight from the
  EasyDRF headers. Every MSC length it gives matches the hard-coded `streamlen` table in
  `E:FAC/FAC.cpp` (§5.6), and the QAM4 lengths also match QSSTV's hard-coded values. That
  is how it was checked.

Sample rate is 48 kHz throughout, as in EasyDRF **[c tables/TableDRMGlobal.h:43]**.

---

## 1. OFDM numerology

### 1.1 Robustness modes

| | A | B | E |
|---|---|---|---|
| FFT size N = Tu at 48 kHz | 1152 **[c TableDRMGlobal.h:49]** | 1024 **[c :54]** | 640 **[c :59]** |
| Tg/Tu | 1/9 **[c :51-52]** | 1/4 **[c :56-57]** | 1/2 **[c :61-62]** |
| Tg (samples) | 128 [d 1152·1/9] | 256 [d 1024/4] | 320 [d 640/2] |
| Ts = Tu+Tg (samples) | 1280 | 1280 | 960 |
| Tu / Tg / Ts (ms) | 24 / 2.667 / 26.667 | 21.333 / 5.333 / 26.667 | 13.333 / 6.667 / 20 |
| Carrier spacing 1/Tu | 41.667 Hz [d 48000/1152] | 46.875 Hz [d 48000/1024] | 75 Hz [d 48000/640] |
| Symbols per frame Ns | 15 **[c :50]** | 15 **[c :55]** | 20 **[c :60]** |
| Frame length | 400 ms [d 15·1280/48000] | 400 ms | 400 ms [d 20·960/48000] |
| Superframe | 3 frames **[c :47]** = 1.2 s | same | same |

The guard length is computed as `N*enum/denom` in integer arithmetic
**[c ofdmcellmapping/CellMappingTable.cpp:173]**. One frame is 19 200 samples in every
mode, the same as DecDRM's `SAMPLES_PER_FRAME`.

The robustness mode is **not signalled**. A receiver detects it from guard-interval
correlation (§7).

HamDRM A and B have the same Tu/Tg as DRM30 A and B (`D:params.rs:55-72`). HamDRM E is
**new**: it is not DRM30 C (704, 4/11) and not DRM30 D (448, 11/14).

### 1.2 Carrier ranges (spectrum occupancy)

`iTableCarrierKmin[SO][A,B,E] = {{2,1,1},{2,1,1}}` and
`iTableCarrierKmax = {{54,45,29},{58,51,31}}` **[c tables/TableCarrier.h:87-95]**.

| | SO_0 ("2.3 kHz") | SO_1 ("2.5 kHz") |
|---|---|---|
| A | k = 2..54 (53 carriers) | k = 2..58 (57) |
| B | k = 1..45 (45) | k = 1..51 (51) |
| E | k = 1..29 (29) | k = 1..31 (31) |

SO naming: `E:FAC/FAC.cpp:404,429` marks "2.5 khz" for SO bit 1 and "2.3 khz" for 0.
k = 0 is never inside a range. Mode A also marks k = ±1 as unused
**[c CellMappingTable.cpp:416-424]**, which has no effect because Kmin = 2. All
carriers are on the positive side of DC (single-sided, unlike DRM30 SO 2..5).

Nominal width (Kmax−Kmin+1)·Δf [d]: A 2208/2375 Hz, B 2109/2391 Hz, E 2175/2325 Hz.

### 1.3 Audio frequency on air

**Result: carrier k is transmitted at f = F_dc + k·Δf, with F_dc = 350 Hz by default.**
The signal is real-valued and upper-sideband, so higher k means higher audio frequency.

Derivation:
1. The IFFT places carrier k in bin `iIndexDCFreq + k`, where
   `iIndexDCFreq = VIRTUAL_INTERMED_FREQ·N/48000 = N/8` [d 6000·N/48000]: A 144, B 128,
   E 80 **[c CellMappingTable.cpp:179-186; OFDM.cpp:46-47]**. So carrier k sits at
   6000 + k·Δf Hz in complex baseband. The 6000 Hz term is phase-continuous across
   symbols because 6000·Ts/48000 is an integer (160 for A/B, 120 for E)
   **[c TableDRMGlobal.h:34-41]**.
2. EasyDRF builds with `USEPAPR 1` **[c Parameter.h:39]**. `OFDM.cpp:105-106` then mixes
   by −(6000 + OFFSET) Hz with OFFSET = 1225 Hz **[c TableDRMGlobal.h:44]**. This puts
   carrier k at k·Δf − 1225 Hz, roughly centred on 0 Hz for the PAPR clipper.
3. `DRMSignalIO.cpp:196-216` (constants at `:284-286`) runs a Weaver mixer at 12 kHz
   with Fc = OFFSET + rDefCarOffset and outputs `I·sin(θ) − Q·cos(θ)`, θ = −2π·Fc·n/12000.
   This equals Re{z·e^{j(π/2 + 2πFc·n/12000)}}: an up-shift by Fc plus a constant π/2.
   Net position: k·Δf − 1225 + 1225 + rDefCarOffset = **rDefCarOffset + k·Δf** [d].
4. The `USEPAPR 0` path (`OFDM.cpp:102-103`) and QSSTV's `QT:OFDM.cpp:100-103` shift by
   (rDefCarOffset − 6000) instead. They give the same carrier positions.
5. rDefCarOffset defaults to 350 Hz:
   - EasyDRF: `E:DrmTransmitter.cpp:212` and `E:..\hamdrm.cpp:423`. User-settable through
     `SetDCFreq` (`..\hamdrm.cpp:157-160`) and the TX settings dialog
     (`..\Dialog.cpp:2692-2707`).
   - QSSTV TX: `QT:CDrmTransmitter.cpp:217`.

**OFFSET 1225 does not change on-air compatibility.** It is an internal IF used only by
the PAPR stage: the down-mix and the Weaver up-mix cancel. Its side effects are a
clipper and band filters (pass 0–1250 Hz, stop from 1400 Hz, at 12 kHz around the
1225 Hz point **[c TransmitterFilter.h:69-90]**). Those raise EVM but do not change the
format.

Carrier audio frequencies with F_dc = 350 Hz [d]:

| | SO_0 | SO_1 |
|---|---|---|
| A | 433.3–2600.0 Hz | 433.3–2766.7 Hz |
| B | 396.9–2459.4 Hz | 396.9–2740.6 Hz |
| E | 425.0–2525.0 Hz | 425.0–2675.0 Hz |

Receiver expectations:
- EasyDRF RX searches for DC around `centerfreq = 350` Hz with a window of 5 Hz
  (user setting) **[c ..\Dialog.cpp:2845-2846]**.
- QSSTV RX searches the whole FFT range (`QR:getfoffsint.cpp:95-161`).
- EasyPal: unknown (closed source, not in the references).

The time/phase reference is standard DRM: the cell value c(s,k) modulates
e^{j2πkΔf(t − Tg − s·Ts)} within symbol s, and the guard is a cyclic prefix of the last
Tg samples **[c OFDM.cpp:61-67]**.

---

## 2. Pilots

All pilot phases are in units of 2π/1024. Cell values are
`amp·(cos 2πφ/1024 + j sin 2πφ/1024)` **[c CellMappingTable.cpp:606-615]**. s is the
symbol index inside the frame (0..Ns−1).

**Precedence** (later ones overwrite earlier ones) **[c CellMappingTable.cpp:238-424]**:
MSC → FAC → scattered (gain) → time → frequency → DC. A scattered cell overwritten by a
time or frequency pilot keeps both flags, but the value is the later pilot's (amplitude
√2). This matches DecDRM `D:cellmap.rs:139-191`. No FAC cell collides with any pilot in
any mode [py], so FAC is always 45 cells per frame.

### 2.1 Frequency pilots (every symbol, amp √2)

| Mode | {k, φ} | Source |
|---|---|---|
| A | {9,205} {27,836} {36,215} | **[c TableCarrier.h:68-72]** |
| B | {8,331} {24,651} {32,555} | **[c :74-78]** |
| E | {5,788} {15,1014} {20,332} | **[c :80-84]** |

- In **every mode the pilots sit at 375, 1125 and 1500 Hz above DC**
  [d 9·41.667 = 8·46.875 = 5·75 = 375, and so on]. The DC search exploits this:
  `E:sync/FreqSyncAcq.cpp:272-277` uses mode B's pilot table for all modes, and
  `QR:getfoffsint.cpp:95-97` scales 9/27/36 by Tu/288.
- **Mode E special case:** on odd s, the first two pilots (k = 5 and 15) get φ+512
  **[c CellMappingTable.cpp:379-402]**. This is the DRM30 mode-D rule moved to mode E.
  The reason is that 5·Ts/Tu = 7.5 and 15·Ts/Tu = 22.5 cycles are not integers, while
  20·1.5 = 30 is.
- Relation to DRM30: A and B use the DRM30 phases at half the carrier index (DRM30 A
  18/54/72, B 16/48/64; `D:tables.rs:101-109`). E uses DRM30 D's phases on carriers
  5/15/20 instead of 7/21/28.

### 2.2 Time (sync) pilots (s = 0 only, amp √2)

**[c tables/TableCarMap.h:40-90]** ({k, φ}):

- **A** (16): {6,973} {7,205} {11,717} {12,264} {15,357} {16,357} {23,952} {29,440}
  {30,856} {33,88} {34,88} {38,68} {39,836} {41,836} {45,836} {46,1008}
- **B** (15): {6,304} {10,331} {11,108} {14,620} {17,192} {18,704} {27,44} {28,432}
  {30,588} {33,844} {34,651} {38,651} {40,651} {41,460} {44,944}
- **E** (8): {7,432} {8,331} {13,108} {14,620} {21,192} {22,704} {26,44} {27,304}

The phase sequences follow DRM30 A and B (`D:tables.rs:125-134`), but the **carrier
positions are new**. Every listed k lies inside Kmin..Kmax for both SOs. That matters
because EasyDRF's counter-based matcher only advances on a hit
**[c CellMappingTable.cpp:336-358]**.

In mode E, time pilots k = 13 and 21 coincide with scattered positions. The time-pilot
value wins.

### 2.3 Scattered (gain reference) pilots

Position rule **[c CellMappingTable.cpp:274-276]**:

    k = k0 + x·(s mod y) + x·y·p,  p ≥ 0 integer, Kmin ≤ k ≤ Kmax

| Mode | x (FreqInt) | y (TimeInt) | k0 | Source |
|---|---|---|---|---|
| A | 4 | 5 | 2 | **[c TableCarMap.h:95-96,105]** |
| B | 2 | 3 | 1 | **[c :98-99,107]** |
| E | 1 | 4 | 1 | **[c :101-102,109]** |

EasyDRF computes k0 as (int)(x/2 + 0.5) **[c :274]**, which gives the same values.
Because Kmin ≥ k0 in every mode, the starting p is 0 and DecDRM's
`(k−k0−x·(s mod y)) rem (x·y) == 0` test selects exactly the same cells [d].

Phase **[c CellMappingTable.cpp:289-303]**: n = s mod y, m = ⌊s/y⌋,
p = (k − k0 − n·x)/(x·y), and

    φ = (4·Z[n][m] + p·W[n][m] + p²·(1+s)·Q) mod 1024

- **A**: W 5×3, Z 5×3, Q = 36 **[c TableCarMap.h:113-127]**. Identical to DRM30 A.
- **B**: W 3×5, Z 3×5, Q = 12 **[c :131-141]**. Identical to DRM30 B.
- **E** (new) **[c :145-157]**, Q = 10:
  - W rows: {512,0,512,0,512} {0,512,0,512,0} {512,0,512,0,512} {0,512,0,512,0}
  - Z rows: {0,57,164,64,12} {168,255,161,106,118} {25,232,132,233,38} {168,255,161,106,118}
  - These are B's tables with a fourth row. Columns m = 0..4 cover s = 0..19.

Amplitude √2. **Boosted** (amplitude 2) when k is in
`iScatPilGain[SO]` **[c TableCarMap.h:161-174]**:

| | SO_0 | SO_1 |
|---|---|---|
| A | {2, **4**, 50, 54} | {2, 6, 54, 58} |
| B | {1, 3, 43, 45} | {1, 3, 49, 51} |
| E | {1, 29, 0, 0} | {1, 31, 0, 0} |

- k = 4 is never a mode-A scattered position, so **A/SO_0 has only 3 boosted carriers
  (2, 50, 54)**, and k = 6 stays at √2 (see §9). Zeros never match.
- Boosted cells per superframe [py]: A 27/36, B 60/60, E 30/30.
- The DRM30 boosted tables (`D:tables.rs:230-268`) do **not** apply.

### 2.4 Power normalisation

Data cells have mean power 1, pilots 2 and boosted pilots 4. DC cells are 0
**[c CellMappingTable.cpp:460-486]**. `rAvPowPerSymbol` (sum over the carriers of one
symbol, averaged over the superframe) [py]:

| | SO_0 | SO_1 |
|---|---|---|
| A | 61.0667 | 65.6667 |
| B | 59.3333 | 66.3333 |
| E | 39.8000 | 42.3000 |

The TX scales by `6000/sqrt(rAvPowPerSymbol)` **[c DRMSignalIO.cpp:404]** (level only).

### 2.5 Changes vs DecDRM tables/cellmap (DRM30)

1. FAC table: 45 cells/frame, new positions (§3.1). DRM30 has 65.
2. Frequency, time, scattered-E and boosted tables are all replaced (§2.1–2.3).
3. The mode-D frequency-pilot rule (`D:cellmap.rs:181`) must apply to HamDRM mode E.
4. **No SDC.** Every non-pilot, non-FAC cell is MSC, including the first symbols of
   the superframe (`D:cellmap.rs:124,139` must use 0 SDC symbols).
5. Every frame of a superframe has an identical cell pattern [py]. The scattered period
   y divides Ns (15/5, 15/3, 20/4).

---

## 3. Cell mapping

### 3.1 FAC cell positions (per frame; symbol: carriers)

**[c TableCarrier.h:9-24 (A), 26-42 (B), 44-64 (E)]**; also identical in
`QR:mkfacmap.cpp`.

- **A**: s1:10,22,30,50; s2:14,26,34; s3:18,30,38; s4:22,34,42; s5:18,26,38,46;
  s6:22,30,42,50; s7:26,34,46; s8:10,30,38,50; s9:14,34,42; s10:18,38,46;
  s11:10,22,42,50; s12:14,26,46; s13:18,30; s14:22,34
- **B**: s0:21; s1:11,23,35; s2:13,25,37; s3:15,27,39; s4:5,17,29,41; s5:7,19,31;
  s6:9,21,33; s7:11,23,35; s8:13,25,37; s9:15,27,39; s10:5,17,29,41; s11:7,19,31;
  s12:9,21,33; s13:11,23,35; s14:13,25,37
- **E**: s1:7,23; s2:8,16,24; s3:9,17; s4:10,18; s5:11,19; s6:4,12; s7:13,21;
  s8:6,14,22; s9:7,23; s10:8,16,24; s11:9,13,17; s12:10,18; s13:11,19;
  s14:4,12,16; s15:13,21; s16:6,14,22; s17:7,23; s18:8,16,24; s19:9,17

Each mode has 45 cells per frame, the same in every frame of the superframe. FAC
cells are filled in order of increasing (s, k) **[c OFDMCellMapping.cpp:74-79]**.

### 3.2 MSC cells [py]

| Mode/SO | MSC cells/superframe | N_MUX (cells/frame) | dummy cells |
|---|---|---|---|
| A/0 | 1941 | 647 | 0 |
| A/1 | 2112 | 704 | 0 |
| B/0 | 1365 | 455 | 0 |
| B/1 | 1590 | 530 | 0 |
| E/0 | 1017 | 339 | 0 |
| E/1 | 1107 | 369 | 0 |

`N_MUX = floor(total/3)` and `dummy = total − 3·N_MUX`
**[c CellMappingTable.cpp:496-504]**. Because every frame is identical, each
transmission frame holds exactly N_MUX MSC cells. So MSC frame f is exactly the MSC
cells of transmission frame f, and **there are no dummy cells**. (EasyDRF does not
define QAM4 dummy cells, `OFDMCellMapping.cpp:124-133`, but that never matters.)

MSC cells per symbol for s = 0..Ns−1 [py]:
- A/0: 31,43,44,44,45,43,43,44,43,45,44,43,44,45,46
- A/1: 35,47,48,48,48,47,47,48,47,48,48,47,48,49,49
- B/0: 18,31,32,31,30,32,31,31,32,31,30,32,31,31,32
- B/1: 23,36,37,36,35,37,36,36,37,36,35,37,36,36,37
- E/0: 13,17,17,18,17,17,18,18,16,17,17,17,17,17,17,18,16,17,17,18
- E/1: 15,18,18,20,19,18,19,20,18,18,18,19,19,18,18,20,18,18,18,20

**MSC cell ordering**: increasing carrier within a symbol, symbols in time order
**[c OFDMCellMapping.cpp:54-71]**. Same as DRM30/Dream.

### 3.3 Cell maps (one frame; test vectors) [py]

Legend: `.` MSC, `X` FAC, `T` time pilot, `f` frequency pilot, `0` scattered,
`*` boosted scattered. Columns run k = Kmin..Kmax.

```
mode A SO0 (k=2..54)                                     mode A SO1 (k=2..58)
 0 *...TT.f.TT..TT.....0T...f.TT..TT.f.TT.T0..TT........  *...TT.f.TT..TT.....0T...f.TT..TT.f.TT.T0..TT............
 1 ....0..fX...........X...0f..X.....f.........0...X....  ....*..fX...........X...0f..X.....f.........0...X........
 2 .......f0...X...........Xf..0...X.f.............*....  .......f0...X...........Xf..0...X.f.............0........
 3 .......f....0...X........f..X...0.f.X...............*  .......f....0...X........f..X...0.f.X...............*....
 4 .......f........0...X....f......X.f.0...X............  .......f........0...X....f......X.f.0...X...............*
 5 *......f........X...0...Xf........f.X...0...X........  *......f........X...0...Xf........f.X...0...X............
 6 ....0..f............X...0f..X.....f.....X...0...X....  ....*..f............X...0f..X.....f.....X...0...X........
 7 .......f0...............Xf..0...X.f.........X...*....  .......f0...............Xf..0...X.f.........X...0........
 8 .......fX...0............f..X...0.f.X...........X...*  .......fX...0............f..X...0.f.X...........X...*....
 9 .......f....X...0........f......X.f.0...X............  .......f....X...0........f......X.f.0...X...............*
10 *......f........X...0....f........f.X...0...X........  *......f........X...0....f........f.X...0...X............
11 ....0..fX...........X...0f........f.....X...0...X....  ....*..fX...........X...0f........f.....X...0...X........
12 .......f0...X...........Xf..0.....f.........X...*....  .......f0...X...........Xf..0.....f.........X...0........
13 .......f....0...X........f..X...0.f.................*  .......f....0...X........f..X...0.f.................*....
14 .......f........0...X....f......X.f.0................  .......f........0...X....f......X.f.0...................*

mode B SO0 (k=1..45)                            mode B SO1 (k=1..51)
 0 *....T0f.TT.0T..TT0.X..f0.TT.T0fTT..0T.TT.*T.  *....T0f.TT.0T..TT0.X..f0.TT.T0fTT..0T.TT.0T....*..
 1 ..*....f0.X...0.....0.Xf..0....f0.X...0.....*  ..*....f0.X...0.....0.Xf..0....f0.X...0.....0.....*
 2 ....0..f..0.X...0.....0fX...0..f..0.X...0....  ....0..f..0.X...0.....0fX...0..f..0.X...0.....0....
 3 *.....0f....0.X...0....f0.X...0f....0.X...*..  *.....0f....0.X...0....f0.X...0f....0.X...0.....*..
 4 ..*.X..f0.....0.X...0..f..0.X..f0.....0.X...*  ..*.X..f0.....0.X...0..f..0.X..f0.....0.X...0.....*
 5 ....0.Xf..0.....0.X...0f....0.Xf..0.....0....  ....0.Xf..0.....0.X...0f....0.Xf..0.....0.....0....
 6 *.....0fX...0.....0.X..f0.....0fX...0.....*..  *.....0fX...0.....0.X..f0.....0fX...0.....0.....*..
 7 ..*....f0.X...0.....0.Xf..0....f0.X...0.....*  ..*....f0.X...0.....0.Xf..0....f0.X...0.....0.....*
 8 ....0..f..0.X...0.....0fX...0..f..0.X...0....  ....0..f..0.X...0.....0fX...0..f..0.X...0.....0....
 9 *.....0f....0.X...0....f0.X...0f....0.X...*..  *.....0f....0.X...0....f0.X...0f....0.X...0.....*..
10 ..*.X..f0.....0.X...0..f..0.X..f0.....0.X...*  ..*.X..f0.....0.X...0..f..0.X..f0.....0.X...0.....*
11 ....0.Xf..0.....0.X...0f....0.Xf..0.....0....  ....0.Xf..0.....0.X...0f....0.Xf..0.....0.....0....
12 *.....0fX...0.....0.X..f0.....0fX...0.....*..  *.....0fX...0.....0.X..f0.....0fX...0.....0.....*..
13 ..*....f0.X...0.....0.Xf..0....f0.X...0.....*  ..*....f0.X...0.....0.Xf..0....f0.X...0.....0.....*
14 ....0..f..0.X...0.....0fX...0..f..0.X...0....  ....0..f..0.X...0.....0fX...0..f..0.X...0.....0....

mode E SO0 (k=1..29)            mode E SO1 (k=1..31)
 0 *...f.TT0...TTf.0..fTT..0TT.*  *...f.TT0...TTf.0..fTT..0TT.0..
 1 .0..f0X..0...0f..0.f.0X..0...  .0..f0X..0...0f..0.f.0X..0...0.
 2 ..0.f.0X..0...fX..0f..0X..0..  ..0.f.0X..0...fX..0f..0X..0...*
 3 ...0f..0X..0..f0X..f...0...0.  ...0f..0X..0..f0X..f...0...0...
 4 *...f...0X..0.f.0X.f0...0...*  *...f...0X..0.f.0X.f0...0...0..
 5 .0..f0...0X..0f..0Xf.0...0...  .0..f0...0X..0f..0Xf.0...0...0.
 6 ..0Xf.0...0X..f...0f..0...0..  ..0Xf.0...0X..f...0f..0...0...*
 7 ...0f..0...0X.f0...fX..0...0.  ...0f..0...0X.f0...fX..0...0...
 8 *...fX..0...0Xf.0..f0X..0...*  *...fX..0...0Xf.0..f0X..0...0..
 9 .0..f0X..0...0f..0.f.0X..0...  .0..f0X..0...0f..0.f.0X..0...0.
10 ..0.f.0X..0...fX..0f..0X..0..  ..0.f.0X..0...fX..0f..0X..0...*
11 ...0f..0X..0X.f0X..f...0...0.  ...0f..0X..0X.f0X..f...0...0...
12 *...f...0X..0.f.0X.f0...0...*  *...f...0X..0.f.0X.f0...0...0..
13 .0..f0...0X..0f..0Xf.0...0...  .0..f0...0X..0f..0Xf.0...0...0.
14 ..0Xf.0...0X..fX..0f..0...0..  ..0Xf.0...0X..fX..0f..0...0...*
15 ...0f..0...0X.f0...fX..0...0.  ...0f..0...0X.f0...fX..0...0...
16 *...fX..0...0Xf.0..f0X..0...*  *...fX..0...0Xf.0..f0X..0...0..
17 .0..f0X..0...0f..0.f.0X..0...  .0..f0X..0...0f..0.f.0X..0...0.
18 ..0.f.0X..0...fX..0f..0X..0..  ..0.f.0X..0...fX..0f..0X..0...*
19 ...0f..0X..0..f0X..f...0...0.  ...0f..0X..0..f0X..f...0...0...
```

---

## 4. FAC

### 4.1 Block layout (48 bits per frame, MSB first)

Writer: `E:FAC/FAC.cpp:46-227`. Reader: `:286-777`. Bit order: `Enqueue` writes MSB
first **[c Vector.h:160-173]**.

| Bits | Width | Field | Values |
|---|---|---|---|
| 0–1 | 2 | Identity | TX: frame 0 → `11`, frame 1 → `01`, frame 2 → `10` **[c FAC.cpp:63-78]**. RX: `00` or `11` → frame 0 **[c :329-346]** |
| 2 | 1 | Spectrum occupancy | 0 = SO_0 (2.3 kHz), 1 = SO_1 (2.5 kHz) **[c :81-90]** |
| 3 | 1 | Interleaver depth | 0 = long (D=5), 1 = short (D=1) **[c :93-102]** |
| 4 | 1 | MSC mode | 0 = 64-QAM; 1 = 16-QAM, and also sent as 1 for 4-QAM **[c :105-116]** |
| 5 | 1 | Protection level | 0 or 1 = `MSCPrLe.iPartB` **[c :119-128]**. EasyDRF's UI calls 0 "Normal" and 1 "Low" (`..\Dialog.cpp:2685-2688`) |
| 6 | 1 | Audio/Data | 0 = audio (digital voice), 1 = data **[c :131-176]** |
| 7–8 | 2 | if data: Packet ID; if audio: audio coding (0 LPC, 1 CELP, 2 SPEEX, 3 SSTV) | **[c :137-151, 168-169]** |
| 9 | 1 | if data: **4-QAM flag** (1 = MSC is 4-QAM); if audio: text flag | **[c :153-162, 171-174]** |
| 10–30 | 21 | Label: 3 chars × 7 bits | frame f carries label chars 3f..3f+2. The label is at most 9 chars; missing chars = 0; each char is `& 127` **[c :180-200]**. RX drops 0 chars and assembles all 3 frames **[c :681-708]** |
| 31 | 1 | Spare / **audio 4-QAM flag** | Written (=1) only when audio *and* 4-QAM **[c :205-212]** (EasyDRF 2022 extension). RX reads it only for audio **[c :711-751]**. Otherwise not written, so 0 |
| 32–39 | 8 | Unused | Never written (zero-initialised vector); included in the CRC. **Send 0, ignore on RX** |
| 40–47 | 8 | CRC-8 | over bits 0–39 (5 bytes) **[c :217-226]** |

Notes:
- Effective MSC scheme on RX **[c FAC.cpp:756-762]**: the 4-QAM flag (bit 9 for data,
  bit 31 for audio) wins. Otherwise bit 4: 0 = 64-QAM, 1 = 16-QAM.
- RX sets service ID 73, language 7 and service description 3 itself; none of these is
  transmitted **[c :322-324]**.
- QSSTV TX audio branch writes coding = `11` and text = 0 and never writes bit 31
  **[c QT:FAC/FAC.cpp:130-135]**.

**CRC-8**: G(x) = x⁸+x⁴+x³+x²+1 (mask `(1<<2)|(1<<3)|(1<<4)`). The register starts at
all ones and the output is the ones' complement. Bytes are fed MSB first
**[c CRC.cpp:37-80, 98]**. This is the same as the DRM30 FAC CRC (DecDRM `fec/crc.rs`
`crc8`). QSSTV checks by inverting the received CRC bits and requiring a zero remainder
over 48 bits (`QR:channeldecode.cpp:364-377`), which is equivalent.

### 4.2 FAC coding chain

Source: `E:mlc/MLC.cpp:444-481`.

1. **Energy dispersal** over all 48 bits, CRC included. The PRBS is x⁹+x⁵+1 with
   all-ones preset, re-initialised every block **[c mlc/EnergyDispersal.cpp:45-81]**.
2. **Convolutional code**: mother code 1/4, K = 7, generators octal 133/171/145/133
   (stored bit-reversed as 0155/0117/0123/0155) **[c TableMLC.h:44-51]**. Puncturing
   pattern index 6, rate **3/5** (`iCodRateCombFDC4SM = 6`, **[c TableMLC.h:467-469]**;
   pattern `{3,5, 0011,0001,0011}` **[c :177-185]**).
   - **The tail bits continue the part-B pattern** instead of using the special tail
     pattern ("In case of FAC do not use special tailbit-pattern" **[c
     ChannelCode.cpp:122-137]**). DecDRM already has this as `is_fac`
     (`D:fec/puncture.rs:59`).
   - Coded length [d]: (48+6)/3·5 = 90 bits = 2·45 cells.
3. **Bit interleaver**: t0 = 21 over 90 bits (`iInterlSequ4SM = {1,…}` →
   `BitInterleaver[1]` with t0 = 21 **[c TableMLC.h:481; MLC.cpp:162-163]**).
4. **4-QAM** mapping: cell i = (a[2i] + j·a[2i+1]) with 0 → +1/√2 and 1 → −1/√2
   **[c QAMMapping.cpp:51-62; TableQAMMapping.h:79-82]**.
5. 45 cells mapped to the §3.1 positions. FAC has no cell interleaving.

---

## 5. MSC

### 5.1 Constellations

Standard DRM30 SM mappings **[c TableQAMMapping.h:39-48, 72-82; QAMMapping.cpp:51-118]**.
Level j supplies bits y_j[2i] (I) and y_j[2i+1] (Q); the MSB comes from level 0.

- 64-QAM: index (y0,y1,y2) → {1.0801, −0.1543, 0.4629, −0.7715, 0.7715, −0.4629, 0.1543,
  −1.0801}
- 16-QAM: (y0,y1) → {0.9487, −0.3162, 0.3162, −0.9487}
- 4-QAM: ±0.7071

These are identical to DecDRM `tables.rs:399-412`.

### 5.2 Code rates per FAC protection bit (`iCodRateComb*`)

**[c TableMLC.h:449-464]**. Puncturing pattern indices give RX/RY:
0=1/4, 2=1/3, 4=1/2, 6=3/5, 7=2/3, 9=3/4, 10=4/5 **[c TableMLC.h:74-282]**.

| Scheme | prot 0 | prot 1 |
|---|---|---|
| 4-QAM (`CS_1_SM`) | R0 = 3/5 (idx 6), both levels **[c :449-451]** | same |
| 16-QAM (`CS_2_SM`) | R0 1/3, R1 2/3 (idx 2,7); mean 1/2 | R0 1/2, R1 3/4 (idx 4,9); mean 5/8 |
| 64-QAM (`CS_3_SM`) | R0 1/4, R1 1/2, R2 3/4 (idx 0,4,9); mean 1/2 | R0 1/3, R1 2/3, R2 4/5 (idx 2,7,10); mean 3/5 |

- Only rows 0 and 1 of the DRM30 tables are reachable, because the FAC field is 1 bit.
  Rows 2 and 3 of `iCodRateCombMSC64SM` are unused.
- **DecHAMDRM extension:** 4-QAM with rate 1/2 (idx 4) when FAC bits 32–39 are `0xA2`
  or `0xA3` (our "4-QAM normal"); the standard 3/5 is our "4-QAM low". See DESIGN.md.
- 4-QAM in the MSC is **HamDRM-only** (DRM30 has no MSC 4-QAM) **[c MLC.cpp:494-530]**.
- **No partitioning**: N1 = 0, there is no higher protected part and no hierarchical
  mode or VSPP. All bits are "part B" **[c MLC.cpp:509, 554, 613; EnergyDispersal init
  with VSPP = 0, MLC.cpp:151]**.

Bits per level **[c MLC.cpp:516-518, 571-576, 630-635]**: M_j = RX_j·⌊(2·N_MUX − 12)/RY_j⌋.

**Tail-bit pattern** per level: r = (2·N_MUX − 12) − RY·⌊(2·N_MUX − 12)/RY⌋, which
selects row r of `iPunctPatTailbits` **[c ChannelCode.cpp:65-79; TableMLC.h:286-442]**.
This is the DRM30 rule.

### 5.3 Bit and cell interleaving

- **Bit interleaver** per level **[c TableMLC.h:481-483]**:
  - 4-QAM: level 0 t0 = 21
  - 16-QAM: level 0 t0 = 13, level 1 t0 = 21
  - 64-QAM: level 0 none, level 1 t0 = 13, level 2 t0 = 21
  - Block size 2·N_MUX (part A is empty).
- Permutation **[c interleaver/BlockInterleaver.cpp:35-69]**:
  - Rule: s = 2^(⌊log2 n⌋+1), q = s/4 − 1, Π(0) = 0,
    Π(i) = (t0·Π(i−1) + q) mod s, repeated while ≥ n; then out[i] = in[Π(i)]
    (`BitInterleaver.cpp:45-59`).
  - Dream's s differs from the standard only when n is a power of two. No HamDRM size is
    (90, 678, 738, 910, 1060, 1294, 1408, 339, 369, 455, 530, 647, 704) [d], so DecDRM's
    `fec/interleaver.rs` is safe.
- **Cell (symbol) interleaver** **[c interleaver/SymbolInterleaver.h:39-43;
  SymbolInterleaver.cpp:41-64]**:
  - t0 = 5 over N_MUX cells.
  - Depth D = 5 (long, 2 s) or 1 (short, 400 ms).
  - out_n[i] = in_{n − (i mod D)}[Π(i)]. Same as DRM30.
  - EasyDRF TX default is short **[c DrmTransmitter.cpp:202]**.

### 5.4 Energy dispersal

Same PRBS as the FAC, applied over all M = ΣM_j bits of each MSC frame and reset every
frame **[c MLC.cpp:44, 151; EnergyDispersal.cpp:45-81]**. DecDRM `fec/dispersal.rs` is
identical.

### 5.5 Decoded MSC bits per frame (iNumDecodedBitsMSC) [py]

Per-level bits are in brackets.

| Mode/SO | N_MUX | 4-QAM | 16/p0 | 16/p1 | 64/p0 | 64/p1 |
|---|---|---|---|---|---|---|
| A/0 | 647 | 768 | 1281 [427,854] | 1601 [641,960] | 1921 [320,641,960] | 2305 [427,854,1024] |
| A/1 | 704 | 837 | 1395 [465,930] | 1745 [698,1047] | 2094 [349,698,1047] | 2511 [465,930,1116] |
| B/0 | 455 | 537 | 897 [299,598] | 1121 [449,672] | 1345 [224,449,672] | 1613 [299,598,716] |
| B/1 | 530 | 627 | 1047 [349,698] | 1310 [524,786] | 1572 [262,524,786] | 1883 [349,698,836] |
| E/0 | 339 | 399 | 666 [222,444] | 831 [333,498] | 997 [166,333,498] | 1198 [222,444,532] |
| E/1 | 369 | 435 | 726 [242,484] | 906 [363,543] | 1087 [181,363,543] | 1306 [242,484,580] |

The 4-QAM column equals QSSTV's hard-coded `L[1]` values (`QR:channeldecode.cpp:665-705`).

Worked check, B/0 4-QAM [d]: 2·455 − 12 = 898. M = 3·⌊898/5⌋ = 537. Tail row
898 − 5·179 = 3, which carries 15 bits. Coded bits: 179·5 + 15 = 910 = 2·455.

### 5.6 streamlen / packlen tables in FAC.cpp

`E:FAC/FAC.cpp:39-40` holds the globals. `:402-561` sets them by mode, SO, protection
and QAM for 16/64-QAM, and `:627-663` / `:711-751` cover 4-QAM. Values are
(streamlen, packlen) in bytes.

| Mode/SO | 4-QAM | 16/p0 | 16/p1 | 64/p0 | 64/p1 |
|---|---|---|---|---|---|
| A/0 | 96, 93 | 160, 157 | 200, 197 | 240, 237 | 288, 285 |
| A/1 | 104, 101 | 174, 171 | 218, 215 | 261, 258 | 313, 310 |
| B/0 | 67, 64 | 112, 109 | 140, 137 | 168, 165 | 201, 198 |
| B/1 | 78, 75 | 130, 127 | 163, 160 | 196, 193 | 235, 232 |
| E/0 | 49, 46 | 83, 80 | 103, 100 | 124, 121 | 149, 146 |
| E/1 | 54, 51 | 90, 87 | 113, 110 | 135, 132 | 163, 160 |

What the two numbers mean:
- **streamlen** = ⌊iNumDecodedBitsMSC/8⌋. This is the stream-0 length in bytes, used by
  the MSC demux as `Stream[0].iLenPartB` (**[c FAC.cpp:765; MSCMultiplexer.cpp:144]**).
- **packlen** = streamlen − 3. This is the packet data-field length (1 header byte and
  2 CRC bytes make up the rest).
- The TX computes the same from its own bit count:
  - EasyDRF: `calcpacklen(bits) = bits/8 − 3` **[c callsign2.cpp:91-95]**, applied at
    `..\hamdrm.cpp:148` and `..\Dialog.cpp:2709-2710`.
  - QSSTV: `PacLen = bits/8 − 3` **[c QT:CDrmTransmitter.cpp:95-98]**.
- Every entry equals the [py] value from §5.5. A receiver can compute them; no table is
  needed.

---

## 6. MSC multiplex

There is no SDC. The FAC alone implies the layout:
- one service (index 0), one stream (ID 0);
- the stream starts at bit 0 and is streamlen·8 bits long
  **[c MSCMultiplexer.cpp:133-165]**;
- the trailing (iNumDecodedBitsMSC mod 8) bits are not written by the TX (whatever the
  buffer holds) and are ignored by both receivers.

**Data mode** (pictures/files; EasyPal and QSSTV traffic):
- **Exactly one packet per MSC frame**: `iNumPack = bits / ((packlen+3)·8) = 1`
  **[c sourcedecoders/AudioSourceDecoder.cpp:518-537, 544-550]**.
- Packet layout **[c datadecoding/DataDecoder.cpp:36-147]**:

| Byte(s) | Content |
|---|---|
| 0 | First flag (1 bit), Last flag (1), Packet ID (2, = FAC bits 7–8), PPI (1), Continuity index (3, incremented mod 8 per packet) |
| 1 .. packlen | Data field. If PPI = 1: the first byte is the count of useful bytes, then the useful bytes, then 0x00 padding to packlen bytes |
| packlen+1 .. packlen+2 | CRC-16 over bytes 0..packlen |

- **CRC-16**: x¹⁶+x¹²+x⁵+1, all-ones preset, complemented output, MSB first
  **[c CRC.cpp:99]**.
- RX code: `DataDecoder.cpp:182-368`. QSSTV parses the same layout over ⌊bits/8⌋ bytes
  (`QR:sourcedecoder.cpp:84-160`).
- Above this is DAB MOT (data groups and segments, segment length packlen − 11
  **[c DataDecoder.cpp:157]**). That is out of PHY scope.
- In the padded case, EasyDRF writes one padding byte too many, and the CRC then
  overwrites it. The on-air result is still the layout above [d from
  `DataDecoder.cpp:109-146`].

**Audio mode** (digital voice): the whole frame of iNumDecodedBitsMSC bits goes to the
codec (LPC-10 / SPEEX / CELP / "SSTV" framing, `AudioSourceDecoder.cpp:300-486`). An
optional 4-byte text-message piece fills the last 4 bytes of the byte-aligned part when
bits > 1030 **[c AudioSourceDecoder.cpp:491-510]**. The codec framing is out of scope
here (unknown/not specified).

---

## 7. Sync specifics that matter for interop

- **Mode detection**:
  - EasyDRF correlates the guard interval for all three modes (signal decimated by 8)
    and picks the mode whose correlation magnitude, at the symbol-rate frequency
    1/Ts, is largest. Ts is 26.667 ms for A and B and 20 ms for E. **[c
    sync/TimeSync.cpp:349-390, 670-728]**
  - QSSTV uses ML guard correlation at 12 kHz with (Ts, Tu) = (320, 288), (320, 256),
    (240, 160) **[c QR:getmode.cpp; QR:drmdefs.h:23-31]**.
  - A and B share Ts and differ only in Tg/Tu, so detection must use the correct lag Tu.
- **DC / integer frequency acquisition**: correlate the PSD with pilots at
  +375/+1125/+1500 Hz from DC (§2.1).
  - EasyDRF searches F_dc ∈ 350 ± 5 Hz by default with 6144-point PSD blocks
    **[c FreqSyncAcq.cpp:272-311; FreqSyncAcq.h:54; ..\Dialog.cpp:2845-2846]**.
  - QSSTV searches ±Tu/2 bins around its K_dc **[c QR:getfoffsint.cpp:160]**.
  - **A TX must put DC at 350 Hz** (±a few Hz) to be decoded by EasyDRF with default
    settings.
- **Frame sync**: time pilots in s = 0. Superframe position comes from the FAC identity
  (no SDC); a FAC CRC failure just advances the frame counter
  **[c Data.cpp:172-197]**.

---

## 8. Differences from DRM30 / DecDRM (one line each)

**Unchanged; DecDRM code reusable as is:**
- `fec/conv.rs`: mother code and generators.
- `fec/puncture.rs`: the 13 patterns, the tail patterns and the `is_fac` rule.
- `fec/viterbi.rs`.
- `fec/qam.rs` and the QAM tables: 4, 16 and 64 SM.
- `fec/dispersal.rs`.
- `fec/interleaver.rs`: t0 13/21; safe because no HamDRM size is a power of two.
- `interleave.rs`: cell interleaver, t0 = 5, D = 5 or 1.
- `fec/crc.rs`: CRC-8 and CRC-16.
- Scattered-pilot phase formula and the A/B W/Z/Q tables.
- Pilot amplitudes (√2, boosted 2) and the cell-type precedence order.
- OFDM cyclic prefix and per-symbol phase reference.
- Frame = 400 ms, superframe = 3 frames, 48 kHz numerology for A and B.

**Needs a HamDRM parameter or table:**
- `params.rs`: add mode **E** (N = 640, Tg/Tu = 1/2, Ns = 20); drop C and D.
- `params.rs` carrier ranges: SO ∈ {0,1} only; ranges are A 2..54/58, B 1..45/51,
  E 1..29/31.
- `params.rs` `sdc_symbols()`: must be 0 (no SDC at all).
- `tables.rs` `NUM_FAC_CELLS`: 45 (was 65); positions from §3.1.
- `tables.rs` freq pilots: new carriers A 9/27/36, B 8/24/32, E 5/15/20 (same 375/1125/1500 Hz).
- `tables.rs` time pilots: new carrier lists (A 16, B 15, E 8 entries).
- `tables.rs` scattered: new mode-E set (x 1, y 4, k0 1, 4×5 W/Z, Q 10).
- `tables.rs` boosted pilots: new tables (§2.3), including A/SO_0 {2,4,50,54}.
- `cellmap.rs:181`: apply the odd-symbol +512 frequency-pilot rule to HamDRM mode E.
- `fac.rs`: replace with the 48-bit HamDRM FAC (§4.1). CRC-8 covers 40 bits, not 64.
- `fec/mlc.rs` `MlcParams::fac`: 48 bits (`FAC_BITS` 72 → 48), 45 cells, rate idx 6, t0 21.
- `fec/mlc.rs` `MlcParams::msc`: allow 4-QAM (rate idx 6, interleaver t0 21; panics today at `:233`).
- `fec/mlc.rs` `MlcParams::msc`: protection index limited to 0/1; part A always 0.
- `mux/*`: no SDC parsing. One stream of ⌊bits/8⌋ bytes, one packet per frame (§6).
- `tx/output.rs`: real USB audio with carrier 0 at F_dc = 350 Hz (not a centred IF).
- `rx/freqacq.rs`: HamDRM pilot offsets; search near F_dc = 350 Hz.
- `rx/timesync.rs` mode detection: hypotheses A/B/E with HamDRM Tu/Tg.

---

## 9. Disagreements and open points

**EasyDRF vs QSSTV:**

1. **Boosted pilots, A/SO_0.** EasyDRF and QSSTV TX transmit {2,4,50,54}
   (**[c TableCarMap.h:162]**, `QT:tables/TableCarMap.h:215`). Since k = 4 is never a
   scattered position, only 2, 50 and 54 are boosted. QSSTV RX expects {2,6,50,54}
   (`QR:drm.cpp:99`), so its reference amplitude at k = 6 is 2 instead of √2. The
   DRM30 analogue would be {2,6,98,102}. **Transmit {2,4,50,54}**, which is what goes on
   air.
2. **Time pilot B k = 44 phase.** EasyDRF and QSSTV TX use 944 (**[c TableCarMap.h:77]**,
   `QT:…TableCarMap.h:130`), which matches DRM30's phase sequence. QSSTV RX has 950
   (`QR:drm.cpp:39`). The 2° difference is harmless. Use 944.
3. **Mode E frequency-pilot inversion.** QSSTV RX applies the +512 rule only to its
   mode index 3, i.e. DRM30 D (`QR:demodulator.cpp:670-673`), never to E. This is
   harmless: in E the scattered positions that coincide with k = 5 and 15 occur only on
   even s.
4. **4-QAM in audio mode** (FAC bit 31) exists only in EasyDRF (2022). QSSTV TX never
   sets it, and QSSTV RX does not decode audio
   (`QR:sourcedecoder.cpp:79-82`).
5. **4-QAM flag interpretation.** QSSTV maps msc_mode = bit4 + 2·bit9 (data only,
   `QR:channeldecode.cpp:406-410`), so bit4 = 0 with bit9 = 1 is undefined there.
   EasyDRF lets bit 9 override bit 4. TX always sends bit4 = 1 with 4-QAM.
6. **QSSTV RX quirks** (not format differences):
   - zeroes FAC cell #9 before decoding (`QR:channeldecode.cpp:356-357`);
   - defines `Tg_C = 64` where Ts − Tu = 80 (`QR:drmdefs.h:30-32`);
   - searches DC over the full range, where EasyDRF uses 350 ± 5 Hz.
7. **TX audio path.** EasyDRF adds PAPR clipping and band filtering with the 1225 Hz
   internal offset. QSSTV TX shifts directly. Carrier frequencies on air are the same.

**Could not determine (unknown):**
- EasyPal's behaviour on any point (closed source, not in the references): its default
  F_dc, whether it shares the A/SO_0 boosted-pilot table, its FAC bits 31–39, and its
  audio-mode 4-QAM support.
- Contents of the trailing (bits mod 8) MSC bits and of FAC bits 32–39 as sent by
  EasyPal. EasyDRF and QSSTV never write them; the vectors start zeroed, so they are
  in practice 0. Receivers ignore the trailing MSC bits; FAC bits 32–39 are covered by
  the CRC, so send 0.
- Digital-voice codec framing (LPC-10/SPEEX/CELP payload layout): outside this PHY spec.
