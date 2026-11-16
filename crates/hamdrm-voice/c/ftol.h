/* Replacement for HawkVoice's ftol.h (EasyDRF version): the original used 32-bit x87
   inline assembly under MSVC. lrintf rounds to nearest (ties to even) in the default
   floating-point mode, which is what `fistp` does in the default x87 mode. */
#include <math.h>
#define lrintf2(x) ((int)lrintf(x))
