/*
 * mimalloc's purge delay, named by the header the linked library was built
 * from (`DEP_MIMALLOC_INCLUDE_DIR`): `libmimalloc-sys` declares no constant
 * for it (UF6), so the compiler resolves `mi_option_purge_delay` here, and a
 * renamed option fails `pgdt`'s build. Compiled by `build.rs` and called
 * from `src/alloc.rs`'s `purge`.
 */
#include <mimalloc.h>

void pgdt_purge_delay_set_default(long ms) {
  mi_option_set_default(mi_option_purge_delay, ms);
}

void pgdt_purge_delay_set(long ms) {
  mi_option_set(mi_option_purge_delay, ms);
}

long pgdt_purge_delay(void) {
  return mi_option_get(mi_option_purge_delay);
}
