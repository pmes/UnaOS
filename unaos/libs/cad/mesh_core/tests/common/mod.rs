// Shared fixtures for the mesh_core suites (built by the tests themselves; no binary files in the tree).
#![allow(dead_code)]

use mesh_core::{Mesh, Vec3};

/// The unit cube [0,1]^3: vertex i = (i & 1, (i >> 1) & 1, (i >> 2) & 1), 12 outward counter-clockwise triangles.
pub fn cube() -> Mesh {
    let positions = (0..8u32)
        .map(|i| Vec3::new((i & 1) as f64, ((i >> 1) & 1) as f64, ((i >> 2) & 1) as f64))
        .collect();
    let triangles = vec![
        [0, 2, 3], [0, 3, 1], // z = 0
        [4, 5, 7], [4, 7, 6], // z = 1
        [0, 1, 5], [0, 5, 4], // y = 0
        [2, 6, 7], [2, 7, 3], // y = 1
        [0, 4, 6], [0, 6, 2], // x = 0
        [1, 3, 7], [1, 7, 5], // x = 1
    ];
    Mesh { positions, triangles }
}

/// A UV sphere of radius `r` about the origin: poles plus `stacks - 1` rings of `slices`, outward oriented, closed.
pub fn sphere(r: f64, stacks: u32, slices: u32) -> Mesh {
    use std::f64::consts::PI;
    let mut positions = vec![Vec3::new(0.0, 0.0, r)];
    for i in 1..stacks {
        let th = PI * i as f64 / stacks as f64;
        for j in 0..slices {
            let ph = 2.0 * PI * j as f64 / slices as f64;
            positions.push(Vec3::new(r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()));
        }
    }
    let south = positions.len() as u32;
    positions.push(Vec3::new(0.0, 0.0, -r));
    let ring = |i: u32, j: u32| 1 + (i - 1) * slices + (j % slices);
    let mut triangles = Vec::new();
    for j in 0..slices {
        triangles.push([0, ring(1, j), ring(1, j + 1)]);
    }
    for i in 1..stacks - 1 {
        for j in 0..slices {
            let (a, b, c, d) = (ring(i, j), ring(i, j + 1), ring(i + 1, j), ring(i + 1, j + 1));
            triangles.push([a, c, d]);
            triangles.push([a, d, b]);
        }
    }
    for j in 0..slices {
        triangles.push([ring(stacks - 1, j), south, ring(stacks - 1, j + 1)]);
    }
    Mesh { positions, triangles }
}
