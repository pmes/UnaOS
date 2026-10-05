/* SPDX-License-Identifier: GPL-3.0-or-later */
/* WINDOW2 host proof: load a static-PIE exactly the way linuxabi/elf.rs now does — every PT_LOAD at
   PIE_BASE + (p_vaddr - lowest page), no relocation by the loader, a System V stack with AT_PHDR biased,
   AT_BASE 0, AT_ENTRY biased — and jump to it. If the program runs, its self-relocation works at our base. */
#define _GNU_SOURCE
#include <elf.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#define PIE_BASE 0x17F00000000ULL
int main(int argc, char **argv, char **envp) {
    if (argc < 2) return 2;
    int fd = open(argv[1], O_RDONLY); struct stat st; fstat(fd, &st);
    unsigned char *f = mmap(0, st.st_size, PROT_READ, MAP_PRIVATE, fd, 0);
    Elf64_Ehdr *eh = (void *)f; Elf64_Phdr *ph = (void *)(f + eh->e_phoff);
    if (eh->e_type != ET_DYN) { fprintf(stderr, "not ET_DYN\n"); return 3; }
    unsigned long lo = ~0UL, phdr_va = 0;
    for (int i = 0; i < eh->e_phnum; i++) { if (ph[i].p_type == PT_INTERP) { fprintf(stderr, "INTERP\n"); return 4; }
        if (ph[i].p_type == PT_LOAD && (ph[i].p_vaddr & ~0xFFFUL) < lo) lo = ph[i].p_vaddr & ~0xFFFUL; }
    unsigned long bias = PIE_BASE - lo;
    for (int i = 0; i < eh->e_phnum; i++) { if (ph[i].p_type != PT_LOAD) continue;
        unsigned long a = (ph[i].p_vaddr + bias) & ~0xFFFUL, e = (ph[i].p_vaddr + bias + ph[i].p_memsz + 0xFFF) & ~0xFFFUL;
        if (mmap((void *)a, e - a, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0) != (void *)a) { /* page shared with previous segment */ }
        memcpy((void *)(ph[i].p_vaddr + bias), f + ph[i].p_offset, ph[i].p_filesz);
        if (eh->e_phoff >= ph[i].p_offset && eh->e_phoff < ph[i].p_offset + ph[i].p_filesz) phdr_va = ph[i].p_vaddr + bias + (eh->e_phoff - ph[i].p_offset);
    }
    for (int i = 0; i < eh->e_phnum; i++) { if (ph[i].p_type != PT_LOAD) continue;  /* W^X perms after the copy, as the kernel's leaf bits */
        unsigned long a = (ph[i].p_vaddr + bias) & ~0xFFFUL, e = (ph[i].p_vaddr + bias + ph[i].p_memsz + 0xFFF) & ~0xFFFUL;
        int p = PROT_READ | ((ph[i].p_flags & PF_W) ? PROT_WRITE : 0) | ((ph[i].p_flags & PF_X) ? PROT_EXEC : 0);
        if (ph[i].p_flags & PF_W) p |= PROT_READ; mprotect((void *)a, e - a, p | ((ph[i].p_flags & PF_W) ? PROT_WRITE : 0)); }
    size_t sz = 1 << 20; unsigned long *stk = mmap(0, sz, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    unsigned long *sp = (unsigned long *)((char *)stk + sz - 4096); static unsigned char rnd[16] = {1,2,3};
    int n = 0, nenv = 0; while (envp[nenv]) nenv++;
    sp[n++] = argc - 1; for (int i = 1; i < argc; i++) sp[n++] = (unsigned long)argv[i]; sp[n++] = 0;
    for (int i = 0; i < nenv; i++) sp[n++] = (unsigned long)envp[i]; sp[n++] = 0;
    unsigned long aux[][2] = {{AT_PHDR, phdr_va}, {AT_PHENT, 56}, {AT_PHNUM, eh->e_phnum}, {AT_PAGESZ, 4096}, {AT_BASE, 0},
        {AT_FLAGS, 0}, {AT_ENTRY, eh->e_entry + bias}, {AT_UID, getuid()}, {AT_EUID, geteuid()}, {AT_GID, getgid()}, {AT_EGID, getegid()},
        {AT_SECURE, 0}, {AT_RANDOM, (unsigned long)rnd}, {AT_NULL, 0}};
    for (unsigned i = 0; i < sizeof aux / sizeof aux[0]; i++) { sp[n++] = aux[i][0]; sp[n++] = aux[i][1]; }
    fprintf(stderr, "[pieload] base=%#lx entry=%#lx phdr=%#lx\n", PIE_BASE, eh->e_entry + bias, phdr_va);
    __asm__ volatile("mov %0, %%rsp; xor %%edx, %%edx; jmp *%1" :: "r"(sp), "r"(eh->e_entry + bias) : "memory");
    return 0;
}
