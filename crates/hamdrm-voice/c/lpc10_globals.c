/* The LPC-10 frame length is a global in EasyDRF (AudioSourceDecoder.cpp): it adapts
   to the available bit rate (180..540 samples). DecHAMDRM sets it before each call,
   under a lock on the Rust side. */
int LPC10_SAMPLES_PER_FRAME = 180;
