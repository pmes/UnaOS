//! HELPVERB (R75) — the verbs document themselves.
//!
//! One table, [`DOCS`]: for every word in `midden_core::HOST_VERBS` / `CORE_VERBS` a group, a one-line
//! summary, a usage string and example lines, written from the verb's own match arm in `shell.rs` (or
//! the module that owns it). `help` lists by group; `help <verb>` and `<verb> --help` print the usage,
//! summary and examples; `man <verb>` opens the same text in the FILEVIEW window.
//! `tests helpdoc` is the registry check: every registered verb has a doc (a verb without one is listed
//! as `missing` and the witness FAILs).
//!
//! Witness: `:: HELPVERB: verbs=<n> documented=<n> missing=[..] groups=<n> -> PASS|FAIL ::`.
use crate::console::Console;
use alloc::string::String;
use alloc::vec::Vec;

pub struct VerbDoc {
    pub name: &'static str,
    pub group: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
    pub examples: &'static [&'static str],
}

const fn d(name: &'static str, group: &'static str, summary: &'static str, usage: &'static str, examples: &'static [&'static str]) -> VerbDoc {
    VerbDoc { name, group, summary, usage, examples }
}

/// Display order of the groups.
pub const GROUPS: &[&str] = &["files", "windows", "users", "network", "system", "audio", "tests", "self-host"];

pub static DOCS: &[VerbDoc] = &[
    // ── files ──
    d("ls", "files", "list a directory (or one file), `-l` for size and time, wildcards ok", "ls [-l] [path|*.EXT]", &["ls", "ls -l /boot", "ls *.TXT"]),
    d("dir", "files", "alias of ls", "dir [-l] [path|*.EXT]", &["dir"]),
    d("cd", "files", "change the working directory (no argument returns to /)", "cd [path]", &["cd /boot", "cd .."]),
    d("pwd", "files", "print the working directory", "pwd", &["pwd"]),
    d("cat", "files", "print a file (wildcards ok)", "cat <path>", &["cat README.TXT", "cat *.TXT"]),
    d("type", "files", "alias of cat", "type <path>", &["type README.TXT"]),
    d("head", "files", "print the first lines of a file (default 10)", "head <path> [lines]", &["head LOG.TXT 5"]),
    d("tail", "files", "print the last lines of a file (default 10)", "tail <path> [lines]", &["tail LOG.TXT 20"]),
    d("find", "files", "recursive glob search under a root (default .), with a match tally", "find [root] <pattern>", &["find *.ELF", "find /apps *.ELF"]),
    d("du", "files", "recursive size of a directory subtree, per child plus a total", "du [dir]", &["du", "du /apps"]),
    d("stat", "files", "one entry's detail: path, volume, kind, size, object id, mtime", "stat <path>", &["stat /boot/KERNEL.ELF"]),
    d("hexdump", "files", "bounded hex dump of a file (len capped at 4096)", "hexdump <path> [off] [len]", &["hexdump A.BIN", "hexdump A.BIN 0x200 64"]),
    d("touch", "files", "create an empty file if absent", "touch <path>", &["touch NEW.TXT"]),
    d("append", "files", "append text at the end of a file, creating it if absent", "append <path> <text>", &["append LOG.TXT hello"]),
    d("write", "files", "create or truncate a file with the given text", "write <path> <text...>", &["write A.TXT hello world"]),
    d("rm", "files", "delete files (`-r` a directory tree, `-f` quiet on missing); wildcards ok", "rm [-r] [-f] <path> [path ...]", &["rm OLD.TXT", "rm -rf TMPDIR", "rm *.TMP"]),
    d("del", "files", "alias of rm", "del [-r] [-f] <path> [path ...]", &["del OLD.TXT"]),
    d("mkdir", "files", "create a directory", "mkdir <path>", &["mkdir DOCS"]),
    d("md", "files", "alias of mkdir", "md <path>", &["md DOCS"]),
    d("rmdir", "files", "remove an EMPTY directory", "rmdir <path>", &["rmdir DOCS"]),
    d("rd", "files", "alias of rmdir", "rd <path>", &["rd DOCS"]),
    d("cp", "files", "copy files; `-r` copies a tree, no-clobber by default (`-f` overwrites)", "cp [-r] [-f|-n] <src...> <dst>", &["cp A.TXT B.TXT", "cp -r DOCS BACKUP", "cp *.TXT DOCS/"]),
    d("copy", "files", "alias of cp", "copy [-r] [-f|-n] <src...> <dst>", &["copy A.TXT B.TXT"]),
    d("mv", "files", "move or rename a file or directory (one directory-entry relink)", "mv [-f|-n] <src...> <dst>", &["mv A.TXT B.TXT", "mv *.TXT DOCS/"]),
    d("move", "files", "alias of mv", "move [-f|-n] <src...> <dst>", &["move A.TXT DOCS/"]),
    d("ren", "files", "alias of mv", "ren [-f|-n] <src> <dst>", &["ren A.TXT B.TXT"]),
    d("rename", "files", "alias of mv", "rename [-f|-n] <src> <dst>", &["rename A.TXT B.TXT"]),
    d("sync", "files", "confirm storage is write-through (a no-op: every write is already durable)", "sync", &["sync"]),
    d("grep", "files", "fixed-string search in one file (^ $ anchors; -i -n -v -c)", "grep [-i] [-n] [-v] [-c] <pattern> <path>", &["grep -n error LOG.TXT", "grep -ic warn LOG.TXT"]),
    d("wc", "files", "lines, words and bytes of one file (-l -w -c select)", "wc [-l|-w|-c] <path>", &["wc README.TXT", "wc -l LOG.TXT"]),
    d("df", "files", "capacity of each attached volume", "df", &["df"]),
    d("mount", "files", "list attached volumes with FAT geometry; or write/append/rm/mkdir through the one namespace", "mount | mount <write|append|rm|mkdir> <path> [text ...]", &["mount", "mount mkdir /usb/X"]),
    d("setfattr", "files", "set a typed attribute (value typed by shape: 42, 1.5, \"text\", b64:.., [f,f]) or drop one", "setfattr <path> <key>=<value> | setfattr -x <key> <path>", &["setfattr NOTES.TXT kind=\"note\"", "setfattr A.JPG emb=[0.1,0.9,0.2]", "setfattr -x kind NOTES.TXT"]),
    d("getfattr", "files", "print one typed attribute, or all of them, as `key: type value`", "getfattr <path> [<key>]", &["getfattr NOTES.TXT", "getfattr NOTES.TXT kind"]),
    d("query", "files", "find every readable object whose attributes match; prints `id path`", "query <expr>", &["query kind == \"note\"", "query size > 100", "query similarity(emb, [0.1,0.9,0.2]) > 0.8"]),
    d("snap", "files", "snapshots of the native volume: list, create, drop, browse", "snap list | create <name> | drop <gen> | ls <gen> [path] | cat <gen> <path>", &["snap list", "snap create before-edit", "snap cat 3 NOTES.TXT"]),
    d("trash", "files", "move a file to the trash, list, restore or empty it", "trash <path> | trash list | trash restore <name> | trash empty", &["trash OLD.TXT", "trash list", "trash restore OLD.TXT"]),
    d("dd", "files", "raw 512-byte block: read one (`if=`) or patch one byte (`of=` `byte=`)", "dd if=<lba> | dd of=<lba> byte=<0xNN>", &["dd if=0", "dd of=2048 byte=0xAA"]),
    d("fdisk", "files", "list block devices (read-only; no partition editor)", "fdisk -l", &["fdisk -l"]),
    d("lsusb", "files", "list the USB devices the bus enumerated", "lsusb", &["lsusb"]),
    d("which", "files", "say whether a word is a verb, a program, or neither", "which <word>", &["which ls", "which HELLO"]),
    // ── windows ──
    d("view", "windows", "open a text file in the read-only viewer window", "view <path>", &["view README.TXT"]),
    d("edit", "windows", "open a text file under /home/<user> in the editor window (Ctrl-S saves)", "edit <path>", &["edit /home/ann/NOTES.TXT"]),
    d("man", "windows", "open a verb's help text in the viewer window", "man <verb>", &["man ls"]),
    d("activity", "windows", "open the activity monitor window (q closes, k kills the selected process)", "activity", &["activity"]),
    d("settings", "windows", "open the settings window", "settings", &["settings"]),
    d("pref", "windows", "read or set a preference in Principia's store (<home>/.config/unaos/preferences.toml)", "pref get <ns.key> | pref set <ns.key> <value> | pref list [<ns>]", &["pref list system", "pref get system.display.brightness", "pref set system.display.idle_min 5"]),
    d("shortcuts", "windows", "print the desktop keyboard shortcuts", "shortcuts", &["shortcuts"]),
    d("wallpaper", "windows", "set the desktop backdrop from a PNG (<= 4 MB), or `off` for the flat colour", "wallpaper <path.png> | wallpaper off", &["wallpaper /home/ann/SKY.PNG", "wallpaper off"]),
    d("screenshot", "windows", "capture the panel to SCREEN<n>.PNG at the volume root", "screenshot", &["screenshot"]),
    d("shot", "windows", "screenshot selection mode: drag a region or click a window (Esc cancels)", "shot region | shot window", &["shot region", "shot window"]),
    d("clear", "windows", "clear the console screen", "clear", &["clear"]),
    // ── users ──
    d("login", "users", "open a session as a user", "login <name> <password>", &["login ann secret"]),
    d("logout", "users", "close the session and return to the login screen", "logout", &["logout"]),
    d("adduser", "users", "create a user (root only; the password is asked for, never shown)", "adduser <name>", &["adduser ann"]),
    d("deluser", "users", "delete a user (root only; not yourself)", "deluser <name>", &["deluser ann"]),
    d("passwd", "users", "change a password (root or yourself; asked twice, never shown)", "passwd [name]", &["passwd", "passwd ann"]),
    d("users", "users", "list users with uid, home and whether a password is set", "users", &["users"]),
    d("whoami", "users", "print the session user, uid and login stage", "whoami", &["whoami"]),
    // ── network ──
    d("ifconfig", "network", "show the network interface: MAC, IP, gateway, lease, clock sync", "ifconfig", &["ifconfig"]),
    d("ping", "network", "ICMP echo to an IPv4 address (1-16 requests, default 4)", "ping <a.b.c.d> [count]", &["ping 10.0.2.2", "ping 10.0.2.2 2"]),
    d("arp", "network", "resolve an IPv4 address to its MAC", "arp <a.b.c.d>", &["arp 10.0.2.2"]),
    d("nc", "network", "send a message over TCP, or UDP with -u, and print the reply", "nc [-u] <a.b.c.d> <port> [message]", &["nc 10.0.2.2 80 hi", "nc -u 10.0.2.2 7 ping"]),
    d("curl", "network", "minimal HTTP/1.0 GET to an IPv4 host, response printed", "curl [http://]<a.b.c.d>[:port][/path]", &["curl 10.0.2.2/", "curl http://10.0.2.2:8080/index.html"]),
    d("fetch", "network", "download an http URL to a file (or `-` to print it); https unsupported", "fetch <http-url> [<dest-path>] | fetch - <http-url>", &["fetch http://10.0.2.2/a.txt A.TXT", "fetch - http://10.0.2.2/a.txt"]),
    d("dns", "network", "resolve a hostname (aarch64 net6 builds; elsewhere refuses by name)", "dns <hostname>", &["dns example.com"]),
    // ── system ──
    d("help", "system", "list verbs by group, or show one verb's usage", "help [verb]", &["help", "help ls"]),
    d("echo", "system", "print the arguments", "echo [text...]", &["echo hello"]),
    d("ver", "system", "print the OS version", "ver", &["ver"]),
    d("version", "system", "alias of ver", "version", &["version"]),
    d("gneiss", "system", "print a line of house philosophy", "gneiss", &["gneiss"]),
    d("exit", "system", "explain there is nothing to exit to (use shutdown / reboot / clear)", "exit", &["exit"]),
    d("quit", "system", "alias of exit", "quit", &["quit"]),
    d("date", "system", "show the wall clock; `-s` sets it", "date | date -s YYYY-MM-DD HH:MM[:SS]", &["date", "date -s 2026-10-01 09:30"]),
    d("time", "system", "ISO-8601 UTC time and what set it (`unsynced` until set)", "time", &["time"]),
    d("uptime", "system", "time since boot, plus the wall clock when set", "uptime", &["uptime"]),
    d("sleep", "system", "pause for N milliseconds (decimal or 0x-hex, capped at 10000)", "sleep <ms>", &["sleep 500"]),
    d("env", "system", "the build's live facts, then the shell variables", "env", &["env"]),
    d("set", "system", "show, set or unset shell variables", "set | set NAME | set NAME VALUE... | set -u NAME", &["set GREETING hello", "set -u GREETING"]),
    d("history", "system", "the last typed lines, numbered; `-c` clears", "history [n] | history -c", &["history", "history 5", "history -c"]),
    d("ps", "system", "scheduler task table and per-core census", "ps", &["ps"]),
    d("top", "system", "per-core scheduler load table", "top", &["top"]),
    d("batmon", "system", "one fresh battery line (SMC machines)", "batmon", &["batmon"]),
    d("dmesg", "system", "print the boot-milestone ring with timestamps", "dmesg", &["dmesg"]),
    d("run", "system", "load an ELF64 user program and run it in the foreground, reporting its exit status", "run <path>", &["run /apps/ELFHELLO.ELF"]),
    d("bg", "system", "run a user program in the background; its window stays open", "bg <path>", &["bg /apps/VUG.ELF"]),
    d("storm", "system", "launch a fleet of background vug programs (default 6) and measure headroom", "storm [n]", &["storm", "storm 12"]),
    d("jobs", "system", "list background programs and reap the exited ones", "jobs", &["jobs"]),
    d("kill", "system", "kill a background program by pid (see `jobs`)", "kill <pid>", &["kill 3"]),
    d("shutdown", "system", "power off through the platform firmware", "shutdown", &["shutdown"]),
    d("off", "system", "alias of shutdown", "off", &["off"]),
    d("reboot", "system", "warm-restart through the platform firmware", "reboot", &["reboot"]),
    d("panic", "system", "deliberately panic the kernel (tests the exception handler)", "panic", &["panic"]),
    d("install", "system", "no arguments: census of every disk partition and the refusal each would give; `install <disk> <slot>` installs into that one partition; `install ssd` self-installs to SATA", "install | install <disk> <slot> [--as-esp] | install ssd [--dry-run]", &["install", "install 0 2 --as-esp"]),
    d("v3d", "system", "replay the visible GPU battery on the live screen (aarch64 v3d builds)", "v3d", &["v3d"]),
    d("burst", "system", "fire the multi-thread scheduler burst across the cores (aarch64; refuses by name elsewhere)", "burst", &["burst"]),
    d("simmer", "system", "animate per-core load on every non-boot core (aarch64; refuses by name elsewhere)", "simmer", &["simmer"]),
    d("linux", "system", "run a static Linux x86_64 ELF through the Linux ABI shim and print its witness", "linux <path> [args...]", &["linux /apps/HELLO.LNX"]),
    // ── audio ──
    d("play", "audio", "stream a WAV file to the sound device, or stop playback", "play <path.wav> | play stop", &["play /home/ann/TUNE.WAV", "play stop"]),
    // ── tests ──
    d("tests", "tests", "run the desktop test fixtures (all, one by name, or list them); refused until first-boot setup is done", "tests [name] | tests list", &["tests list", "tests helpdoc", "tests"]),
    d("tste", "tests", "run the in-OS self-test suite: a PASS/FAIL/SKIP table", "tste", &["tste"]),
    d("selftest", "tests", "alias of tste", "selftest", &["selftest"]),
    // ── self-host ──
    d("src", "self-host", "extract, check or verify the bundled source tree under /SRC/ on the system volume", "src extract [--dry-run] | src status | src verify", &["src status", "src extract --dry-run", "src verify"]),
];

pub fn find(name: &str) -> Option<&'static VerbDoc> {
    DOCS.iter().find(|x| x.name == name)
}

/// Every word the registry knows (CORE + HOST, all availabilities) that has no usable doc.
pub fn missing() -> (usize, Vec<&'static str>) {
    let mut n = 0usize;
    let mut miss = Vec::new();
    let all = midden_core::CORE_VERBS.iter().copied().chain(midden_core::HOST_VERBS.iter().map(|(v, _)| *v));
    for v in all {
        n += 1;
        match find(v) {
            Some(x) if !x.summary.starts_with("(undocumented") && !x.usage.is_empty() && GROUPS.contains(&x.group) => {}
            _ => miss.push(v),
        }
    }
    (n, miss)
}

pub fn render(x: &VerbDoc) -> String {
    let mut s = alloc::format!("{} - {}\n\nusage: {}\n", x.name, x.summary, x.usage);
    if !x.examples.is_empty() {
        s.push_str("\nexamples:\n");
        for e in x.examples { s.push_str("  "); s.push_str(e); s.push('\n'); }
    }
    s
}

fn print_text(console: &mut Console, text: &str) {
    for l in text.lines() { console.println(l); }
}

fn list(console: &mut Console) {
    console.println("verbs by area (`help <verb>` for usage and examples, `man <verb>` in a window):");
    for g in GROUPS {
        console.println(&alloc::format!("{}:", g));
        for x in DOCS.iter().filter(|x| x.group == *g) {
            console.println(&alloc::format!("  {:<10} {}", x.name, x.summary));
        }
    }
}

fn help_one(console: &mut Console, w: &str) {
    match find(&w.to_ascii_lowercase()) {
        Some(x) => print_text(console, &render(x)),
        None => console.println(&alloc::format!("help: no help for `{}` (try `help`)", w)),
    }
}

#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn man_one(console: &mut Console, w: &str) {
    match find(&w.to_ascii_lowercase()) {
        Some(x) => match crate::video::fileview::open_text(&alloc::format!("man:{}", x.name), &render(x)) {
            Ok(_) => console.println(&alloc::format!("man: {} - window open", x.name)),
            Err(e) => console.println(&alloc::format!("man: {}", e)),
        },
        None => console.println(&alloc::format!("man: no manual entry for `{}`", w)),
    }
}
#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn man_one(console: &mut Console, w: &str) { help_one(console, w); }

/// The dispatcher's one check: `help`, `help <verb>`, `man <verb>`, `<verb> --help`. True = handled.
pub fn intercept(line: &str, console: &mut Console) -> bool {
    let mut it = line.split_whitespace();
    let Some(first) = it.next() else { return false };
    let first = first.to_ascii_lowercase();
    let a1 = it.next();
    match first.as_str() {
        "help" => { match a1 { None => list(console), Some(w) => help_one(console, w) } true }
        "man" => { match a1 { None => console.println("usage: man <verb>   (open its help in a window)"), Some(w) => man_one(console, w) } true }
        "echo" => false,
        _ if a1 == Some("--help") && it.next().is_none() && find(&first).is_some() => { help_one(console, &first); true }
        _ => false,
    }
}

/// `tests helpdoc` — the registry check.
pub fn selftest() {
    let (verbs, miss) = missing();
    let mut groups = 0usize;
    for g in GROUPS { if DOCS.iter().any(|x| x.group == *g) { groups += 1; } }
    let mut names = String::new();
    for m in &miss { if !names.is_empty() { names.push(','); } names.push_str(m); }
    let ok = miss.is_empty() && groups == GROUPS.len();
    serial_println!(":: HELPVERB: verbs={} documented={} missing=[{}] groups={} -> {} ::",
        verbs, verbs - miss.len(), names, groups, if ok { "PASS" } else { "FAIL" });
}

/// Register the `helpdoc` fixture exactly once (called from `tests::shell_verb` / `run`).
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) { crate::tests::register("helpdoc", selftest); }
}
