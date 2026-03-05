use std::sync::Arc;
use wgpu::{
    Backends, Device, DeviceDescriptor, Features, Instance, InstanceDescriptor, Limits,
    PowerPreference, Queue, RequestAdapterOptions, Surface,
};

pub struct Cortex<'a> {
    pub instance: Instance,
    pub surface: Surface<'a>,
    pub device: Arc<Device>,
    pub queue: Arc<Queue>,
    pub config: wgpu::SurfaceConfiguration,
}

impl<'a> Cortex<'a> {
    /// Ignites the visual cortex. Binds to the Quartzite-provided window.
    pub async fn ignite(
        window: impl Into<wgpu::SurfaceTarget<'a>>,
        width: u32,
        height: u32,
    ) -> Self {
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::VULKAN | Backends::METAL, // Legacy is dead to us.
            ..Default::default()
        });

        let surface = instance
            .create_surface(window)
            .expect("Surface creation failed. Quartzite betrayed us.");

        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .expect("Silicon rejected the adapter request. Engine stalled.");

        // We demand PolygonMode::Line for Vug's wireframes.
        let (device, queue) = adapter
            .request_device(
                &DeviceDescriptor {
                    label: Some("Euclase_Primary_Cortex"),
                    required_features: Features::POLYGON_MODE_LINE,
                    required_limits: Limits::default(),
                    ..Default::default()
                }
            )
            .await
            .expect("Device request failed. Insufficient GPU authority.");

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::AutoNoVsync, // Tear the screen if you have to. Go fast.
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(&device, &config);

        Self {
            instance,
            surface,
            device: Arc::new(device),
            queue: Arc::new(queue),
            config,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }
}
