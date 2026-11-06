/* cc1: code generation (follows merlin's codegen.rs). Placeholder while it is
 * ported. */
#include "cc.h"

int codegen_generate(struct program *prog, struct buf *out, struct diag **error) {
    (void)prog;
    (void)out;
    *error = diag_error(pos_at(0, 0, 0), "code generation is not here yet");
    return 0;
}
