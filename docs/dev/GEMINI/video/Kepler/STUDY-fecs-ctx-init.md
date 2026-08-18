# STUDY: Kepler (GK107) FECS Context/Init Microcode

## 1. What does the real FECS context-switch ucode do at init?
According to the `envytools` documentation (`docs/hw/graph/fermi/ctxctl/intro.rst` and `rnndb/graph/gf100_pgraph/ctxctl.xml`), the FECS (Front End Context Switch) microcode operates in several phases:

*   **Self-Init:** The microcode initializes its own processor state, setting up interrupt routing (`INTR_ROUTE` `0x404`) and configuring the `CC_WATCHDOG` (`0x430`) to protect against hangs.
*   **PGRAPH Strand/State Init:** The ucode initializes the PGRAPH state via the `STRAND` registers (`0x900`-`0x93c`), managing context strand information. It issues initialization commands to the hardware via the FIFO command interface (`WRCMD_CMD` `0x504`), typically executing commands like `RESTORE_GOLDEN` (`0x15`) or `DISCOVER_IMAGE_SIZE` (`0x10`) to prime the pipeline default state.
*   **Host-Interface Mailbox Protocol:** The microcode waits for signals from the host (driver) through the `CC_SCRATCH` registers (`0x800`-`0x840`) and the `ENGINE_STATUS` / `ENGINE_TRIGGER` handshake (`0xc00`, `0xc08`). It synchronizes state via requests and acknowledgments (e.g., `DAEMON2CTXCTL_REQ`, `CTXCTL2DAEMON_ACK`).
*   **Context Load/Save Loop:** Once initialized, the ucode enters an idle loop awaiting PFIFO channel switch requests (`CSREQ` range, `CHAN_CUR` `0xb00`, `CHAN_NEXT` `0xb04`). When a switch occurs, the ucode commands PGRAPH to halt, saves the current context to VRAM, loads the new channel's context from VRAM, and triggers PGRAPH to resume (using `START_CTXSW` `0x39` and `STOP_CTXSW` `0x38` via `WRCMD_CMD`).

## 2. What is the FECS ↔ Host Handshake Surface?
The handshake surface between the host OS and the FECS microcode consists of:
*   **Mailbox Registers:** The `CC_SCRATCH` registers (starting at `0x800`), which act as general-purpose mailboxes for passing configuration parameters (like VRAM addresses of context buffers) and status codes between host and Falcon.
*   **Method Registers:** `WRCMD_DATA` (`0x500`) and `WRCMD_CMD` (`0x504`), where the host or Falcon can queue specific commands (e.g., `BIND_POINTER`, `HALT_PIPELINE`).
*   **Status & Trigger:** `ENGINE_STATUS` (`0xc00`) and `ENGINE_TRIGGER` (`0xc08`), which contain bits like `DAEMON2CTXCTL_REQ`, `CHAN_VALID`, and `CHSW_PENDING` to orchestrate context switch readiness.

**"A Context Exists" Concretely:**
At the register level, a context exists when the `CHAN_CUR` register (`0xb00` in the `CSREQ` range) contains a valid channel identifier, and the `CHAN_VALID` bit (bit 1) is asserted in `ENGINE_STATUS` (`0xc00`). This signals to PFIFO that the graphics pipeline is bound to a valid channel context in VRAM, allowing PFIFO to permit channel execution.

## 3. Minimal Hypothesis to Flip PFIFO Validation
PFIFO blocks execution (returning `err=2`, `stat=5`) because it observes that no context is bound in the PGRAPH engine. The smallest hypotheses to satisfy PFIFO's check, ordered by minimality:

*   **Hypothesis 1: `CHAN_CUR` (`0xb00`) dictates channel validity.**
    *   *Test:* Write the channel ID (and potentially a `VALID` high bit) directly to `0xb00` (and `0xb04` `CHAN_NEXT`), mimicking a context switch.
*   **Hypothesis 2: `ENGINE_STATUS` (`0xc00`) `CHAN_VALID` bit dictates validity.**
    *   *Test:* Write bit 1 (`CHAN_VALID`) to `0xc00`.
*   **Hypothesis 3: The `CC_SCRATCH` (`0x800`) / `ENGINE_TRIGGER` (`0xc08`) host handshake must complete.**
    *   *Test:* Write a handshake completion ack into `ENGINE_TRIGGER`.
*   **Hypothesis 4: `DMACTL` `REQUIRE_CTX` interacts with `CHAN_CUR`.**
    *   *Test:* With `REQUIRE_CTX` left at its default, populate `CHAN_CUR` and observe if the Falcon execution or PFIFO changes behavior.

## Ground Truth Recon Probe (PROBE)
To validate the reset values of this handshake surface, we will perform a read-only probe of the aforementioned registers at the FECS base (`0x409000`).
