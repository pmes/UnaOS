use resonance::{AudioNode, BLOCK_SIZE, Gain, GraphContext, Mixer, Sample, SineOscillator};

#[test]
fn test_osc_interference() {
    // 1. Setup Context
    let sample_rate = 44100.0;
    let context = GraphContext::new(sample_rate);

    // 2. Instantiate Nodes
    let mut osc1 = SineOscillator::new(440.0); // A4
    let mut osc2 = SineOscillator::new(444.0); // 4Hz detune (beating)
    let mut mixer = Mixer::new();

    // 3. Prepare Buffers
    // We need buffers for the output of each node.
    let mut osc1_out = [0.0; BLOCK_SIZE];
    let mut osc2_out = [0.0; BLOCK_SIZE];
    let mut mix_out = [0.0; BLOCK_SIZE];

    // 4. Process Loop (Simulate one block)
    // Run Osc1
    osc1.process(&[], &mut [&mut osc1_out], &context);

    // Run Osc2
    osc2.process(&[], &mut [&mut osc2_out], &context);

    // Run Mixer (Inputs are Osc1 and Osc2 outputs)
    let mixer_inputs: &[&[Sample; BLOCK_SIZE]] = &[&osc1_out, &osc2_out];
    mixer.process(mixer_inputs, &mut [&mut mix_out], &context);

    // 5. Assertions
    // We expect the mixer output to be the sum of the two oscillators.
    for i in 0..BLOCK_SIZE {
        let expected = osc1_out[i] + osc2_out[i];
        assert!(
            (mix_out[i] - expected).abs() < 1e-6,
            "Mixer output mismatch at index {}",
            i
        );
    }

    // Verify it's not silence (unless we are extremely unlucky and they cancel out perfectly at t=0, which they won't for these freqs/phases)
    let mut has_signal = false;
    for i in 0..BLOCK_SIZE {
        if mix_out[i].abs() > 1e-6 {
            has_signal = true;
            break;
        }
    }
    assert!(has_signal, "Output should not be silent");
}

#[test]
fn test_gain_envelope_simulation() {
    // 1. Setup Context
    let sample_rate = 44100.0;
    let context = GraphContext::new(sample_rate);

    // 2. Instantiate Nodes
    let mut osc = SineOscillator::new(440.0);
    let mut gain = Gain::new(0.0); // Base gain 0 (controlled by envelope)

    // 3. Prepare Buffers
    let mut osc_out = [0.0; BLOCK_SIZE];
    let mut env_out = [1.0; BLOCK_SIZE]; // Simulated envelope signal (constant 1.0 for this test block)
    let mut final_out = [0.0; BLOCK_SIZE];

    // 4. Process Loop
    // Run Osc
    osc.process(&[], &mut [&mut osc_out], &context);

    // Run Gain
    // Input 0: Audio (Osc Output)
    // Input 1: Control (Envelope)
    let gain_inputs: &[&[Sample; BLOCK_SIZE]] = &[&osc_out, &env_out];
    gain.process(gain_inputs, &mut [&mut final_out], &context);

    // 5. Assertions
    // Since envelope is 1.0 and base gain is 0.0, effective gain is 1.0.
    // Output should match oscillator output.
    for i in 0..BLOCK_SIZE {
        assert!(
            (final_out[i] - osc_out[i]).abs() < 1e-6,
            "Gain output mismatch at index {}",
            i
        );
    }
}
