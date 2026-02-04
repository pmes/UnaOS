# Architectural Decision Record: Why Rust?

**Context:** unaOS requires high-performance multithreading (BeOS-style), military-grade security (The Immune System), and maximum energy efficiency (The Ecology).

## 1. The Historical Context (The C++ Problem)
Historically, operating systems like BeOS, Windows, and macOS were written in C and C++. While powerful, these languages place the burden of memory safety entirely on the programmer.
* **The Flaw:** A single mistake in pointer arithmetic can crash the entire system (Blue Screen/Kernel Panic) or create a security vulnerability.
* **The Statistic:** Microsoft and Google admit that ~70% of all serious security bugs in their products are memory safety errors.

## 2. The Solution: Rust

We have selected **Rust** as the primary language for unaOS. This is not a stylistic preference; it is a structural necessity to achieve our three core goals.

### A. Solving "The BeOS Problem" (Concurrency)
**Goal:** We want "Pervasive Multithreading"—thousands of threads running simultaneously to keep the UI responsive.
* **The Trap:** In C++, sharing data between threads leads to "Race Conditions" (random, unreproducible crashes).
* **The Rust Fix:** Rust's **Ownership Model** mathematically guarantees thread safety at compile time. The compiler literally forbids code where two threads fight over data. We can build the "Hive" without it collapsing.

### B. The "Immune System" (Security)
**Goal:** "Super high security, but not hindering."
* **The Trap:** Traditional security relies on antivirus software scanning for known threats.
* **The Rust Fix:** Rust provides **Memory Safety** by default. It eliminates entire classes of attacks (buffer overflows, dangling pointers) before the code even runs. The OS is secure by physics, not by policing.

### C. The "Eco Factor" (Efficiency)
**Goal:** "Efficiency is Ecology."
* **The Trap:** Languages like Java or Python use "Garbage Collection," wasting CPU cycles and battery life to manage memory in the background.
* **The Rust Fix:** Rust uses **Zero-Cost Abstractions**. It runs as fast as raw Assembly, with no background garbage collector. Every electron is used for computation, extending the life of old hardware.

## 3. Comparative Evidence

To illustrate the difference, here is how we handle a persistent process in the kernel.

**The Old Way (C++) - Risky**
```cpp
// DANGER: If 'process' is deleted while thread is running, CRASH!
void spawn_process(Process* p) {
    create_thread([p] {
        p->run(); // What if 'p' is null now? Undefined Behavior.
    });
}

**The New Way (Rust) - Safe**
```rust
// SAFE: The compiler forces us to handle the memory lifespan.
fn spawn_process(process: Arc<Process>) {
    // 'Arc' (Atomic Reference Count) ensures the process
    // CANNOT be deleted from memory until this thread is done.
    thread::spawn(move || {
        process.run();
    });
}
