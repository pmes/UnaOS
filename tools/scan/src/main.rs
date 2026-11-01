// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `scan` — Vug's headless scan face (VUGSCANTOOL, LEDGER SR471).
//!
//! CHARTER: Vug (CODEX §2, 3D) — headless face: a real depth recording or dump in, scan_core's mesh as PLY and a
//! residual report out; no logic of its own.
//!
//! ```text
//! scan <in.prb | in.csv | in.raw> -o <out.ply> [--report <out.txt>]
//!      [--meta <x_Depth_metadata.csv>] [--width W --height H] [--csv-raw]
//!      [--fx F --fy F --ppx F --ppy F] [--units-um N] [--frame N]
//!      [--turntable DEG --radius MM | --poses <file>] [--voxel MM] [--trunc MM] [--band MM]
//! ```
//!
//! Inputs (the measured roads from a D435i to this machine; librealsense v2.55.1):
//! - `.prb`: probe_core's recording (`probe_core::recording`: PRB1 records back to back, what the kernel's
//!   `/dev/probe/depth0` record is); the depth (Z16) records are fused, intrinsics and units from each record.
//!   The kernel writes no intrinsics today (sonde.rs `wire::encode(.., None)`), so the four flags supply them.
//! - `.csv`: rs-convert's depth CSV (`tools/convert/converters/converter-csv.cpp:83-90`): one line per image row,
//!   comma-separated `depth_frame::get_distance` values in METRES, 0 for a hole, no header, no intrinsics (the four
//!   flags are required). `--csv-raw` reads integer Z16 units instead. Quantised at `--units-um` (default 1000).
//! - `.raw`: realsense-viewer's snapshot (`common/stream-model.cpp:1505-1513`): `<base>_Depth.raw` is
//!   `height * stride` bytes of Z16 (`common/model-views.cpp:153-167`), and `--meta <base>_Depth_metadata.csv`
//!   (`model-views.cpp:169-200`) gives `Format`, `Resolution x/y`, `Bytes per pixel` and `Fx`/`Fy`/`PPx`/`PPy`. The
//!   metadata carries no depth units: the D400 default (1000 um) unless `--units-um`. Without `--meta`, `--width`
//!   `--height` and the four intrinsic flags.
//!
//! Poses: ONE depth frame (or `--frame N`) is fused from the identity pose. Several frames need a pose source, else
//! the run is refused (`ManyFramesNoPose`; frame-to-model ICP is VUG-SCAN-POSE, owed):
//! - `--turntable DEG --radius MM`, the stop-gap: the world is frame 0's camera frame; the turntable axis passes
//!   through (0, 0, MM) parallel to the image's vertical (camera +y); `DEG` > 0 is the table turning
//!   counter-clockwise seen from above, so frame k's camera sits rotated by +k·DEG (right-handed about camera +y)
//!   about that axis in the table's frame.
//! - `--poses <file>`: one line per fused frame, 12 numbers, the 3x4 camera → world `[R | t]` row-major, t in mm;
//!   blank lines and `#` lines skipped; R must be a rotation (|RᵀR − I| ≤ [`POSE_RIGID`], det > 0).
//!
//! The run is scan_core's: `Tsdf::new(voxel, trunc)` (default 4 mm, 3 × voxel), `integrate` per frame, `extract`,
//! then `fit_plane` over every vertex (band `--band` mm, default 3, and 15°) and `fit_cylinder` over the vertices
//! farther than the band from that plane, with their TSDF normals. The mesh goes out through `mesh_core::ply::write`
//! (binary little-endian). The report (stdout, and `--report`) names the input, the intrinsics' source, frames,
//! vertices, triangles, confidence, boundary loops, both fits' residuals or their refusal, and a `scan:` line in
//! scan_core's own test token shape. A refusal prints `scan: refused <Name> {..}` and exits 1; usage exits 2.

use std::fmt::Write as _;
use std::path::Path;

use probe_core::recording::{self, RecordingError};
use probe_core::{FrameError, StreamKind};
use scan_core::{
    Cylinder, DepthScale, Fit, FitError, Frame, Intrinsics, PixelFormat, Plane, ScanError, Tol, Topo, Transform, Tsdf,
    Vec3, extract, fit_cylinder, fit_plane,
};

/// |RᵀR − I| bound for a `--poses` rotation: the rounding of a matrix printed to 6 decimals.
const POSE_RIGID: f64 = 1e-6;
/// The angular half of the fit bands (scan_core's own cylinder test band).
const BAND_DEG: f64 = 15.0;
/// The D400 depth unit when nothing states one (`DepthScale::D400_DEFAULT`, d400-device.cpp:912).
const D400_UNITS_UM: u32 = 1000;

#[derive(Debug)]
#[allow(dead_code)] // the fields are read through Debug: a refusal is printed with them
enum Refusal {
    Io { path: String, error: String },
    InputKind { path: String },
    Recording(RecordingError),
    NoDepthFrames,
    FrameIndex { frame: usize, frames: usize },
    NoDepthUnits { index: usize },
    UnitsZero,
    PartialIntrinsics,
    NoIntrinsics { index: usize },
    CsvEmpty,
    CsvNumber { row: usize, col: usize },
    CsvRagged { row: usize, cols: usize, expected: usize },
    CsvDepthRange { row: usize, col: usize },
    RawNoGeometry,
    RawSize { len: usize, height: u32 },
    Meta { key: &'static str },
    MetaFormat { format: String, bytes_per_pixel: String },
    Frame(FrameError),
    ManyFramesNoPose { frames: usize },
    PoseFlags,
    TurntableNoRadius,
    PoseLine { line: usize, numbers: usize },
    PoseCount { poses: usize, frames: usize },
    PoseNotRigid { line: usize },
    Volume(ScanError),
    Integrate { frame: usize, error: ScanError },
    Extract(ScanError),
}

fn usage() -> ! {
    eprintln!("usage: scan <in.prb | in.csv | in.raw> -o <out.ply> [--report <out.txt>]");
    eprintln!("            [--meta <x_Depth_metadata.csv>] [--width W --height H] [--csv-raw]");
    eprintln!("            [--fx F --fy F --ppx F --ppy F] [--units-um N] [--frame N]");
    eprintln!("            [--turntable DEG --radius MM | --poses <file>] [--voxel MM] [--trunc MM] [--band MM]");
    std::process::exit(2)
}

#[derive(Default)]
struct Args {
    input: Option<String>,
    out: Option<String>,
    report: Option<String>,
    meta: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    csv_raw: bool,
    k: [Option<f32>; 4], // fx, fy, ppx, ppy
    units_um: Option<u32>,
    frame: Option<usize>,
    turntable: Option<f64>,
    radius: Option<f64>,
    poses: Option<String>,
    voxel: f64,
    trunc: Option<f64>,
    band: f64,
}

fn num<T: std::str::FromStr>(it: &mut impl Iterator<Item = String>) -> T {
    it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())
}

fn parse_args() -> Args {
    let mut a = Args { voxel: 4.0, band: 3.0, ..Args::default() };
    let mut it = std::env::args().skip(1);
    while let Some(s) = it.next() {
        match s.as_str() {
            "-o" => a.out = Some(it.next().unwrap_or_else(|| usage())),
            "--report" => a.report = Some(it.next().unwrap_or_else(|| usage())),
            "--meta" => a.meta = Some(it.next().unwrap_or_else(|| usage())),
            "--width" => a.width = Some(num(&mut it)),
            "--height" => a.height = Some(num(&mut it)),
            "--csv-raw" => a.csv_raw = true,
            "--fx" => a.k[0] = Some(num(&mut it)),
            "--fy" => a.k[1] = Some(num(&mut it)),
            "--ppx" => a.k[2] = Some(num(&mut it)),
            "--ppy" => a.k[3] = Some(num(&mut it)),
            "--units-um" => a.units_um = Some(num(&mut it)),
            "--frame" => a.frame = Some(num(&mut it)),
            "--turntable" => a.turntable = Some(num(&mut it)),
            "--radius" => a.radius = Some(num(&mut it)),
            "--poses" => a.poses = Some(it.next().unwrap_or_else(|| usage())),
            "--voxel" => a.voxel = num(&mut it),
            "--trunc" => a.trunc = Some(num(&mut it)),
            "--band" => a.band = num(&mut it),
            _ if s.starts_with('-') => usage(),
            _ if a.input.is_none() => a.input = Some(s),
            _ => usage(),
        }
    }
    if a.input.is_none() || a.out.is_none() {
        usage()
    }
    a
}

fn read(path: &str) -> Result<Vec<u8>, Refusal> {
    std::fs::read(path).map_err(|e| Refusal::Io { path: path.into(), error: e.to_string() })
}

/// One depth frame as loaded: the frame, its scale, the intrinsics its input carried.
struct Loaded {
    frame: Frame,
    scale: DepthScale,
    intrinsics: Option<Intrinsics>,
}

fn units(a: &Args) -> Result<u32, Refusal> {
    match a.units_um {
        Some(0) => Err(Refusal::UnitsZero),
        Some(u) => Ok(u),
        None => Ok(D400_UNITS_UM),
    }
}

/// `.prb`: the recording's depth records.
fn load_prb(a: &Args, b: &[u8]) -> Result<Vec<Loaded>, Refusal> {
    let rec = recording::read(b).map_err(Refusal::Recording)?;
    let mut out = Vec::new();
    for (index, r) in rec.depth().enumerate() {
        let scale = match (a.units_um, r.depth_scale()) {
            (Some(0), _) => return Err(Refusal::UnitsZero),
            (Some(u), _) => DepthScale::from_micrometres(u),
            (None, Some(s)) => s,
            (None, None) => return Err(Refusal::NoDepthUnits { index }),
        };
        out.push(Loaded { frame: r.frame.clone(), scale, intrinsics: r.intrinsics() });
    }
    Ok(out)
}

/// `.csv`: rs-convert's metres (or, `--csv-raw`, integer Z16 units), one line per row.
fn load_csv(a: &Args, b: &[u8]) -> Result<Vec<Loaded>, Refusal> {
    let um = units(a)?;
    let text = String::from_utf8_lossy(b);
    let rows: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if rows.is_empty() {
        return Err(Refusal::CsvEmpty);
    }
    let expected = rows[0].split(',').count();
    let mut data = Vec::with_capacity(rows.len() * expected * 2);
    for (row, line) in rows.iter().enumerate() {
        let cells: Vec<&str> = line.split(',').map(str::trim).collect();
        if cells.len() != expected {
            return Err(Refusal::CsvRagged { row, cols: cells.len(), expected });
        }
        for (col, c) in cells.iter().enumerate() {
            let raw: u16 = if a.csv_raw {
                c.parse().map_err(|_| Refusal::CsvNumber { row, col })?
            } else {
                let m: f64 = c.parse().map_err(|_| Refusal::CsvNumber { row, col })?;
                let units = (m * 1_000_000.0 / um as f64).round();
                if !(0.0..=u16::MAX as f64).contains(&units) {
                    return Err(Refusal::CsvDepthRange { row, col });
                }
                units as u16
            };
            data.extend_from_slice(&raw.to_le_bytes());
        }
    }
    let frame = Frame::packed(StreamKind::Depth, PixelFormat::Z16, expected as u32, rows.len() as u32, 0, 0, data)
        .map_err(Refusal::Frame)?;
    Ok(vec![Loaded { frame, scale: DepthScale::from_micrometres(um), intrinsics: None }])
}

/// The value of `key` in realsense-viewer's `Key,Value` metadata lines.
fn meta_value<'a>(text: &'a str, key: &'static str) -> Result<&'a str, Refusal> {
    text.lines()
        .filter_map(|l| l.split_once(','))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim())
        .ok_or(Refusal::Meta { key })
}

fn meta_num<T: std::str::FromStr>(text: &str, key: &'static str) -> Result<T, Refusal> {
    meta_value(text, key)?.parse().map_err(|_| Refusal::Meta { key })
}

/// `.raw`: realsense-viewer's snapshot bytes, geometry and intrinsics from `--meta` (or the flags).
fn load_raw(a: &Args, b: &[u8]) -> Result<Vec<Loaded>, Refusal> {
    let um = units(a)?;
    let (width, height, intrinsics) = match &a.meta {
        Some(p) => {
            let text = String::from_utf8_lossy(&read(p)?).into_owned();
            let (format, bpp) = (meta_value(&text, "Format")?, meta_value(&text, "Bytes per pixel")?);
            if format != "Z16" || bpp != "2" {
                return Err(Refusal::MetaFormat { format: format.into(), bytes_per_pixel: bpp.into() });
            }
            let (w, h): (u32, u32) = (meta_num(&text, "Resolution x")?, meta_num(&text, "Resolution y")?);
            let k = [meta_num(&text, "Fx")?, meta_num(&text, "Fy")?, meta_num(&text, "PPx")?, meta_num(&text, "PPy")?];
            (w, h, Some(Intrinsics::from_rect_params(w, h, k)))
        }
        None => match (a.width, a.height) {
            (Some(w), Some(h)) => (w, h, None),
            _ => return Err(Refusal::RawNoGeometry),
        },
    };
    if height == 0 || b.len() % height as usize != 0 {
        return Err(Refusal::RawSize { len: b.len(), height });
    }
    let stride = b.len() / height as usize;
    let frame = Frame::new(StreamKind::Depth, PixelFormat::Z16, width, height, stride, 0, 0, b.to_vec())
        .map_err(Refusal::Frame)?;
    Ok(vec![Loaded { frame, scale: DepthScale::from_micrometres(um), intrinsics }])
}

/// `--turntable`: frame k's camera → table pose (the module doc's geometry).
fn turntable_pose(k: usize, deg: f64, radius_mm: f64) -> Transform {
    let th = (k as f64 * deg).to_radians();
    let (c, s) = (th.cos(), th.sin());
    // Ry(θ), right-handed about camera +y, about the axis through (0, 0, R).
    let x = Vec3::new(c, 0.0, -s);
    let y = Vec3::new(0.0, 1.0, 0.0);
    let z = Vec3::new(s, 0.0, c);
    let origin = Vec3::new(-radius_mm * s, 0.0, radius_mm - radius_mm * c);
    Transform::from_axes(x, y, z, origin)
}

/// `--poses`: 12 numbers per line, the 3x4 camera → world `[R | t]` row-major.
fn load_poses(path: &str) -> Result<Vec<Transform>, Refusal> {
    let text = String::from_utf8_lossy(&read(path)?).into_owned();
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let line = i + 1;
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let v: Vec<f64> = l
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|t| !t.is_empty())
            .map(|t| t.parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| Refusal::PoseLine { line, numbers: 0 })?;
        if v.len() != 12 || v.iter().any(|x| !x.is_finite()) {
            return Err(Refusal::PoseLine { line, numbers: v.len() });
        }
        let r = [[v[0], v[1], v[2]], [v[4], v[5], v[6]], [v[8], v[9], v[10]]];
        let mut worst = 0.0f64;
        for i in 0..3 {
            for j in 0..3 {
                let rtr: f64 = (0..3).map(|k| r[k][i] * r[k][j]).sum();
                worst = worst.max((rtr - if i == j { 1.0 } else { 0.0 }).abs());
            }
        }
        let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1]) - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
            + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
        if worst > POSE_RIGID || det <= 0.0 {
            return Err(Refusal::PoseNotRigid { line });
        }
        let col = |j: usize| Vec3::new(r[0][j], r[1][j], r[2][j]);
        out.push(Transform::from_axes(col(0), col(1), col(2), Vec3::new(v[3], v[7], v[11])));
    }
    Ok(out)
}

fn v3(v: Vec3) -> String {
    format!("({:.5},{:.5},{:.5})", v.x, v.y, v.z)
}

fn run(a: &Args) -> Result<String, Refusal> {
    let input = a.input.as_deref().unwrap_or_default();
    let kind = Path::new(input).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let bytes = read(input)?;
    let mut frames = match kind.as_str() {
        "prb" => load_prb(a, &bytes)?,
        "csv" => load_csv(a, &bytes)?,
        "raw" => load_raw(a, &bytes)?,
        _ => return Err(Refusal::InputKind { path: input.into() }),
    };
    if frames.is_empty() {
        return Err(Refusal::NoDepthFrames);
    }
    let posed = a.turntable.is_some() || a.poses.is_some();
    if (a.turntable.is_some() && a.poses.is_some()) || (posed && a.frame.is_some()) {
        return Err(Refusal::PoseFlags);
    }
    if let Some(n) = a.frame {
        if n >= frames.len() {
            return Err(Refusal::FrameIndex { frame: n, frames: frames.len() });
        }
        frames = vec![frames.swap_remove(n)];
    }
    let poses: Vec<Transform> = match (a.turntable, &a.poses) {
        (Some(deg), None) => {
            let r = a.radius.ok_or(Refusal::TurntableNoRadius)?;
            (0..frames.len()).map(|k| turntable_pose(k, deg, r)).collect()
        }
        (None, Some(p)) => {
            let p = load_poses(p)?;
            if p.len() != frames.len() {
                return Err(Refusal::PoseCount { poses: p.len(), frames: frames.len() });
            }
            p
        }
        _ if frames.len() == 1 => vec![Transform::IDENTITY],
        _ => return Err(Refusal::ManyFramesNoPose { frames: frames.len() }),
    };
    // Intrinsics: all four flags override; a partial set is refused; else each input's own.
    let flags = match a.k {
        [Some(fx), Some(fy), Some(ppx), Some(ppy)] => Some([fx, fy, ppx, ppy]),
        [None, None, None, None] => None,
        _ => return Err(Refusal::PartialIntrinsics),
    };
    let source = if flags.is_some() { "flags" } else if kind == "raw" { "meta" } else { "record" };
    let mut intr = Vec::with_capacity(frames.len());
    for (index, l) in frames.iter().enumerate() {
        let i = match flags {
            Some(k) => Intrinsics::from_rect_params(l.frame.width, l.frame.height, k),
            None => l.intrinsics.ok_or(Refusal::NoIntrinsics { index })?,
        };
        intr.push(i);
    }
    let trunc = a.trunc.unwrap_or(3.0 * a.voxel);
    let mut t = Tsdf::new(a.voxel, trunc, Tol::DEFAULT).map_err(Refusal::Volume)?;
    for (k, l) in frames.iter().enumerate() {
        t.integrate(&l.frame, l.scale, &intr[k], &poses[k]).map_err(|error| Refusal::Integrate { frame: k, error })?;
    }
    let sm = extract(&t).map_err(Refusal::Extract)?;
    let loops: Result<Vec<Vec<u32>>, Topo> = sm.mesh.boundary_loops();
    let band = Tol { lin: a.band, ang: BAND_DEG.to_radians() };
    let plane: Result<Fit<Plane>, FitError> = fit_plane(&sm.mesh.positions, band);
    let (mut cp, mut cn) = (Vec::new(), Vec::new());
    for (i, &p) in sm.mesh.positions.iter().enumerate() {
        if plane.as_ref().map_or(true, |f| f.shape.distance(p).abs() > a.band) {
            cp.push(p);
            cn.push(sm.normals[i]);
        }
    }
    let cyl: Result<Fit<Cylinder>, FitError> = fit_cylinder(&cp, &cn, band);

    // The PLY: scan_core's mesh into mesh_core's writer (one field-wise copy while scan_core carries its own
    // geom/mesh seam; the seat's shim swap makes both the same geom_core types).
    let mesh = mesh_core::Mesh {
        positions: sm.mesh.positions.iter().map(|p| mesh_core::Vec3::new(p.x, p.y, p.z)).collect(),
        triangles: sm.mesh.triangles.clone(),
    };
    let out = a.out.as_deref().unwrap_or_default();
    std::fs::write(out, mesh_core::ply::write(&mesh)).map_err(|e| Refusal::Io { path: out.into(), error: e.to_string() })?;

    let k0 = intr[0];
    let (cmin, cmean) = if sm.confidence.is_empty() {
        (0.0, 0.0)
    } else {
        let min = sm.confidence.iter().cloned().fold(f32::INFINITY, f32::min);
        (min, sm.confidence.iter().map(|&c| c as f64).sum::<f64>() / sm.confidence.len() as f64)
    };
    let mut r = String::new();
    let _ = writeln!(r, "input={kind} path={input}");
    let _ = writeln!(
        r,
        "intrinsics={source} width={} height={} fx={} fy={} ppx={} ppy={} units_um={}",
        k0.width,
        k0.height,
        k0.fx,
        k0.fy,
        k0.ppx,
        k0.ppy,
        frames[0].scale.micrometres()
    );
    let pose_src = if a.turntable.is_some() { "turntable" } else if a.poses.is_some() { "file" } else { "identity" };
    let _ = writeln!(r, "frames={} poses={pose_src} voxel_mm={} trunc_mm={}", sm.frames, a.voxel, trunc);
    let _ = writeln!(r, "vertices={}", sm.mesh.positions.len());
    let _ = writeln!(r, "triangles={}", sm.mesh.triangles.len());
    let _ = writeln!(r, "confidence: min={cmin:.3} mean={cmean:.3}");
    let loops_s = match &loops {
        Ok(l) => format!("{}", l.len()),
        Err(e) => format!("refused:{e:?}"),
    };
    let _ = writeln!(r, "boundary_loops={loops_s}");
    match &plane {
        Ok(f) => {
            let _ = writeln!(
                r,
                "plane: normal={} d={:.3} rms_mm={:.3} max_mm={:.3} inliers={} outliers={}",
                v3(f.shape.normal),
                f.shape.d,
                f.rms_mm,
                f.max_mm,
                f.inliers,
                f.outliers
            );
        }
        Err(e) => {
            let _ = writeln!(r, "plane: refused {e:?}");
        }
    }
    match &cyl {
        Ok(f) => {
            let _ = writeln!(
                r,
                "cylinder: axis={} point={} r={:.3} rms_mm={:.3} max_mm={:.3} inliers={} outliers={}",
                v3(f.shape.axis),
                v3(f.shape.point),
                f.shape.radius,
                f.rms_mm,
                f.max_mm,
                f.inliers,
                f.outliers
            );
        }
        Err(e) => {
            let _ = writeln!(r, "cylinder: refused {e:?}");
        }
    }
    let plane_s = plane.as_ref().map_or("refused".into(), |f| format!("{:.3} normal={}", f.rms_mm, v3(f.shape.normal)));
    let cyl_s = cyl.as_ref().map_or("refused".into(), |f| format!("{:.3} rms={:.3}", f.shape.radius, f.rms_mm));
    let _ = writeln!(
        r,
        "scan: frames={} verts={} tris={} loops={loops_s} plane rms={plane_s} cyl r={cyl_s}",
        sm.frames,
        sm.mesh.positions.len(),
        sm.mesh.triangles.len()
    );
    Ok(r)
}

fn main() {
    let a = parse_args();
    match run(&a) {
        Ok(report) => {
            print!("{report}");
            if let Some(p) = &a.report {
                if let Err(e) = std::fs::write(p, &report) {
                    eprintln!("scan: refused {:?}", Refusal::Io { path: p.clone(), error: e.to_string() });
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("scan: refused {e:?}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::turntable_pose;
    use scan_core::Vec3;
    use scan_core::fixture::orbit;

    /// The turntable stop-gap is the fixture's orbit seen from frame 0: a table turning counter-clockwise from
    /// above by k·D is a camera orbiting at azimuth −k·D, and its pose in frame 0's camera frame is P0⁻¹·Pk.
    #[test]
    fn turntable_is_the_orbit_seen_from_frame_zero() {
        let (c, r, d) = (Vec3::new(-20.0, 30.0, 50.0), 600.0, 25.0f64);
        let p0 = orbit(c, r, 0.0, 0.0).inverse();
        let probes = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, -20.0, 300.0), Vec3::new(-50.0, 40.0, 900.0)];
        for k in 0..5usize {
            let pk = orbit(c, r, 0.0, -(k as f64 * d).to_radians());
            let t = turntable_pose(k, d, r);
            for &p in &probes {
                let a = t.apply_point(p);
                let b = p0.apply_point(pk.apply_point(p));
                assert!((a - b).mag() < 1e-9, "k={k} p={p:?}: {a:?} vs {b:?}");
            }
        }
    }
}
