/* cc1: the preprocessor (follows merlin's preprocess.rs). */
#include "cc.h"

int preprocess_text(const char *name, const char *src, uint n, struct options *opts, struct buf *out,
                    struct diag ***diags, uint *ndiags) {
    (void)name;
    (void)src;
    (void)n;
    (void)opts;
    (void)out;
    *diags = malloc(sizeof(struct diag *));
    (*diags)[0] = diag_error(pos_at(0, 0, 0), "the preprocessor is not here yet");
    *ndiags = 1;
    return 0;
}
