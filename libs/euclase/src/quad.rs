// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! The textured quad — euclase's presentation layer.
//!
//! One static frame (tightly packed 8-bit sRGB RGBA, top row first) uploaded
//! as an [`wgpu::TextureFormat::Rgba8UnormSrgb`] texture and drawn as a single
//! quad under a [`Mat4`] transform. This is what an image viewer *is* on a
//! GPU: `facet` uploads its packed frame once and re-draws the quad on demand
//! (input events + resize), with zoom/pan folded into the uniform matrix.
//!
//! # Coordinate conventions (the row-order seam)
//!
//! * **Quad space**: the unit square `(0,0)..(1,1)`, `(0,0)` at the image's
//!   *bottom*-left — matching the unflipped AppKit view convention the facet
//!   CPU path established (never `isFlipped` a view drawing a picture).
//! * **Texcoords** are top-down (`v=0` = first uploaded texel row = image top
//!   row); the shader flips `v = 1 - y`, so a right-side-up quad shows a
//!   right-side-up picture.
//! * [`view_rect_to_ndc`] maps a bottom-left-origin dest rect (view points)
//!   into clip space; NDC's y-up matches the unflipped view's y-up, so there
//!   is no hidden flip anywhere else.
//!
//! Color: the texture is sRGB (`Rgba8UnormSrgb`), so sampling decodes to
//! linear; rendered into an sRGB swapchain the hardware re-encodes on store —
//! byte-preserving for an untransformed frame, and filtering (when zoomed)
//! happens in linear light.

use crate::cortex::Cortex;
use crate::mat4::Mat4;
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// The one uniform: quad space → clip space, column-major (WGSL `mat4x4<f32>`;
/// [`Mat4`] is `repr(C)` column-major, so it uploads verbatim).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct QuadUniform {
    transform: Mat4,
}

/// A textured-quad presenter bound to one device + target format.
///
/// Built once per surface ([`TexturedQuad::forge`]), fed frames with
/// [`upload_frame`](TexturedQuad::upload_frame), positioned with
/// [`set_transform`](TexturedQuad::set_transform), and drawn with
/// [`render`](TexturedQuad::render) (swapchain) or
/// [`render_to_view`](TexturedQuad::render_to_view) (any color target — this
/// is also the headless-test seam).
pub struct TexturedQuad {
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    /// Rebuilt on every `upload_frame` (the texture view changes).
    bind_group: Option<wgpu::BindGroup>,
    /// Pixel size of the current frame, `(0, 0)` until one is uploaded.
    frame_size: (u32, u32),
}

impl TexturedQuad {
    /// Build the pipeline for a given color target format.
    ///
    /// `filter` is the min/mag sampling mode. `Linear` is the default choice
    /// (closest to AppKit's CPU-blit look); `Nearest` shows square texels at
    /// high zoom — an open eye-witness question at MAX_ZOOM=64.
    pub fn forge(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        filter: wgpu::FilterMode,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Facet_Quad_Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/facet_quad.wgsl").into()),
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Facet_Quad_Bind_Layout"),
            entries: &[
                // @binding(0): the Mat4 transform uniform.
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            core::mem::size_of::<QuadUniform>() as u64,
                        ),
                    },
                    count: None,
                },
                // @binding(1): the frame texture.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // @binding(2): the sampler.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Facet_Quad_Pipeline_Layout"),
            bind_group_layouts: &[&bind_layout],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Facet_Quad_Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[], // The quad is vertex-index-generated in the shader.
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None, // The frame is opaque; the clear is the field.
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Facet_Quad_Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: filter,
            min_filter: filter,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest, // No mips: one static frame.
            ..Default::default()
        });

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Facet_Quad_Uniform"),
            contents: bytemuck::bytes_of(&QuadUniform {
                transform: Mat4::identity(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        Self {
            pipeline,
            bind_layout,
            sampler,
            uniform_buf,
            bind_group: None,
            frame_size: (0, 0),
        }
    }

    /// Upload one frame: tightly packed 8-bit **sRGB** RGBA
    /// (`width * height * 4` bytes, row-major, top row first — exactly the
    /// buffer facet's `pack_srgba` produces). Uploaded as `Rgba8UnormSrgb`,
    /// so the packed bytes land texel-exact and sampling decodes them to
    /// linear for the color-managed render path.
    pub fn upload_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) {
        let expected = width as usize * height as usize * 4;
        if width == 0 || height == 0 || rgba.len() < expected {
            log::warn!(
                "[EUCLASE] upload_frame: bad buffer ({} bytes for {}x{}, need {})",
                rgba.len(),
                width,
                height,
                expected
            );
            return;
        }

        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Facet_Quad_Frame"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba[..expected],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Facet_Quad_Bind_Group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }));
        self.frame_size = (width, height);
    }

    /// Pixel size of the current frame, `(0, 0)` until one is uploaded.
    pub fn frame_size(&self) -> (u32, u32) {
        self.frame_size
    }

    /// Write the quad-space → clip-space transform into the uniform.
    pub fn set_transform(&self, queue: &wgpu::Queue, transform: &Mat4) {
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&QuadUniform {
                transform: *transform,
            }),
        );
    }

    /// Draw the quad into an arbitrary color target view over `clear`.
    ///
    /// This is the render core [`render`](Self::render) wraps for the
    /// swapchain, and the seam the headless tests exercise (render into an
    /// offscreen texture, no surface required). A no-frame quad clears only.
    pub fn render_to_view(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        clear: wgpu::Color,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Facet_Quad_Encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Facet_Quad_Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(bind_group) = &self.bind_group {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..4, 0..1); // The vertex-index-generated strip.
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Draw the quad into the cortex's current swapchain texture and present.
    pub fn render(&self, cortex: &Cortex, clear: wgpu::Color) -> Result<(), wgpu::SurfaceError> {
        let output = cortex.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.render_to_view(&cortex.device, &cortex.queue, &view, clear);
        output.present();
        Ok(())
    }
}

/// Map a dest rect in view points (bottom-left origin, y-up — the unflipped
/// AppKit convention) onto NDC clip space, as the quad's transform.
///
/// `dest` is `(dx, dy, dw, dh)` — where the image should land inside a view
/// whose bounds size is `bounds` — exactly the tuple facet's `image_dest`
/// zoom/pan math produces. Quad space `(0,0)..(1,1)` lands on that rect:
/// view point `p = dest.xy + q * dest.wh`, then NDC `n = 2p/bounds - 1`.
/// Both spaces are y-up, so no flip appears here (the only flip in the whole
/// path is the texcoord `v = 1 - y` in the shader).
pub fn view_rect_to_ndc(dest: (f64, f64, f64, f64), bounds: (f64, f64)) -> Mat4 {
    let (dx, dy, dw, dh) = dest;
    let (bw, bh) = (bounds.0.max(1.0), bounds.1.max(1.0));
    let sx = (2.0 * dw / bw) as f32;
    let sy = (2.0 * dh / bh) as f32;
    let tx = (2.0 * dx / bw - 1.0) as f32;
    let ty = (2.0 * dy / bh - 1.0) as f32;
    Mat4::from_cols(
        crate::vec4::Vec4::new(sx, 0.0, 0.0, 0.0),
        crate::vec4::Vec4::new(0.0, sy, 0.0, 0.0),
        crate::vec4::Vec4::new(0.0, 0.0, 1.0, 0.0),
        crate::vec4::Vec4::new(tx, ty, 0.0, 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec3::Vec3;

    // -- pure math ------------------------------------------------------------

    #[test]
    fn ndc_full_bounds_is_identity_scale() {
        // A dest rect covering the whole bounds maps the unit quad to all of
        // clip space: (0,0) -> (-1,-1), (1,1) -> (1,1).
        let m = view_rect_to_ndc((0.0, 0.0, 800.0, 600.0), (800.0, 600.0));
        let lo = m.transform_point3(Vec3::new(0.0, 0.0, 0.0));
        let hi = m.transform_point3(Vec3::new(1.0, 1.0, 0.0));
        assert!((lo.x + 1.0).abs() < 1e-6 && (lo.y + 1.0).abs() < 1e-6);
        assert!((hi.x - 1.0).abs() < 1e-6 && (hi.y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ndc_centered_half_rect() {
        // A half-size centered rect: quad corners land at ±0.5 in NDC.
        let m = view_rect_to_ndc((200.0, 150.0, 400.0, 300.0), (800.0, 600.0));
        let lo = m.transform_point3(Vec3::new(0.0, 0.0, 0.0));
        let hi = m.transform_point3(Vec3::new(1.0, 1.0, 0.0));
        assert!((lo.x + 0.5).abs() < 1e-6 && (lo.y + 0.5).abs() < 1e-6);
        assert!((hi.x - 0.5).abs() < 1e-6 && (hi.y - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ndc_offset_rect_tracks_pan() {
        // Panning the dest rect right by 100pt in an 800pt view moves NDC x
        // by +0.25 at both corners; y is untouched.
        let a = view_rect_to_ndc((0.0, 0.0, 400.0, 300.0), (800.0, 600.0));
        let b = view_rect_to_ndc((100.0, 0.0, 400.0, 300.0), (800.0, 600.0));
        let pa = a.transform_point3(Vec3::new(0.0, 0.0, 0.0));
        let pb = b.transform_point3(Vec3::new(0.0, 0.0, 0.0));
        assert!((pb.x - pa.x - 0.25).abs() < 1e-6);
        assert!((pb.y - pa.y).abs() < 1e-6);
    }

    // -- GPU (headless, adapter-gated) ----------------------------------------

    /// A windowless device, or `None` when the environment has no adapter
    /// (headless CI): the GPU tests then skip gracefully — and say so.
    fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL,
            ..Default::default()
        });
        let adapter = match pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            },
        )) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("SKIP quad GPU test: no adapter in this environment ({e})");
                return None;
            }
        };
        match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Euclase_Quad_Test_Device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        })) {
            Ok(dq) => Some(dq),
            Err(e) => {
                eprintln!("SKIP quad GPU test: device request failed ({e})");
                None
            }
        }
    }

    /// Render `quad` into a fresh W×H sRGB target and read the pixels back.
    fn render_and_read(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quad: &TexturedQuad,
        width: u32,
        height: u32,
        clear: wgpu::Color,
    ) -> Vec<u8> {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Quad_Test_Target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        quad.render_to_view(device, queue, &view, clear);

        // Read back (rows padded to the 256-byte copy alignment).
        let padded = (width * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Quad_Test_Readback"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map failed"));
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll failed");
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * padded) as usize;
            out.extend_from_slice(&data[start..start + (width * 4) as usize]);
        }
        out
    }

    /// The row-order witness: a 1×2 frame (top row red, bottom row blue)
    /// rendered full-target must come back top row red — i.e. uploaded row 0
    /// is DRAWN on top, exactly like the CPU path's unflipped drawInRect.
    /// Pure 0/255 channels survive the sRGB decode/encode round trip exactly.
    #[test]
    fn gpu_row_order_top_row_stays_on_top() {
        let Some((device, queue)) = headless_device() else {
            return;
        };
        let mut quad =
            TexturedQuad::forge(&device, wgpu::TextureFormat::Rgba8UnormSrgb, wgpu::FilterMode::Linear);
        #[rustfmt::skip]
        let frame: [u8; 8] = [
            255, 0, 0, 255, // row 0 (top): red
            0, 0, 255, 255, // row 1 (bottom): blue
        ];
        quad.upload_frame(&device, &queue, &frame, 1, 2);
        quad.set_transform(&queue, &view_rect_to_ndc((0.0, 0.0, 2.0, 2.0), (2.0, 2.0)));

        // 2x2 RGBA readback: row 0 (the TOP row of the target) is px[0..8],
        // row 1 is px[8..16].
        let px = render_and_read(&device, &queue, &quad, 2, 2, wgpu::Color::BLACK);
        assert_eq!(&px[0..4], &[255, 0, 0, 255], "top row must be red");
        assert_eq!(&px[4..8], &[255, 0, 0, 255], "top row must be red");
        assert_eq!(&px[8..12], &[0, 0, 255, 255], "bottom row must be blue");
        assert_eq!(&px[12..16], &[0, 0, 255, 255], "bottom row must be blue");
    }

    /// The color witness: mid-range sRGB bytes (the Moonstone field triple)
    /// must survive upload → sample → sRGB re-encode within ±1/255 — the
    /// "GPU indistinguishable from CPU" axis, measured at the byte level.
    #[test]
    fn gpu_srgb_bytes_round_trip() {
        let Some((device, queue)) = headless_device() else {
            return;
        };
        let mut quad =
            TexturedQuad::forge(&device, wgpu::TextureFormat::Rgba8UnormSrgb, wgpu::FilterMode::Linear);
        let (r, g, b) = (0x2D, 0x2B, 0x55);
        let frame: [u8; 4] = [r, g, b, 255];
        quad.upload_frame(&device, &queue, &frame, 1, 1);
        quad.set_transform(&queue, &view_rect_to_ndc((0.0, 0.0, 1.0, 1.0), (1.0, 1.0)));

        let px = render_and_read(&device, &queue, &quad, 1, 1, wgpu::Color::BLACK);
        for (got, want) in px[..3].iter().zip([r, g, b]) {
            assert!(
                (*got as i32 - want as i32).abs() <= 1,
                "sRGB byte drifted: got {got}, want {want} (±1)"
            );
        }
        assert_eq!(px[3], 255);
    }

    /// Outside the dest rect the target keeps the clear (the field shows
    /// around an aspect-fit picture), and the quad half fills where mapped.
    #[test]
    fn gpu_clear_shows_around_the_quad() {
        let Some((device, queue)) = headless_device() else {
            return;
        };
        let mut quad =
            TexturedQuad::forge(&device, wgpu::TextureFormat::Rgba8UnormSrgb, wgpu::FilterMode::Linear);
        let frame: [u8; 4] = [255, 255, 255, 255];
        quad.upload_frame(&device, &queue, &frame, 1, 1);
        // Right half of a 2-wide target.
        quad.set_transform(&queue, &view_rect_to_ndc((1.0, 0.0, 1.0, 1.0), (2.0, 1.0)));

        let px = render_and_read(
            &device,
            &queue,
            &quad,
            2,
            1,
            wgpu::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        );
        assert_eq!(&px[0..4], &[0, 0, 0, 255], "left half keeps the clear");
        assert_eq!(&px[4..8], &[255, 255, 255, 255], "right half is the frame");
    }
}
