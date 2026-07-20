use boa_engine::{Context, Source};
use boa_engine::{Finalize, Trace};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

pub struct Worker {
    pub sender: Sender<String>,
}

#[derive(Debug, Trace, Finalize)]
pub struct WorkerData {
    pub id: u32,
}

impl boa_engine::JsData for WorkerData {}

pub fn spawn_worker() -> (Worker, Receiver<String>) {
    let (tx_in, rx_in) = mpsc::channel::<String>();
    let (tx_out, rx_out) = mpsc::channel::<String>();

    thread::spawn(move || {
        let mut context = Context::default();
        
        while let Ok(msg) = rx_in.recv() {
            let source = Source::from_bytes(msg.as_bytes());
            if let Ok(res) = context.eval(source) {
                if let Ok(res_str) = res.to_string(&mut context) {
                    let _ = tx_out.send(res_str.to_std_string_escaped());
                }
            } else {
                let _ = tx_out.send("Error".to_string());
            }
        }
    });

    (Worker { sender: tx_in }, rx_out)
}
