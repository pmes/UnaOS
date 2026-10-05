// SPDX-License-Identifier: GPL-3.0-or-later
// SELFBUILD6 (B360) dyn probe: the shared object. No headers (freestanding declarations against musl).
int dyn_ctor_ran;                          /* set by the constructor: init_array ran before main */
int dyn_counter = 40;                      /* a data symbol the PIE reads through GLOB_DAT */
__thread int dyn_tls_ie __attribute__((tls_model("initial-exec"))) = 7;   /* TPOFF64 */
__thread int dyn_tls_gd = 11;              /* -fPIC default: general-dynamic -> DTPMOD64/DTPOFF64 + __tls_get_addr */
int printf(const char *, ...);

__attribute__((constructor)) static void dyn_ctor(void) { dyn_ctor_ran = 1; }
__attribute__((destructor)) static void dyn_dtor(void) { printf("dyn: destructor ran\n"); }

int dyn_add(int a, int b) { return a + b + dyn_counter - 40; }
int dyn_tls_sum(void) { return dyn_tls_ie + dyn_tls_gd; }
int *dyn_tls_gd_addr(void) { return &dyn_tls_gd; }
