#!/bin/bash
# Builds OpenSBI's fw_jump firmware for the generic platform, as an Excalibur
# hart runs it, with clang and ld.lld and without make.
# Usage: build-opensbi.sh <opensbi dir> <out dir>
#
# Clone OpenSBI at the tag named in tests/opensbi/README.md:
#   git clone --depth 1 --branch v1.9 https://github.com/riscv-software-src/opensbi
# Writes <out dir>/fw_jump.elf and <out dir>/fw_jump.bin (the build tree is
# <out dir>/build). Needs clang, ld.lld, llvm-ar and llvm-objcopy (the official
# Windows LLVM installer has them) and Python 3 (OpenSBI's Kconfiglib).
#
# OpenSBI's releases are built for rv64imafdc, and Excalibur has neither C
# nor F/D, so the firmware is built here for the ISA Grail runs. This does what
#   make LLVM=1 PLATFORM=generic PLATFORM_RISCV_XLEN=64 \
#        PLATFORM_RISCV_ISA=rv64ima_zicsr_zifencei PLATFORM_RISCV_ABI=lp64 \
#        REPRODUCIBLE=y \
#        platform-cflags-y="-Wno-error=unused-but-set-global -Wno-error=sometimes-uninitialized"
# does in OpenSBI's Makefile, step by step, so that it needs no GNU make (which
# Windows doesn't have) and so that each step can be read:
#   1. Kconfig: the generic platform's defconfig becomes .config, auto.conf
#      (CONFIG_FOO=y, read in step 2) and autoconf.h (included by every file).
#   2. The objects.mk files: which objects each configuration builds, and the
#      flags the platform and the firmware add.
#   3. The carray files: the C arrays of drivers and SBI extensions, from the
#      lists of step 2 (scripts/carray.sh).
#   4. Every object, with the Makefile's CFLAGS or ASFLAGS, into the archive
#      libplatsbi.a (libsbi, libsbiutils and the platform).
#   5. fw_jump.elf: firmware/fw_jump.S, linked with libplatsbi.a by the linker
#      script that fw_jump.elf.ldS preprocesses into; fw_jump.bin, its bytes.
#
# The layout is fw_jump's on the generic platform (platform/generic/objects.mk,
# firmware/fw_jump.S): the image is position independent (-fPIE, linked -pie,
# FW_TEXT_START 0x0) and relocates itself to where it is loaded, 0x8000_0000 on
# Grail and QEMU virt. It enters the next stage at its own address plus
# FW_JUMP_OFFSET, 0x20_0000: 0x8020_0000. It copies the device tree it was
# given in a1 to its own address plus FW_JUMP_FDT_OFFSET, 0x220_0000
# (0x8220_0000), and gives the next stage that copy in a1: the machine needs
# RAM past 0x8220_0000 and the tree's size (Grail's own tree, at the top of
# RAM, must not be there).
#
# Two flags are added (to platform-cflags-y, which the generic platform leaves
# empty), for two warnings that -Werror makes errors with clang 23 and that
# change no code:
# - -Wno-error=unused-but-set-global, which clang 23 has and v1.9 predates:
#   lib/utils/serial/cadence-uart.c keeps its clock and baud rate in statics it
#   never reads again;
# - -Wno-error=sometimes-uninitialized: without F, clang finds that
#   sbi_trap_emulate_store() (lib/sbi/sbi_trap_ldst.c) can leave `val`
#   uninitialized, a false positive (`fp`, the only way past its assignment, is
#   set only under __riscv_flen); OpenSBI's master has since reworked the
#   function and initializes it.
# They stay warnings.
#
# REPRODUCIBLE=y (-ffile-prefix-map) keeps the checkout's path out of the
# binary, so the same OpenSBI and clang give the same bytes anywhere.
set -eu
[ $# -eq 2 ] || { echo "usage: $0 <opensbi dir> <out dir>" >&2; exit 2; }

# Paths as the tools take them: on Windows (Git Bash) clang and Python are
# native programs, which want C:/... paths, not /c/....
native() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else echo "$1"; fi; }
SRC=$(native "$(realpath "$1")")
OUT=$(native "$(realpath -m "$2")")
PYTHON=${PYTHON:-python3}
CC=clang AR=llvm-ar OBJCOPY=llvm-objcopy

# The Makefile's directories (O=$OUT/build).
B=$OUT/build
PLAT_SRC=$SRC/platform/generic
PLAT_B=$B/platform/generic
KDIR=$PLAT_B/kconfig
rm -rf "$B"; mkdir -p "$KDIR"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

# --- 1. Kconfig, as the Makefile's $(KCONFIG_CONFIG) and $(KCONFIG_AUTOCONFIG)
# rules: the Kconfig files read the source paths from the environment.
export PYTHONDONTWRITEBYTECODE=1
export KCONFIG_CONFIG=$KDIR/.config
export OPENSBI_SRC_DIR=$SRC OPENSBI_PLATFORM=generic OPENSBI_PLATFORM_SRC_DIR=$PLAT_SRC
(cd "$SRC" && "$PYTHON" scripts/Kconfiglib/defconfig.py --kconfig Kconfig "$PLAT_SRC/configs/defconfig" >/dev/null)
(cd "$SRC" && "$PYTHON" scripts/Kconfiglib/genconfig.py --header-path "$KDIR/autoconf.h" \
  --sync-deps "$KDIR" --file-list "$KDIR/auto.list" Kconfig)
AUTOHEADER=$KDIR/autoconf.h

# --- The compiler probes the Makefile makes (CC_SUPPORT_*, OPENSBI_LD_*), on an
# empty C file. They decide flags and, for the vector extension, an object.
TARGET="--target=riscv64-unknown-elf"
: > "$TMP/empty.c"
probe() { "$CC" $TARGET -mno-relax "$@" -x c "$TMP/empty.c" -o "$TMP/a.out" >"$TMP/probe" 2>&1; }
probe -fuse-ld=lld -fPIE -nostdlib -Wl,-pie || { echo "the linker cannot make PIEs, which OpenSBI needs" >&2; exit 1; }
probe -fuse-ld=lld -Wl,--exclude-libs,ALL && LD_EXCLUDE_LIBS=y || LD_EXCLUDE_LIBS=n
probe -nostdlib -mno-save-restore; grep -q -e "-save-restore" "$TMP/probe" && SAVE_RESTORE=n || SAVE_RESTORE=y
probe -nostdlib -mstrict-align; grep -q -e "-mstrict-align" -e "-mno-unaligned-access" "$TMP/probe" && STRICT_ALIGN=n || STRICT_ALIGN=y
"$CC" $TARGET -mno-relax -nostdlib -march=rv64gv -dM -E -x c "$TMP/empty.c" 2>&1 | grep -q 'riscv.*vector' \
  && CC_SUPPORT_VECTOR=y || CC_SUPPORT_VECTOR=n

# --- 2. The objects.mk files, in the Makefile's order (each directory's found
# with `find | sort -r`: the platform's, lib/sbi's, lib/utils', the firmware's),
# read by a small evaluator of the make they are written in: assignments
# (=, :=, +=, ?=), $(VAR) and $(addprefix), and ifeq/ifneq/ifdef/ifndef/else/
# endif. Rules and recipes (firmware/payloads' test.bin, not built here) are
# skipped, and so is libfdt's $(foreach ... $(eval CFLAGS_<file> ...)), which
# sets variables no rule of the Makefile reads. Its output is shell
# assignments: mk_<variable> for each list below, carray_<name> for each
# carray-<name>-y.
mks() { (cd "$SRC" && find "$1" -iname objects.mk | LC_ALL=C sort -r | sed "s|^|$SRC/|"); }
MKS="$(mks platform/generic) $(mks lib/sbi) $(mks lib/utils) $(mks firmware)"
"$PYTHON" -I - "$KDIR/auto.conf" PLATFORM_RISCV_XLEN=64 CC_SUPPORT_VECTOR=$CC_SUPPORT_VECTOR \
  "libsbiutils_dir=$SRC/lib/utils" "platform_build_dir=$PLAT_B" "build_dir=$B" -- $MKS > "$TMP/mk.sh" <<'EOF'
import re, shlex, sys

args = sys.argv[1:]
autoconf, defs, files = args[0], args[1:args.index("--")], args[args.index("--") + 1:]
var = {}
for line in open(autoconf):
    m = re.match(r"(CONFIG_\w+)=(.*)$", line.rstrip("\n"))
    if m:
        var[m.group(1)] = m.group(2)
for d in defs:
    k, v = d.split("=", 1)
    var[k] = v

def expand(s):
    # Innermost references first: $(VAR), ${VAR}, $(addprefix p,list).
    ref = re.compile(r"\$[({]([^$(){}]*)[)}]")
    while True:
        m = ref.search(s)
        if not m:
            return s
        body = m.group(1)
        if body.startswith("addprefix "):
            prefix, words = body[len("addprefix "):].split(",", 1)
            val = " ".join(prefix.strip() + w for w in words.split())
        else:
            val = var.get(body.strip(), "")
        s = s[:m.start()] + val + s[m.end():]

def condition(line):
    word, rest = line.split(None, 1) if " " in line.strip() else (line.strip(), "")
    if word in ("ifdef", "ifndef"):
        defined = var.get(rest.strip(), "") != ""
        return defined if word == "ifdef" else not defined
    a, b = rest.strip()[1:-1].split(",", 1)
    equal = expand(a).strip() == expand(b).strip()
    return equal if word == "ifeq" else not equal

for path in files:
    lines = open(path).read().replace("\\\n", " ").split("\n")
    live = [True]  # one entry per open conditional: is its branch taken?
    for raw in lines:
        if raw.startswith("\t"):
            continue  # a recipe
        line = raw.split("#", 1)[0].strip() if raw.lstrip().startswith("#") else raw.strip()
        if not line:
            continue
        word = line.split()[0]
        if word in ("ifeq", "ifneq", "ifdef", "ifndef"):
            live.append(live[-1] and condition(line))
            continue
        if word == "else":
            live[-1] = not live[-1] and all(live[:-1])
            continue
        if word == "endif":
            live.pop()
            continue
        if not all(live) or line.startswith("$("):
            continue  # a branch not taken, or libfdt's $(foreach ...)
        m = re.match(r"^(\S+?)\s*(\+=|:=|\?=|=)\s*(.*)$", line)
        if not m:
            continue  # a rule (firmware/payloads' test.o and test.dep)
        name, op, value = expand(m.group(1)), m.group(2), expand(m.group(3))
        if op == "+=" and var.get(name):
            var[name] = var[name] + " " + value
        elif op != "?=" or name not in var:
            var[name] = value

lists = ["libsbi-objs-y", "libsbiutils-objs-y", "platform-objs-y", "firmware-bins-y",
         "libsbiutils-genflags-y", "platform-genflags-y", "firmware-genflags-y",
         "platform-cppflags-y", "platform-cflags-y", "platform-asflags-y", "platform-ldflags-y",
         "firmware-cppflags-y", "firmware-cflags-y", "firmware-asflags-y", "firmware-ldflags-y"]
for name in lists:
    print("mk_%s=%s" % (name.replace("-", "_"), shlex.quote(var.get(name, "").strip())))
for name in sorted(var):
    m = re.match(r"^carray-(\w+)-y$", name)
    if m:
        print("carray_%s=%s" % (m.group(1), shlex.quote(var[name].strip())))
EOF
. "$TMP/mk.sh"
mk_platform_cflags_y="$mk_platform_cflags_y -Wno-error=unused-but-set-global -Wno-error=sometimes-uninitialized"

# --- The Makefile's flags. GENFLAGS go to every compiler run; the strings are
# expanded by the shell (eval below) as make hands its recipes to /bin/sh, so
# the firmware's -DFW_PAYLOAD_PATH=\"...\" is a string.
VERSION_GIT=$(cd "$SRC" && git describe 2>/dev/null || true)
GENFLAGS="$TARGET -Wno-unused-command-line-argument -I$PLAT_SRC/include -I$SRC/include"
[ -n "$VERSION_GIT" ] && GENFLAGS="$GENFLAGS -DOPENSBI_VERSION_GIT=\"\\\"$VERSION_GIT\\\"\""
GENFLAGS="$GENFLAGS -include $SRC/include/sbi/sbi_visibility.h -include $AUTOHEADER"
GENFLAGS="$GENFLAGS $mk_libsbiutils_genflags_y $mk_platform_genflags_y $mk_firmware_genflags_y"
ISA="-mabi=lp64 -march=rv64ima_zicsr_zifencei -mcmodel=medany -mno-relax"
OPT=""
[ $SAVE_RESTORE = y ] && OPT="$OPT -mno-save-restore"
[ $STRICT_ALIGN = y ] && OPT="$OPT -mstrict-align"
REPRO="-ffile-prefix-map=$SRC="
CFLAGS="-g -Wall -Werror -ffreestanding -nostdlib -fno-stack-protector -fno-strict-aliasing"
CFLAGS="$CFLAGS -ffunction-sections -fdata-sections -fno-omit-frame-pointer -fno-optimize-sibling-calls"
CFLAGS="$CFLAGS -fno-asynchronous-unwind-tables -fno-unwind-tables -std=gnu11 $REPRO"
[ $CC_SUPPORT_VECTOR = y ] && CFLAGS="$CFLAGS -DOPENSBI_CC_SUPPORT_VECTOR"
CFLAGS="$CFLAGS $OPT $ISA $GENFLAGS $mk_platform_cflags_y -fPIE -pie $mk_firmware_cflags_y -O2"
CPPFLAGS="$GENFLAGS $mk_platform_cppflags_y $mk_firmware_cppflags_y"
ASFLAGS="-g -Wall -nostdlib -fno-omit-frame-pointer -fno-optimize-sibling-calls -fPIE $REPRO"
ASFLAGS="$ASFLAGS $OPT $ISA $GENFLAGS $mk_platform_asflags_y $mk_firmware_asflags_y"
ELFFLAGS="-fuse-ld=lld -Wl,--gc-sections"
[ $LD_EXCLUDE_LIBS = y ] && ELFFLAGS="$ELFFLAGS -Wl,--exclude-libs,ALL"
ELFFLAGS="$ELFFLAGS -Wl,--build-id=none -Wl,--no-dynamic-linker -Wl,-pie $mk_platform_ldflags_y $mk_firmware_ldflags_y"

# --- 3 and 4. One object: <src dir>/<obj>.o from <obj>.c, <obj>.S or, for a
# <name>.carray.o, the C file carray.sh writes from <name>.carray and the
# carray-<name>-y list. Each gets the Makefile's dynamic_flags: its own
# directory on the include path and __OBJNAME__.
compile() { # compile <source dir> <build dir> <object>
  local o=$3 base src flags
  base=${o%.o}
  mkdir -p "$(dirname "$2/$o")"
  if [ "${base%.carray}" != "$base" ]; then
    local name; name=$(basename "${base%.carray}")
    eval "local list=\${carray_$name:-}"
    bash "$SRC/scripts/carray.sh" -i "$1/$base" -l "$list" > "$2/$base.c"
    src=$2/$base.c flags=$CFLAGS
  elif [ -f "$1/$base.c" ]; then
    src=$1/$base.c flags=$CFLAGS
  else
    src=$1/$base.S flags=$ASFLAGS
  fi
  local objname; objname=$(basename "$o" .o); objname=${objname//-/_}
  eval "\"\$CC\" $flags -I\"\$(dirname \"\$src\")\" -D__OBJNAME__=$objname -c \"\$src\" -o \"\$2/\$o\""
}
OBJS=""
for o in $mk_libsbi_objs_y; do compile "$SRC/lib/sbi" "$B/lib/sbi" "$o"; OBJS="$OBJS $B/lib/sbi/$o"; done
for o in $mk_libsbiutils_objs_y; do compile "$SRC/lib/utils" "$PLAT_B/lib/utils" "$o"; OBJS="$OBJS $PLAT_B/lib/utils/$o"; done
for o in $mk_platform_objs_y; do compile "$PLAT_SRC" "$PLAT_B" "$o"; OBJS="$OBJS $PLAT_B/$o"; done
# Through a response file: Windows limits a command line to 32 KiB, which the
# few hundred objects' paths pass.
printf '%s\n' $OBJS > "$TMP/objs.rsp"
"$AR" rcs "$PLAT_B/lib/libplatsbi.a" "@$(native "$TMP/objs.rsp")"

# --- 5. fw_jump: its object, its linker script (the preprocessed .ldS, the
# lines of the preprocessor's own markers dropped), the link, the bytes.
F=$PLAT_B/firmware
compile "$SRC/firmware" "$F" fw_jump.o
eval "\"\$CC\" -E $CPPFLAGS -x c \"\$SRC/firmware/fw_jump.elf.ldS\"" | grep -v "#" > "$F/fw_jump.elf.ld"
eval "\"\$CC\" $CFLAGS \"\$F/fw_jump.o\" \"\$PLAT_B/lib/libplatsbi.a\" $ELFFLAGS -Wl,-T\"\$F/fw_jump.elf.ld\" -o \"\$F/fw_jump.elf\""
"$OBJCOPY" -S -O binary "$F/fw_jump.elf" "$F/fw_jump.bin"
cp "$F/fw_jump.elf" "$F/fw_jump.bin" "$OUT/"
echo "$OUT/fw_jump.elf, $OUT/fw_jump.bin ($(wc -c < "$OUT/fw_jump.bin") bytes, OpenSBI ${VERSION_GIT:-?})"
