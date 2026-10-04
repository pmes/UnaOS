# Holocron — Secrets ("The Key")

CODEX §2: keyring, SSH agent, wallet, biometric auth. HOLOCRON1 (LEDGER SR33) built the keyring, its bus
surface, the SSH agent and the first consumer (Vein's Claude API key). Design, formats, oracles and the honest
ceiling: [`docs/dev/evidence/host-1004/HOLOCRON1.md`](../../docs/dev/evidence/host-1004/HOLOCRON1.md).

> **Until CRYPTOCORE (SR27) folds, the host build seals with the TEST suite (0xFE), which is not
> cryptography.** The daemon says so on every start. `--features crypto_core` is the production suite.

## Layout

* `unaos/libs/sys/holocron_core` — `no_std`, zero dependencies: formats, ring, bus codec, dispatcher, agent
  framing, consumer rule. Shared with the metal.
* this crate — the host's I/O: `store` (`~/.holocron`), `unafs_store` (typed attributes), `principal`
  (SO_PEERCRED), `daemon` (bus + agent sockets), `client`.
* `tools/holocron` — the CLI.

## Use

```sh
holocron daemon &                       # bus: ~/.holocron/.bus.sock · agent: ~/.holocron/.agent.sock
holocron init                           # the ring password (twice)
holocron put vein claude.api_key --kind api-key --label Claude < keyfile
holocron keygen id_ed25519 --label me@host
eval "$(holocron agent-env)"; ssh-add -l
holocron lock
```

## Bus verbs (144..=151)

`SecretGet(ns, name)`, `SecretPut(ns, name, kind, label, data)`, `SecretList(ns)`, `SecretDelete(ns, name)`,
`Unlock(create?, password)`, `Lock`, `Sign(key, data)`, `Status`. Answered only to the ring's owner principal;
unlock is rate-limited. Secrets never ride the `bandy` Synapse (a broadcast channel with no principal).
