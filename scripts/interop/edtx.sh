#!/bin/bash
# usage: edtx.sh NAME MODE(0=A,1=B,2=E) BW(0=2.3,1=2.5) QAM INTERLEAVE(0 short,1 long) ECC(1..3 instances, 4..7 RS1..4) PROT FILES(; separated, unix paths) [extra env...]
# EasyDRF (our patched test build) transmits into out/NAME.wav. No sound card, no PTT.
name=$1; mode=$2; bw=$3; qam=$4; il=$5; ecc=$6; prot=$7; files=$8; shift 8
out=/f/codebases/DecHAMDRM/out
cd /f/codebases/easydrf-build/runtx || exit 1
cat > settings.txt <<S
0 TX_Port
EDTEST Call
0 TX_Sound_OUT_Device
0 TX_Sound_IN_Device
0 RX_Sound_OUT_Device
0 RX_Sound_IN_Device
${TEXT:-0} Send_Text_Message
8 Display_Type
0 RX_Audfilt
0 FAC_mute
$qam QAM
$mode MODE
$bw BW
$il Interleave
0 FastReset
$ecc Instances/RS
313 551 764 877 WindowPosition
1 TxLevel
1 Allow_Text_Message
1 DV_Compressor
S
winfiles=""
IFS=';' read -ra arr <<< "$files"
for f in "${arr[@]}"; do [ -n "$f" ] && winfiles="$winfiles;$(cygpath -w "$f")"; done
winfiles=${winfiles#;}
rm -f "$out/$name.wav"
if [ -n "$winfiles" ]; then export EASYDRF_TXFILES="$winfiles"; fi
[ -n "$MIC" ] && export EASYDRF_MIC="$(cygpath -w "$MIC")"
[ -n "$BSR" ] && export EASYDRF_TXBSR="$(cygpath -w "$BSR")"
[ -n "$CODEC" ] && export EASYDRF_CODEC="$CODEC"
EASYDRF_TXWAV="$(cygpath -w "$out/$name.wav")" EASYDRF_PROT=$prot EASYDRF_SPEED=${SPEED:-2} timeout 300 ../Release/EasyDRF.exe -t
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='EasyDRF.exe'\" | Where-Object { \$_.ExecutablePath -like 'F:\codebases\easydrf-build\*' } | ForEach-Object { Stop-Process -Id \$_.ProcessId -Force; 'killed leftover' }"
ls -la "$out/$name.wav" | awk '{print $5, $9}'
