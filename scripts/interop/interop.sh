#!/bin/bash
# usage: interop.sh NAME tx-args...   (decodes with our EasyDRF build)
name=$1; shift
cd /f/codebases/DecHAMDRM
./target/release/dechamdrm tx ${SRC:-out/noise.png} --out out/$name.wav --call TEST "$@" 2>&1 | tail -1
cd /f/codebases/easydrf-build/run && rm -f "Rx Files"/* Corrupt/*
EASYDRF_WAV="$(cygpath -w /f/codebases/DecHAMDRM/out/$name.wav)" EASYDRF_SPEED=2 timeout 150 ../Release/EasyDRF.exe
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='EasyDRF.exe'\" | Where-Object { \$_.ExecutablePath -like 'F:\codebases\easydrf-build\*' } | ForEach-Object { Stop-Process -Id \$_.ProcessId -Force; 'killed leftover' }"
if cmp -s "Rx Files/$(basename ${SRC:-out/noise.png})" /f/codebases/DecHAMDRM/${SRC:-out/noise.png}; then echo "$name OK"; else echo "$name FAIL: $(ls 'Rx Files' Corrupt)"; fi
