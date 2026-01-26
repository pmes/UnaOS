# The Immune System: Behavioral Intrusion Detection

## 1. The Biological Metaphor
Traditional Antivirus (AV) is flawed because it relies on a "blacklist" of known viruses. If a virus is new (Zero-Day), the AV is blind.
* **unaOS Approach:** We define "Healthy Behavior." Anything that deviates from the healthy baseline is treated as an infection.

## 2. The "Self" vs. "Non-Self" Check
* **Code Signing:** Every binary, library, and script that is part of the base OS is cryptographically signed by the **unaOS Root Authority**.
* **The Check:** When a program loads, the kernel checks its signature.
    * **Signed (Self):** Allowed to execute.
    * **Unsigned (Non-Self):** immediately sandboxed. It cannot touch the network or disk until the user explicitly creates a rule for it.

## 3. Anomaly Heuristics (The White Blood Cells)
The kernel monitors process behavior in real-time using low-overhead counters.
* **Red Flags:**
    * A text editor trying to open a network socket. (Why?)
    * A calculator app trying to scan the hard drive. (Why?)
    * A background process using 100% GPU. (Crypto-mining?)
* **Reaction:** The kernel pauses the process *instantly* and prompts the user: *"Calculator is attempting suspicious behavior. Kill it?"*

## 4. Ecology Benefit
Because we don't need to constantly scan the hard drive for "virus signatures," we save massive amounts of I/O and battery life.
