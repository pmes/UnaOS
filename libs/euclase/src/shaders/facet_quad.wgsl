// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// facet_quad — one textured quad under a Mat4 transform.
//
// The quad lives in "image space": the unit square (0,0)..(1,1) with (0,0)
// the image's BOTTOM-left corner (the unflipped AppKit view convention the
// facet CPU path established — image_view.rs documents why views drawing
// pictures stay unflipped). The uniform transform maps image space to clip
// space; the consumer derives it from its dest-rect math.
//
// Texture coordinates are top-down (wgpu convention: v=0 is the first texel
// row uploaded, i.e. the image's top row), so v = 1 - y: quad-space y=1 (the
// top edge) samples texel row 0. This is the row-order seam the CPU path's
// readout row-flip (image_view.rs) mirrors.

struct QuadUniform {
    transform: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> quad: QuadUniform;
@group(0) @binding(1) var t_frame: texture_2d<f32>;
@group(0) @binding(2) var s_frame: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Vertex-index-generated unit quad, drawn as a 4-vertex triangle strip:
// vi 0..3 -> (0,0), (1,0), (0,1), (1,1).
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let x = f32(vi & 1u);
    let y = f32(vi >> 1u);
    var out: VsOut;
    out.pos = quad.transform * vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x, 1.0 - y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(t_frame, s_frame, in.uv);
}
