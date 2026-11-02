#!/bin/bash
# Regenerates crates/merlin-as/tests/golden/encode.golden from encode.s using llvm-mc.
# Output format, one line per instruction:
#   <source> | <word> <word> ... | <offset>:<R_RISCV_TYPE>:<symbol>[+addend] ...
# Words are the bytes llvm-mc emits before relocation (immediates of relocated
# fields are zero).
set -eu
DIR=$(cd "$(dirname "$0")/../crates/merlin-as/tests/golden" && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
: > "$DIR/encode.golden"
while IFS= read -r line; do
  src=$(echo "$line" | sed 's/#.*//; s/^[[:space:]]*//; s/[[:space:]]*$//')
  [ -z "$src" ] && continue
  printf '%s\n' "$src" > "$TMP/a.s"
  llvm-mc --triple=riscv32 -mattr=+m,+zicsr,+zifencei,-relax -riscv-asm-relax-branches=0 -filetype=obj "$TMP/a.s" -o "$TMP/a.o"
  words=$(llvm-objcopy -O binary --only-section=.text "$TMP/a.o" "$TMP/a.bin" && od -An -tx4 -v "$TMP/a.bin" | xargs)
  relocs=$(llvm-readelf -r "$TMP/a.o" | awk '/R_RISCV/ { off=$1; sub(/^0+/,"",off); if (off=="") off="0"; sym=$5; add=""; for(i=6;i<=NF;i++) add=add $i; gsub(/ /,"",add); if (add=="+0") add=""; if (sym ~ /^\.Lpcrel/) sym="<auipc>"; printf "%s:%s:%s%s ", off, $3, sym, add }' | sed 's/ $//')
  printf '%s | %s | %s\n' "$src" "$words" "$relocs" >> "$DIR/encode.golden"
done < "$DIR/encode.s"
echo "wrote $(wc -l < "$DIR/encode.golden") lines"
