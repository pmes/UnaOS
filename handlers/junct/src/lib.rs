use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use resonance::{dsp::{Complex, FftContext}, BLOCK_SIZE};
use bandy::SMessage;
use tokio::sync::broadcast;

pub struct JunctHandler {
    _stream: cpal::Stream,
}

impl JunctHandler {
    pub fn new(bandy_tx: broadcast::Sender<SMessage>) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        // If no device, we warn but don't crash the whole app?
        // Logic says "Junct ... aggregate the host OS microphone".
        // If fail, we return Error.
        let device = host.default_input_device().ok_or_else(|| anyhow::anyhow!("No input device"))?;
        let config = device.default_input_config()?;
        let channels = config.channels() as usize;
        let config: cpal::StreamConfig = config.into();

        let fft = FftContext::new(BLOCK_SIZE);
        let mut buffer = vec![Complex::default(); BLOCK_SIZE];
        let mut buf_idx = 0;

        let err_fn = |err| eprintln!("an error occurred on stream: {}", err);

        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _: &_| {
                for frame in data.chunks(channels) {
                    let sample = if !frame.is_empty() { frame[0] } else { 0.0 };

                    if buf_idx < BLOCK_SIZE {
                         buffer[buf_idx] = Complex::new(sample, 0.0);
                         buf_idx += 1;
                    }

                    if buf_idx >= BLOCK_SIZE {
                        fft.process(&mut buffer);

                        let magnitude: Vec<f32> = buffer.iter()
                            .take(BLOCK_SIZE / 2)
                            .map(|c| (c.re * c.re + c.im * c.im).sqrt())
                            .collect();

                        let _ = bandy_tx.send(SMessage::Spectrum { magnitude });
                        buf_idx = 0;
                    }
                }
            },
            err_fn,
            None
        )?;

        stream.play()?;

        Ok(Self { _stream: stream })
    }
}
