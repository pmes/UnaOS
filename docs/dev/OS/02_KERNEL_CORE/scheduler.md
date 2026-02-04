# The "Hive" Scheduler: Pervasive Multithreading

## 1. Philosophy
The User Interface is the highest authority. A mouse cursor that stutters is a failure of the operating system.
* **Rule 1:** The GUI thread is sacred. It is preemptive and has a higher base priority than any background compilation or download.

## 2. Threading Model: 1:1 Preemptive
We map one User Thread to one Kernel Thread.
* **Why not Green Threads?** While Go/Erlang use "Green Threads" (M:N), an OS needs deterministic hardware control.
* **Optimization:** Our `ThreadControlBlock` (TCB) is minimal. Context switching is optimized to be lighter than Linux, allowing thousands of threads to exist without thrashing.

## 3. Priority Classes
The Scheduler sorts threads into four strict bands (The "Caste System"):
1.  **Real-Time (Audio/Hardware):** Must run *now*. (Missed deadline = audio glitch).
2.  **Interactive (UI/Input):** Must run within 16ms (60fps).
3.  **Normal (App Logic):** Standard timeslices.
4.  **Idle / Scavenger (Indexing/Updates):** Runs only when the CPU is cold. "Efficiency is Ecology."
