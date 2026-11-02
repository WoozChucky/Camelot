#!/bin/bash
# Regenerates crates/merlin-as/tests/golden/encode.golden from encode.s with
# clang's integrated assembler (LLVM 21; the official Windows LLVM installer
# ships clang but not llvm-mc).
# Output format, one line per instruction:
#   <source> | <word> <word> ... | <offset>:<R_RISCV_TYPE>:<symbol>[+addend] ...
# Words are the bytes the assembler emits before relocation (immediates of
# relocated fields are zero).
#
# One exception: a conditional branch to a symbol. clang relaxes it into the
# inverted branch over a `jal` even with -mno-relax, and LLVM 21 no longer has
# the -riscv-asm-relax-branches=0 option that turned this off. merlin-as does
# no branch relaxation (spec, "Differences from clang"), so for those lines the
# script keeps the line already in encode.golden, which llvm-mc 18 made with
# that option, and fails if there is none.
set -eu
DIR=$(cd "$(dirname "$0")/../crates/merlin-as/tests/golden" && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cp "$DIR/encode.golden" "$TMP/old.golden"
: > "$DIR/encode.golden"
while IFS= read -r line; do
  src=$(echo "$line" | sed 's/#.*//; s/^[[:space:]]*//; s/[[:space:]]*$//')
  [ -z "$src" ] && continue
  printf '%s\n' "$src" > "$TMP/a.s"
  clang --target=riscv32 -march=rv32ima_zicsr_zifencei -mno-relax -c -x assembler "$TMP/a.s" -o "$TMP/a.o"
  words=$(llvm-objcopy -O binary --only-section=.text "$TMP/a.o" "$TMP/a.bin" && od -An -tx4 -v "$TMP/a.bin" | xargs)
  relocs=$(llvm-readobj --elf-output-style=GNU -r "$TMP/a.o" | awk '/R_RISCV/ { off=$1; sub(/^0+/,"",off); if (off=="") off="0"; sym=$5; add=""; for(i=6;i<=NF;i++) add=add $i; gsub(/ /,"",add); if (add=="+0") add=""; if (sym ~ /^\.Lpcrel/) sym="<auipc>"; printf "%s:%s:%s%s ", off, $3, sym, add }' | sed 's/ $//')
  case "$src" in
    b*) if [[ $relocs == *R_RISCV_JAL* ]]; then
          kept=$(awk -v s="$src | " 'index($0, s) == 1' "$TMP/old.golden")
          [ -n "$kept" ] || { echo "$src: clang relaxed this branch and encode.golden has no line for it" >&2; exit 1; }
          printf '%s\n' "$kept" >> "$DIR/encode.golden"
          continue
        fi ;;
  esac
  printf '%s | %s | %s\n' "$src" "$words" "$relocs" >> "$DIR/encode.golden"
done < "$DIR/encode.s"
echo "wrote $(wc -l < "$DIR/encode.golden") lines"
