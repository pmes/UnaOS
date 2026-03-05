// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

// The Mathematical Definition of the Stria Crystal
// No polygons. Just pure logic.

#[derive(Clone, Copy)]
struct Vec3 { x: f32, y: f32, z: f32 }

// The "Signed Distance Function" (SDF)
// Returns the distance from point 'p' to the surface of our Crystal.
fn stria_crystal_sdf(p: Vec3) -> f32 {
    // 1. Define an Octahedron (The Diamond Shape)
    let p = p.abs(); // Fold space (symmetry)
    let m = p.x + p.y + p.z - 1.0;
    
    // 2. Add "Noise" (The Geological Imperfections/Striae)
    // We disturb the perfect shape with a sine wave to make it look "etched"
    let stria_grooves = (p.y * 20.0).sin() * 0.02;
    
    // 3. Return the exact distance
    (m * 0.57735027) - stria_grooves
}

// The Ray Marcher
// "Shoots" a pixel into the world to see if it hits the crystal
fn cast_ray(ro: Vec3, rd: Vec3) -> Option<Vec3> {
    let mut t = 0.0; // Distance traveled
    for _ in 0..64 { // Max 64 steps for performance
        let p = ro + rd * t;
        let d = stria_crystal_sdf(p); // Ask the math: "Are we there yet?"
        
        if d < 0.001 { return Some(p); } // Hit!
        if t > 20.0 { break; } // Missed (went into the void)
        t += d;
    }
    None
}