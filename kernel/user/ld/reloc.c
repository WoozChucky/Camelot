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
 * S and P are u64 addresses and A an i64 addend, and every value is
 * computed as the hardware adds: S + A (and S + A - P) modulo 2^64, then
 * read as signed for the range checks. On RV32 S and P are below 2^32 and
 * A is a 32-bit addend, so that is the exact value; on RV64 it wraps at
 * 2^64 as auipc and jal do, which is what ld.lld computes on both widths.
 *
 *   - A hi/lo pair builds a 32-bit value. On RV32 it wraps modulo 2^32 and
 *     reaches every address: never out of range. On RV64 lui and auipc
 *     sign-extend hi << 12, so a pair reaches -2^31 - 0x800 to
 *     2^31 - 0x801: HI20, PCREL_HI20 and CALL(_PLT) are checked against
 *     that window (check_hi_window), the LO12 halves never.
 *   - BRANCH and JAL: +-4 KiB and +-1 MiB on both widths.
 *   - SET32 and 32_PCREL must fit 32 bits read as signed; the other ADD,
 *     SUB and SET types keep the low bits of their value.
 *   - R_RISCV_32 must fit 32 bits read as unsigned (an address) or signed
 *     (a negative offset; on RV64 also a higher-half address).
 *   - R_RISCV_64 (`.dword sym`, which assemblers emit on RV32 too) holds any
 *     value. On RV32, in a section that is not loaded (debug information,
 *     ctx->debug), it and the other 64-bit data fields get the 32-bit value
 *     sign-extended, as ld.lld's relocateNonAlloc does: `.dword _start` at
 *     0x80000000 is 0xffffffff80000000 there, and 0x80000000 in .data.
 *
 * R_RISCV_ALIGN is refused: ld.lld deletes the padding beyond what the
 * alignment needs, and merlin-ld does not. */
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
 * that adds them, so the high part rounds up when bit 11 is set. Only the
 * low 32 bits of `value` reach the fields, on both widths. */
static uint hi20(uint64_t value) {
    return (uint)((value + 0x800) >> 12) & 0xfffff;
}

static uint lo12(uint64_t value) {
    return (uint)value & 0xfff;
}

/* The range check of a hi/lo pair's value `v` (S + A for HI20, S + A - P for
 * PCREL_HI20 and CALL(_PLT)): none on RV32, where the pair wraps modulo 2^32
 * and reaches everything; on RV64, the high part (v + 0x800) >> 12 must be
 * a signed 20-bit number, or lui/auipc would sign-extend it into another
 * value (ld.lld's checkInt(hi, 20)), and the error gives that high part. */
static int check_hi_window(int64_t v, struct reloc_context *ctx, struct reloc_error *e) {
    if (!ctx->elf64) {
        return RE_OK;
    }
    /* Arithmetic shift: the sign of v + 0x800 stays. The addition wraps as
     * ld.lld's does on its uint64_t. */
    int64_t hi = (int64_t)((uint64_t)v + 0x800) >> 12;
    int64_t min = -((int64_t)1 << 19), max = ((int64_t)1 << 19) - 1;
    if (hi < min || hi > max) {
        e->value = hi;
        e->min = min;
        e->max = max;
        return RE_OUT_OF_RANGE;
    }
    return RE_OK;
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
static int check_offset(int64_t value, uint bits, uint *out, struct reloc_error *e) {
    int64_t min = -((int64_t)1 << (bits - 1));
    int64_t max = ((int64_t)1 << (bits - 1)) - 1;
    if (value < min || value > max) {
        e->value = value;
        e->min = min;
        e->max = max;
        return value & 1 ? RE_RANGE_AND_ALIGN : RE_OUT_OF_RANGE;
    }
    if (value & 1) {
        e->value = value;
        e->min = 2;
        return RE_MISALIGNED;
    }
    *out = (uint)value;
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
static int apply_data(uint type, uint n, uint64_t v, uchar *bytes, struct reloc_error *e) {
    uint64_t old = 0;
    for (uint k = 0; k < n; k++) {
        old |= (uint64_t)bytes[k] << (8 * k);
    }
    uint64_t out = v;
    if (type == R_RISCV_ADD8 || type == R_RISCV_ADD16 || type == R_RISCV_ADD32 || type == R_RISCV_ADD64) {
        out = old + v;
    } else if (type == R_RISCV_SUB8 || type == R_RISCV_SUB16 || type == R_RISCV_SUB32 || type == R_RISCV_SUB64) {
        out = old - v;
    } else if (type == R_RISCV_SUB6) {
        out = (old & 0xc0) | ((old - v) & 0x3f);
    } else if (type == R_RISCV_SET6) {
        out = (old & 0xc0) | (v & 0x3f);
    } else if (type == R_RISCV_SET32 || type == R_RISCV_32_PCREL) {
        int64_t sv = (int64_t)v;
        if (sv < INT32_MIN || sv > INT32_MAX) {
            e->value = sv;
            e->min = INT32_MIN;
            e->max = INT32_MAX;
            return RE_OUT_OF_RANGE;
        }
    }
    for (uint k = 0; k < n; k++) {
        bytes[k] = (uchar)(out >> (8 * k));
    }
    return RE_OK;
}

int reloc_apply(uint type, uint64_t place, uint64_t sym, int64_t addend, uchar *bytes, uint64_t have,
                struct reloc_context *ctx, struct reloc_error *e) {
    /* The two values of every formula, S + A and S + A - P, modulo 2^64 as
     * the hardware adds, read as signed for the range checks. On RV32 they
     * are exact (S and P below 2^32, A a 32-bit addend). */
    uint64_t abs = sym + (uint64_t)addend;
    uint64_t pcrel = abs - place;
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
    /* On RV32, in a section that is not loaded, ld.lld's relocateNonAlloc
     * sign-extends the 32-bit value of every relocation it applies there:
     * it shows in the 64-bit fields, R_RISCV_64 and ADD64/SUB64. */
    int nonalloc = !ctx->elf64 && ctx->debug;
    if (data_field(type)) {
        uint64_t v = type == R_RISCV_32_PCREL ? pcrel : abs;
        if (nonalloc) {
            v = (uint64_t)(int64_t)(int)(uint)v;
        }
        return apply_data(type, need, v, bytes, e);
    }
    if (type == R_RISCV_64) {
        /* A doubleword holding an address, never out of range. */
        uint64_t v = nonalloc ? (uint64_t)(int64_t)(int)(uint)abs : abs;
        write32(bytes, (uint)v);
        write32(bytes + 4, (uint)(v >> 32));
        return RE_OK;
    }
    uint inst = read32(bytes);
    uint patched, imm;
    int r;
    if (type == R_RISCV_32) {
        /* A data word holding an address: it must fit 32 bits, read as
         * unsigned (an address) or signed (a negative offset, and on RV64 a
         * higher-half address 0xffff_ffff_8xxx_xxxx). */
        int64_t v = (int64_t)abs;
        if (v < INT32_MIN || v > (int64_t)UINT32_MAX) {
            e->value = v;
            e->min = INT32_MIN;
            e->max = UINT32_MAX;
            return RE_OUT_OF_RANGE;
        }
        patched = (uint)abs;
    } else if (type == R_RISCV_BRANCH || type == R_RISCV_JAL) {
        int branch = type == R_RISCV_BRANCH;
        r = check_offset((int64_t)pcrel, branch ? 13 : 21, &imm, e);
        if (r != RE_OK) {
            return r;
        }
        patched = branch ? set_b(inst, imm) : set_j(inst, imm);
    } else if (type == R_RISCV_CALL || type == R_RISCV_CALL_PLT) {
        /* auipc ra, %hi(off) + jalr ra, %lo(off)(ra), off = S + A - P with P
         * the auipc's address: the jalr adds to ra (P + hi), so both halves
         * use the same off. The range is checked before the jalr is
         * written, so that on error neither word changes. */
        r = check_hi_window((int64_t)pcrel, ctx, e);
        if (r != RE_OK) {
            return r;
        }
        write32(bytes + 4, set_i(read32(bytes + 4), lo12(pcrel)));
        patched = set_u(inst, hi20(pcrel));
    } else if (type == R_RISCV_PCREL_HI20) {
        r = check_hi_window((int64_t)pcrel, ctx, e);
        if (r != RE_OK) {
            return r;
        }
        patched = set_u(inst, hi20(pcrel));
    } else if (type == R_RISCV_PCREL_LO12_I || type == R_RISCV_PCREL_LO12_S) {
        /* The low part of the same value as the HI20 at the auipc this
         * relocation's symbol labels: that HI20's S + A - P, P = S here.
         * Not range-checked: the HI20 is. */
        if (addend != 0) {
            return RE_LO12_ADDEND;
        }
        int k;
        if (!umap_get(&ctx->by_place, sym, &k)) {
            return RE_MISSING_HI20;
        }
        uint64_t value = ctx->sites[k].sym + (uint64_t)ctx->sites[k].addend - sym;
        patched = type == R_RISCV_PCREL_LO12_I ? set_i(inst, lo12(value)) : set_s(inst, lo12(value));
    } else if (type == R_RISCV_HI20) {
        /* The HI20 carries the range check of the absolute pair. */
        r = check_hi_window((int64_t)abs, ctx, e);
        if (r != RE_OK) {
            return r;
        }
        patched = set_u(inst, hi20(abs));
    } else if (type == R_RISCV_LO12_I) {
        patched = set_i(inst, lo12(abs));
    } else {
        patched = set_s(inst, lo12(abs));
    }
    write32(bytes, patched);
    return RE_OK;
}
