/// Represents a hardware-agnostic network device that can transmit and receive raw frames.
pub trait NetworkDevice {
    /// Transmits a raw network frame over the device.
    fn transmit(&mut self, buffer: &[u8]);

    /// Receives a raw network frame from the device, if available.
    fn receive(&mut self) -> Option<&[u8]>;

    /// Returns the physical MAC address of the device.
    fn mac_address(&self) -> [u8; 6];
}
