/// A zero-copy parser for an IPv4 header.
pub struct Ipv4Header<'a> {
    buffer: &'a [u8],
}

impl<'a> Ipv4Header<'a> {
    /// Creates a new Ipv4Header parser from a raw byte slice.
    /// Returns `None` if the buffer is too small to contain a minimal IPv4 header.
    pub fn new(buffer: &'a [u8]) -> Option<Self> {
        // Minimum IPv4 header length is 20 bytes.
        if buffer.len() < 20 {
            return None;
        }

        // Check the version field (first 4 bits of the first byte)
        let version = buffer[0] >> 4;
        if version != 4 {
            return None;
        }

        let ihl = buffer[0] & 0x0F;
        let header_length = (ihl as usize) * 4;

        // Ensure we have enough bytes for the reported header length
        if buffer.len() < header_length {
            return None;
        }

        Some(Self { buffer })
    }

    /// Header length in bytes, derived from the IHL field.
    pub fn header_length(&self) -> usize {
        let ihl = self.buffer[0] & 0x0F;
        (ihl as usize) * 4
    }

    /// Total length of the IPv4 packet (header + payload), in bytes.
    pub fn total_length(&self) -> u16 {
        u16::from_be_bytes([self.buffer[2], self.buffer[3]])
    }

    /// Protocol field (e.g., TCP, UDP, ICMP).
    pub fn protocol(&self) -> u8 {
        self.buffer[9]
    }

    /// Source IPv4 address.
    pub fn source_ip(&self) -> [u8; 4] {
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&self.buffer[12..16]);
        ip
    }

    /// Destination IPv4 address.
    pub fn destination_ip(&self) -> [u8; 4] {
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&self.buffer[16..20]);
        ip
    }

    /// Computes the 16-bit IPv4 header checksum natively.
    /// Returns true if the checksum is valid.
    pub fn verify_checksum(&self) -> bool {
        let mut sum: u32 = 0;
        let hlen = self.header_length();

        for i in (0..hlen).step_by(2) {
            let word = u16::from_be_bytes([self.buffer[i], self.buffer[i + 1]]);
            sum += word as u32;
        }

        // Add back carry outs from top 16 bits to low 16 bits
        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }

        // The checksum should evaluate to 0xFFFF before inverting.
        // Therefore, taking the 1's complement of the sum should yield 0.
        (!sum as u16) == 0
    }

    /// The payload of the IPv4 packet.
    pub fn payload(&self) -> &'a [u8] {
        let header_len = self.header_length();
        let total_len = self.total_length() as usize;

        // Ensure bounds are safe to access
        let end_idx = total_len.min(self.buffer.len());

        if header_len > end_idx {
            &[]
        } else {
            &self.buffer[header_len..end_idx]
        }
    }
}
