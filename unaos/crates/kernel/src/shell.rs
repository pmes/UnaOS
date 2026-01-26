use alloc::vec::Vec;
use alloc::string::String;
use x86_64::instructions::port::Port;
use crate::console::Console;
use crate::vug;
use unaos_kernel::pal::TargetPal;

pub struct History {
    entries: Vec<String>,
    position: usize,
}

impl History {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            position: 0,
        }
    }

    pub fn push(&mut self, cmd: String) {
        if !cmd.trim().is_empty() {
            self.entries.push(cmd);
            self.position = self.entries.len();
        }
    }
}

pub fn dispatch_command(cmd_line: &str, console: &mut Console, pal: &mut TargetPal) {
    // Split command and args (simple whitespace split)
    let mut parts = cmd_line.trim().split_whitespace();
    let command = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();

    match command {
        "ver" | "version" => {
            console.println("unaOS v0.1.0 (Kernel: Jules 1 / Cortex: Jules 6)");
        },
        "help" => {
            console.println("COMMANDS: ver, help, clear, echo, panic, gneiss");
        },
        "clear" => {
            // Clear both the screen and the console buffer?
            // Usually 'clear' clears the visible screen.
            // For now, we will rely on console.draw() to repaint.
            // To effectively clear, we might want to clear the lines in console?
            // Or just clear screen. But draw() repaints lines.
            // Let's implement a 'clear' on console if needed, or just let draw handle it.
            // If the user wants a blank slate, we should probably clear the history buffer.
            // BUT, the prompt said "Reset cursor logic here".
            // Let's implement a clear method on Console.
            console.clear();
        },
        "echo" => {
            let content = args.join(" ");
            console.println(&content);
        },
        "panic" => {
            // Test the Exception Handler
            panic!("Manual Panic Requested by Architect!");
        },
        "gneiss" => {
             console.println("Gneiss is Home.");
        },
        "vug" => {
             if args.len() > 0 && args[0] == "bebox" {
                 console.println("Initializing GeekPort Simulation...");
                 vug::run_bebox_mode(pal);
             } else {
                 console.println("Initiating Vug: Standard Spectrum...");
                 vug::run_test_pattern(pal);
             }
        },
        "shutdown" | "off" => {
             console.println("Shutting down...");
             unsafe {
                 let mut port = Port::<u32>::new(0xf4);
                 port.write(0x10);
             }
             unaos_kernel::hlt_loop();
        },
        "" => {}, // Ignore empty enter
        _ => {
            console.println("Unknown command. Type 'help' for assistance.");
        }
    }
}
