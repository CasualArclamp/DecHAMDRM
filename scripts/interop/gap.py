import sys, numpy as np, wave
src, dst, t0, dur = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
w = wave.open(src); x = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).copy(); w.close()
rng = np.random.default_rng(1)
a, b = int(t0*48000), int((t0+dur)*48000)
x[a:b] = (rng.standard_normal(b-a)*3000).astype(np.int16)   # a noise burst instead of the signal
o = wave.open(dst, 'wb'); o.setnchannels(1); o.setsampwidth(2); o.setframerate(48000); o.writeframes(x.tobytes()); o.close()
