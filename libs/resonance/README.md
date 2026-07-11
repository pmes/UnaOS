# resonance

Real-time audio engine and DSP library for UnaOS userspace.

## What it does

Resonance is a node-based audio processing library. It builds a directed graph of
DSP nodes, runs that graph at audio rate inside a real-time callback, and drives a
host output device through [`cpal`](https://crates.io/crates/cpal). Audio is
processed in fixed-size blocks of `BLOCK_SIZE` (64) samples; the internal sample
type is `f64` (`Sample`).

Responsibilities:

- **Graph construction and evaluation** — own a set of processing nodes, wire their
  outputs to inputs, and evaluate the whole graph one block at a time.
- **Real-time device output** — initialise the default host output device, run the
  graph from the audio callback, and convert the engine's `f64` samples to the
  device's `f32` frames.
- **Lock-free control** — accept parameter changes from another thread over a
  bounded ring buffer (`ringbuf`), so the audio thread never blocks or allocates.
- **Frequency-domain primitives** — a small radix-2 FFT for spectral work.

## Key types and entry points

- **`AudioGraph`** (`graph.rs`) — owns the nodes, their connections, and per-node
  output buffers. `add_node` inserts a boxed node and returns a `NodeId`; `connect`
  wires a source node's output into a destination input port; `process` evaluates
  every node in insertion order (assumed topological) and returns the last node's
  output block. Inputs are resolved into a stack-allocated array of references, so
  `process` performs no heap allocation.
- **`AudioNode`** trait (`core.rs`) — the contract every processing node implements:
  `process(inputs, outputs, context)` over `[Sample; BLOCK_SIZE]` buffers, plus an
  optional `set_param(id, value)`. `GraphContext` carries the sample rate (and its
  reciprocal) into each node.
- **`AudioEngine`** (`audio.rs`) — `AudioEngine::new(graph)` moves the graph into a
  `cpal` output stream and returns the engine together with a producer handle for
  sending commands. The engine must be kept alive to keep audio running.
- **`AudioCommand`** (`commands.rs`) — the messages the control thread sends to the
  audio thread: `SetParam { node_id, param_id, value }`, `SetMasterFrequency`, and
  `Stop`.
- **Built-in nodes** (`nodes/`) — `SineOscillator` (with optional FM input via
  input port 0), `Gain` (a VCA with optional modulation input), and `Mixer`
  (summing).
- **`FftContext`** (`dsp.rs`) — a precomputed-twiddle, in-place Cooley-Tukey FFT
  over `Complex` buffers whose length is a power of two.
- **`create_test_graph`** — a convenience that builds a 440 Hz sine into a 0.1 gain,
  useful for bring-up.

## How it fits into UnaOS

Resonance is the `libs/` crate that implements the audio engine and DSP graph
described as `src/dsp` in [`docs/CODEX.md`](../../docs/CODEX.md). It is the audio
backbone the **Stria** handler ("The Studio") is intended to build on. The engine
also implements `bandy`'s `BandyMember` trait: `AudioEngine::process_frame`
packages processed audio as an `SMessage::AudioChunk` and publishes it to the
in-process message bus, so audio can flow to other handlers and vessels over the
Synapse. See [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
for the vessel/handler/library model.

## Status

Partial — honest prototype. The graph evaluation, the built-in oscillator/gain/mixer
nodes, the `cpal` output path, and the FFT are implemented and unit-tested. The
engine re-tunes the graph to the real device sample rate at start (a 440 Hz patch
plays true 440 Hz on a 48 kHz device), and the command path is live end to end:
frequency, arbitrary node params, and stop/start all reach the audio thread through
the lock-free ring, with a per-block peak level readable from the control side via
`ResonanceHandle`/`ResonanceMeter`. `BandyMember::publish` and `process_frame`
remain dead code — `publish` only logs, nothing calls `process_frame`; wiring the
engine to the Synapse for real is the stria handler arc (AV-A2). Graph evaluation
requires nodes to be added in topological order and supports only forward
(non-feedback) connections.
