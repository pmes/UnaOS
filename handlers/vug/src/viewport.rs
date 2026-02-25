use euclase::cortex::Cortex;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VugVertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
}

pub struct VugViewport {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    num_vertices: u32,
}

impl VugViewport {
    pub fn forge(cortex: &Cortex) -> Self {
        let shader = cortex.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Vug_Wireframe_Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../libs/euclase/src/shaders/vug_wireframe.wgsl").into()),
        });

        let pipeline_layout = cortex.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Vug_Pipeline_Layout"),
            bind_group_layouts: &[], // Camera bind group goes here next
            push_constant_ranges: &[],
        });

        let pipeline = cortex.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Vug_Wireframe_Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<VugVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: cortex.config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList, // CAD Wireframe mode
                polygon_mode: wgpu::PolygonMode::Line,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        // A simple origin triad to prove the engine is alive.
        let vertices: &[VugVertex] = &[
            VugVertex { position: [0.0, 0.0, 0.0], color: [1.0, 0.0, 0.0] }, // X Axis
            VugVertex { position: [1.0, 0.0, 0.0], color: [1.0, 0.0, 0.0] },
            VugVertex { position: [0.0, 0.0, 0.0], color: [0.0, 1.0, 0.0] }, // Y Axis
            VugVertex { position: [0.0, 1.0, 0.0], color: [0.0, 1.0, 0.0] },
            VugVertex { position: [0.0, 0.0, 0.0], color: [0.0, 0.0, 1.0] }, // Z Axis
            VugVertex { position: [0.0, 0.0, 1.0], color: [0.0, 0.0, 1.0] },
        ];

        let vertex_buffer = cortex.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vug_Geometry_Buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self {
            pipeline,
            vertex_buffer,
            num_vertices: vertices.len() as u32,
        }
    }

    pub fn render(&self, cortex: &Cortex) -> Result<(), wgpu::SurfaceError> {
        let output = cortex.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = cortex.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Vug_Render_Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Vug_Render_Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.01, g: 0.01, b: 0.02, a: 1.0 }), // Deep Void Blue
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            render_pass.draw(0..self.num_vertices, 0..1);
        }

        cortex.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}
