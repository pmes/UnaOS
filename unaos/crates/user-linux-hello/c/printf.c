/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 The Architect & Una
 *
 * SELFBUILD3 (B353) fixture: the first program UnaOS compiles ON ITSELF against a real libc — tcc under the Linux ABI shim
 * links it with musl's crt1.o/libc.a staged under /apps/LIB: `linux /apps/TCC.LNX -static -o <home>/hellop.lnx /apps/PRINTF.C`.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(void)
{
    char *b = malloc(64);
    if (!b)
        return 2;
    strcpy(b, "hello printf from tcc+musl on unaos");
    printf("%s %d\n", b, 6 * 7);
    free(b);
    return 0;
}
