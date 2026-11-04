/* as: one instruction (mnemonic + operands) to machine words (merlin-as's
 * encode.rs, which explains the formats, the immediates' bit scrambling and
 * the pseudo-instructions; the encodings come from tables.h, generated from
 * its tables).
 *
 * encode() returns one emit per 32-bit word. Their number never depends on a
 * label's address. An operand that names a label becomes a fixup: the
 * immediate bits stay zero and the emit carries the relocation. Anything
 * wrong is an error message, in the order merlin-as checks the operands, so
 * that a line with several mistakes gets the same message. */
#include "as.h"
#include "tables.h"

#define ZERO 0
#define RA 1
#define T1 6

/* ---- Field packers ---- */

static uint r_type(uint opcode, uint funct3, uint funct7, uint rd, uint rs1, uint rs2) {
    return funct7 << 25 | rs2 << 20 | rs1 << 15 | funct3 << 12 | rd << 7 | opcode;
}

uint i_imm(int imm) {
    return ((uint)imm & 0xfff) << 20;
}

static uint i_type(uint opcode, uint funct3, uint rd, uint rs1, int imm) {
    return i_imm(imm) | rs1 << 15 | funct3 << 12 | rd << 7 | opcode;
}

uint s_imm(int imm) {
    uint i = (uint)imm;
    return ((i >> 5) & 0x7f) << 25 | (i & 0x1f) << 7;
}

static uint s_type(uint opcode, uint funct3, uint rs1, uint rs2, int imm) {
    return s_imm(imm) | rs2 << 20 | rs1 << 15 | funct3 << 12 | opcode;
}

uint b_imm(int imm) {
    uint i = (uint)imm;
    return ((i >> 12) & 1) << 31 | ((i >> 5) & 0x3f) << 25 | ((i >> 1) & 0xf) << 8 | ((i >> 11) & 1) << 7;
}

static uint b_type(uint opcode, uint funct3, uint rs1, uint rs2, int imm) {
    return b_imm(imm) | rs2 << 20 | rs1 << 15 | funct3 << 12 | opcode;
}

uint u_imm(uint imm20) {
    return (imm20 & 0xfffff) << 12;
}

static uint u_type(uint opcode, uint rd, uint imm20) {
    return u_imm(imm20) | rd << 7 | opcode;
}

uint j_imm(int imm) {
    uint i = (uint)imm;
    return ((i >> 20) & 1) << 31 | ((i >> 1) & 0x3ff) << 21 | ((i >> 11) & 1) << 20 | ((i >> 12) & 0xff) << 12;
}

static uint j_type(uint opcode, uint rd, int imm) {
    return j_imm(imm) | rd << 7 | opcode;
}

void hi_lo(int value, uint *hi, int *lo) {
    int l = (int)((uint)value << 20) >> 20; /* the low 12 bits, sign-extended */
    *lo = l;
    *hi = ((uint)value - (uint)l) >> 12;
}

/* ---- Errors ---- */

/* The error of the instruction being encoded. */
static char **err_out;

static int fail(const char *msg) {
    *err_out = tmp_strndup(msg, (uint)strlen(msg));
    return 0;
}

/* The message being made (fmt). */
static char *msgbuf;

/* ---- Operands ---- */

enum { IMM_NUM, IMM_LABEL, IMM_RELOC };
struct imm {
    int kind;
    i64 num;
    int mod;
    char *name;
    i64 addend;
};

/* An immediate field ready to pack: its value, or zero and the relocation. */
struct field {
    int value;
    int has_fixup;
    int kind;
    char *name;
    i64 addend;
};

static void field_num(struct field *f, int value) {
    memset(f, 0, sizeof *f);
    f->value = value;
}

static void field_reloc(struct field *f, int kind, char *name, i64 addend) {
    memset(f, 0, sizeof *f);
    f->has_fixup = 1;
    f->kind = kind;
    f->name = name;
    f->addend = addend;
}

/* The words being made. */
static struct emit *emits;
static int nemits;

static void emit_word(uint word) {
    struct emit *e = &emits[nemits++];
    memset(e, 0, sizeof *e);
    e->word = word;
    e->auipc = -1;
}

static void emit_field(uint word, struct field *f) {
    struct emit *e = &emits[nemits++];
    memset(e, 0, sizeof *e);
    e->word = word;
    e->auipc = -1;
    if (f->has_fixup) {
        e->has_fixup = 1;
        e->kind = f->kind;
        e->name = f->name;
        e->addend = f->addend;
    }
}

static void emit_symbol(uint word, int kind, char *name, i64 addend) {
    struct emit *e = &emits[nemits++];
    memset(e, 0, sizeof *e);
    e->word = word;
    e->has_fixup = 1;
    e->kind = kind;
    e->name = name;
    e->addend = addend;
    e->auipc = -1;
}

/* The low half of a pc-relative pair: against the auipc, word 0. */
static void emit_auipc_low(uint word, int kind) {
    struct emit *e = &emits[nemits++];
    memset(e, 0, sizeof *e);
    e->word = word;
    e->has_fixup = 1;
    e->kind = kind;
    e->auipc = 0;
}

/* A 32-bit value, signed or unsigned: -1 and 0xffffffff are the same word. */
static int word32(i64 v, const char *what, int *out) {
    if (i64_in(v, i64_of((int)0x80000000), i64_shl(i64_u(1), 32))) {
        *out = (int)v.lo;
        return 1;
    }
    msgbuf = fmt("%s %s does not fit in 32 bits", what, i64_dec(v));
    return fail(msgbuf);
}

static int signed_field(i64 v, uint bits, const char *what, int *out) {
    i64 half = i64_shl(i64_u(1), bits - 1);
    if (i64_in(v, i64_neg(half), half)) {
        *out = (int)v.lo;
        return 1;
    }
    msgbuf = fmt("%s %s out of range (%s..=%s)", what, i64_dec(v), i64_dec(i64_neg(half)),
             i64_dec(i64_sub(half, i64_u(1))));
    return fail(msgbuf);
}

static int unsigned_field(i64 v, uint bits, const char *what, uint *out) {
    i64 max = i64_sub(i64_shl(i64_u(1), bits), i64_u(1));
    if (!i64_is_neg(v) && i64_cmp(v, max) <= 0) {
        *out = v.lo;
        return 1;
    }
    if (bits >= 12) {
        if (i64_is_neg(v)) {
            msgbuf = fmt("%s -%s out of range (0..=%s)", what, i64_hex(i64_neg(v)), i64_hex(max));
        } else {
            msgbuf = fmt("%s %s out of range (0..=%s)", what, i64_hex(v), i64_hex(max));
        }
    } else {
        msgbuf = fmt("%s %s out of range (0..=%s)", what, i64_dec(v), i64_dec(max));
    }
    return fail(msgbuf);
}

/* A branch or jump offset: even, `bits` bits signed. */
static int offset_field(i64 v, uint bits, const char *what, int *out) {
    i64 half = i64_shl(i64_u(1), bits - 1);
    if (!i64_in(v, i64_neg(half), half)) {
        msgbuf = fmt("%s %s out of range (%s..=%s)", what, i64_dec(v), i64_dec(i64_neg(half)),
                 i64_dec(i64_sub(half, i64_u(2))));
        return fail(msgbuf);
    }
    if (v.lo & 1) {
        msgbuf = fmt("%s %s must be even", what, i64_dec(v));
        return fail(msgbuf);
    }
    *out = (int)v.lo;
    return 1;
}

static int imm(struct expr *e, struct imm *out) {
    memset(out, 0, sizeof *out);
    i64 v;
    if (ctx_constant(e, &v)) {
        out->kind = IMM_NUM;
        out->num = v;
        return 1;
    }
    if (e->kind == E_MOD) {
        const char *m = modifier_name(e->op);
        if (ctx_constant(e->a, &v)) {
            /* `%hi(0x12345678)`: no symbol, so split the number here. */
            char *what = fmt("%s() value", m);
            int w;
            if (!word32(v, what, &w)) {
                return 0;
            }
            uint hi;
            int lo;
            hi_lo(w, &hi, &lo);
            if (e->op == M_HI) {
                out->kind = IMM_NUM;
                out->num = i64_u(hi);
                return 1;
            }
            if (e->op == M_LO) {
                out->kind = IMM_NUM;
                out->num = i64_of(lo);
                return 1;
            }
            msgbuf = fmt("%s() needs a symbol, not a number", m);
            return fail(msgbuf);
        }
        if (!ctx_symbol(e->a, &out->name, &out->addend)) {
            msgbuf = fmt("%s() needs a symbol or a number", m);
            return fail(msgbuf);
        }
        out->kind = IMM_RELOC;
        out->mod = e->op;
        return 1;
    }
    if (ctx_symbol(e, &out->name, &out->addend)) {
        out->kind = IMM_LABEL;
        return 1;
    }
    return fail("expected a number, a constant or symbol+offset");
}

static const char *what(struct operand *op) {
    if (op->kind == OPD_REG) {
        return "a register";
    }
    if (op->kind == OPD_EXPR) {
        return "an immediate";
    }
    return "a memory operand";
}

static int reg(struct operand *op, uint *out) {
    if (op->kind == OPD_REG) {
        *out = (uint)op->reg;
        return 1;
    }
    msgbuf = fmt("expected a register, found %s", what(op));
    return fail(msgbuf);
}

static int expr_of(struct operand *op, struct expr **out) {
    if (op->kind == OPD_EXPR) {
        *out = op->e;
        return 1;
    }
    msgbuf = fmt("expected an immediate, found %s", what(op));
    return fail(msgbuf);
}

static int mem(struct operand *op, struct expr **offset, uint *base) {
    if (op->kind == OPD_MEM) {
        *offset = op->e;
        *base = (uint)op->reg;
        return 1;
    }
    msgbuf = fmt("expected offset(register), found %s", what(op));
    return fail(msgbuf);
}

static int not_yet_constant(const char *name, const char *more) {
    msgbuf = fmt("'%s' is not a constant (yet): .equ/.set constants must be defined before use%s",
             name, more);
    return fail(msgbuf);
}

/* A 12-bit signed immediate (I-type, or S-type when `store`). */
static int imm12(struct expr *e, int store, struct field *f) {
    struct imm im;
    if (!imm(e, &im)) {
        return 0;
    }
    if (im.kind == IMM_NUM) {
        int v;
        if (!signed_field(im.num, 12, "immediate", &v)) {
            return 0;
        }
        field_num(f, v);
        return 1;
    }
    if (im.kind == IMM_RELOC) {
        if (im.mod == M_LO) {
            field_reloc(f, store ? R_RISCV_LO12_S : R_RISCV_LO12_I, im.name, im.addend);
            return 1;
        }
        if (im.mod == M_PCREL_LO) {
            field_reloc(f, store ? R_RISCV_PCREL_LO12_S : R_RISCV_PCREL_LO12_I, im.name, im.addend);
            return 1;
        }
        msgbuf = fmt("%s() is a 20-bit upper part and belongs in %s, not in a 12-bit immediate", modifier_name(im.mod),
                 im.mod == M_HI ? "lui" : "auipc");
        return fail(msgbuf);
    }
    if (ctx_undefined(im.name)) {
        return not_yet_constant(im.name, "");
    }
    return fail("a label can't be a 12-bit immediate: use %lo(sym) or %pcrel_lo(label)");
}

/* The 20-bit immediate of lui (a number or %hi) or auipc (or %pcrel_hi). */
static int imm20(struct expr *e, uint opcode, struct field *f) {
    int lui = opcode == OPC_LUI;
    int m = lui ? M_HI : M_PCREL_HI;
    struct imm im;
    if (!imm(e, &im)) {
        return 0;
    }
    if (im.kind == IMM_NUM) {
        uint v;
        if (!unsigned_field(im.num, 20, "upper immediate", &v)) {
            return 0;
        }
        field_num(f, (int)v);
        return 1;
    }
    if (im.kind == IMM_RELOC && im.mod == m) {
        field_reloc(f, lui ? R_RISCV_HI20 : R_RISCV_PCREL_HI20, im.name, im.addend);
        return 1;
    }
    msgbuf = fmt("%s takes a number in 0..=0xfffff or %s(sym)", lui ? "lui" : "auipc",
             modifier_name(m));
    return fail(msgbuf);
}

static int branch_target(struct expr *e, struct field *f) {
    struct imm im;
    if (!imm(e, &im)) {
        return 0;
    }
    if (im.kind == IMM_NUM) {
        int v;
        if (!offset_field(im.num, 13, "branch offset", &v)) {
            return 0;
        }
        field_num(f, v);
        return 1;
    }
    if (im.kind == IMM_LABEL) {
        field_reloc(f, R_RISCV_BRANCH, im.name, im.addend);
        return 1;
    }
    msgbuf = fmt("a branch target can't use %s()", modifier_name(im.mod));
    return fail(msgbuf);
}

static int jump_target(struct expr *e, struct field *f) {
    struct imm im;
    if (!imm(e, &im)) {
        return 0;
    }
    if (im.kind == IMM_NUM) {
        int v;
        if (!offset_field(im.num, 21, "jump offset", &v)) {
            return 0;
        }
        field_num(f, v);
        return 1;
    }
    if (im.kind == IMM_LABEL) {
        field_reloc(f, R_RISCV_JAL, im.name, im.addend);
        return 1;
    }
    msgbuf = fmt("a jump target can't use %s()", modifier_name(im.mod));
    return fail(msgbuf);
}

/* A plain number in 0..2^bits. */
static int uimm(struct expr *e, uint bits, const char *what_, uint *out) {
    i64 v;
    if (ctx_constant(e, &v)) {
        return unsigned_field(v, bits, what_, out);
    }
    msgbuf = fmt("%s must be a number", what_);
    return fail(msgbuf);
}

/* The symbol of `la`, `call`, `lw rd, sym`...: never a number or modifier. */
static int symbol(struct operand *op, const char *m, char **name, i64 *addend) {
    struct expr *e;
    struct imm im;
    if (!expr_of(op, &e) || !imm(e, &im)) {
        return 0;
    }
    if (im.kind == IMM_LABEL) {
        *name = im.name;
        *addend = im.addend;
        return 1;
    }
    if (im.kind == IMM_NUM) {
        msgbuf = fmt("%s needs a symbol, not a number", m);
    } else {
        msgbuf = fmt("%s takes a plain symbol, not %s()", m, modifier_name(im.mod));
    }
    return fail(msgbuf);
}

static int csr_addr(struct operand *op, uint *out) {
    if (op->kind != OPD_EXPR) {
        msgbuf = fmt("expected a CSR name or number, found %s", what(op));
        return fail(msgbuf);
    }
    struct expr *e = op->e;
    if (ctx_csr(e, out)) {
        return 1;
    }
    i64 v;
    if (ctx_constant(e, &v)) {
        msgbuf = fmt("CSR number %s out of range (0..=0xfff)", i64_dec(v));
        return fail(msgbuf);
    }
    if (e->kind == E_SYM) {
        msgbuf = fmt("unknown CSR '%s'", e->sym);
        return fail(msgbuf);
    }
    return fail("expected a CSR name or number");
}

/* A fence's set: letters in order from `iorw`, or 0. */
static int fence_set(struct operand *op, uint *out) {
    struct expr *e;
    if (!expr_of(op, &e)) {
        return 0;
    }
    i64 v;
    if (ctx_constant(e, &v) && v.lo == 0 && v.hi == 0) {
        *out = 0;
        return 1;
    }
    const char *bad = "a fence set is 0 or letters taken in order from 'iorw'";
    if (e->kind != E_SYM) {
        return fail(bad);
    }
    uint bits = 0, last = 16;
    for (const char *p = e->sym; *p; p++) {
        uint bit;
        if (*p == 'i') {
            bit = 8;
        } else if (*p == 'o') {
            bit = 4;
        } else if (*p == 'r') {
            bit = 2;
        } else if (*p == 'w') {
            bit = 1;
        } else {
            return fail(bad);
        }
        if (bit >= last) {
            return fail(bad);
        }
        bits |= bit;
        last = bit;
    }
    *out = bits;
    return 1;
}

/* ---- Tables ---- */

static const struct as_op3 *find3(const struct as_op3 *t, const char *m) {
    for (; t->name; t++) {
        if (strcmp(t->name, m) == 0) {
            return t;
        }
    }
    return NULL;
}

static const struct as_op2 *find2(const struct as_op2 *t, const char *m) {
    for (; t->name; t++) {
        if (strcmp(t->name, m) == 0) {
            return t;
        }
    }
    return NULL;
}

int csr_parse(const char *name, uint *addr) {
    const struct as_op2 *c = find2(CSR_NAMES, name);
    if (c == NULL) {
        return 0;
    }
    *addr = c->value;
    return 1;
}

/* ---- The instructions with more to them ---- */

static const char *mnemonic;

static int usage(const char *form) {
    if (form[0] == 0) {
        msgbuf = fmt("%s takes no operands", mnemonic);
    } else {
        msgbuf = fmt("usage: %s %s", mnemonic, form);
    }
    return fail(msgbuf);
}

static int branch(uint funct3, uint rs1, uint rs2, struct operand *target) {
    struct expr *e;
    struct field f;
    if (!expr_of(target, &e) || !branch_target(e, &f)) {
        return 0;
    }
    emit_field(b_type(OPC_BRANCH, funct3, rs1, rs2, f.value), &f);
    return 1;
}

static int jal(uint rd, struct operand *target) {
    struct expr *e;
    struct field f;
    if (!expr_of(target, &e) || !jump_target(e, &f)) {
        return 0;
    }
    emit_field(j_type(OPC_JAL, rd, f.value), &f);
    return 1;
}

static int jalr(uint rd, uint rs1, struct expr *offset) {
    struct field f;
    if (!imm12(offset, 0, &f)) {
        return 0;
    }
    emit_field(i_type(OPC_JALR, 0, rd, rs1, f.value), &f);
    return 1;
}

/* `lw rd, offset(rs1)`, or `lw rd, symbol`: auipc + load through rd. */
static int load(const char *m, uint funct3, struct operand *ops, uint nops) {
    if (nops != 2) {
        msgbuf = fmt("usage: %s rd, offset(rs1)  or  %s rd, symbol", m, m);
        return fail(msgbuf);
    }
    uint rd;
    if (!reg(&ops[0], &rd)) {
        return 0;
    }
    struct operand *src = &ops[1];
    if (src->kind == OPD_MEM) {
        struct field f;
        if (!imm12(src->e, 0, &f)) {
            return 0;
        }
        emit_field(i_type(OPC_LOAD, funct3, rd, (uint)src->reg, f.value), &f);
        return 1;
    }
    if (src->kind == OPD_EXPR) {
        char *name;
        i64 addend;
        if (!symbol(src, m, &name, &addend)) {
            return 0;
        }
        emit_symbol(u_type(OPC_AUIPC, rd, 0), R_RISCV_PCREL_HI20, name, addend);
        emit_auipc_low(i_type(OPC_LOAD, funct3, rd, rd, 0), R_RISCV_PCREL_LO12_I);
        return 1;
    }
    msgbuf = fmt("expected offset(register) or a symbol, found %s", what(src));
    return fail(msgbuf);
}

/* `sw rs2, offset(rs1)`, or `sw rs2, symbol, rt`: auipc + store through rt. */
static int store(const char *m, uint funct3, struct operand *ops, uint nops) {
    if (nops == 2 && ops[1].kind == OPD_EXPR) {
        msgbuf = fmt("a store to a symbol needs a scratch register for the address: %s rs2, symbol, rt", m);
        return fail(msgbuf);
    }
    if (nops == 2) {
        uint rs2, base;
        struct expr *offset;
        struct field f;
        if (!reg(&ops[0], &rs2) || !mem(&ops[1], &offset, &base) || !imm12(offset, 1, &f)) {
            return 0;
        }
        emit_field(s_type(OPC_STORE, funct3, base, rs2, f.value), &f);
        return 1;
    }
    if (nops == 3) {
        uint rs2, rt;
        char *name;
        i64 addend;
        if (!reg(&ops[0], &rs2) || !symbol(&ops[1], m, &name, &addend) || !reg(&ops[2], &rt)) {
            return 0;
        }
        emit_symbol(u_type(OPC_AUIPC, rt, 0), R_RISCV_PCREL_HI20, name, addend);
        emit_auipc_low(s_type(OPC_STORE, funct3, rt, rs2, 0), R_RISCV_PCREL_LO12_S);
        return 1;
    }
    msgbuf = fmt("usage: %s rs2, offset(rs1)  or  %s rs2, symbol, rt", m, m);
    return fail(msgbuf);
}

/* An A instruction's address: (rs1), or a literal 0(rs1) (see encode.rs). */
static int atomic_address(const char *m, struct operand *addr, uint *base) {
    struct expr *offset;
    if (!mem(addr, &offset, base)) {
        msgbuf = fmt("%s: %s takes its address as (rs1)", *err_out, m);
        return fail(msgbuf);
    }
    if (offset->kind == E_NUM && offset->num.lo == 0 && offset->num.hi == 0 && addr->int_token) {
        return 1;
    }
    msgbuf = fmt("%s has no offset field: write the address as (rs1)", m);
    return fail(msgbuf);
}

static int csr_op(uint funct3, uint rd, struct operand *csr, struct operand *src) {
    uint addr, rs1;
    if (!csr_addr(csr, &addr)) {
        return 0;
    }
    if (funct3 & F_CSR_IMM) {
        struct expr *e;
        if (!expr_of(src, &e) || !uimm(e, 5, "CSR immediate", &rs1)) {
            return 0;
        }
    } else if (!reg(src, &rs1)) {
        return 0;
    }
    emit_word(i_type(OPC_SYSTEM, funct3, rd, rs1, (int)addr));
    return 1;
}

static uint trailing_zeros(uint v) {
    uint n = 0;
    while (n < 32 && (v & (1u << n)) == 0) {
        n++;
    }
    return n;
}

/* `li rd, value` in one or two instructions, as LLVM picks them. */
static int li(uint rd, int value) {
    if (value >= -2048 && value <= 2047) {
        emit_word(i_type(OPC_OP_IMM, F_ADDI, rd, ZERO, value));
        return 1;
    }
    uint hi;
    int lo;
    hi_lo(value, &hi, &lo);
    if (lo == 0) {
        emit_word(u_type(OPC_LUI, rd, hi));
        return 1;
    }
    uint shift = trailing_zeros((uint)value);
    if (shift > 0 && (value >> shift) >= -32 && (value >> shift) <= 31) {
        emit_word(i_type(OPC_OP_IMM, F_ADDI, rd, ZERO, value >> shift));
        emit_word(i_type(OPC_OP_IMM, F_SLLI, rd, rd, (int)shift));
        return 1;
    }
    emit_word(u_type(OPC_LUI, rd, hi));
    emit_word(i_type(OPC_OP_IMM, F_ADDI, rd, rd, lo));
    return 1;
}

/* .insn fields. */
static int insn_field(struct operand *op, uint bits, const char *what_, uint *out) {
    struct expr *e;
    return expr_of(op, &e) && uimm(e, bits, what_, out);
}

static int insn_opcode(struct operand *op, uint *out) {
    if (!insn_field(op, 7, "opcode", out)) {
        return 0;
    }
    if ((*out & 3) != 3) {
        msgbuf = fmt("opcode 0x%x is not a 32-bit opcode (its low two bits must be 11)", *out);
        return fail(msgbuf);
    }
    return 1;
}

static const char *insn_format;

static int insn_usage(const char *form) {
    msgbuf = fmt("usage: .insn %s %s", insn_format, form);
    return fail(msgbuf);
}

/* `.insn r|i|s|b|u ...` or `.insn [4,] value`. */
static int insn(const char *m, struct operand *ops, uint nops) {
    const char *format = m + 5;
    while (*format == ' ') {
        format++;
    }
    insn_format = format;
    uint op, f3, f7, rd, rs1, rs2;
    struct field f;
    if (strcmp(format, "r") == 0) {
        if (nops != 6) {
            return insn_usage("opcode, funct3, funct7, rd, rs1, rs2");
        }
        if (!insn_opcode(&ops[0], &op) || !insn_field(&ops[1], 3, "funct3", &f3) ||
            !insn_field(&ops[2], 7, "funct7", &f7) || !reg(&ops[3], &rd) || !reg(&ops[4], &rs1) ||
            !reg(&ops[5], &rs2)) {
            return 0;
        }
        emit_word(r_type(op, f3, f7, rd, rs1, rs2));
        return 1;
    }
    if (strcmp(format, "i") == 0) {
        struct expr *offset;
        if (nops == 4 && ops[3].kind == OPD_MEM) {
            rs1 = (uint)ops[3].reg;
            offset = ops[3].e;
        } else if (nops == 5) {
            if (!reg(&ops[3], &rs1) || !expr_of(&ops[4], &offset)) {
                return 0;
            }
        } else {
            return insn_usage("opcode, funct3, rd, rs1, simm12");
        }
        if (!insn_opcode(&ops[0], &op) || !insn_field(&ops[1], 3, "funct3", &f3) || !reg(&ops[2], &rd) ||
            !imm12(offset, 0, &f)) {
            return 0;
        }
        emit_field(i_type(op, f3, rd, rs1, f.value), &f);
        return 1;
    }
    if (strcmp(format, "s") == 0) {
        if (nops != 4) {
            return insn_usage("opcode, funct3, rs2, simm12(rs1)");
        }
        struct expr *offset;
        uint base;
        if (!insn_opcode(&ops[0], &op) || !insn_field(&ops[1], 3, "funct3", &f3) || !reg(&ops[2], &rs2) ||
            !mem(&ops[3], &offset, &base) || !imm12(offset, 1, &f)) {
            return 0;
        }
        emit_field(s_type(op, f3, base, rs2, f.value), &f);
        return 1;
    }
    if (strcmp(format, "b") == 0) {
        if (nops != 5) {
            return insn_usage("opcode, funct3, rs1, rs2, offset");
        }
        struct expr *target;
        if (!insn_opcode(&ops[0], &op) || !insn_field(&ops[1], 3, "funct3", &f3) || !expr_of(&ops[4], &target) ||
            !branch_target(target, &f) || !reg(&ops[2], &rs1) || !reg(&ops[3], &rs2)) {
            return 0;
        }
        emit_field(b_type(op, f3, rs1, rs2, f.value), &f);
        return 1;
    }
    if (strcmp(format, "u") == 0) {
        if (nops != 3) {
            return insn_usage("opcode, rd, imm20");
        }
        uint imm20v;
        if (!insn_opcode(&ops[0], &op) || !reg(&ops[1], &rd) || !insn_field(&ops[2], 20, "upper immediate", &imm20v)) {
            return 0;
        }
        emit_word(u_type(op, rd, imm20v));
        return 1;
    }
    if (format[0] == 0) {
        struct operand *value;
        if (nops == 1) {
            value = &ops[0];
        } else if (nops == 2) {
            struct expr *len;
            if (!expr_of(&ops[0], &len)) {
                return 0;
            }
            i64 v;
            if (!ctx_constant(len, &v) || v.lo != 4 || v.hi != 0) {
                return fail(".insn length must be 4 (there are no 16-bit instructions)");
            }
            value = &ops[1];
        } else {
            return fail("usage: .insn value  or  .insn 4, value");
        }
        uint w;
        if (!insn_field(value, 32, "instruction word", &w)) {
            return 0;
        }
        if ((w & 3) != 3) {
            msgbuf = fmt("0x%08x is not a 32-bit instruction (its low two bits must be 11)", w);
            return fail(msgbuf);
        }
        emit_word(w);
        return 1;
    }
    msgbuf = fmt("unknown .insn format '%s' (merlin-as knows r, i, s, b and u)", format);
    return fail(msgbuf);
}

static int is(const char *m, const char *name) {
    return strcmp(m, name) == 0;
}

/* The A extension's mnemonics: a base and an ordering suffix. */
static int atomic(const char *m, char *base, uint *funct5, uint *aqrl) {
    static const char *const suffixes[3] = {".aqrl", ".aq", ".rl"};
    static const uint bits[3] = {3, 2, 1};
    uint n = (uint)strlen(m);
    if (n >= 40) {
        return 0;
    }
    strcpy(base, m);
    *aqrl = 0;
    for (int i = 0; i < 3; i++) {
        uint k = (uint)strlen(suffixes[i]);
        if (n >= k && strcmp(m + n - k, suffixes[i]) == 0) {
            base[n - k] = 0;
            *aqrl = bits[i];
            break;
        }
    }
    const struct as_op2 *a = find2(ATOMIC_FUNCT5, base);
    if (a == NULL) {
        return 0;
    }
    *funct5 = a->value;
    return 1;
}

/* Everything else: by mnemonic. */
static int encode_named(const char *m, struct operand *ops, uint nops) {
    uint rd, rs, rs1, rs2;
    if (is(m, "lui") || is(m, "auipc")) {
        if (nops != 2) {
            return usage("rd, imm20");
        }
        uint opcode = is(m, "lui") ? OPC_LUI : OPC_AUIPC;
        struct expr *e;
        struct field f;
        if (!reg(&ops[0], &rd) || !expr_of(&ops[1], &e) || !imm20(e, opcode, &f)) {
            return 0;
        }
        emit_field(u_type(opcode, rd, (uint)f.value), &f);
        return 1;
    }
    if (is(m, "jal")) {
        if (nops == 1) {
            return jal(RA, &ops[0]);
        }
        if (nops == 2) {
            return reg(&ops[0], &rd) && jal(rd, &ops[1]);
        }
        return usage("[rd,] offset");
    }
    if (is(m, "jalr")) {
        if (nops == 1) {
            if (!reg(&ops[0], &rs)) {
                return 0;
            }
            emit_word(i_type(OPC_JALR, 0, RA, rs, 0));
            return 1;
        }
        if (nops == 2 && ops[1].kind == OPD_MEM) {
            return reg(&ops[0], &rd) && jalr(rd, (uint)ops[1].reg, ops[1].e);
        }
        if (nops == 3 && ops[2].kind == OPD_EXPR) {
            return reg(&ops[0], &rd) && reg(&ops[1], &rs1) && jalr(rd, rs1, ops[2].e);
        }
        return usage("rd, offset(rs1)");
    }
    if (is(m, "fence")) {
        uint pred, succ;
        if (nops == 0) {
            pred = succ = 15;
        } else if (nops == 2) {
            if (!fence_set(&ops[0], &pred) || !fence_set(&ops[1], &succ)) {
                return 0;
            }
        } else {
            return usage("[pred, succ]");
        }
        emit_word(i_type(OPC_MISC_MEM, 0, ZERO, ZERO, (int)(pred << 4 | succ)));
        return 1;
    }
    if (is(m, "fence.tso")) {
        if (nops != 0) {
            return usage("");
        }
        emit_word(8u << 28 | 3u << 24 | 3u << 20 | OPC_MISC_MEM);
        return 1;
    }
    if (is(m, "fence.i")) {
        if (nops != 0) {
            return usage("");
        }
        emit_word(i_type(OPC_MISC_MEM, 1, ZERO, ZERO, 0));
        return 1;
    }
    if (is(m, "sfence.vma")) {
        rs1 = rs2 = ZERO;
        if (nops > 2) {
            return usage("[rs1[, rs2]]");
        }
        if (nops >= 1 && !reg(&ops[0], &rs1)) {
            return 0;
        }
        if (nops == 2 && !reg(&ops[1], &rs2)) {
            return 0;
        }
        emit_word(r_type(OPC_SYSTEM, 0, 9, ZERO, rs1, rs2));
        return 1;
    }
    if (is(m, "xcal.dbg") || is(m, "xcal.halt")) {
        if (nops != 1) {
            return usage("rs1");
        }
        if (!reg(&ops[0], &rs1)) {
            return 0;
        }
        emit_word(i_type(OPC_CUSTOM_0, is(m, "xcal.dbg") ? 0 : 1, ZERO, rs1, 0));
        return 1;
    }
    /* Pseudo-instructions. */
    if (is(m, "nop")) {
        if (nops != 0) {
            return usage("");
        }
        emit_word(i_type(OPC_OP_IMM, F_ADDI, ZERO, ZERO, 0));
        return 1;
    }
    if (is(m, "li")) {
        if (nops != 2) {
            return usage("rd, imm");
        }
        struct expr *e;
        struct imm im;
        if (!reg(&ops[0], &rd) || !expr_of(&ops[1], &e) || !imm(e, &im)) {
            return 0;
        }
        if (im.kind == IMM_NUM) {
            int v;
            return word32(im.num, "li value", &v) && li(rd, v);
        }
        if (im.kind == IMM_LABEL && ctx_undefined(im.name)) {
            return not_yet_constant(im.name, " (li loads a number; la loads an address)");
        }
        return fail("li takes a constant (use la to load an address)");
    }
    if (is(m, "mv") || is(m, "not") || is(m, "neg") || is(m, "seqz") || is(m, "snez") || is(m, "sltz") ||
        is(m, "sgtz") || is(m, "zext.b")) {
        if (nops != 2) {
            return usage("rd, rs");
        }
        if (!reg(&ops[0], &rd) || !reg(&ops[1], &rs)) {
            return 0;
        }
        uint w;
        if (is(m, "mv")) {
            w = i_type(OPC_OP_IMM, F_ADDI, rd, rs, 0);
        } else if (is(m, "not")) {
            w = i_type(OPC_OP_IMM, F_XORI, rd, rs, -1);
        } else if (is(m, "neg")) {
            w = r_type(OPC_OP, 0, 0x20, rd, ZERO, rs);
        } else if (is(m, "seqz")) {
            w = i_type(OPC_OP_IMM, F_SLTIU, rd, rs, 1);
        } else if (is(m, "snez")) {
            w = r_type(OPC_OP, 3, 0, rd, ZERO, rs);
        } else if (is(m, "sltz")) {
            w = r_type(OPC_OP, 2, 0, rd, rs, ZERO);
        } else if (is(m, "sgtz")) {
            w = r_type(OPC_OP, 2, 0, rd, ZERO, rs);
        } else {
            w = i_type(OPC_OP_IMM, F_ANDI, rd, rs, 0xff);
        }
        emit_word(w);
        return 1;
    }
    if (is(m, "sgt") || is(m, "sgtu")) {
        /* sgt rd, a, b is slt rd, b, a: the sources swapped (sgtu: sltu). */
        if (nops != 3) {
            return usage("rd, rs, rt");
        }
        uint rt;
        if (!reg(&ops[0], &rd) || !reg(&ops[1], &rs) || !reg(&ops[2], &rt)) {
            return 0;
        }
        emit_word(r_type(OPC_OP, is(m, "sgt") ? 2 : 3, 0, rd, rt, rs));
        return 1;
    }
    if (is(m, "j")) {
        if (nops != 1) {
            return usage("offset");
        }
        return jal(ZERO, &ops[0]);
    }
    if (is(m, "jr")) {
        if (nops != 1) {
            return usage("rs");
        }
        if (!reg(&ops[0], &rs)) {
            return 0;
        }
        emit_word(i_type(OPC_JALR, 0, ZERO, rs, 0));
        return 1;
    }
    if (is(m, "ret")) {
        if (nops != 0) {
            return usage("");
        }
        emit_word(i_type(OPC_JALR, 0, ZERO, RA, 0));
        return 1;
    }
    if (is(m, "beqz") || is(m, "bnez") || is(m, "bltz") || is(m, "bgez") || is(m, "blez") || is(m, "bgtz")) {
        if (nops != 2) {
            return usage("rs, offset");
        }
        if (!reg(&ops[0], &rs)) {
            return 0;
        }
        if (is(m, "beqz")) {
            return branch(F_BEQ, rs, ZERO, &ops[1]);
        }
        if (is(m, "bnez")) {
            return branch(F_BNE, rs, ZERO, &ops[1]);
        }
        if (is(m, "bltz")) {
            return branch(F_BLT, rs, ZERO, &ops[1]);
        }
        if (is(m, "bgez")) {
            return branch(F_BGE, rs, ZERO, &ops[1]);
        }
        if (is(m, "blez")) {
            return branch(F_BGE, ZERO, rs, &ops[1]);
        }
        return branch(F_BLT, ZERO, rs, &ops[1]);
    }
    if (is(m, "bgt") || is(m, "ble") || is(m, "bgtu") || is(m, "bleu")) {
        if (nops != 3) {
            return usage("rs, rt, offset");
        }
        uint funct3 = is(m, "bgt") ? F_BLT : is(m, "ble") ? F_BGE : is(m, "bgtu") ? F_BLTU : F_BGEU;
        uint rt;
        /* bgt a, b is blt b, a: rt is checked first. */
        if (!reg(&ops[1], &rt) || !reg(&ops[0], &rs)) {
            return 0;
        }
        return branch(funct3, rt, rs, &ops[2]);
    }
    if (is(m, "la") || is(m, "lla")) {
        if (nops != 2) {
            return usage("rd, symbol");
        }
        char *name;
        i64 addend;
        if (!reg(&ops[0], &rd) || !symbol(&ops[1], m, &name, &addend)) {
            return 0;
        }
        emit_symbol(u_type(OPC_AUIPC, rd, 0), R_RISCV_PCREL_HI20, name, addend);
        emit_auipc_low(i_type(OPC_OP_IMM, F_ADDI, rd, rd, 0), R_RISCV_PCREL_LO12_I);
        return 1;
    }
    if (is(m, "call") || is(m, "tail")) {
        if (nops != 1) {
            return usage("symbol");
        }
        char *name;
        i64 addend;
        if (!symbol(&ops[0], m, &name, &addend)) {
            return 0;
        }
        uint scratch = is(m, "call") ? RA : T1;
        uint link = is(m, "call") ? RA : ZERO;
        emit_symbol(u_type(OPC_AUIPC, scratch, 0), R_RISCV_CALL_PLT, name, addend);
        emit_word(i_type(OPC_JALR, 0, link, scratch, 0));
        return 1;
    }
    if (is(m, "csrr")) {
        if (nops != 2) {
            return usage("rd, csr");
        }
        struct operand zero;
        memset(&zero, 0, sizeof zero);
        zero.kind = OPD_REG;
        zero.reg = ZERO;
        return reg(&ops[0], &rd) && csr_op(F_CSRRS, rd, &ops[1], &zero);
    }
    if (is(m, "csrw") || is(m, "csrs") || is(m, "csrc") || is(m, "csrwi") || is(m, "csrsi") || is(m, "csrci")) {
        if (nops != 2) {
            return usage("csr, rs");
        }
        uint funct3 = m[3] == 'w' ? F_CSRRW : m[3] == 's' ? F_CSRRS : F_CSRRC;
        /* `csrw mstatus, 8` means csrwi, as in LLVM. */
        if (m[4] == 'i' || ops[1].kind == OPD_EXPR) {
            funct3 |= F_CSR_IMM;
        }
        return csr_op(funct3, ZERO, &ops[0], &ops[1]);
    }
    if (is(m, "rdcycle") || is(m, "rdtime") || is(m, "rdinstret") || is(m, "rdcycleh") || is(m, "rdtimeh") ||
        is(m, "rdinstreth")) {
        if (nops != 1) {
            return usage("rd");
        }
        uint counter = is(m, "rdcycle")    ? CSR_CYCLE
                       : is(m, "rdtime")   ? CSR_TIME
                       : is(m, "rdinstret") ? CSR_INSTRET
                       : is(m, "rdcycleh") ? CSR_CYCLEH
                       : is(m, "rdtimeh")  ? CSR_TIMEH
                                           : CSR_INSTRETH;
        if (!reg(&ops[0], &rd)) {
            return 0;
        }
        emit_word(i_type(OPC_SYSTEM, F_CSRRS, rd, ZERO, (int)counter));
        return 1;
    }
    if (is(m, "unimp")) {
        if (nops != 0) {
            return usage("");
        }
        emit_word(i_type(OPC_SYSTEM, F_CSRRW, ZERO, ZERO, CSR_CYCLE));
        return 1;
    }
    if (is(m, ".insn") || strncmp(m, ".insn ", 6) == 0) {
        return insn(m, ops, nops);
    }
    msgbuf = fmt("unknown instruction '%s'", m);
    return fail(msgbuf);
}

int encode(const char *m, struct operand *ops, uint nops, struct emit *out, char **err) {
    emits = out;
    nemits = 0;
    err_out = err;
    mnemonic = m;
    uint rd, rs1, rs2;
    const struct as_op3 *t3;
    const struct as_op2 *t2;
    int ok;

    if ((t3 = find3(OP_FUNCT, m)) != NULL) {
        /* R-type: OP and M. */
        if (nops != 3) {
            ok = usage("rd, rs1, rs2");
        } else {
            ok = reg(&ops[0], &rd) && reg(&ops[1], &rs1) && reg(&ops[2], &rs2);
            if (ok) {
                emit_word(r_type(OPC_OP, t3->a, t3->b, rd, rs1, rs2));
            }
        }
    } else if ((t2 = find2(OP_IMM_FUNCT3, m)) != NULL) {
        /* I-type: OP-IMM. */
        struct expr *e;
        struct field f;
        if (nops != 3) {
            ok = usage("rd, rs1, imm");
        } else {
            ok = reg(&ops[0], &rd) && reg(&ops[1], &rs1) && expr_of(&ops[2], &e) && imm12(e, 0, &f);
            if (ok) {
                emit_field(i_type(OPC_OP_IMM, t2->value, rd, rs1, f.value), &f);
            }
        }
    } else if ((t3 = find3(SHIFT_FUNCT, m)) != NULL) {
        struct expr *e;
        uint shamt;
        if (nops != 3) {
            ok = usage("rd, rs1, shamt");
        } else {
            /* The shift amount is checked before the registers. */
            ok = expr_of(&ops[2], &e) && uimm(e, 5, "shift amount", &shamt) && reg(&ops[0], &rd) &&
                 reg(&ops[1], &rs1);
            if (ok) {
                emit_word(i_type(OPC_OP_IMM, t3->a, rd, rs1, (int)(t3->b << 5 | shamt)));
            }
        }
    } else if ((t2 = find2(LOAD_FUNCT3, m)) != NULL) {
        ok = load(m, t2->value, ops, nops);
    } else if ((t2 = find2(STORE_FUNCT3, m)) != NULL) {
        ok = store(m, t2->value, ops, nops);
    } else if ((t2 = find2(BRANCH_FUNCT3, m)) != NULL) {
        if (nops != 3) {
            ok = usage("rs1, rs2, offset");
        } else {
            ok = reg(&ops[0], &rs1) && reg(&ops[1], &rs2) && branch(t2->value, rs1, rs2, &ops[2]);
        }
    } else if ((t2 = find2(CSR_FUNCT3, m)) != NULL) {
        if (nops != 3) {
            ok = usage(t2->value & F_CSR_IMM ? "rd, csr, uimm5" : "rd, csr, rs1");
        } else {
            ok = reg(&ops[0], &rd) && csr_op(t2->value, rd, &ops[1], &ops[2]);
        }
    } else if ((t2 = find2(SYSTEM_FUNCT12, m)) != NULL) {
        if (nops != 0) {
            ok = usage("");
        } else {
            emit_word(i_type(OPC_SYSTEM, 0, ZERO, ZERO, (int)t2->value));
            ok = 1;
        }
    } else {
        char base[40];
        uint funct5, aqrl;
        if (atomic(m, base, &funct5, &aqrl)) {
            /* A extension: funct7 is funct5 then aq and rl; funct3 010.
             * lr.w has no rs2. rs2 is checked first, then the address,
             * then rd, as in merlin-as. */
            uint base_reg;
            struct operand *addr;
            ok = 1;
            if (strcmp(base, "lr.w") == 0) {
                if (nops != 2) {
                    ok = usage("rd, (rs1)");
                } else {
                    rs2 = ZERO;
                    addr = &ops[1];
                }
            } else if (nops != 3) {
                ok = usage("rd, rs2, (rs1)");
            } else {
                ok = reg(&ops[1], &rs2);
                addr = &ops[2];
            }
            ok = ok && atomic_address(m, addr, &base_reg) && reg(&ops[0], &rd);
            if (ok) {
                emit_word(r_type(OPC_AMO, 2, funct5 << 2 | aqrl, rd, base_reg, rs2));
            }
        } else {
            ok = encode_named(m, ops, nops);
        }
    }
    return ok ? nemits : 0;
}
