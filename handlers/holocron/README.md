# Holocron — Secrets and Identity Handler

**Status: design-stage (not yet implemented).** This directory currently contains
only this design document; there is no crate, entry point, or working code yet.

Holocron is the UnaOS handler responsible for **secrets management and identity**:
a keyring for passwords, SSH keys, API tokens, and signing identities, together
with the authentication agents that present those credentials to other
components. It is the planned replacement for the role filled today by tools such
as 1Password, the system keychain, `ssh-agent`, and `gpg-agent`.

Like every UnaOS handler, Holocron is intended to be a self-contained domain
service crate that exposes an async entry point (by convention `ignite(...)`),
subscribes to the message bus, and reacts to messages — it does not call other
handlers directly. See [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
for the handler/vessel model and [`docs/CODEX.md`](../../docs/CODEX.md) for the
full handler manifest.

## Planned responsibilities

- **Vault** — a single encrypted store for passwords, SSH keys, API tokens, and
  signing identities, with hardware-backed protection (TPM / Secure Enclave)
  where available and a software cryptography backend otherwise.
- **Memory hygiene** — secret material is zeroized as soon as it is no longer
  needed and is never written to disk in plaintext.
- **Unified agent** — one unlock action makes SSH, signing, and web credentials
  available for the session, replacing the separate `ssh-agent` / `gpg-agent`
  daemons.
- **Context-aware authorization** — when another handler requests a credential
  (for example a shell `sudo` from the Midden handler, or a Git push from the
  Vairë handler), Holocron prompts for explicit confirmation out-of-band rather
  than releasing keys automatically.
- **Key lifecycle** — generation of modern keys (e.g. Ed25519) without raw
  OpenSSL invocations, plus policy-driven rotation reminders.
- **Credential injection** — supplies credentials to consumers without exposing
  the underlying secret (e.g. bounded-lifetime clipboard entries, form fill via
  the web handler) so the requesting component never sees the raw store.

## Integration with the Synapse / SMessage bus

Holocron is planned to follow the standard handler contract defined by `bandy`:

- It subscribes to the **Synapse** (the broadcast message bus) and reacts to
  **`SMessage`** variants rather than being invoked directly.
- Credential requests from other handlers and authorization
  prompts/results are expected to be modeled as dedicated `SMessage` variants.
  Adding such variants is a deliberate, reviewed change to the shared `SMessage`
  enum and is **not yet defined**.

## Scope notes

This document describes intended behavior only. None of the cryptographic
storage, agent, or bus integration described above exists in code yet; the
specific `SMessage` variants, crate layout, and `ignite(...)` signature will be
defined when implementation begins.
