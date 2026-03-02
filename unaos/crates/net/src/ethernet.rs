/// Represents the EtherType field of an Ethernet II frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtherType {
    Ipv4,
    Arp,
    Ipv6,
    Unknown(u16),
}

impl EtherType {
    pub fn from_u16(value: u16) -> Self {
        match value {
            0x0800 => EtherType::Ipv4,
            0x0806 => EtherType::Arp,
            0x86DD => EtherType::Ipv6,
            _ => EtherType::Unknown(value),
        }
    }
}

/// A zero-copy parser for an Ethernet II frame.
pub struct EthernetFrame<'a> {
    buffer: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    /// Creates a new EthernetFrame parser from a raw byte slice.
    /// Returns `None` if the buffer is too small to contain an Ethernet header.
    pub fn new(buffer: &'a [u8]) -> Option<Self> {
        // Minimum Ethernet II header size is 14 bytes:
        // 6 bytes (Dest MAC) + 6 bytes (Src MAC) + 2 bytes (EtherType)
        if buffer.len() < 14 {
            return None;
        }
        Some(Self { buffer })
    }

    /// The destination MAC address (bytes 0..6).
    pub fn destination_mac(&self) -> [u8; 6] {
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&self.buffer[0..6]);
        mac
    }

    /// The source MAC address (bytes 6..12).
    pub fn source_mac(&self) -> [u8; 6] {
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&self.buffer[6..12]);
        mac
    }

    /// The raw 16-bit EtherType field (bytes 12..14).
    pub fn ethertype_raw(&self) -> u16 {
        u16::from_be_bytes([self.buffer[12], self.buffer[13]])
    }

    /// The parsed EtherType.
    pub fn ethertype(&self) -> EtherType {
        EtherType::from_u16(self.ethertype_raw())
    }

    /// The payload of the Ethernet frame (everything after byte 14).
    pub fn payload(&self) -> &'a [u8] {
        &self.buffer[14..]
    }
}
