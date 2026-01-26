
#[derive(Debug, Clone, Copy)]
#[repr(C, align(64))]
pub struct DeviceContext {
    pub slot: [u32; 8],      // Slot Context (32 bytes)
    pub ep0:  [u32; 8],      // Endpoint 0 Context (32 bytes)
    pub eps:  [[u32; 8]; 30] // Endpoints 1-30 (30 * 32 bytes)
}

impl DeviceContext {
    pub const fn new() -> Self {
        Self {
            slot: [0; 8],
            ep0: [0; 8],
            eps: [[0; 8]; 30],
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C, align(64))]
pub struct InputContext {
    pub control: [u32; 8],   // Input Control Context (32 bytes)
    pub device:  DeviceContext
}

impl InputContext {
    pub const fn new() -> Self {
        Self {
            control: [0; 8],
            device: DeviceContext::new(),
        }
    }
}
