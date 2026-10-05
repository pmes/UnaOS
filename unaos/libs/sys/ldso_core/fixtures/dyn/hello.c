// SPDX-License-Identifier: GPL-3.0-or-later
// SELFBUILD6 (B360) dyn probe: the PIE. Prints `dyn=ok` when every loader duty checks out, else `dyn=fail(<id>)`.
typedef unsigned long size_t;
int printf(const char *, ...);
void *dlopen(const char *, int);
void *dlsym(void *, const char *);
int dlclose(void *);
char *dlerror(void);
struct dl_phdr_info { unsigned long addr; const char *name; const void *phdr; unsigned short phnum; };
int dl_iterate_phdr(int (*)(struct dl_phdr_info *, size_t, void *), void *);
typedef struct { const char *fname; void *fbase; const char *sname; void *saddr; } Dl_info;
int dladdr(const void *, Dl_info *);
typedef unsigned long pthread_t;
int pthread_create(pthread_t *, const void *, void *(*)(void *), void *);
int pthread_join(pthread_t, void **);
char *strstr(const char *, const char *);

extern int dyn_ctor_ran, dyn_counter;
extern __thread int dyn_tls_ie;
int dyn_add(int, int);
int dyn_tls_sum(void);
int *dyn_tls_gd_addr(void);

int *counter_ptr = &dyn_counter;          /* R_X86_64_64 against a symbol in the .so */
__thread int exe_tls = 3;                 /* the executable's own block, local-exec, right under tp */

static int count_cb(struct dl_phdr_info *i, size_t sz, void *d) { (void)i; (void)sz; ++*(int *)d; return 0; }
static void *thr(void *a) { (void)a; return (void *)(long)(dyn_tls_sum() * 100 + exe_tls); }

int main(int argc, char **argv) {
    (void)argc;
    int fail = 0;
    if (!dyn_ctor_ran) fail = 1;                               /* constructors before main */
    else if (dyn_add(2, 3) != 5) fail = 2;                     /* JUMP_SLOT */
    else if (*counter_ptr != 40 || counter_ptr != &dyn_counter) fail = 3;   /* 64 + GLOB_DAT agree */
    else if (dyn_tls_sum() != 18) fail = 4;                    /* TPOFF64 + DTPMOD64/DTPOFF64 initial images */
    else if (exe_tls != 3) fail = 5;                           /* the exe's local-exec block */
    else if (dyn_tls_ie != 7) fail = 6;                        /* IE from the exe */
    if (!fail) {
        *dyn_tls_gd_addr() = 20;                               /* per-thread: the main thread's copy only */
        pthread_t t; void *r = 0;
        if (pthread_create(&t, 0, thr, 0) || pthread_join(t, &r)) fail = 7;
        else if ((long)r != 1803) fail = 8;                    /* a new thread gets the INITIAL images (7+11, 3) */
        else if (dyn_tls_sum() != 27) fail = 9;
    }
    int nobj = 0;
    if (!fail) { dl_iterate_phdr(count_cb, &nobj); if (nobj != 3) fail = 10; }
    if (!fail) {
        void *h = dlopen("libplug.so", 1);
        if (!h) { printf("dlopen: %s\n", dlerror()); fail = 11; }
        else {
            int (*pe)(int) = (int (*)(int))dlsym(h, "plug_entry");
            if (!pe) fail = 12;
            else if (pe(10) != 16) fail = 13;                  /* 10 + ctor(1) + plug TLS 5 */
            else if (dlsym(h, "no_such_symbol") || !dlerror()) fail = 14;
            dlclose(h);
        }
    }
    Dl_info di;
    if (!fail && (!dladdr((void *)dyn_add, &di) || !di.fname || !strstr(di.fname, "libdyn.so"))) fail = 15;
    int n2 = 0;
    if (!fail) { dl_iterate_phdr(count_cb, &n2); if (n2 != nobj + 1) fail = 16; }
    if (fail) printf("dyn=fail(%d) objects=%d\n", fail, nobj);
    else printf("dyn=ok objects=%d after_dlopen=%d argv0=%s\n", nobj, n2, argv[0]);
    return fail;
}
