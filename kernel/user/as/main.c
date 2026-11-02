/* as: `as [-o out.o] input.s`, merlin-as's command line (main.rs). One
 * pass over the text, a statement at a time. */
#include "as.h"

static const char USAGE[] = "usage: as [-o output.o] input.s\n"
                            "\n"
                            "  -o FILE   output object (default: input with .o extension)";

/* Whether the bytes are valid UTF-8, as Rust's read_to_string checks. */
static int valid_utf8(const uchar *s, uint n) {
    uint i = 0;
    while (i < n) {
        uint c = s[i];
        uint len, min;
        if (c < 0x80) {
            i++;
            continue;
        } else if ((c & 0xe0) == 0xc0) {
            len = 2;
            min = 0x80;
            c &= 0x1f;
        } else if ((c & 0xf0) == 0xe0) {
            len = 3;
            min = 0x800;
            c &= 0x0f;
        } else if ((c & 0xf8) == 0xf0) {
            len = 4;
            min = 0x10000;
            c &= 0x07;
        } else {
            return 0;
        }
        if (i + len > n) {
            return 0;
        }
        for (uint k = 1; k < len; k++) {
            if ((s[i + k] & 0xc0) != 0x80) {
                return 0;
            }
            c = c << 6 | (s[i + k] & 0x3f);
        }
        if (c < min || c > 0x10ffff || (c >= 0xd800 && c <= 0xdfff)) {
            return 0;
        }
        i += len;
    }
    return 1;
}

/* The whole file, or NULL with *err set. */
static char *read_file(const char *path, uint *len, int *err) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        *err = fd;
        return NULL;
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    char chunk[4096];
    for (;;) {
        int n = read(fd, chunk, sizeof chunk);
        if (n < 0) {
            close(fd);
            *err = n;
            return NULL;
        }
        if (n == 0) {
            break;
        }
        buf_put(&b, chunk, (uint)n);
    }
    close(fd);
    buf_byte(&b, 0);
    *len = b.len - 1;
    return (char *)b.data;
}

static int write_file(const char *path, const uchar *data, uint len) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC);
    if (fd < 0) {
        return fd;
    }
    uint done = 0;
    while (done < len) {
        uint n = len - done > 4096 ? 4096 : len - done;
        int r = write(fd, data + done, n);
        if (r <= 0) {
            close(fd);
            return r < 0 ? r : -ENOSPC;
        }
        done += (uint)r;
    }
    return close(fd);
}

int main(int argc, char **argv) {
    char *input = NULL;
    char *output = NULL;
    for (int i = 1; i < argc; i++) {
        char *a = argv[i];
        if (strcmp(a, "-o") == 0) {
            if (i + 1 >= argc) {
                fprintf(2, "as: -o needs a file name\n");
                return 2;
            }
            output = argv[++i];
        } else if (strcmp(a, "-h") == 0 || strcmp(a, "--help") == 0) {
            printf("%s\n", USAGE);
            return 0;
        } else if (a[0] == '-') {
            fprintf(2, "as: unknown option %s\n%s\n", a, USAGE);
            return 2;
        } else {
            input = a;
        }
    }
    if (input == NULL) {
        fprintf(2, "%s\n", USAGE);
        return 2;
    }
    if (output == NULL) {
        uint n = (uint)strlen(input);
        if (n >= 2 && input[n - 2] == '.' && (input[n - 1] == 's' || input[n - 1] == 'S')) {
            n -= 2;
        }
        output = perm_alloc(n + 3);
        memcpy(output, input, n);
        memcpy(output + n, ".o", 3);
    }

    uint len;
    int err;
    char *src = read_file(input, &len, &err);
    if (src == NULL) {
        fprintf(2, "as: cannot read %s: %s\n", input, strerror(-err));
        return 2;
    }
    if (!valid_utf8((uchar *)src, len)) {
        fprintf(2, "as: cannot read %s: stream did not contain valid UTF-8\n", input);
        return 2;
    }

    /* One pass: lex, parse and assemble each statement. merlin-as lexes the
     * whole file before it parses, and parses it all before it assembles, so
     * its errors are the lexer's (the first one only), else the parser's,
     * else the assembler's: each phase's errors are kept apart, and only the
     * first phase that had any is shown. The work done after an error is
     * thrown away. */
    struct lexer lx;
    struct token *toks;
    struct stmt *stmts;
    int n;
    lexer_init(&lx, src, len);
    asm_init();
    for (;;) {
        error_phase(PHASE_LEX);
        n = lex_statement(&lx, &toks);
        if (n <= 0) {
            break;
        }
        error_phase(PHASE_PARSE);
        int count = parse_statement(toks, n, &stmts);
        error_phase(PHASE_ASM);
        for (int i = 0; i < count; i++) {
            asm_statement(&stmts[i]);
        }
        tmp_reset();
    }
    if (n < 0) {
        errors_print(input, PHASE_LEX);
        return 1;
    }
    if (error_count(PHASE_PARSE) > 0) {
        errors_print(input, PHASE_PARSE);
        return 1;
    }
    error_phase(PHASE_ASM);
    struct object *obj = asm_finish();
    if (obj == NULL) {
        errors_print(input, PHASE_ASM);
        return 1;
    }

    struct buf out;
    memset(&out, 0, sizeof out);
    elf_write(obj, &out);
    int r = write_file(output, out.data, out.len);
    if (r < 0) {
        fprintf(2, "as: cannot write %s: %s\n", output, strerror(-r));
        return 2;
    }
    return 0;
}
