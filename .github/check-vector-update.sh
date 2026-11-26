#!/usr/bin/env bash
# The accumulator's update in a release binary is vector code at the leg's full register width,
# with no 16-bit lane handled alone. Usage: check-vector-update.sh avx2|sse2|neon DISASSEMBLY
set -eu
kind=$1
dis=$2
label='^[0-9a-f]+ <.*accumulator.*5moved.*>:$'
# Every copy of the update the binary holds, one per width, each from its label to the next blank.
body=$(awk -v label="$label" '$0 ~ label { on = 1; next } /^$/ { on = 0 } on { print }' "$dis")
copies=$(grep -cE "$label" "$dis" || true)
if [ "$copies" -eq 0 ]; then
  echo "::error::no copy of the accumulator update in the disassembly"
  exit 1
fi
count() { grep -cE "$1" <<< "$body" || true; }
s='[[:space:]]'
case $kind in
  avx2) add="vpaddw .*%ymm" sub="vpsubw .*%ymm" narrow="(v?padd|v?psub)w .*%xmm" ;;
  sse2) add="${s}paddw$s.*%xmm" sub="${s}psubw$s.*%xmm" narrow="%ymm|%zmm" ;;
  neon)
    add="${s}add(\.8h$s|$s+v[0-9]+\.8h)"
    sub="${s}sub(\.8h$s|$s+v[0-9]+\.8h)"
    narrow='\.4h|\.h\['
    ;;
  *) echo "::error::unknown kind $kind"; exit 1 ;;
esac
# A lane handled alone is loaded, stored, moved or added as a halfword.
scalar="movzwl|movswl|${s}(movw|addw|subw)$s|pinsrw|pextrw|ldrh|strh|ldrsh|umov|smov"
adds=$(count "$add")
subs=$(count "$sub")
narrows=$(count "$narrow")
scalars=$(count "$scalar")
echo "$copies copies of the update: $adds $kind vector additions and $subs subtractions," \
  "$narrows narrower, $scalars on a halfword"
if [ "$adds" -eq 0 ] || [ "$subs" -eq 0 ]; then
  echo "::error::the accumulator update has no $kind vector additions or subtractions"
  exit 1
fi
if [ "$narrows" -ne 0 ] || [ "$scalars" -ne 0 ]; then
  echo "::error::the accumulator update is not vector code at the full $kind width throughout"
  grep -E "$narrow|$scalar" <<< "$body" | head -20
  exit 1
fi
