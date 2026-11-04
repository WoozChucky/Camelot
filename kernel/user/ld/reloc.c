/* ld: applying one relocation (merlin-ld's reloc.rs, which explains every
 * formula and field at length).
 *
 * An object is assembled before anybody knows where its sections go, so a
 * word that depends on an address is left with a hole and a relocation: at
 * offset r_offset, put formula F(S, A, P) into the field of type r_type.
 * S is the symbol's final address (for a PCREL_LO12, the label on its
 * auipc), A the addend, P the place, the address of the bytes patched.
 *
 *   R_RISCV_32               S + A                the whole word
 *   R_RISCV_64               S + A                the whole doubleword
 *   R_RISCV_BRANCH           S + A - P            B-type imm, +-4 KiB, even
 *   R_RISCV_JAL              S + A - P            J-type imm, +-1 MiB, even
 *   R_RISCV_CALL(_PLT)       S + A - P            auipc (hi) + jalr (lo)
 *   R_RISCV_PCREL_HI20       S + A - P            U-type imm, the high part
 *   R_RISCV_PCREL_LO12_I/S   its HI20's S + A - P I- or S-type imm, the low part
 *   R_RISCV_HI20             S + A                U-type imm, the high part
 *   R_RISCV_LO12_I/S         S + A                I- or S-type imm, the low part
 *   R_RISCV_ADD8/16/32/64    field + (S + A)      a byte, half, word or doubleword
 *   R_RISCV_SUB8/16/32/64    field - (S + A)      a byte, half, word or doubleword
 *   R_RISCV_SUB6, SET6       field - (S + A), S + A  the low 6 bits of a byte
 *   R_RISCV_SET8/16/32       S + A                a byte, half or word
 *   R_RISCV_32_PCREL         S + A - P            the whole word
 *   R_RISCV_RELAX, NONE      nothing
 *
 * Values are computed exactly (64 bits), then checked against what the
 * field holds. A hi/lo pair builds a full 32-bit value and wraps modulo
 * 2^32 as the hardware does, so only R_RISCV_32, BRANCH, JAL, SET32 and
 * 32_PCREL can be out of range. R_RISCV_ALIGN is refused: ld.lld deletes
 * the padding beyond what the alignment needs, and merlin-ld does not.
 *
 * R_RISCV_64 is `.dword sym` (`.quad`), which assemblers emit on RV32 too,
 * for a 64-bit datum holding an address: S + A in 64 bits (S zero-extended,
 * A sign-extended), never checked. In a section that is not loaded (debug
 * information, ctx->debug) it is the 32-bit value sign-extended, as ld.lld's
 * relocateNonAlloc does on RV32: `.dword _start` at 0x80000000 is
 * 0xffffffff80000000 there, and 0x80000000 in .data. */
#include "ld.h"

const char *reloc_type_name(uint type) {
    for (uint i = 0; RELOC_NAMES[i].name != NULL; i++) {
        if (RELOC_NAMES[i].type == type) {
            return RELOC_NAMES[i].name;
        }
    }
    /* One message uses one name at a time: a static buffer will do. */
    static char unknown[32];
    snprintf(unknown, sizeof unknown, "relocation type %u", type);
    return unknown;
}

/* ---- Instruction fields (spec section 3): each keeps the other bits ---- */

/* U-type (lui, auipc): imm[31:12] to bits 31:12. */
static uint set_u(uint inst, uint hi20) {
    return (inst & 0x00000fffu) | (hi20 << 12);
}

/* I-type (addi, loads, jalr): imm[11:0] to bits 31:20. */
static uint set_i(uint inst, uint imm) {
    return (inst & 0x000fffffu) | ((imm & 0xfff) << 20);
}

/* S-type (stores): imm[11:5] to bits 31:25, imm[4:0] to bits 11:7. */
static uint set_s(uint inst, uint imm) {
    imm &= 0xfff;
    return (inst & 0x01fff07fu) | ((imm >> 5) << 25) | ((imm & 0x1f) << 7);
}

/* B-type (branches): imm[12] to bit 31, imm[10:5] to 30:25, imm[4:1] to
 * 11:8, imm[11] to bit 7 (bit 0 is not stored). */
static uint set_b(uint inst, uint imm) {
    return (inst & 0x01fff07fu) | (((imm >> 12) & 1) << 31) | (((imm >> 5) & 0x3f) << 25) |
           (((imm >> 1) & 0xf) << 8) | (((imm >> 11) & 1) << 7);
}

/* J-type (jal): imm[20] to bit 31, imm[10:1] to 30:21, imm[11] to bit 20,
 * imm[19:12] to 19:12. */
static uint set_j(uint inst, uint imm) {
    return (inst & 0x00000fffu) | (((imm >> 20) & 1) << 31) | (((imm >> 1) & 0x3ff) << 21) |
           (((imm >> 11) & 1) << 20) | (((imm >> 12) & 0xff) << 12);
}

/* The %hi / %lo split: the low 12 bits are sign-extended by the instruction
 * that adds them, so the high part rounds up when bit 11 is set. */
static uint hi20(uint value) {
    return ((value + 0x800) >> 12) & 0xfffff;
}

static uint lo12(uint value) {
    return value & 0xfff;
}

static uint read32(uchar *b) {
    return (uint)b[0] | (uint)b[1] << 8 | (uint)b[2] << 16 | (uint)b[3] << 24;
}

static void write32(uchar *b, uint v) {
    b[0] = (uchar)v;
    b[1] = (uchar)(v >> 8);
    b[2] = (uchar)(v >> 16);
    b[3] = (uchar)(v >> 24);
}

/* Whether `value` is a signed `bits`-bit even number: a branch's (13) or a
 * jump's (21) offset. ld.lld's range, that of a `bits`-bit number; out of
 * range and odd is two errors (RE_RANGE_AND_ALIGN, the range first). */
static int check_offset(i64 value, uint bits, uint *out, struct reloc_error *e) {
    i64 min = i64_neg(i64_shl(i64_u(1), bits - 1));
    i64 max = i64_sub(i64_shl(i64_u(1), bits - 1), i64_u(1));
    if (i64_cmp(value, min) < 0 || i64_cmp(value, max) > 0) {
        e->value = value;
        e->min = min;
        e->max = max;
        return value.lo & 1 ? RE_RANGE_AND_ALIGN : RE_OUT_OF_RANGE;
    }
    if (value.lo & 1) {
        e->value = value;
        e->min = i64_u(2);
        return RE_MISALIGNED;
    }
    *out = value.lo;
    return RE_OK;
}

/* The label differences (ADD/SUB pairs), SETs and R_RISCV_32_PCREL: the
 * size of their field in bytes, 0 for any other type. */
static uint data_field(uint type) {
    if (type == R_RISCV_ADD8 || type == R_RISCV_SUB8 || type == R_RISCV_SUB6 || type == R_RISCV_SET6 ||
        type == R_RISCV_SET8) {
        return 1;
    }
    if (type == R_RISCV_ADD16 || type == R_RISCV_SUB16 || type == R_RISCV_SET16) {
        return 2;
    }
    if (type == R_RISCV_ADD32 || type == R_RISCV_SUB32 || type == R_RISCV_SET32 || type == R_RISCV_32_PCREL) {
        return 4;
    }
    if (type == R_RISCV_ADD64 || type == R_RISCV_SUB64) {
        return 8;
    }
    return 0;
}

/* ld.lld's read-modify-write of a data_field type's field (`n` bytes) with
 * `v`, S + A (S + A - P for 32_PCREL): ADD adds, SUB subtracts, SUB6 and
 * SET6 change the low 6 bits of the byte, the SETs write. Only SET32 and
 * 32_PCREL are checked, as signed 32-bit numbers. */
static int apply_data(uint type, uint n, i64 v, uchar *bytes, struct reloc_error *e) {
    i64 old = i64_u(0);
    for (uint k = 0; k < n; k++) {
        if (k < 4) {
            old.lo |= (uint)bytes[k] << (8 * k);
        } else {
            old.hi |= (uint)bytes[k] << (8 * (k - 4));
        }
    }
    i64 out = v;
    if (type == R_RISCV_ADD8 || type == R_RISCV_ADD16 || type == R_RISCV_ADD32 || type == R_RISCV_ADD64) {
        out = i64_add(old, v);
    } else if (type == R_RISCV_SUB8 || type == R_RISCV_SUB16 || type == R_RISCV_SUB32 || type == R_RISCV_SUB64) {
        out = i64_sub(old, v);
    } else if (type == R_RISCV_SUB6) {
        out.lo = (old.lo & 0xc0) | ((old.lo - v.lo) & 0x3f);
    } else if (type == R_RISCV_SET6) {
        out.lo = (old.lo & 0xc0) | (v.lo & 0x3f);
    } else if (type == R_RISCV_SET32 || type == R_RISCV_32_PCREL) {
        i64 min = i64_of((int)0x80000000u), max = i64_u(0x7fffffffu);
        if (i64_cmp(v, min) < 0 || i64_cmp(v, max) > 0) {
            e->value = v;
            e->min = min;
            e->max = max;
            return RE_OUT_OF_RANGE;
        }
    }
    for (uint k = 0; k < n; k++) {
        bytes[k] = (uchar)(k < 4 ? out.lo >> (8 * k) : out.hi >> (8 * (k - 4)));
    }
    return RE_OK;
}

int reloc_apply(uint type, uint place, i64 sym, int addend, uchar *bytes, uint have, struct reloc_context *ctx,
                struct reloc_error *e) {
    /* S is 64 bits: a debug relocation against discarded code has S = -A
     * modulo 2^64 (relocate.c), so that S + A is 0. The fields take the low
     * 32 bits, `sl`. */
    i64 s = sym, a = i64_of(addend), p = i64_u(place);
    uint sl = sym.lo;
    uint need;
    if (type == R_RISCV_NONE || type == R_RISCV_RELAX) {
        return RE_OK;
    }
    if (type == R_RISCV_ALIGN) {
        return RE_ALIGN_PADDING;
    }
    if (type == R_RISCV_64 || type == R_RISCV_CALL || type == R_RISCV_CALL_PLT) {
        need = 8;
    } else if (type == R_RISCV_32 || type == R_RISCV_BRANCH || type == R_RISCV_JAL || type == R_RISCV_PCREL_HI20 ||
               type == R_RISCV_PCREL_LO12_I || type == R_RISCV_PCREL_LO12_S || type == R_RISCV_HI20 ||
               type == R_RISCV_LO12_I || type == R_RISCV_LO12_S) {
        need = 4;
    } else {
        need = data_field(type);
        if (need == 0) {
            return RE_UNSUPPORTED;
        }
    }
    if (have < need) {
        e->need = need;
        e->have = have;
        return RE_TRUNCATED;
    }
    /* S + A; in a section that is not loaded, the 32-bit value sign-extended,
     * as ld.lld's relocateNonAlloc gives every value there on RV32. */
    i64 value = ctx->debug ? i64_of((int)(sl + (uint)addend)) : i64_add(s, a);
    if (data_field(type)) {
        if (type == R_RISCV_32_PCREL) {
            value = ctx->debug ? i64_of((int)(sl + (uint)addend - place)) : i64_sub(value, p);
        }
        return apply_data(type, need, value, bytes, e);
    }
    if (type == R_RISCV_64) {
        /* A doubleword holding an address, never out of range. */
        write32(bytes, value.lo);
        write32(bytes + 4, value.hi);
        return RE_OK;
    }
    uint inst = read32(bytes);
    uint patched, imm;
    int r;
    if (type == R_RISCV_32) {
        /* A data word holding an address: it must fit 32 bits, read as
         * unsigned (an address) or signed (a negative offset). */
        i64 value = i64_add(s, a);
        i64 min = i64_of((int)0x80000000u), max = i64_u(0xffffffffu);
        if (i64_cmp(value, min) < 0 || i64_cmp(value, max) > 0) {
            e->value = value;
            e->min = min;
            e->max = max;
            return RE_OUT_OF_RANGE;
        }
        patched = value.lo;
    } else if (type == R_RISCV_BRANCH || type == R_RISCV_JAL) {
        int branch = type == R_RISCV_BRANCH;
        r = check_offset(i64_sub(i64_add(s, a), p), branch ? 13 : 21, &imm, e);
        if (r != RE_OK) {
            return r;
        }
        patched = branch ? set_b(inst, imm) : set_j(inst, imm);
    } else if (type == R_RISCV_CALL || type == R_RISCV_CALL_PLT) {
        /* auipc ra, %hi(off) + jalr ra, %lo(off)(ra), off = S + A - P with P
         * the auipc's address: the jalr adds to ra (P + hi), so both halves
         * use the same off. */
        uint off = sl + (uint)addend - place;
        write32(bytes + 4, set_i(read32(bytes + 4), lo12(off)));
        patched = set_u(inst, hi20(off));
    } else if (type == R_RISCV_PCREL_HI20) {
        patched = set_u(inst, hi20(sl + (uint)addend - place));
    } else if (type == R_RISCV_PCREL_LO12_I || type == R_RISCV_PCREL_LO12_S) {
        /* The low part of the same value as the HI20 at the auipc this
         * relocation's symbol labels: that HI20's S + A - P, P = S here. */
        if (addend != 0) {
            return RE_LO12_ADDEND;
        }
        int k;
        if (!umap_get(&ctx->by_place, sl, &k)) {
            return RE_MISSING_HI20;
        }
        uint value = ctx->sites[k].sym + (uint)ctx->sites[k].addend - sl;
        patched = type == R_RISCV_PCREL_LO12_I ? set_i(inst, lo12(value)) : set_s(inst, lo12(value));
    } else if (type == R_RISCV_HI20) {
        patched = set_u(inst, hi20(sl + (uint)addend));
    } else if (type == R_RISCV_LO12_I) {
        patched = set_i(inst, lo12(sl + (uint)addend));
    } else {
        patched = set_s(inst, lo12(sl + (uint)addend));
    }
    write32(bytes, patched);
    return RE_OK;
}
