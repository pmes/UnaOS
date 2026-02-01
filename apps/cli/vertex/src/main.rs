use clap::Parser;
use serde::Serialize;
use std::net::UdpSocket;
use std::process;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// The Shard ID (e.g., s9-mule)
    id: String,

    /// The Status or Color (e.g., Online, green)
    status: String,
}

#[derive(Serialize)]
struct Packet {
    id: String,
    status: String,
}

fn map_status(input: &str) -> String {
    let lower = input.to_lowercase();
    match lower.as_str() {
        "online" | "green" => "Online",
        "oncall" | "teal" => "OnCall",
        "active" | "seafoam" => "Active",
        "thinking" | "purple" => "Thinking",
        "paused" | "yellow" => "Paused",
        "error" | "red" => "Error",
        "offline" | "grey" => "Offline",
        _ => {
            // If unknown, pass it through capitalizing the first letter just in case,
            // or return as is if the user knows what they are doing.
            // But let's try to be smart and return Title Case if possible, or just the input.
            // Given the strict enum on the receiving end, maybe we should warn?
            // For now, let's just return the input capitalized if it matches nothing known,
            // hoping it's a valid variant we missed or just pass it through.
            // Actually, let's just return the input as Title Case to match typical Enum variants if they typed "online" but meant the enum.
            // But wait, if they typed "FOO", "Foo" might be wrong.
            // Let's strictly map known ones and pass others as-is but with first letter capitalized?
            // The prompt implies I should support the mapping.
            // Let's just return the input if no match, maybe they are sending a new status the CLI doesn't know about yet but the backend does.
            // But for "online" (lowercase), we want "Online".
            // So let's handle the capitalization for them if it matches a known one (covered above).
            // If it's unknown, we pass it through.
            input
        }
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_status() {
        assert_eq!(map_status("green"), "Online");
        assert_eq!(map_status("Online"), "Online");
        assert_eq!(map_status("online"), "Online");
        assert_eq!(map_status("teal"), "OnCall");
        assert_eq!(map_status("purple"), "Thinking");
        assert_eq!(map_status("red"), "Error");
        assert_eq!(map_status("Unknown"), "Unknown");
    }
}

fn main() {
    let args = Cli::parse();

    let final_status = map_status(&args.status);
    let payload = Packet {
        id: args.id.clone(),
        status: final_status.clone(),
    };

    let json = serde_json::to_string(&payload).unwrap_or_else(|e| {
        eprintln!("Failed to serialize payload: {}", e);
        process::exit(1);
    });

    // Bind to 0.0.0.0:0 to let OS pick a random port
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap_or_else(|e| {
        eprintln!("Failed to bind UDP socket: {}", e);
        process::exit(1);
    });

    let target = "127.0.0.1:4200";
    match socket.send_to(json.as_bytes(), target) {
        Ok(_) => {
            println!("Vertex Signal Fired.");
            println!("Target: {}", target);
            println!("Payload: {{ id: \"{}\", status: \"{}\" }}", args.id, final_status);
        }
        Err(e) => {
            eprintln!("Failed to send packet: {}", e);
            process::exit(1);
        }
    }
}
