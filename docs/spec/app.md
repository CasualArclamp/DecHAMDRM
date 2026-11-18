# HamDRM application layer — specification

Scope: everything above the MSC. The PHY spec (`phy.md`) ends with "the MSC delivers
N decoded bits per 400 ms DRM frame"; this document starts there. FAC fields are only
quoted where the application layer depends on them.

Sources (read-only, GPL): `E:` = `F:\codebases\easydrf-ref`, `Q:` = `F:\codebases\qsstv-ref\src`,
`D:` = `F:\codebases\DecDRM\crates`. Every number carries `file:line`; **(derived)**
marks values computed from cited code, **unknown** marks what the sources do not settle.
EasyPal itself is closed source; it descends from HB9TLK's `hamdrm.dll`, whose source is
merged into EasyDRF (`E:EasyDRF_changes.txt` entry of April 3rd 2021, and `E:hamdrm.cpp`).
"EasyPal-compatible" below therefore means "what the DLL code and QSSTV's EasyPal mode do".

Bit order: all multi-bit fields are MSB first unless stated (Dream `CVector::Enqueue`).
"CRC-16" everywhere = CCITT x^16+x^12+x^5+1, register initialised to all ones, MSB-first,
result inverted (`E:common/CRC.cpp:37-100`, polynomial mask at `:99`). Same as DRM/DAB
and `D:decdrm-data/src/crc.rs`.

---

## 1. Packet mode

### 1.1 Stream and packet length

HamDRM has one service and one stream; the whole MSC frame is that stream (there is no
SDC). Audio and data both come from stream 0, part B only (`E:common/MSCMultiplexer.cpp:143-165`).

Packet length (data field bytes, ES 201 980 "packet length") is not transmitted. Both
ends derive it from the mode:

- TX: `packlen = floor(iNumDecodedBitsMSC / 8) - 3` (`E:common/callsign2.cpp:91-94`,
  original DLL `E:common/libs/callsign.cpp:80-86`; QSSTV `Q:drmtx/common/CDrmTransmitter.cpp:93-98`).
- RX: looked up from the FAC fields (robustness mode, spectrum occupancy 2.3/2.5 kHz,
  QAM, protection level) in `E:common/FAC/FAC.cpp:402-560` (QAM16/64) and `:627-663`
  (QAM4 data). The RX stream length is always `packlen + 3`.

Table (packlen from FAC.cpp; segment size = packlen − 11, see 2.3, **derived**):

| Mode / BW | QAM4 | QAM16 prot 0 | QAM16 prot 1 | QAM64 prot 0 | QAM64 prot 1 |
|---|---|---|---|---|---|
| A 2.3 kHz | 93 / 82 | 157 / 146 | 197 / 186 | 237 / 226 | 285 / 274 |
| A 2.5 kHz | 101 / 90 | 171 / 160 | 215 / 204 | 258 / 247 | 310 / 299 |
| B 2.3 kHz | 64 / 53 | 109 / 98 | 137 / 126 | 165 / 154 | 198 / 187 |
| B 2.5 kHz | 75 / 64 | 127 / 116 | 160 / 149 | 193 / 182 | 232 / 221 |
| E 2.3 kHz | 46 / 35 | 80 / 69 | 100 / 89 | 121 / 110 | 146 / 135 |
| E 2.5 kHz | 51 / 40 | 87 / 76 | 110 / 99 | 132 / 121 | 160 / 149 |

Cells read "packlen / body segment size". QAM4 has one entry per mode and bandwidth,
independent of protection (`FAC.cpp:627-663`). The "else" branch of each table is mode E
although one comment says "2.5 khz A" (`FAC.cpp:509-511, 652-655`). The PHY spec must
guarantee `floor(iNumDecodedBitsMSC/8) = packlen + 3` for every row; otherwise TX and RX
disagree.

Packets per frame: `floor(stream bytes / (packlen + 3)) = 1` (TX
`E:common/sourcedecoders/AudioSourceDecoder.cpp:522-536`, RX `E:common/datadecoding/DataDecoder.cpp:401,416`).
So **exactly one packet per 400 ms frame**. The `iNumDecodedBitsMSC mod 8` bits left over
are not written (**derived**). QSSTV RX also treats the whole MSC as one packet
(`Q:drmrx/sourcedecoder.cpp:84-92`).

Note: packlen can exceed 255 (mode A QAM64). ES 201 980 limits it to 255 (8-bit SDC
field), so code that caps it (see section 10) must be widened.

### 1.2 Packet format (TX `E:DataDecoder.cpp:36-147`, RX `:182-368`)

```
byte 0 : First 1 | Last 1 | PacketId 2 | PPI 1 | CI 3
bytes  : data field, packlen bytes
2 bytes: CRC-16 over header + data field
```

- First = 1 on the first packet of a data unit (`:51-54`). Last = 1 when the remaining
  bytes ≤ packlen (`:57-66`).
- PacketId = `Service[0].DataParam.iPacketID` (`:69`), default 0 (`E:common/Parameter.cpp:53`).
  The FAC carries the packet id (`FAC.cpp:168-169, 623`). The RX decodes every id
  (`DataDecoder.cpp:341-349`) but shows only the FAC one.
- PPI = 1 when the remaining bytes < packlen (`:72-75`). Then the first data byte holds
  the useful byte count, followed by the data, then padding (`:104-118`). EasyDRF fills the
  padding with whatever bits follow the data unit in memory (stale) and its CRC overwrites
  the last padding byte (`:142-146`). Receivers ignore padding; a new TX should write 0x00.
- CI: 3 bits, +1 modulo 8 per packet, reset to 0 at encoder init only (`:78-83`, `:172`).
- The RX drops a data unit when the CI does not follow (`:256-257`) or a packet CRC
  fails (`:363-365`). An empty packet (First=Last=PPI=1, count 0) carries nothing (`:310-314`).
  EasyDRF TX never sends empty packets: every frame carries the next data group (**derived**
  from `:120-129`).
- Data units = MSC data groups (section 2); a new one always starts in a new packet.

**Length-byte overflow (derived):** the PPI count is 8 bits (`:109`, `:284`;
QSSTV `Q:drmrx/sourcedecoder.cpp:133-137`). When packlen ≥ 257 (A 2.5 QAM64, A 2.3 QAM64
prot 1) and the last body data group is 256…packlen−1 bytes, the count wraps and that
data group's CRC fails on every receiver. Full-size groups are not padded, so only the
last segment of some files is hit.

---

## 2. MSC data groups and MOT

Header mode only (data group types 3 and 4); no MOT directory. FAC data service ⇒
receiver assumes DAB user application 2 = MOT SlideShow (`FAC.cpp:620-625`).

### 2.1 Data group (TX `E:common/datadecoding/DABMOT.cpp:349-564`)

```
Ext 1 = 0 | CRCflag 1 = 1 | SegFlag 1 = 1 | UAflag 1 = 1 | Type 4 (3 header, 4 body)
CI 4 | RepIdx 4
Last 1 | SegmentNumber 15
Rfa 3 | TIdFlag 1 = 1 | LenInd 4 = 2 | TransportId 16
data field: RepCount 3 = 0 | SegmentSize 13 | segment bytes
CRC-16 over everything above
```

Header = 2 + 2 + 3 bytes, segmentation header 2, CRC 2 ⇒ 11 bytes overhead (**derived**).

- CI: separate counters for type 3 and type 4, +1 mod 16 per data group (`:420-437`).
- **RepIdx** (EasyDRF deviation): 0 in plain mode; in RS modes it carries 4 bits of
  `EncFileSize / 255` (number of 255-byte RS blocks): nibble `k = SegmentNumber mod 4`
  carries bits 4k…4k+3 (`:445-464`, `E:RS-defs.h:28` selects method 1). The old method 0
  (`mod 7`, 28 bits of byte size) is compiled out.
- **Rfa** (EasyDRF deviation): `max(ECCmode − 3, 0)` = RS level 0…4 (`:499-501`).
  QSSTV writes 0 (`Q:drmtx/common/datadecoding/DABMOT.cpp:481`).
- **TransportId**: see 2.4; in RS modes EasyDRF adds `DMmodehash` (`:525-530`).
- Segment size field = bytes in this segment (`:330-332`); RepCount always 0 (`:328`).
- EasyDRF RX always checks the CRC, whatever the CRC flag (`:729-731`); QSSTV checks it only
  when the flag is set (`Q:drmrx/sourcedecoder.cpp:238-253`).

### 2.2 MOT header (`E:DABMOT.cpp:125-233`; QSSTV identical layout `Q:drmtx/common/datadecoding/DABMOT.cpp:184-285`)

Header core (7 bytes): BodySize 28 | HeaderSize 13 | ContentType 6 | ContentSubType 9,
then three parameters **in this order**:

| Param | Bytes | Encoding |
|---|---|---|
| TriggerTime (id 5) | 5 | PLI 2, id 5, 32 zero bits ("now") (`:170-178`) |
| VersionNumber (id 6) | 2 | PLI 1, id 6, value 0 (`:189-196`) |
| ContentName (id 12) | 3 + n | PLI 3, id 12, Ext 0, DataFieldLength 7 = 1 + n, charset 0 (4 bits), rfa 0 (4 bits), n name bytes (`:208-233`) |

- HeaderSize = 7 + 5 + 3 + n + 2 = 17 + n (`:131-134`).
- n = file name length capped at 80 (`:72-74`; QSSTV `:106-107`). Bytes are written raw
  (Windows ANSI / Latin-1); charset is always 0 even for non-ASCII. Decode as Latin-1
  (QSSTV `Q:drmrx/sourcedecoder.cpp:395`).
- ContentType/SubType: EasyDRF always 2/1 (`:125-126`). QSSTV: 2/0 gif, 2/1 jpg, jpeg,
  jp2, jfif, rs1…rs4, 2/2 bmp, 2/3 png, default 2/1 (`Q:…/DABMOT.cpp:134-180`).
  **Interop rule:** the EasyDRF RX copies the body only if ContentType == 2 and
  SubType < 4 (`E:DABMOT.cpp:1549-1552`). Any other value delivers an empty object.
  Always send 2/0…2/3 (2/1 for arbitrary files).
- BodySize = body bytes as sent (after LZMA and RS, sections 3.3 and 3.4).

### 2.3 Segmentation

- Body segment size = packlen − 11 (`E:DataDecoder.cpp:157`); QSSTV uses MSC bytes − 14
  = the same (`Q:drmtx/common/datadecoding/MOTSlideShow.cpp:79`, `…/DABMOT.cpp:294`).
  A full body data group fills one packet exactly (**derived**).
- Header partition size = 98 bytes (`E:DABMOT.cpp:238`, QSSTV `:289`). The header is at most
  17 + 80 = 97 bytes, so it is always one segment (**derived**). The EasyDRF RX accepts
  only header segment 0: a type-3 group with SegmentNumber ≠ 0 resets the object
  (`E:DABMOT.cpp:987-1020`). Never segment the header.
- The header data group (28 + n bytes) can span several packets in small modes (e.g. 3
  packets at packlen 46) (**derived**).
- The last body segment holds the remainder; zero-length bodies are allowed
  (`EasyDRF_changes.txt`, November 25th 2021).

### 2.4 Transport ID

Filename hash, same in EasyDRF (`E:DABMOT.cpp:86-99`) and QSSTV (`Q:…/DABMOT.cpp:114-123`):

```
x = 0; a = 0                           // u8
for i, c in name[0..min(len,80)]:      // name exactly as sent in ContentName
    x ^= c; a = a + c (mod 256); a ^= i as u8
tid = 256*a + x;  if tid <= 2 { tid += len }
```

- `bsr.bin` uses TIds 0…2 instead. EasyDRF: counter starting at 0, +1 (wrapping 3→0)
  before each leader object, so the first value is 1 (`E:DABMOT.cpp:50, 76-83`). QSSTV:
  starts at 2, +1 mod 3 per object, so the first value is 0 (`Q:…/DABMOT.cpp:56, 98-103`).
  The EasyDRF picture pool never caches TIds 0…2 (`E:common/libs/poolid.cpp:19-26`).
- **RS modes (EasyDRF only):** TId on air = (hash + DMmodehash) mod 65536 (`E:DABMOT.cpp:525-530`), with
  `DMmodehash = robmode + specocc*4 + qam*16 + RSlevel*32`, where robmode is the ASCII code
  'A'=65/'B'=66/'E'=69, specocc 3 (2.3 kHz) or 5 (2.5 kHz), qam 4/16/64, RSlevel 1…4
  (`E:Dialog.cpp:2354-2378`). It is updated by the 100 ms GUI timer during TX
  (`Dialog.cpp:855`), so the first data group of a TX session may carry a stale hash
  (0 at program start) (**derived**). The RX does not compute a hash; any constant TId per
  object works.

### 2.5 TX schedule

EasyDRF (`E:common/datadecoding/MOTSlideShow.cpp:70-85, 256-348`, `E:DABMOT.cpp:566-643`):

1. For the **first** file queued, one or more *leader* objects come first: same name and
   data, but only body segments 0…S−1 plus the last segment are sent (`MOTSlideShow.cpp:256-307`;
   the last segment is always sent, `DABMOT.cpp:305-309, 604-613`). S = start delay =
   24 at startup (`Dialog.cpp:63, 752`), 14 with "long lead-in" off (`Dialog.cpp:62, 3330`),
   12 in the constructor (`MOTSlideShow.h:45`); DLL comment: EasyPal default 9, long 16
   (`E:hamdrm.h:64`). More leader copies are added only while the total stays below
   S segments (tiny files) (`MOTSlideShow.cpp:303-305`).
2. Then each queued object: header data group (type 3), then body segments 0…last in
   order (type 4). Each object is sent once; the queue wraps to index 1, which skips the
   leader (`MOTSlideShow.cpp:81-83`).
3. "Instances" (ECCmode 1…3) queue each file 1…3 times (`E:getfilenam.cpp:170-173`,
   `Dialog.cpp:77`). RS modes queue it once (`getfilenam.cpp:171`).
4. TX stops 2 s (20 × 100 ms ticks) after the object counter reaches the number of queued
   objects (`Dialog.cpp:64, 1480-1481, 2347-2348`).

QSSTV (`Q:drmtx/common/datadecoding/DABMOT.cpp:53-54, 560-632`), one repetition
(`Q:…/MOTSlideShow.cpp:87`): run-in of 24 entries, each cycle being [header, last segment,
0, 1, 2, …]; then the body with [header, last segment] inserted before every 50th segment
(every 30th entry for a BSR fix, `:603-614`, using a stale counter); then a run-out of
10 entries like the run-in. Header segments are coded as negative list entries (`:574`).

### 2.6 RX reassembly

EasyDRF (`E:DABMOT.cpp:682-1230`):

- One current object (`MOTObjectRaw`) keyed by TId. When a group with another TId
  arrives, the current object is stored in a 5-entry LRU pool keyed by TId, and the new
  TId's state is loaded from the pool if present (`:994-999, 1095-1100`;
  `E:common/datadecoding/picpool.cpp:98-147`, `E:common/libs/poolid.h:19`). Segments
  from earlier passes therefore accumulate across retransmissions and BSR repairs.
- Body segments are stored by number (`:1597-1634`). If the segment size changes on a
  non-last segment (mode change), the TId's cache is dropped (`:1047-1052`).
- Total segments = last-flag segment number + 1 (`:1104-1108`). The object is complete
  when the header is ready and every segment 0…total−1 is present (`:1110-1121, 1176-1204`).
- Plain mode needs the MOT header for the filename. RS modes do not (section 3.4).
- "Old header" = this MOT header. `GetName` parses BodySize (→ `HdrFileSize`) and the
  ContentName as soon as header segment 0 arrives, before the body is complete
  (`:1215-1220, 1232-1304`).

QSSTV (`Q:drmrx/sourcedecoder.cpp:294-504`): list of the 5 newest transport blocks by TId
(`Q:drmrx/sourcedecoder.h:8`, `sourcedecoder.cpp:794-848`); missing segments are padded
with zeros of the regular segment size (`:469-480`). It saves when complete; RS files are
tried at ≥ 63 % of segments, once the last segment has been seen (`:491-503`). Objects
already saved are ignored (`alreadyReceived`).

### 2.7 Deviations from EN 301 234 / EN 300 401 (summary)

RepIdx reused for the RS block count; user-access Rfa reused for the RS level; TId =
filename hash (+ mode hash); no MOT directory; header never segmented; ContentName
charset 0 with raw ANSI bytes; ContentType 2 whatever the file type; RX ignores the CRC
flag; TriggerTime and VersionNumber always zero.

---

## 3. File coding: compression and Reed–Solomon

There are **two incompatible RS schemes**: EasyDRF's own (signalled in the data group)
and the EasyPal/QSSTV one (signalled by the `.rsN` filename extension). Their interleavers
are identical; field, generator, byte order and header differ.

### 3.1 Interleaver (shared)

For an RS-coded byte string of B blocks × 255 (L = 255B), stored block by block
(`block b, byte p` at index `255b + p`), the transmitted body is column-wise:

```
tx[p*B + b] = coded[255*b + p]      0 ≤ b < B, 0 ≤ p < 255
```

EasyDRF `distribute()` (`E:common/RS/RS-coder.cpp:675-710`, "from QSSTV", `cols = L/255`)
and QSSTV `distribute()` (`Q:utils/reedsolomoncoder.cpp:181-202`; the 64-byte chunking
does not change the mapping) both implement this (**derived**: pointer += B, wraps by
−(L−1)). RX inverse: `coded[255b+p] = rx[p*B+b]`.

### 3.2 EasyPal / QSSTV RS (`Q:utils/reedsolomoncoder.cpp`, `Q:utils/rs.cpp`, `Q:utils/rs.h`)

- Code: RS(255, k) over GF(256), k = 224 / 192 / 160 / 128 for RS1…RS4 (`rs.h:16-19`).
- Field polynomial `Pp = {1,0,1,1,1,0,0,0,1}` = x^8+x^4+x^3+x^2+1 = **0x11D** (`rs.cpp:26`), α = 0x02.
- Generator g(x) = ∏_{i=0}^{254−k} (x + α^{B0+i}), B0 = 1, PRIM = 1 (`rs.h:38-39`, `rs.cpp:168-191`).
- Byte order (Karn 1996 code): a block in memory is `[d0 … d_{k−1}, p0 … p_{254−k}]`. The
  encoder treats d_i as the coefficient of x^{i+255−k} and p_j as that of x^j
  (`rs.cpp:202-229`, loop from i = k−1 down to 0). The decoder treats memory index j as the
  coefficient of x^j (`rs.cpp:264-276`). This works because a cyclic shift of an RS
  codeword is a codeword (**derived**). A Rust port must reproduce the encoder exactly
  (parity = remainder of Σ d_i x^{i+255−k} mod g, p_j = coefficient of x^j).
- Payload (`reedsolomoncoder.cpp:247-305`), for a file of `got` bytes:
  `B = ceil((got + 7) / k)` (`:266-268`); data stream = 7-byte header + file, zero padded
  to B·k; each k-byte chunk is encoded into a 255-byte block, then interleaved (3.1).
  Header bytes: `[0] = k − (got mod k)` ("surplus"), `[1] = B & 0xFF`, `[2] = B >> 8`,
  `[3..5]` = first 3 chars of the original extension (NUL-padded if shorter), `[6] = 0`
  (`:273-284`).
- On-air name = `baseName + ".rs1"…".rs4"` (Qt `baseName()` = up to the **first** dot)
  (`Q:drmtx/drmtx.cpp:117-126`); MOT ContentType 2/1 (`Q:…/DABMOT.cpp:155-166`).
- RX (`reedsolomoncoder.cpp:52-178`): detect by extension rs1…rs4 (`Q:drmrx/sourcedecoder.cpp:670-678`);
  erasures = every byte of every missing segment, mapped through the interleaver
  (`:118-137`); Karn errors-and-erasures decode per block (`:208-245`). Checks
  `B == byte1 + 256·byte2` (`:151-157`), file size = `B·k − byte0` (`:158`),
  name = completeBaseName + "." + ext (`:163-171`).
- QSSTV bugs (derived): byte0 is read as signed `char` (`:158`), so surplus > 127 gives a
  wrong size (clamped by `left()`). When `got mod k > k − 7`, B is one block larger than
  the surplus formula assumes, and the file comes out k bytes too long. Extensions longer
  than 3 chars are truncated ("jpeg" → "jpe").

### 3.3 EasyDRF compression (all modes)

- Files whose name ends `.html .htm .css .js .xml .json .ico .svg .mid .txt`
  (case-insensitive) are compressed (`E:getfilenam.cpp:139-151`, `E:common/datadecoding/MOTSlideShow.cpp:116-127`);
  others are sent raw.
- Format: LzmaLib `LzmaCompress` (raw LZMA stream, **no end marker**) with level 9,
  dictSize 2^25, lc 3, lp 0, pb 2, fb 255, 1 thread (`MOTSlideShow.cpp:170-189`). Body =
  `props[5] | original_size u24 LE | lzma stream` (`:181-193`).
- Name sent = original name + `".lz"` (`:122-124`). The RX strips `.lz` and decodes exactly
  `original_size` bytes (plain mode `E:Dialog.cpp:2126-2177`, RS mode `E:DABMOT.cpp:1839-1876`).
- Input files are silently truncated to 524 288 bytes (`MOTSlideShow.cpp:101, 112`).
- gzip (`.gz`) is compiled out (`USEGZIP` unset; `E:RS-defs.h:96-101`, `DABMOT.cpp:1797`).
- Applies in plain mode too, so QSSTV/EasyPal receive `name.txt.lz` in this private
  format (**derived**).

### 3.4 EasyDRF RS (`E:common/RS/RS-coder.cpp`, `E:common/datadecoding/MOTSlideShow.cpp:129-247`, `E:DABMOT.cpp:1645-1938`)

States `ECCmode` 4…7 = RS1…RS4 (`E:Dialog.cpp:77`); changelog: "NOT EasyPal compatible"
(`EasyDRF_changes.txt`, September 11th 2021).

- Code: RS(255, k), k = 224 / 192 / 160 / 128 (`RS-coder.cpp:37-41`).
- Field: schifra `primitive_polynomial06 = {1,1,1,0,0,0,0,1,1}` (coefficients of x^0…x^8)
  = x^8+x^7+x^2+x+1 = **0x187** (`E:common/RS/schifra_galois_field.hpp:479-480`, used at `RS-coder.cpp:61`), α = 2.
- Generator: ∏_{i=120}^{120+254−k} (x + α^i) (`RS-coder.cpp:57-65`;
  `schifra_sequential_root_generator_polynomial_creator.hpp:50-57`).
- Byte order: schifra systematic; `block[0..k)` = data, `data[0]` is the highest-degree
  coefficient (`schifra_reed_solomon_encoder.hpp:98-118`), parities follow,
  `fec(i) = parities[fec_len−1−i]` (`:72-74`); standard "first byte = highest power".
- Payload: `EZ header | body`, where body = raw file or the LZMA form of 3.3.
  EZ header (`MOTSlideShow.cpp:135-150`):

  | Offset | Bytes | Value |
  |---|---|---|
  | 0 | 15 | ASCII `EasyDRFHeader/|` |
  | 15 | 1 | version = 2 |
  | 16 | 2 | header size H = 21 + len(name), LE |
  | 18 | 3 | **original** file size (before LZMA, after the 512 KiB cap), LE |
  | 21 | H−21 | name as sent (with `.lz` if compressed), no NUL |

  Total L = H + body; B = ceil(L / k) blocks (`RS-coder.cpp:78-80`, `MOTSlideShow.cpp:220-239`); the last chunk is padded with
  uninitialised heap bytes (buffer contents past L) — write zeros. Encoded size 255·B,
  then interleaved (3.1, `MOTSlideShow.cpp:245-246`). This is the MOT body; the MOT header
  carries BodySize = 255·B and the name as sent (no `.rsN`).
- Signalling: Rfa = RS level (2.1), RepIdx nibbles = B (2.1), TId + mode hash (2.4).
- RX (`DABMOT.cpp:803-1172`, `1645-1938`):
  - Size: `RSfilesize` = MOT BodySize if the header was received (preferred), otherwise
    255 × the 16-bit block count assembled from 4 RepIdx nibbles (`:886-923, 955-965`).
    Totals are reset when the TId changes (`:823-867`).
  - Each CRC-good body segment is copied to byte offset `segnum × segsize` of a
    1 050 000-byte buffer (two buffers, flipped per TId) (`:1066-1091`). A bit per segment
    marks reception (`:1619-1629`).
  - A decode is attempted when the count of good segments has grown since the last attempt and
    `ceil(100·good/total) ≥ 88 / 75 / 63 / 50 %` for RS1…RS4 (`:1141-1166`). total =
    ceil(size / segsize) (`E:common/datadecoding/DABMOT.h:162-174`).
  - De-interleave data and per-byte erasure flags (erasure = byte of a missing segment),
    schifra decode with erasures (`RS-coder.cpp:314-401`); a failed block gives zeros and
    an error count. Output size = (L/255)·k (`DABMOT.cpp:1722-1745`).
  - Success needs 0 failed blocks and bytes 0…14 == `EasyDRFHeader/|` (`:1764-1771`).
    Then H = bytes 16-17, size = bytes 18-20, name = bytes 21…H−1 (`:1776-1785`); `.lz` →
    LZMA (3.3); else write `size` bytes from offset H (`:1878-1898`). The version byte is
    not checked.
  - RS levels 5…7 enter the RS path with a 100 % threshold, match no decoder and fail
    harmlessly (**derived** from `:1149-1153, 1722-1745`). This is a possible hook for our own
    better-FEC modes.

### 3.5 Interop matrix (derived)

| TX → RX | EasyDRF | QSSTV (EasyPal mode) |
|---|---|---|
| EasyDRF plain | ok | ok, except `.lz` files saved raw |
| EasyDRF RS | ok | complete reception saves interleaved RS data under the plain name |
| QSSTV plain | ok | ok |
| QSSTV RS (`.rsN`) | saves the `.rsN` file undecoded | ok |

---

## 4. BSR (bad segment report)

A BSR is an ordinary file named **`bsr.bin`**, sent as a MOT object (TId 0…2, 2.4),
content in ASCII lines terminated by `\n`.

### 4.1 Request format

Uncompressed form (EasyDRF writer `E:DABMOT.cpp:1306-1361`, reader `E:common/bsr.cpp:92-129`):

```
<transport id>            decimal, as received (EasyDRF RS mode: includes the mode hash)
H_OK | H_NO               MOT header received or not
<segment size>            bytes, from the first received segment (default 116)
<n1>
<n2> ...                  missing body segment numbers, ascending
[-99                      optional terminator; anything after it is ignored by EasyDRF
 <file name>              QSSTV "EasyPal" variant
 <mode code, 5 digits>]
```

- EasyDRF lists only missing segments below the highest segment number received
  (`DABMOT.cpp:1349-1356`), and writes no `-99`.
- Range compression ("fast", `E:hamdrm.h:102-104`: "not compatible to older versions"):
  EasyDRF `compressBSR` (`bsr.cpp:136-195`) writes runs of ≥ 3 consecutive numbers as
  `first, -1, last`; QSSTV writes any run of ≥ 2 that way (`Q:drmrx/sourcedecoder.cpp:752-780`).
  The decoder expands `-1, m` to prev+1…m (`bsr.cpp:197-255`; QSSTV `Q:drmtx/drmtx.cpp:260-280`).
  EasyDRF GUI always compresses (`E:Dialog.cpp:2494-2497`); the DLL only when `fast != 0`
  (`E:hamdrm.cpp:283-288`).
- QSSTV (`Q:drmrx/sourcedecoder.cpp:726-791`, buttons `Q:drmtx/bsrform.cpp:45-58, 81-89`,
  `Q:mainwidgets/txwidget.cpp:444`): always `H_OK`; "Compatible" = compressed, `-99` only;
  "EasyPal" = uncompressed, `-99`, then the file name and mode code
  `robMode*10000 + spectrum*1000 + protection*100 + qamCode*10 + interleaver`, qamCode
  0 = 4-QAM, 1 = 16, 2 = 64 (`:836-841`). EasyPal's own trailer: **unknown** (the DLL host
  wrote it).
- Sending: EasyDRF queues `bsr.bin` 2 times, or 4 times ("send 3") (`Dialog.cpp:2504-2512`).
  In RS modes it would RS-code `bsr.bin` too (no exception in `MOTSlideShow.cpp:131`;
  TODO at `:129`), while the RX handles `bsr.bin` only when the RS level is 0
  (`Dialog.cpp:1952, 1967`) — BSR is broken in EasyDRF RS modes (**derived**). QSSTV
  never RS-codes a BSR (`Q:drmtx/drmtx.cpp:291-297`).

### 4.2 Responder

- On receiving `bsr.bin`: EasyDRF decompresses it (`Dialog.cpp:1967-1985`; DLL `hamdrm.cpp:312-327`)
  and looks the TId up in a table of the last 5 files sent, keyed by the plain filename
  hash (`bsr.cpp:56-88`, `numstredele = 5` at `E:common/bsr.h:34`). No match = "not for us".
  An RS-mode request carries hash + mode hash and never matches (**derived**). QSSTV
  rejects requests whose line 2 is not `H_OK` (`Q:drmtx/drmtx.cpp:250-253`), matches the TId
  in its last 6 sessions (`:152, 326-337`) and ignores line 3.
- Repair transmission = the same object (same name, so same TId; `.lz` is stripped and
  re-added, recompressed deterministically, `bsr.cpp:271-279`), header first, then only
  the requested segments plus the last one (`DABMOT.cpp:243-267, 305-309`). The list must be
  ascending. Copies: N instances (1 or 3), plus 1 more if ≤ 50 segments, another if ≤ 10,
  another if ≤ 3 (`bsr.cpp:280-294`). The leader logic of 2.5 also runs (segments
  0…S−1 + last first) (**derived** from `MOTSlideShow.cpp:256-307`). QSSTV: section 2.5
  fix schedule, RS re-encoded identically (`Q:drmtx/drmtx.cpp:299-315`).
- The receiver merges the repair segments into the cached object by TId (2.6).

---

## 5. Callsign, label, text messages, picture pool

- **Label/callsign** travels in the FAC: 21 bits = 3 × 7-bit chars per frame, frame
  id 0/1/2 carries chars 0-2/3-5/6-8, max 9 chars, NUL padded, `& 127` (TX `FAC.cpp:179-200`,
  RX `:680-708`: shown once all three frames have arrived). EasyDRF `setcall` keeps the first 10
  input chars filtered to `A-Z a-z 0-9 - space /` with no case folding
  (`E:common/callsign2.cpp:45-82`); the DLL folded to upper case, rejected `-`, and required a
  letter and a digit (`E:common/libs/callsign.cpp:24-77`). EasyDRF accepts any callsign
  (`callsign2.cpp:85-88`). The RX "confirmed" call = label seen twice in a row
  (`Dialog.cpp:1856-1858`).
- **Text message** (voice mode only): DRM ES 201 980 §6.5 text message, 4 bytes at the end
  of each frame's audio stream (`E:common/TextMessage.cpp:36-258`, `E:common/tables/TableDRMGlobal.h:83`):
  segments of ≤ 16 bytes, at most 8 segments, each preceded by 4×0xFF; 16-bit header
  (toggle, first, last, command 0, len−1 (4), field 2 = 1111 for the first segment or
  rfa 0 + segnum (3), rfa 4); CRC-16 over header + body. TX writes it only when the audio
  block is > 1030 bits (`AudioSourceDecoder.cpp:492`); RX reads it when bits − 32 > 1007
  (`:648`) — small mismatch for 1031…1039 bits. FAC text flag = 1 (`FAC.cpp:153-161`,
  `Dialog.cpp:781-782`).
- **Picture pool** = the 5-entry RX segment cache of 2.6. There is no separate ID object.

---

## 6. Waterfall text and FSK ID

### 6.1 EasyDRF waterfall text (`E:WFText.cpp`, `E:WFText.h`, `E:common/DRMSignalIO.cpp:97-264`)

Buttons G / B / ID (`E:Dialog.cpp:932-964`): text = `good.txt` / `bad.txt` (≤ 20 bytes),
else "GOOD COPY" / "BAD COPY", or the callsign (`WFText.cpp:82-117`). Mode is forced to B,
QAM64 while it plays (`Dialog.cpp:475-490`); the OFDM output is replaced
(`DRMSignalIO.cpp:64-121`).

Glyphs: 8×8 font `chrmask[96]` for ASCII 32…127 (`WFText.h:13-109`, from ChirpHel / DF6NM; resampling FIR = 1 tap of 1.0, `:112-113`);
`c' = clamp(c − 32, 0, 95)` (`WFText.cpp:150`). For column x (0 = left) and row y
(0 = top), pixel = bit `(7−x)·8 + y` of the 64-bit mask (`:171`).

Generation (R = number of chars ≤ 20, sample rate 12 kHz complex) (`:179-247`):

```
N        = 512 + max(R−9, 0)*8                      // IFFT size
OUT      = floor(1.65*N);  IN = 64*R
start    = floor(3*IN/N + 18)                        // first bin
rep      = max(min(floor(100/R), 50), 5) + 1         // repeats per pixel row
for row in 0..8:          // row 0 = glyph row y=7 (bottom) first   (:167-176)
  pix[0..8R)  = glyph row (left→right), value 8 or 0
  s[m]        = pix[floor(m*IN/OUT)] for m < OUT (0 beyond 8R)    // FIRFracDec, 1 tap
  repeat rep times:
    X[k] = s[k−start] · exp(j·π·8·k²/N)   for start ≤ k < N−start, else 0
    emit x = IFFT_N(X) / N                                           // backward, 1/N scale
```

- The 8R pixels map to bins start…start+⌊8R·OUT/IN⌋ ≈ start + 0.206·N, i.e. about
  2475 Hz wide for any R (**derived**).
- Shift: x[n] · e^{−j2π·1640·n/12000} (`:73, 258-268`), then the TX chain: scale ×31000
  (`DRMSignalIO.cpp:104-111`), low-pass/clipper/low-pass at the 12 kHz 0-Hz IF
  (pass 0…1250 Hz, stop 1400 Hz, `E:common/TransmitterFilter.h:76-90`; clip threshold 4096 in
  this mode, `DRMSignalIO.cpp:134`; limiter to 23166 = −3 dBFS, `:149, 169`), up-conversion to
  `Fc = 1225 + DC` Hz (`:196-216, 284-287`, `E:common/tables/TableDRMGlobal.h:44`), real part,
  ×2 ×2 interpolation to 48 kHz (`:221-247`).
- Audio frequency of bin k: `f = 12000·k/N − 1640 + 1225 + DC` (**derived**); DC default
  350 Hz (`E:common/DrmTransmitter.cpp:212`). Example R = 9: N = 512, start = 21, bins 21…126
  → about 427…2888 Hz at DC = 350.
- Duration = `8 · rep · N / 12000` s (**derived**): 4.10 s for 9 chars, 5.80 s for 6.
- First-sent row = bottom glyph row, so the text reads upright on a waterfall whose newest
  line is at the top; frequency increases left to right.
- The chirp (quadratic phase, rate 8) only spreads each row's energy in time; the
  waterfall shows |X[k]|.

### 6.2 QSSTV waterfall text (`Q:sound/waterfalltext.cpp`, `Q:mainwidgets/txfunctions.cpp:154-222`)

Sent before and after every DRM TX: start text / end text (defaults "START PIC"/"END PIC",
"START BIN"/"END BIN", "START RPT"/"END RPT", "BSR", "FIX"; `Q:config/waterfallconfig.cpp:58-65`,
`txfunctions.cpp:154-198`), optional 1 s 1700 Hz VOX tone first (`:207`).

- 48 kHz real output; FFT length = TXSTRIPE·SUBSAMPLINGFACTOR/2 = 1024·4/2 = **2048**
  (`waterfalltext.cpp:45`, `Q:appdefs.h:9-14`), bin = 23.4375 Hz.
- Image width W = floor((2600 − 300)/bin) = **98** bins, first bin round(300/bin) = **13**
  (`waterfalltext.cpp:16-18, 57-60`) (**derived**: 304.7…2601.6 Hz).
- Text rendered by Qt in a bitmap of width W, height = font bounding box (font Arial 12, not bold, by default;
  `waterfallconfig.cpp:68-70`; `waterfalltext.cpp:135-161`). Macros `%m` my call, `%s` SNR,
  `%c` last received call (`:164-173`). Exact pixels depend on the Qt font rasteriser (**unknown**
  for bit-exactness).
- Per image line from the **bottom** up (`:98-107`): X[13+i] = A·e^{−jπi²/W} for lit pixels
  (`(rgb & 0xFFFFFF) != 0`), A = 6000/√W (`:62-71, 114-121`); unnormalised backward FFT;
  output = real part; each line repeated 3 times (`:98, 124-129`). Duration
  = lines·3·2048/48000 s (`:83`). Samples pass the synthesizer's TX filter (`Q:dsp/synthes.cpp:155-157`; response **unknown**).

### 6.3 QSSTV FSK ID (`Q:mainwidgets/txfunctions.cpp:474-546`)

Used **after SSTV images only** (`:268-278`), not with DRM. Phase-continuous tones
(`Q:dsp/synthes.cpp:63-76`):

1. 1500 Hz 300 ms, 2100 Hz 100 ms, 1900 Hz 22 ms.
2. Chars of 6 bits, **LSB first**, 22 ms per bit, bit 1 = 1900 Hz, bit 0 = 2100 Hz (`:474-487`).
3. Sequence: 0x2A, then each callsign char as `c − 0x20` (chars after the first upper-cased;
   the first char is not, a QSSTV quirk, `:518-535`), then 0x01, then
   `XOR of all (c − 0x20) & 0x3F`, then 1900 Hz 100 ms (`:538-545`).
   Time = (len + 3)·0.132 s + 0.422 s (`Q:sstv/sstvtx.cpp:177-184`); it omits the final 100 ms tone.

EasyDRF has no FSK ID; its "ID" button sends waterfall text (6.1).

---

## 7. Digital voice (EasyDRF; QSSTV has none — `Q:drmrx/sourcedecoder.cpp:79-82`)

FAC audio service: audio/data flag 0, coding 2 bits: 0 = LPC-10, 1 = CELP, 2 = Speex,
3 = "SSTV" (`FAC.cpp:137-151` TX, `:576-593` RX), then text flag 1 bit. In QAM4 voice mode
the otherwise spare FAC bit is 1 (`FAC.cpp:204-212, 711-751`); that is PHY.
Voice TX default codec: LPC-10 (`Dialog.cpp:746`); menu: Speex/LPC-10 (`:1257-1266`).
Audio input 48 kHz, 19200 samples per 400 ms frame (`AudioSourceDecoder.cpp:545, 558`); TX
pre-processing (HPF, compressor, noise gate) does not affect the format.

Let T = audio bits per frame = iNumDecodedBitsMSC (TX, `:544`) = stream bytes × 8 (RX, `:926`).
Bits are one per array element, in stream order.

### 7.1 LPC-10 (HVDI variant, `E:common/sourcedecoders/lpc10enc.c`, `lpc10dec.c`)

- Frame = 54 bits (`lpc10.h:38`): pitch/voicing 7, RMS 5, RC1…RC10 with widths
  {5,5,5,5,4,4,4,4,3,3} (`lpc10enc.c:111`), each field **LSB first** (`lpc10enc.c:1307-1313`,
  `lpc10dec.c:621-666`). Quantisers are the HVDI LPC-10 tables (not FS-1015 bit order).
- Layout (`AudioSourceDecoder.cpp:297-346`): `Tb = floor(T/8)·8`;
  `blocks = floor((Tb − 27)/54)`; frames at bits `54i`; then a 27-bit check
  `C[j] = XOR_{w<2·blocks} bit[27w + j]` at bit `54·blocks` (`:49, 336-344`).
  The remaining bits are unwritten. RX recomputes the check and mutes the frame on mismatch (`:679-692`).
- Samples per LPC frame, `Ns = clamp((180 / max(blocks,6))·18, 180, 540)` (integer
  division, `:305`, `:668`, `LPCscale` `:85`, `lpc10.h:34`); TX uses the previous frame's
  block count (first frame: 540) (**derived**). The 48 kHz input is fractionally decimated to
  `blocks·Ns` samples per 400 ms (`:316`), i.e. an effective rate of `blocks·Ns/0.4` Hz
  (≈ 7.9 kHz; less for < 6 blocks); the RX stretches back the same way (`:707-733`).
  The decoder is not told Ns; it is a time-stretch choice.
- Works in all modes (`EasyDRF_changes.txt`, November 14th 2022). Possible overlap: text (last 32
  bits) overwrites LPC frame/check bits when `(Tb−27) mod 54 < 32`, and the RX check then
  fails (RX uses the same `blocks`, `:928`) (**derived**).

### 7.2 Speex (`AudioSourceDecoder.cpp:398-455`, RX `:758-846`)

- libspeex narrowband, complexity 10, `SPEEX_SET_BITRATE 2400` (`:599-602`), enhancer on at
  RX (`:1034-1042`). The 2400 bit/s request selects NB mode 1 (2150 bit/s, 43 bits per
  20 ms) (**derived** from libspeex behaviour; library not in the repo).
- 48 kHz → 8 kHz (4 kHz anti-alias FIR, `FIRFiltDec`, `:409`): 3200 samples = 20 frames of 160.
- Per frame `speex_bits_write(…, 20)` → 6 bytes (43 bits + libspeex terminator padding);
  frame i occupies bits 48i…48i+47, **each byte LSB first** (`:426-437`).
- Check: 15 bits at 960…974 = XOR of 64 consecutive 15-bit words over bits 0…959
  (`:440-449`); bits 975…T−1 = 0 (`:451-452`).
- TX only when T > 1040 bits (`:400`); RX only when T > 1030 (`:758`), and the check is
  verified only when T − (text 32) ≥ 990 (`:765`).

### 7.3 CELP and "SSTV"

CELP: TX sends all zeros (`:456-462`); RX does nothing (`:877-881`). Coding 3 ("SSTV"): a
24-bit XOR check scheme, no payload source (`:464-486, 847-876`) — unused.

### 7.4 Text in voice mode

Section 5; the last 32 bits of the byte-aligned block (`:505-511`).

---

## 8. TUNE, PTT, levels

- **TUNE** (`E:common/OFDM.cpp:49-55`, `E:Dialog.cpp:491-505, 965-970`): mode B forced, QAM64; the
  OFDM IFFT input holds only 3 cells of amplitude 4 at carriers `kmin+7, kmin+23, kmin+31`
  with kmin = 1 for mode B (`E:common/tables/TableCarrier.h:87-90`) → carriers 8, 24, 32 (with
  guard interval, then the normal TX chain). At the 46.875 Hz mode-B spacing and DC 350 Hz:
  725, 1475, 1850 Hz (**derived**; spacing from phy.md). Runs until the operator stops it.
- **PTT**: RTS+DTR asserted at TX start; released 1 s after TX stops (thread `Sleep(1000)`)
  (`E:common/ptt.cpp:130-146`, `E:Dialog.cpp:3784-3808`; changelog November 6th 2022).
  Data TX ends 2 s after the last object (2.5).
- **Levels**: peak limited to 23166/32767 = −3 dBFS by the PAPR limiter (`DRMSignalIO.cpp:149, 169`),
  then ×1.0 (TxLevel = 1) or ×0.3535 (TxLevel = 0) → −3 or −12 dBFS
  (`DRMSignalIO.cpp:254`, `Dialog.cpp:2689-2690`). PAPR clip thresholds 4096/5792/6553 for
  QAM4/16/64 (`:130-134`). Mono, same sample on both channels (`:264`).

---

## 9. Receive-side file naming and saving

- EasyDRF plain: `Rx Files\<ContentName>` with `.lz` stripped and the content decompressed
  (`Dialog.cpp:2123-2203`, path `:161`); RS: `Rx Files\<name from EZ header>` with the same
  `.lz` rule (`DABMOT.cpp:1857-1898`). The name is used unsanitised (path separators,
  `..`) — our RX must sanitise it. Received files are never executed (`Dialog.cpp:2208-2218`).
- Corrupt save (button): `Corrupt\<name>`, missing segments as zeros of the first
  segment's size (`Dialog.cpp:1276-1300`, `DABMOT.cpp:1376-1422`).
- `bsr.bin` → saved as `bsrreq.bin` and handled as a request (4.2), not saved as a file
  (`Dialog.cpp:1967-1985`).
- "Old header" (changelog November 25th 2021, "Improved filename loading from old
  header") = the MOT header (2.2). Its ContentName/BodySize are shown as soon as header
  segment 0 arrives (`DABMOT.cpp:1232-1304`); in RS mode the "new header" (EZ header)
  name wins after decoding (`:1787-1790`). Display falls back to "unknown" on a new TId (`:851-853`).
- QSSTV: `<rxDRMImagesPath>/<ContentName>`; RS: `completeBaseName + "." + ext3`
  (`Q:drmrx/sourcedecoder.cpp:664, 672-678`; `Q:utils/reedsolomoncoder.cpp:163-171`); `.txt`/`.chat`
  are shown as text (`:610-619`); names starting `de_` or `.de` are QSSTV "hybrid" objects
  (the body holds encrypted FTP data; the picture is downloaded) (`:525-563`). EasyPal sends these too (2019 recording); format and handling in
  `hamdrm-data/src/hybrid.rs` and DESIGN.md "Hybrid mode".

---

## 10. Reuse from DecDRM vs new code

| Piece | Reuse | Notes |
|---|---|---|
| CRC-16 | `D:decdrm-data/src/crc.rs` | identical polynomial/init/inversion |
| Packet demux/encoder | `D:decdrm-data/src/packet.rs` (`PacketDemux`, `PacketEncoder`, `build_packet`) | raise `MAX_PACKET_LEN` (`:38`, 255 + 3) to ≥ 313; keep one packet per frame; zero padding is fine on air |
| Data group parse/build | `D:decdrm-data/src/datagroup.rs` | add the 3 user-access Rfa bits (`UserAccess` `:48-55` has none) for the EasyDRF RS level; `repetition` is already exposed |
| MOT header | `D:decdrm-data/src/mot/header.rs` | build params in the order TriggerTime, Version, ContentName; ContentName charset 0 with raw Latin-1 bytes (`charset_text` `:201` would map charset 0 through EBU Latin) |
| MOT reassembly | `D:decdrm-data/src/mot/reassembly.rs`, `mot/decoder.rs` | needs the missing-segment list (BSR, erasures), per-TId cache across passes (no staleness drop), single-segment header |
| MOT carousel encoder | partly `mot/encoder.rs` | new schedulers: EasyDRF leader + single pass, QSSTV run-in/out, BSR subset |
| Text message | `D:decdrm-core/src/mux/text.rs` | same ES 201 980 §6.5 format |
| gzip | `D:decdrm-data/src/compress.rs` | not needed (gzip compiled out) |
| FAC → packlen table, TId hash, mode hash | new | sections 1.1, 2.4 |
| RS (0x11D, B0=1, Karn order) + 7-byte header | new | section 3.2 |
| RS (0x187, fcr 120, schifra order) + EZ header | new | section 3.4 |
| Interleaver | new (trivial) | section 3.1 |
| LZMA raw (no end mark, known size) | new; pure-Rust crate such as `lzma-rs` | section 3.3; check bit-identical output is *not* required (RX only) except for BSR repairs of `.lz` files, where the repair must reproduce the original bytes — cache the sent body instead of recompressing |
| BSR text format, compress/expand | new | section 4 |
| Waterfall text (both), FSK ID, TUNE | new | sections 6, 8 |
| LPC-10 (HVDI) and Speex NB codecs, voice framing | new (port HVDI LPC-10; Speex via C lib or port) | section 7 |
| Filename sanitising | new | section 9 |

---

## 11. Disagreements and unknowns

Disagreements EasyDRF ↔ QSSTV:

1. RS schemes incompatible (3.2 vs 3.4): field 0x11D vs 0x187, roots α^1… vs α^120…, byte
   order, header, signalling (`.rsN` name vs Rfa/RepIdx), EasyDRF also adds the mode hash
   to the TId. Interleaver identical.
2. EasyDRF LZMA `.lz` files (3.3) are unknown to QSSTV.
3. ContentType: EasyDRF always 2/1; QSSTV by extension (both ≤ 2/3, so both pass the
   EasyDRF RX check).
4. TX schedules differ (2.5); neither uses MOT repetition fields.
5. BSR: QSSTV needs `H_OK`, EasyDRF may send `H_NO`; run compression differs (≥ 3 vs ≥ 2;
   both decoders accept both); QSSTV "EasyPal" trailer after `-99`; first `bsr.bin` TId 1 vs 0.
6. RX decode thresholds: EasyDRF 88/75/63/50 % per RS level; QSSTV 63 % plus last segment.
7. CRC flag: EasyDRF always checks; QSSTV honours the flag.
8. QSSTV has no digital voice, no text message, no TUNE tones (it has VOX 1700 Hz); EasyDRF
   has no FSK ID. Waterfall text: different generators (6.1 vs 6.2).

Unknowns:

- EasyPal's exact BSR trailer and whether EasyPal reads the `-1` range form (DLL comment
  says "fast" is not compatible with older versions, `E:hamdrm.h:102-104`).
- Whether EasyPal's RS equals QSSTV's in every detail (QSSTV is the only open
  implementation; the signed-surplus and size edge cases of 3.2 are unverified against EasyPal).
- EasyPal lead-in default in practice (DLL comment 9/16 vs EasyDRF 24/14).
- Speex mode/padding bits rely on libspeex behaviour (library binary only in the repo).
- QSSTV synthesizer TX filter response and Qt font rasterisation (6.2).
- Padding packet contents in EasyDRF are memory-dependent (receivers ignore them).
- `DMmodehash` timing makes the first data group's TId unpredictable in EasyDRF RS mode.
