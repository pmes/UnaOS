use bandy::SMessage;
use wgpu::*;

pub struct VisualCortex {
    device: Device,
    queue: Queue,
    surface: Surface<'static>,
    config: SurfaceConfiguration,
}

impl VisualCortex {
    /// Ignites the wgpu substrate. Panics if the hardware is unworthy.
    pub async fn ignite(window: std::sync::Arc<winit::window::Window>) -> Self {
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::VULKAN | Backends::METAL,
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
            .expect("Surface creation failed.");
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("No GPU found. Cortex dead.");

        let (device, queue) = adapter
            .request_device(&DeviceDescriptor::default(), None)
            .await
            .expect("Device request denied.");

        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter, size.width, size.height)
            .expect("Surface config unsupported.");
        surface.configure(&device, &config);

        Self {
            device,
            queue,
            surface,
            config,
        }
    }

    #[inline(always)]
    pub fn react(&mut self, msg: SMessage) {
        match msg {
            SMessage::EuclaseResize(w, h) => {
                self.config.width = w.max(1);
                self.config.height = h.max(1);
                self.surface.configure(&self.device, &self.config);
            }
            SMessage::VugPulse => self.render(),
            _ => {} // Ignore non-visual stimuli
        }
    }

    #[inline(always)]
    fn render(&self) {
        let frame = self.surface.get_current_texture().expect("Frame dropped.");
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Vug_Pulse"),
            });

        {
            let _rpass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Vug_Pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color::BLACK),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // Vug shader pipeline binds go here.
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}
