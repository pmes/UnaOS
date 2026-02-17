use resonance::{AudioGraph, BLOCK_SIZE, Gain, Mixer, Sample, SineOscillator};

#[test]
fn test_graph_chain_mixer() {
    let sample_rate = 44100.0;
    let mut graph = AudioGraph::new(sample_rate);

    // 1. Create Nodes
    // Osc 1: 440 Hz
    let osc1 = Box::new(SineOscillator::new(440.0));
    let osc1_id = graph.add_node(osc1);

    // Osc 2: 444 Hz (Detuned)
    let osc2 = Box::new(SineOscillator::new(444.0));
    let osc2_id = graph.add_node(osc2);

    // Mixer
    let mixer = Box::new(Mixer::new());
    let mixer_id = graph.add_node(mixer);

    // 2. Connect
    // Osc1 -> Mixer Input 0
    graph.connect(osc1_id, mixer_id, 0);
    // Osc2 -> Mixer Input 1
    graph.connect(osc2_id, mixer_id, 1);

    // 3. Process
    let output_block = graph.process();

    // 4. Verify
    // The output should be the sum of two sines.
    // Check for signal presence.
    let mut has_signal = false;
    for &sample in output_block {
        if sample.abs() > 1e-6 {
            has_signal = true;
            break;
        }
    }
    assert!(has_signal, "Output block should not be silent");
}

#[test]
fn test_graph_chain_gain() {
    let sample_rate = 44100.0;
    let mut graph = AudioGraph::new(sample_rate);

    // Osc: 440 Hz
    let osc = Box::new(SineOscillator::new(440.0));
    let osc_id = graph.add_node(osc);

    // Gain: 0.5
    let gain = Box::new(Gain::new(0.5));
    let gain_id = graph.add_node(gain);

    // Osc -> Gain Input 0
    graph.connect(osc_id, gain_id, 0);

    // Process
    let output_block = graph.process();

    // Verify manually that output is ~0.5 * sin(...)
    // First sample of sine is 0.0, second is sin(TAU * 440/44100)
    // We can't check sample 0 easily (it's 0), check sample 1.
    let expected_sample_1 = (std::f64::consts::TAU * 440.0 / 44100.0).sin() * 0.5;
    let actual_sample_1 = output_block[1];

    assert!(
        (actual_sample_1 - expected_sample_1).abs() < 1e-6,
        "Gain output mismatch: expected {}, got {}",
        expected_sample_1,
        actual_sample_1
    );
}

#[test]
#[should_panic(expected = "Invalid node ID")]
fn test_invalid_connection_panic() {
    let mut graph = AudioGraph::new(44100.0);
    let osc = Box::new(SineOscillator::new(440.0));
    let id = graph.add_node(osc);

    // Try to connect a non-existent node
    use resonance::NodeId;
    let bad_id = NodeId(999);
    graph.connect(id, bad_id, 0);
}
