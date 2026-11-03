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
 *   R_RISCV_BRANCH           S + A - P            B-type imm, +-4 KiB, even
 *   R_RISCV_JAL              S + A - P            J-type imm, +-1 MiB, even
 *   R_RISCV_CALL(_PLT)       S + A - P            auipc (hi) + jalr (lo)
 *   R_RISCV_PCREL_HI20       S + A - P            U-type imm, the high part
 *   R_RISCV_PCREL_LO12_I/S   its HI20's S + A - P I- or S-type imm, the low part
 *   R_RISCV_HI20             S + A                U-type imm, the high part
 *   R_RISCV_LO12_I/S         S + A                I- or S-type imm, the low part
 *   R_RISCV_RELAX, NONE      nothing
 *
 * Values are computed exactly (64 bits), then checked against what the
 * field holds. A hi/lo pair builds a full 32-bit value and wraps modulo
 * 2^32 as the hardware does, so only R_RISCV_32, BRANCH and JAL can be out
 * of range. R_RISCV_ALIGN needs relaxation, which merlin-ld never does. */
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
 * jump's (21) offset. */
static int check_offset(i64 value, uint bits, uint *out, struct reloc_error *e) {
    i64 min = i64_neg(i64_shl(i64_u(1), bits - 1));
    i64 max = i64_sub(i64_shl(i64_u(1), bits - 1), i64_u(1));
    if (i64_cmp(value, min) < 0 || i64_cmp(value, max) > 0) {
        e->value = value;
        e->min = min;
        /* The largest even offset: an odd one could not be encoded. */
        e->max = i64_sub(max, i64_u(1));
        return RE_OUT_OF_RANGE;
    }
    if (value.lo & 1) {
        e->value = value;
        e->min = i64_u(2);
        return RE_MISALIGNED;
    }
    *out = value.lo;
    return RE_OK;
}

int reloc_apply(uint type, uint place, uint sym, int addend, uchar *bytes, uint have, struct reloc_context *ctx,
                struct reloc_error *e) {
    i64 s = i64_u(sym), a = i64_of(addend), p = i64_u(place);
    uint need;
    if (type == R_RISCV_NONE || type == R_RISCV_RELAX) {
        return RE_OK;
    }
    if (type == R_RISCV_ALIGN) {
        return RE_NEEDS_RELAXATION;
    }
    if (type == R_RISCV_CALL || type == R_RISCV_CALL_PLT) {
        need = 8;
    } else if (type == R_RISCV_32 || type == R_RISCV_BRANCH || type == R_RISCV_JAL || type == R_RISCV_PCREL_HI20 ||
               type == R_RISCV_PCREL_LO12_I || type == R_RISCV_PCREL_LO12_S || type == R_RISCV_HI20 ||
               type == R_RISCV_LO12_I || type == R_RISCV_LO12_S) {
        need = 4;
    } else {
        return RE_UNSUPPORTED;
    }
    if (have < need) {
        e->need = need;
        e->have = have;
        return RE_TRUNCATED;
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
        uint off = sym + (uint)addend - place;
        write32(bytes + 4, set_i(read32(bytes + 4), lo12(off)));
        patched = set_u(inst, hi20(off));
    } else if (type == R_RISCV_PCREL_HI20) {
        patched = set_u(inst, hi20(sym + (uint)addend - place));
    } else if (type == R_RISCV_PCREL_LO12_I || type == R_RISCV_PCREL_LO12_S) {
        /* The low part of the same value as the HI20 at the auipc this
         * relocation's symbol labels: that HI20's S + A - P, P = S here. */
        if (addend != 0) {
            return RE_LO12_ADDEND;
        }
        int k;
        if (!umap_get(&ctx->by_place, sym, &k)) {
            return RE_MISSING_HI20;
        }
        uint value = ctx->sites[k].sym + (uint)ctx->sites[k].addend - sym;
        patched = type == R_RISCV_PCREL_LO12_I ? set_i(inst, lo12(value)) : set_s(inst, lo12(value));
    } else if (type == R_RISCV_HI20) {
        patched = set_u(inst, hi20(sym + (uint)addend));
    } else if (type == R_RISCV_LO12_I) {
        patched = set_i(inst, lo12(sym + (uint)addend));
    } else {
        patched = set_s(inst, lo12(sym + (uint)addend));
    }
    write32(bytes, patched);
    return RE_OK;
}
