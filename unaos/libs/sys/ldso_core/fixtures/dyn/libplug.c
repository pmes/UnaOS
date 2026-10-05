// SPDX-License-Identifier: GPL-3.0-or-later
// SELFBUILD6 (B360) dyn probe: the object the PIE dlopens (as rustc dlopens a proc-macro).
int plug_ctor_ran;
__thread int plug_tls = 5;
int dyn_add(int, int);                     /* resolved against the already-loaded libdyn.so */
__attribute__((constructor)) static void plug_ctor(void) { plug_ctor_ran = 1; }
int plug_entry(int x) { return dyn_add(x, plug_ctor_ran) + plug_tls; }
