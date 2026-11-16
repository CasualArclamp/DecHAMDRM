#!/bin/bash
cd /f/codebases/DecHAMDRM
src=/f/codebases/DecHAMDRM/out/noise.png
for mode in 0 1 2; do for bw in 0 1; do for q in 4:0 16:0 16:1 64:0 64:1; do
  qam=${q%:*}; prot=${q#*:}; n=edm_${mode}_${bw}_${qam}_${prot}
  SPEED=6 bash scripts/interop/edtx.sh $n $mode $bw $qam 0 1 $prot $src >/dev/null 2>&1
  rm -rf out/ed_rx; r=$(./target/release/dechamdrm rx out/$n.wav --out-dir out/ed_rx 2>&1 | tail -1)
  if cmp -s out/ed_rx/noise.png $src; then echo "$n OK   $r"; else echo "$n FAIL $r"; fi
done; done; done
