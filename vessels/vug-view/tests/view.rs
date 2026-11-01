// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGFACE1 (LEDGER SR470): the face's host witness — a fixture cube rendered headless to pixels (and a PNG), the
//! navigation invariants (orbit about the cursor, zoom to the cursor), the preset rules, the pick, the file open
//! through mesh_core's readers, and the 100k-triangle frame time.

use bandy::{SMessage, BUTTON_MIDDLE, BUTTON_RIGHT, MOD_ALT, MOD_SHIFT};
use mesh_core::{stl, Mesh, Tol, Vec3 as MVec3};
use vug_view::model::Model;
use vug_view::raster::{self, rgba, ACCENT, BACKGROUND};
use vug_view::view::{Action, Button, Preset, View, PRESETS};
use vug_view::{Viewer, SURFACE};

const W: u32 = 160;
const H: u32 = 120;

/// A 20 mm cube centred on the origin, counter-clockwise outward triangles.
fn cube() -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
    let s = 10.0;
    let p = vec![[-s, -s, -s], [s, -s, -s], [s, s, -s], [-s, s, -s], [-s, -s, s], [s, -s, s], [s, s, s], [-s, s, s]];
    let t = vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [3, 7, 6], [3, 6, 2], [0, 4, 7], [0, 7, 3], [1, 2, 6], [1, 6, 5]];
    (p, t)
}

fn cube_model() -> Model {
    let (p, t) = cube();
    Model::from_triangles(&p, &t).expect("cube")
}

/// A UV sphere of `stacks * slices * 2` triangles (the pole rows are degenerate slivers, as in any UV sphere).
fn sphere(r: f64, stacks: u32, slices: u32) -> Model {
    let mut p = Vec::new();
    for i in 0..=stacks {
        let th = std::f64::consts::PI * i as f64 / stacks as f64;
        for j in 0..=slices {
            let ph = 2.0 * std::f64::consts::PI * j as f64 / slices as f64;
            p.push([r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()]);
        }
    }
    let row = slices + 1;
    let mut t = Vec::new();
    for i in 0..stacks {
        for j in 0..slices {
            let (a, b, c, d) = (i * row + j, (i + 1) * row + j, (i + 1) * row + j + 1, i * row + j + 1);
            t.push([a, b, c]);
            t.push([a, c, d]);
        }
    }
    Model::from_triangles(&p, &t).expect("sphere")
}

fn front_view(m: &Model) -> View {
    let mut v = View::new(m, (W, H), Preset::Una);
    assert!(v.key(m, "1"));
    v
}

#[test]
fn cube_front_view_pixels_and_png() {
    let m = cube_model();
    let v = front_view(&m);
    let f = raster::render(&m, &v);
    assert_eq!((f.width, f.height, f.rgba.len()), (W, H, (W * H * 4) as usize));
    // A pixel on the front face, off its diagonal: the lit colour of exactly the triangle the pick finds there.
    let (px, py) = (86u32, 62u32);
    let (o, d) = v.ray(px as f64 + 0.5, py as f64 + 0.5);
    let (_, tri) = m.pick([o.x, o.y, o.z], [d.x, d.y, d.z]).expect("the centre ray hits the cube");
    assert!(tri == 4 || tri == 5, "the front face (-Y) is triangles 4 and 5, got {tri}");
    let lit = raster::tri_colour(&m, &v, tri).expect("a lit face");
    assert_eq!(f.pixel(px, py), lit);
    assert_ne!(lit, rgba(BACKGROUND));
    // Both front triangles shade the same within a step (one face, one normal).
    let other = raster::tri_colour(&m, &v, 9 - tri).expect("lit");
    assert!((0..3).all(|k| (lit[k] as i32 - other[k] as i32).abs() <= 2), "{lit:?} vs {other:?}");
    // The corner is the background exactly (sky: no grid above the horizon, the triad is lower-left).
    assert_eq!(f.pixel(0, 0), rgba(BACKGROUND));
    assert_eq!(f.pixel(W - 1, 0), rgba(BACKGROUND));
    // The PNG round trip (pixel_core's encoder and decoder).
    let png = pixel_core::png::encode::encode_rgba(f.width, f.height, &f.rgba).expect("encode");
    let back = pixel_core::png::decode(&png).expect("decode");
    assert_eq!((back.width, back.height), (W, H));
    assert_eq!(back.rgba, f.rgba);
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vug-cube-front.png");
    std::fs::write(&out, &png).expect("write png");
}

#[test]
fn home_sees_the_model_and_the_grid() {
    let m = cube_model();
    let v = View::new(&m, (W, H), Preset::Una);
    let f = raster::render(&m, &v);
    assert_ne!(f.pixel(W / 2, H / 2), rgba(BACKGROUND), "Home looks at the model");
    assert_ne!(f.id[(H / 2 * W + W / 2) as usize], u32::MAX);
    // Below the cube (looking down at 35 degrees) the ground grid has inked some background pixels.
    let inked = (0..W * H).filter(|&i| f.id[i as usize] == u32::MAX).filter(|&i| f.pixel(i % W, i / W) != rgba(BACKGROUND)).count();
    assert!(inked > 0, "the grid draws");
}

#[test]
fn click_picks_and_highlights() {
    let m = cube_model();
    let mut v = front_view(&m);
    v.press(&m, Button::Left, 0, 86.5, 62.5);
    assert!(v.release(&m, 87.0, 62.5), "a click picks");
    let tri = v.picked.expect("picked");
    assert!(tri == 4 || tri == 5);
    let f = raster::render(&m, &v);
    assert_eq!(f.pixel(86, 62), rgba(ACCENT));
    // A click on the sky clears it.
    v.press(&m, Button::Left, 0, 2.0, 2.0);
    assert!(v.release(&m, 2.0, 2.0));
    assert_eq!(v.picked, None);
}

#[test]
fn orbit_keeps_the_pivot_under_the_cursor() {
    let m = cube_model();
    let mut v = front_view(&m);
    let (x, y) = (100.0, 50.0);
    let (pivot, _) = v.hit(&m, x, y).expect("the press hits the cube");
    let before = v.camera;
    v.press(&m, Button::Left, MOD_ALT, x, y); // Alt+Left = Middle = orbit in the una preset
    assert!(v.motion(130.0, 70.0));
    assert!(v.motion(60.0, 90.0));
    assert_ne!(v.camera, before, "the camera moved");
    let (sx, sy) = v.project(pivot).expect("in front");
    assert!((sx - x as f32).abs() < 0.5 && (sy - y as f32).abs() < 0.5, "pivot at ({sx}, {sy}), pressed at ({x}, {y})");
    v.release(&m, 60.0, 90.0);
}

#[test]
fn zoom_keeps_the_cursor_point_under_the_cursor() {
    let m = cube_model();
    let mut v = front_view(&m);
    let (x, y) = (100.0, 50.0);
    v.motion(x, y);
    let (p, _) = v.hit(&m, x, y).expect("hit");
    let d0 = (p - v.camera.eye).magnitude();
    assert!(v.wheel(&m, -60.0));
    let d1 = (p - v.camera.eye).magnitude();
    assert!(d1 < d0, "a negative wheel zooms in ({d0} -> {d1})");
    let (sx, sy) = v.project(p).expect("in front");
    assert!((sx - x as f32).abs() < 0.5 && (sy - y as f32).abs() < 0.5, "({sx}, {sy})");
    // Pan: the grabbed point follows the cursor.
    v.press(&m, Button::Left, MOD_ALT | MOD_SHIFT, x, y);
    assert!(v.motion(x + 20.0, y + 10.0));
    let (sx, sy) = v.project(p).expect("in front");
    assert!((sx - 120.0).abs() < 0.5 && (sy - 60.0).abs() < 0.5, "pan ({sx}, {sy})");
}

#[test]
fn presets_keep_menu_and_orbit_apart() {
    for p in PRESETS {
        let t = p.table();
        let orbit_on = |b: Button| t.iter().any(|&(bb, _, a)| bb == b && a == Action::Orbit);
        for &(b, _, a) in t {
            if a == Action::Menu && p.click_menu() != Some(b) {
                assert!(!orbit_on(b), "{} puts menu and orbit on {b:?}", p.name());
            }
        }
        assert_eq!(p.resolve(Button::Left, 0), Action::Pick, "{}: a bare left click picks", p.name());
        assert_eq!(Preset::parse(&p.name().to_uppercase()), Some(p));
    }
    assert_eq!(Preset::Onshape.click_menu(), Some(Button::Right));
    assert_eq!(Preset::Onshape.resolve(Button::Right, 0), Action::Orbit);
    assert_eq!(Preset::Una.resolve(Button::Left, MOD_ALT), Action::Orbit);
    assert_eq!(Preset::Una.resolve(Button::Left, MOD_ALT | MOD_SHIFT), Action::Pan);
    assert_eq!(Preset::Fusion.resolve(Button::Left, MOD_ALT | MOD_SHIFT), Action::Orbit);
    assert_eq!(Preset::Fusion.resolve(Button::Middle, 0), Action::Pan);
    assert_eq!(Preset::SolidWorks.resolve(Button::Middle, 0), Action::Orbit);
    assert_eq!(Preset::parse("nope"), None);
    assert_eq!(Preset::Onshape.next(), Preset::Una);
}

#[test]
fn onshape_right_click_does_not_orbit() {
    let m = cube_model();
    let mut v = View::new(&m, (W, H), Preset::Onshape);
    let before = v.camera;
    v.press(&m, Button::Right, 0, 80.0, 60.0);
    assert!(!v.motion(82.0, 61.0), "under the click threshold nothing moves");
    v.release(&m, 82.0, 61.0);
    assert_eq!(v.camera, before);
    v.press(&m, Button::Right, 0, 80.0, 60.0);
    assert!(v.motion(100.0, 60.0), "a drag orbits");
    assert_ne!(v.camera, before);
}

#[test]
fn viewer_takes_the_surface_wire() {
    let mut vw = Viewer::new(cube_model(), "cube", (W, H), Preset::Una);
    assert!(vw.input(&SMessage::BrowserText("1".into())));
    let before = vw.view.camera;
    vw.input(&SMessage::BrowserButton { down: true, x: 100.0, y: 50.0, mods: MOD_ALT, surface: 0 });
    assert!(vw.input(&SMessage::BrowserMouseMove(130.0, 70.0, 0)));
    vw.input(&SMessage::BrowserButton { down: false, x: 130.0, y: 70.0, mods: MOD_ALT, surface: 0 });
    assert_ne!(vw.view.camera, before);
    assert!(vw.input(&SMessage::BrowserScroll(0.0, -30.0, 0)));
    assert!(vw.input(&SMessage::BrowserResize(200, 100, 0)));
    assert!(vw.input(&SMessage::SurfaceResend { url: SURFACE.into() }));
    assert!(!vw.input(&SMessage::SurfaceResend { url: "facet://view".into() }));
    match vw.frame() {
        SMessage::SurfaceBlit { url, width, height, pixels, damage, .. } => {
            assert_eq!((url.as_str(), width, height, pixels.len(), damage), (SURFACE, 200, 100, 200 * 100 * 4, None));
        }
        _ => panic!("frame() is a SurfaceBlit"),
    }
}

/// VUG-FACE-BUTTONS (LEDGER SR477): the real right and middle buttons and the pinch, through the public entry only.
#[test]
fn viewer_takes_the_real_buttons() {
    let aux = |button: u8, down: bool, x: f64, y: f64| SMessage::BrowserAuxButton { button, down, x, y, mods: 0, surface: 0 };
    let (x, y) = (100.0, 50.0);
    // Onshape orbits on the right button (SolidWorks on the middle): the pressed model point stays under the cursor.
    for (preset, button) in [(Preset::Onshape, BUTTON_RIGHT), (Preset::SolidWorks, BUTTON_MIDDLE)] {
        let mut vw = Viewer::new(cube_model(), "cube", (W, H), preset);
        assert!(vw.input(&SMessage::BrowserText("1".into())));
        let (pivot, _) = vw.view.hit(&vw.model, x, y).expect("the press hits the cube");
        let before = vw.view.camera;
        vw.input(&aux(button, true, x, y));
        assert!(vw.input(&SMessage::BrowserMouseMove(130.0, 70.0, 0)), "{}: the drag moves the frame", preset.name());
        assert!(vw.input(&SMessage::BrowserMouseMove(60.0, 90.0, 0)));
        vw.input(&aux(button, false, 60.0, 90.0));
        assert_ne!(vw.view.camera, before, "{}: the camera orbited", preset.name());
        let (sx, sy) = vw.view.project(pivot).expect("in front");
        assert!((sx - x as f32).abs() < 0.5 && (sy - y as f32).abs() < 0.5, "{}: pivot at ({sx}, {sy})", preset.name());
    }
    // The una preset keeps the right button for the menu: a right drag leaves the camera; an extra button does nothing.
    let mut vw = Viewer::new(cube_model(), "cube", (W, H), Preset::Una);
    assert!(vw.input(&SMessage::BrowserText("1".into())));
    let before = vw.view.camera;
    vw.input(&aux(BUTTON_RIGHT, true, x, y));
    assert!(!vw.input(&SMessage::BrowserMouseMove(140.0, 80.0, 0)));
    vw.input(&aux(BUTTON_RIGHT, false, 140.0, 80.0));
    assert!(!vw.input(&aux(4, true, x, y)));
    assert!(!vw.input(&SMessage::BrowserMouseMove(20.0, 20.0, 0)));
    assert!(!vw.input(&aux(4, false, 20.0, 20.0)));
    // A wheel tilt (dx only) zooms nothing.
    assert!(!vw.input(&SMessage::BrowserScroll(5.0, 0.0, 0)));
    assert_eq!(vw.view.camera, before);
    // The pinch zooms in toward the model point under the fingers and keeps it there.
    let (p, _) = vw.view.hit(&vw.model, x, y).expect("hit");
    let d0 = (p - vw.view.camera.eye).magnitude();
    assert!(vw.input(&SMessage::BrowserPinch { scale: 1.25, x, y, surface: 0 }));
    let d1 = (p - vw.view.camera.eye).magnitude();
    assert!(d1 < d0, "a spread pinch zooms in ({d0} -> {d1})");
    let (sx, sy) = vw.view.project(p).expect("in front");
    assert!((sx - x as f32).abs() < 0.5 && (sy - y as f32).abs() < 0.5, "pinch ({sx}, {sy})");
    assert!(!vw.input(&SMessage::BrowserPinch { scale: 1.0, x, y, surface: 0 }), "a still pinch changes nothing");
}

#[test]
fn files_open_through_mesh_core() {
    let (p, t) = cube();
    let mesh = Mesh { positions: p.iter().map(|q| MVec3::new(q[0], q[1], q[2])).collect(), triangles: t.clone() };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    // STL: a soup of 36 corners, welded to 8 vertices by the model seam.
    let stl_path = dir.join("vug-cube.stl");
    std::fs::write(&stl_path, stl::write_binary(&stl::Stl::from_mesh(&mesh, Tol::DEFAULT))).expect("write stl");
    let m = Model::open(stl_path.to_str().unwrap()).expect("open stl");
    assert_eq!((m.triangle_count(), m.positions().len() / 3), (12, 8));
    assert_eq!(m.bounds(), ([-10.0; 3], [10.0; 3]));
    // OBJ, by extension.
    let obj_path = dir.join("vug-cube.obj");
    let mut text = String::new();
    for q in &p {
        text += &format!("v {} {} {}\n", q[0], q[1], q[2]);
    }
    for tri in &t {
        text += &format!("f {} {} {}\n", tri[0] + 1, tri[1] + 1, tri[2] + 1);
    }
    std::fs::write(&obj_path, text).expect("write obj");
    let o = Model::open(obj_path.to_str().unwrap()).expect("open obj");
    assert_eq!(o.triangle_count(), 12);
    // A refusal is named, never a panic.
    let bad = dir.join("vug-bad.stl");
    std::fs::write(&bad, b"not a mesh").expect("write");
    assert!(Model::open(bad.to_str().unwrap()).err().expect("refused").contains("STL"));
    assert!(Model::open(dir.join("vug-missing.stl").to_str().unwrap()).is_err());
}

#[test]
fn hundred_thousand_triangles_frame_time() {
    let m = sphere(20.0, 200, 250);
    assert_eq!(m.triangle_count(), 100_000);
    let v = View::new(&m, (800, 600), Preset::Una);
    let _ = raster::render(&m, &v); // warm
    let runs = 3;
    let t0 = std::time::Instant::now();
    let mut f = None;
    for _ in 0..runs {
        f = Some(raster::render(&m, &v));
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0 / runs as f64;
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    eprintln!("vug-view: 100k tris 800x600 in {ms:.1} ms ({profile})");
    let f = f.unwrap();
    assert_ne!(f.pixel(400, 300), rgba(BACKGROUND));
}
