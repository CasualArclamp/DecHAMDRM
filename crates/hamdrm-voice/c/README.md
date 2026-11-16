HawkVoice Direct Interface LPC-10 (Phil Frisbie, Jr., LGPL-2.0-or-later), as modified
for EasyDRF by Daz Man (variable frame length `LPC10_SAMPLES_PER_FRAME`, unpacked bits).
Copied from github.com/DazDSP/EasyDRF `common/sourcedecoders/`. Local changes:
`ftol.h` replaced (portable `lrintf`), `<malloc.h>` → `<stdlib.h>`, and the frame-length
global defined in `lpc10_globals.c`.
