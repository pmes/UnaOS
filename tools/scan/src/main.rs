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
//!      [--turntable DEG --radius MM | --poses <file> | --track] [--voxel MM] [--trunc MM] [--band MM]
//!      [--against <design.vugdoc>] [--intent [<doc.vug>]] [--gdt <face>=<kind>:<tol>]... [--datum A=<face> B=<face> C=<face>]
//! ```
//!
//! Inputs (the measured roads from a D435i to this machine; librealsense v2.55.1):
//! - `.prb`: probe_core's recording (`probe_core::recording`: PRB1 records back to back, what the kernel's
//!   `/dev/probe/depth0` record is); the depth (Z16) records are fused, intrinsics and units from each record.
//!   The kernel's `tests sonderec` (SONDERECORD, LEDGER SR472) writes the rectified intrinsics into each record when
//!   the D435i's coefficients table parsed (`intr=<fx>,<fy>,<ppx>,<ppy>` on its wire); on `intr=none` the four flags supply them.
//! - `.csv`: rs-convert's depth CSV (`tools/convert/converters/converter-csv.cpp:83-90`): one line per image row,
//!   comma-separated `depth_frame::get_distance` values in METRES, 0 for a hole, no header, no intrinsics (the four
//!   flags are required). `--csv-raw` reads integer Z16 units instead. Quantised at `--units-um` (default 1000).
//! - `.raw`: realsense-viewer's snapshot (`common/stream-model.cpp:1505-1513`): `<base>_Depth.raw` is
//!   `height * stride` bytes of Z16 (`common/model-views.cpp:153-167`), and `--meta <base>_Depth_metadata.csv`
//!   (`model-views.cpp:169-200`) gives `Format`, `Resolution x/y`, `Bytes per pixel` and `Fx`/`Fy`/`PPx`/`PPy`. The
//!   metadata carries no depth units: the D400 default (1000 um) unless `--units-um`. Without `--meta`, `--width`
//!   `--height` and the four intrinsic flags.
//!
//! Poses: ONE depth frame (or `--frame N`) is fused from the identity pose. Several frames need `--track` or a pose
//! source, else the run is refused (`ManyFramesNoPose`); the three roads are exclusive (`PoseFlags`, also with `--frame`):
//! - `--track` (VUGSCANTRACK, LEDGER SR493): no pose source but the depth. scan_core's frame-to-model tracker (SR478,
//!   `Tracker::new`, its own pyramid and thresholds): frame 0 is integrated at the identity (the world is frame 0's
//!   camera), each later frame through `Tsdf::integrate_tracked`. A frame the tracker refuses as `Lost`, or one with no
//!   depth pixel (`Scan(NoDepth)`), is SKIPPED: never integrated, the tracker and the volume as they were, counted
//!   `lost` and named on its own report line; any other refusal ends the run (`Track { frame, error }`). Gravity is
//!   not passed: a .prb carries no accel record until SONDE-RECORD-IMU (VUG-SCAN-TOOL-GRAVITY owed). The report adds
//!   `track: frames= tracked= lost= fine_iters_max=` and one `track: frame=` line per frame with its inliers, inlier
//!   fraction, rms and iterations per level. VUGSCANLOOP (LEDGER SR500): the tracked poses then go through scan_core's
//!   `close_loop` (keyframes, loop candidates closed by ICP against a keyframe's own frame, the pose graph solved); on
//!   a closure every tracked frame is fused again at its corrected pose (`reintegrate`). The report adds
//!   `loop: keyframes= candidates= closures= before=<deg>/<mm> after=<deg>/<mm>` (the worst closure's disagreement
//!   with the poses, `none` without a closure: no truth on a recording) and one `loop: candidate` line each.
//! - `--turntable DEG --radius MM`, the stop-gap: the world is frame 0's camera frame; the turntable axis passes
//!   through (0, 0, MM) parallel to the image's vertical (camera +y); `DEG` > 0 is the table turning
//!   counter-clockwise seen from above, so frame k's camera sits rotated by +k·DEG (right-handed about camera +y)
//!   about that axis in the table's frame.
//! - `--poses <file>`: one line per fused frame, 12 numbers, the 3x4 camera → world `[R | t]` row-major, t in mm;
//!   blank lines and `#` lines skipped; R must be a rotation (|RᵀR − I| ≤ [`POSE_RIGID`], det > 0).
//!
//! The run is scan_core's: `Tsdf::new(voxel, trunc)` (default 4 mm, 3 × voxel), `integrate` per frame, `extract`,
//! then `fit_plane` over every vertex (band `--band` mm, default 3, and 15°) and scan_core's `detect` (VUGSCANSHAPES,
//! LEDGER SR497: Schnabel's multi-shape loop, `Floor::DEFAULT`) over every vertex with its TSDF normal. The mesh goes
//! out through `mesh_core::ply::write` (binary little-endian). The report (stdout, and `--report`) names the input, the
//! intrinsics' source, frames, vertices, triangles, confidence, boundary loops, the plane fit's residuals or its
//! refusal, the `cylinder:` line from detect's best-supported cylinder or `cylinder: none (support < floor)` (no
//! number for a cylinder the cloud does not support), `shapes: n=` and one `shape:` line per detected shape with its
//! support, fraction and rms, and a `scan:` line in scan_core's own test token shape. A refusal prints
//! `scan: refused <Name> {..}` and exits 1; usage exits 2.
//!
//! `--against <design.vugdoc>` (VUGSCANDEVIATION, LEDGER SR510): the made part against its model. doc_core reads and
//! replays the document (a failing statement refuses `Design`, none built `DesignEmpty`); every solid is tessellated by
//! scan_core's `design_of` at `DESIGN_CHORD_MM` into one design mesh with each triangle's face (VUGFACEDEVIATION, SR525:
//! the ONE builder, vug-view's too); scan_core's `measure` registers the scan to
//! it from the identity (the scan's world is the design's: a `--poses` / `--turntable` / one-frame scan; a coarse start
//! for a `--track` scan is owed, VUG-SCAN-DEVIATION-COARSE) and measures each vertex's signed deviation (positive proud,
//! negative short), the band being `--band`. The PLY then carries the ALIGNED positions (the design's frame), a
//! `float deviation` (mm, NaN off the design) and `uchar red green blue` from scan_core's `shade` in palette_core's
//! status colours (inside the band POSITIVE, short toward INFO, proud toward CRITICAL, off the design TEXT_SECONDARY).
//! The report adds `deviation: aligned rms= max= p95= inside= faces=<seen>/<total> ...` and one
//! `face: <solid>/<op>.<role>.<a>.<b>:<kind> n= max= rms= mean=` line per design face, by rms, worst first; a refused
//! registration prints `deviation: refused <Name> {..}` and the run exits 1. Without `--against` nothing changes. `--intent [<doc.vug>]` (VUGINTENTSCANWIRE, LEDGER SR515): [`intent_road`]. `--gdt <face>=<kind>:<tol>` (VUGSCANGDT, LEDGER SR542): [`gdt_road`]. `--datum A=<face> B=<face> C=<face>` (VUGSCANGDTDATUMS, LEDGER SR552): [`datum_road`].

use std::fmt::Write as _;
use std::path::Path;

use probe_core::recording::{self, RecordingError};
use probe_core::{FrameError, StreamKind};
use scan_core::{
    AlignParams, Deviation, DesignError, DeviationError, ScanMesh, colour, design_of, measure, shade, // VUGSCANDEVIATION (SR510); VUGFACEDEVIATION (SR525): the builder and the ramp are scan_core's
    DepthScale, Fit, FitError, Floor, Frame, Gap, Intrinsics, Keyframes, LoopParams, PixelFormat, Plane, Primitive,
    Reintegrate, ScanError, Shape, Shot, Tol, Topo, TrackError, Tracker, Transform, Tsdf, Vec3, close_loop, detect, extract,
    fit_plane, reintegrate,
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
    Track { frame: usize, error: TrackError },
    Extract(ScanError),
    // VUGSCANDEVIATION (SR510): the `--against` road's refusals.
    DesignRead(doc_core::Refusal),
    Design { line: u32, id: String, fault: doc_core::Fault },
    DesignEmpty,
    Tessellate { solid: String, error: brep_core::Refusal },
    DesignFaceId { solid: String, id: u64 },
    Deviation(DeviationError), GdtFace { face: String }, GdtKind { face: String, kind: &'static str }, GdtWithoutAgainst, GdtDatumFace { label: char, face: String }, GdtDatum { face: String, error: scan_core::gdt::DatumError }, // VUGSCANGDT (SR542): the `--gdt` road's refusals; VUGSCANGDTDATUMS (SR552) the `--datum` ones
}

fn usage() -> ! {
    eprintln!("usage: scan <in.prb | in.csv | in.raw> -o <out.ply> [--report <out.txt>]");
    eprintln!("            [--meta <x_Depth_metadata.csv>] [--width W --height H] [--csv-raw]");
    eprintln!("            [--fx F --fy F --ppx F --ppy F] [--units-um N] [--frame N]");
    eprintln!("            [--turntable DEG --radius MM | --poses <file> | --track] [--voxel MM] [--trunc MM] [--band MM]");
    eprintln!("            [--against <design.vugdoc>] [--intent [<doc.vug>]] [--gdt <face>=<kind>:<tol>]... [--datum A=<face> B=<face> C=<face>]");
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
    track: bool,
    voxel: f64,
    trunc: Option<f64>,
    band: f64,
    against: Option<String>, intent: Option<Option<String>>, gdt: Vec<(String, scan_core::Kind, f64)>, datum: Option<[String; 3]>, // VUGSCANDEVIATION (SR510); VUGSCANGDT (SR542) gdt; VUGSCANGDTDATUMS (SR552) datum
}

fn num<T: std::str::FromStr>(it: &mut impl Iterator<Item = String>) -> T {
    it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())
}

fn parse_args() -> Args {
    let mut a = Args { voxel: 4.0, band: 3.0, ..Args::default() };
    let mut it = std::env::args().skip(1).peekable();
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
            "--track" => a.track = true,
            "--voxel" => a.voxel = num(&mut it),
            "--trunc" => a.trunc = Some(num(&mut it)),
            "--band" => a.band = num(&mut it),
            "--against" => a.against = Some(it.next().unwrap_or_else(|| usage())), "--intent" => a.intent = Some(intent_doc(&mut it)), "--gdt" => a.gdt.push(gdt_spec(&mut it)), "--datum" => a.datum = Some(datum_spec(&mut it)),
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

/// SONDE-RECORD-IMU (SR491): a recording's motion records — (accel, gyro, an extrinsic carried) — read, not fused.
type MotionCount = (usize, usize, bool);

/// `.prb`: the recording's depth records, and its motion records counted.
fn load_prb(a: &Args, b: &[u8]) -> Result<(Vec<Loaded>, MotionCount), Refusal> {
    let rec = recording::read(b).map_err(Refusal::Recording)?;
    let accel = rec.motion.iter().filter(|m| m.record.kind == probe_core::ImuKind::Accel).count();
    let motion = (accel, rec.motion.len() - accel, rec.extrinsic().is_some());
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
    Ok((out, motion))
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
    scan_core::fixture::from_axes(x, y, z, origin)
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
        out.push(scan_core::fixture::from_axes(col(0), col(1), col(2), Vec3::new(v[3], v[7], v[11])));
    }
    Ok(out)
}

/// `--track` (VUGSCANTRACK, LEDGER SR493): frame 0 at the identity, each later frame tracked against the model and
/// integrated through `Tsdf::integrate_tracked` (SR478's road); a `Lost` or depthless frame skipped and named. Returns
/// the report's `track:` lines.
fn fuse_tracked(t: &mut Tsdf, frames: &[Loaded], intr: &[Intrinsics]) -> Result<String, Refusal> {
    let mut tr = Tracker::new(t, Tol::DEFAULT);
    let mut kf = Keyframes::new(LoopParams::for_volume(t)); // VUGSCANLOOP (SR500): the tracked poses, for close_loop
    let (mut tracked, mut lost, mut fine_max) = (0usize, 0usize, 0u32);
    let mut lines = String::new();
    for (k, l) in frames.iter().enumerate() {
        let anchor = t.frames() == 0;
        match t.integrate_tracked(&mut tr, &l.frame, l.scale, &intr[k], None) {
            Ok((pose, _)) if anchor => {
                kf.observe(k as u32, pose, None);
                let _ = writeln!(lines, "track: frame={k} pose=identity");
            }
            Ok((pose, _)) => {
                tracked += 1;
                kf.observe(k as u32, pose, tr.last());
                let Some(s) = tr.last() else { continue };
                fine_max = fine_max.max(s.level_iterations[2]);
                let frac = if s.valid > 0 { s.inliers as f64 / s.valid as f64 } else { 0.0 };
                let _ = writeln!(
                    lines,
                    "track: frame={k} inliers={} valid={} frac={frac:.3} rms_mm={:.3} iters={}/{}/{}",
                    s.inliers, s.valid, s.rms_mm, s.level_iterations[0], s.level_iterations[1], s.level_iterations[2]
                );
            }
            Err(TrackError::Scan(error)) if anchor => return Err(Refusal::Integrate { frame: k, error }),
            Err(TrackError::Lost { inliers, residual_mm }) => {
                lost += 1;
                let _ = writeln!(lines, "track: frame={k} lost inliers={inliers} rms_mm={residual_mm:.3}");
            }
            Err(TrackError::Scan(ScanError::NoDepth)) => {
                lost += 1;
                let _ = writeln!(lines, "track: frame={k} lost no_depth");
            }
            Err(error) => return Err(Refusal::Track { frame: k, error }),
        }
    }
    let head = format!("track: frames={} tracked={tracked} lost={lost} fine_iters_max={fine_max}\n", frames.len());
    let closed = close_tracked_loop(t, &kf, frames, intr)?;
    Ok(head + &lines + &closed)
}

/// VUGSCANLOOP (LEDGER SR500): the tracked road's loop closure. scan_core's `close_loop` over the observed poses; on a
/// closure the volume is replaced by every tracked frame fused again at its corrected pose (`reintegrate`; a refusal
/// is the run's `Integrate`, the volume as it was). A graph scan_core refuses (`GraphError`) is printed and the tracked
/// volume kept. Returns the report's `loop:` lines.
fn close_tracked_loop(t: &mut Tsdf, kf: &Keyframes, frames: &[Loaded], intr: &[Intrinsics]) -> Result<String, Refusal> {
    let shot = move |k: u32| frames.get(k as usize).map(|l| Shot { frame: &l.frame, scale: l.scale, intr: &intr[k as usize] });
    let lp = match close_loop(kf, t, shot) {
        Ok(lp) => lp,
        Err(e) => return Ok(format!("loop: keyframes={} refused {e:?}\n", kf.nodes().len())),
    };
    if lp.closures > 0 {
        *t = reintegrate(t, &lp.poses, shot).map_err(|e| match e {
            Reintegrate::Scan { frame, error } => Refusal::Integrate { frame: frame as usize, error },
            Reintegrate::NoFrame { frame } => Refusal::FrameIndex { frame: frame as usize, frames: frames.len() },
        })?;
    }
    let g = |x: Option<Gap>| x.map_or("none".to_string(), |g| format!("{:.3}/{:.3}", g.deg, g.mm));
    let mut out = format!(
        "loop: keyframes={} candidates={} closures={} before={} after={}\n",
        lp.keyframes,
        lp.candidates.len(),
        lp.closures,
        g(lp.before),
        g(lp.after)
    );
    for c in &lp.candidates {
        let _ = match &c.outcome {
            Ok(k) => writeln!(
                out,
                "loop: candidate new={} old={} closed frac={:.3} rms_mm={:.3}",
                c.new,
                c.old,
                if k.valid > 0 { k.inliers as f64 / k.valid as f64 } else { 0.0 },
                k.rms_mm
            ),
            Err(r) => writeln!(out, "loop: candidate new={} old={} refused {r:?}", c.new, c.old),
        };
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
    let mut motion: MotionCount = (0, 0, false);
    let mut frames = match kind.as_str() {
        "prb" => {
            let (f, m) = load_prb(a, &bytes)?;
            motion = m;
            f
        }
        "csv" => load_csv(a, &bytes)?,
        "raw" => load_raw(a, &bytes)?,
        _ => return Err(Refusal::InputKind { path: input.into() }),
    };
    if frames.is_empty() {
        return Err(Refusal::NoDepthFrames);
    }
    let posed = a.turntable.is_some() || a.poses.is_some();
    if (a.turntable.is_some() && a.poses.is_some()) || (posed && a.frame.is_some()) || (a.track && (posed || a.frame.is_some())) {
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
        _ if a.track => Vec::new(), // the tracked road's poses are the tracker's (fuse_tracked)
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
    let track = if a.track {
        Some(fuse_tracked(&mut t, &frames, &intr)?)
    } else {
        for (k, l) in frames.iter().enumerate() {
            t.integrate(&l.frame, l.scale, &intr[k], &poses[k]).map_err(|error| Refusal::Integrate { frame: k, error })?;
        }
        None
    };
    let sm = extract(&t).map_err(Refusal::Extract)?;
    let loops: Result<Vec<Vec<u32>>, Topo> = sm.mesh.boundary_loops();
    let band = Tol { lin: a.band, ang: BAND_DEG.to_radians() };
    let plane: Result<Fit<Plane>, FitError> = fit_plane(&sm.mesh.positions, band);
    // VUGSCANSHAPES (SR497): every shape the mesh supports above the floor; the cylinder line is detect's best-supported
    // cylinder (the shapes are ordered by support), never a fit over whatever is left off the plane.
    let detected: Result<Vec<Shape>, FitError> = detect(&sm.mesh.positions, &sm.normals, band, Floor::DEFAULT);
    let shapes: &[Shape] = detected.as_deref().unwrap_or(&[]);
    let cyl = shapes.iter().find_map(|s| if let Primitive::Cylinder(c) = s.kind { Some((c, s)) } else { None });

    // VUGSCANDEVIATION (SR510): the made part against its model, before anything is written.
    let dev = match &a.against {
        Some(p) => Some(against(p, &sm, a.band)?),
        None => None,
    }; let intent = intent_road(a, &detected)?; let gdt = gdt_road(a, &sm, dev.as_ref(), t.trunc_mm())?; // VUGINTENTSCANWIRE (SR515): before anything is written; VUGSCANGDT (SR542) gdt_road too

    // The PLY: scan_core's mesh IS mesh_core's (VUGSHIMSWAP SR473), straight into mesh_core's writer.
    let out = a.out.as_deref().unwrap_or_default();
    let ply = match &dev {
        Some((d, ..)) => deviation_ply(&sm, d, a.band),
        None => mesh_core::ply::write(&sm.mesh),
    };
    std::fs::write(out, ply).map_err(|e| Refusal::Io { path: out.into(), error: e.to_string() })?;

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
    let pose_src = if a.turntable.is_some() {
        "turntable"
    } else if a.poses.is_some() {
        "file"
    } else if a.track {
        "track"
    } else {
        "identity"
    };
    let _ = writeln!(r, "frames={} poses={pose_src} voxel_mm={} trunc_mm={}", sm.frames, a.voxel, trunc);
    r.push_str(track.as_deref().unwrap_or(""));
    let _ = writeln!(r, "motion={} accel={} gyro={} extr={} (read, not fused)", motion.0 + motion.1, motion.0, motion.1, if motion.2 { "yes" } else { "none" });
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
        Some((c, s)) => {
            let _ = writeln!(
                r,
                "cylinder: axis={} point={} r={:.3} rms_mm={:.3} max_mm={:.3} inliers={} outliers={}",
                v3(c.axis),
                v3(c.point),
                c.radius,
                s.rms_mm,
                s.max_mm,
                s.support,
                sm.mesh.positions.len() - s.support as usize
            );
        }
        None => {
            let _ = writeln!(r, "cylinder: none (support < floor)");
        }
    }
    shapes_report(&mut r, &detected);
    if let Some((d, names, _)) = &dev {
        deviation_report(&mut r, d, names); r.push_str(&gdt); // VUGSCANGDT (SR542): the gdt lines after the face lines
    } r.push_str(&intent); // VUGINTENTSCANWIRE (SR515)
    let plane_s = plane.as_ref().map_or("refused".into(), |f| format!("{:.3} normal={}", f.rms_mm, v3(f.shape.normal)));
    let cyl_s = cyl.as_ref().map_or("none".into(), |(c, s)| format!("{:.3} rms={:.3}", c.radius, s.rms_mm));
    let _ = writeln!(
        r,
        "scan: frames={} verts={} tris={} loops={loops_s} plane rms={plane_s} cyl r={cyl_s}",
        sm.frames,
        sm.mesh.positions.len(),
        sm.mesh.triangles.len()
    );
    Ok(r)
}

/// VUGSCANSHAPES (SR497): `shapes: n=<k>` and one `shape:` line per detected shape, by support.
fn shapes_report(r: &mut String, detected: &Result<Vec<Shape>, FitError>) {
    let shapes = match detected {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(r, "shapes: refused {e:?}");
            return;
        }
    };
    let _ = writeln!(r, "shapes: n={}", shapes.len());
    for s in shapes {
        let params = match s.kind {
            Primitive::Plane(p) => format!("normal={} d={:.3}", v3(p.normal), p.d),
            Primitive::Cylinder(c) => format!("axis={} point={} r={:.3}", v3(c.axis), v3(c.point), c.radius),
            Primitive::Sphere(p) => format!("centre={} r={:.3}", v3(p.centre), p.radius),
            Primitive::Cone(c) => {
                format!("apex={} axis={} half_angle_deg={:.3}", v3(c.apex), v3(c.axis), c.half_angle.to_degrees())
            }
        };
        let _ = writeln!(r, "shape: {} support={} frac={:.4} rms={:.3} spread_deg={:.3} {params}", s.kind.name(), s.support, s.fraction, s.rms_mm, s.spread_deg);
    }
}

/// VUGSCANDEVIATION (SR510): the document replayed, every solid tessellated into one design with each triangle's face
/// (named `<solid>/<op>.<role>.<a>.<b>:<kind>`), and the scan measured against it from the identity.
fn against(path: &str, sm: &ScanMesh, band: f64) -> Result<(Deviation, Vec<String>, scan_core::Design), Refusal> {
    let text = String::from_utf8(read(path)?).map_err(|e| Refusal::Io { path: path.into(), error: e.to_string() })?;
    let doc = doc_core::read_with(&text, &doc_host::DirLib::beside(std::path::Path::new(path))).map_err(Refusal::DesignRead)?; // VUGMAKEMACHINELIBFACE (SR548): `lib=<name>` beside the document
    let rp = doc_core::replay(&doc, doc.tol);
    if let Some(f) = rp.failures.first() {
        return Err(Refusal::Design { line: f.line, id: f.id.clone(), fault: f.fault.clone() });
    }
    if rp.solids.is_empty() {
        return Err(Refusal::DesignEmpty);
    }
    // VUGFACEDEVIATION (SR525): the ONE builder (scan_core `design_of`), its refusals on this face's names.
    let (design, names) = design_of(rp.solids.iter().map(|s| (s.id.as_str(), &s.brep)), doc.tol).map_err(|e| match e {
        DesignError::Tessellate { solid, error } => Refusal::Tessellate { solid, error },
        DesignError::FaceId { solid, id } => Refusal::DesignFaceId { solid, id },
        DesignError::Design(e) => Refusal::Deviation(e),
    })?;
    match measure(sm, &design, Transform::IDENTITY, &AlignParams::DEFAULT, band, Tol::DEFAULT) {
        Ok(d) => Ok((d, names, design)),
        Err(e) => {
            eprintln!("deviation: refused {e:?}");
            Err(Refusal::Deviation(e))
        }
    }
}

/// The PLY of the aligned scan with its `deviation` scalar and the band's colours.
fn deviation_ply(sm: &ScanMesh, d: &Deviation, band: f64) -> Vec<u8> {
    let moved = mesh_core::Mesh {
        positions: sm.mesh.positions.iter().map(|p| d.aligned.pose.apply_point(*p)).collect(),
        triangles: sm.mesh.triangles.clone(),
    };
    let scalar: Vec<f32> = d.per_vertex.iter().map(|x| x.map_or(f32::NAN, |v| v as f32)).collect();
    let rgb: Vec<[u8; 3]> = d.per_vertex.iter().map(|x| colour(shade(*x, band))).map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).collect(); // VUGFACEDEVIATION (SR525): scan_core's ramp
    mesh_core::ply::write_scalars(&moved, Some(("deviation", &scalar)), Some(&rgb))
}

/// VUGSCANDEVIATION (SR510): `deviation: aligned ...` and one `face:` line per design face, by rms, worst first.
fn deviation_report(r: &mut String, d: &Deviation, names: &[String]) {
    let s = &d.stats;
    let seen = d.faces.iter().filter(|f| f.n > 0).count();
    let _ = writeln!(
        r,
        "deviation: aligned rms={:.3} max={:.3} p95={:.3} inside={:.4} faces={seen}/{} matched={} unmatched={} band={} align_rms={:.3} pairs={} iters={}",
        s.rms_mm,
        s.max_mm,
        s.p95_mm,
        s.inside,
        d.faces.len(),
        s.matched,
        s.unmatched,
        s.band_mm,
        d.aligned.rms_mm,
        d.aligned.pairs,
        d.aligned.iters
    );
    for f in &d.faces {
        let name = names.get(f.face as usize).map(String::as_str).unwrap_or("?");
        let _ = writeln!(r, "face: {name} n={} max={:.3} rms={:.3} mean={:.3}", f.n, f.max_mm, f.rms_mm, f.mean_mm);
    }
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
        let p0 = orbit(c, r, 0.0, 0.0).invert().unwrap();
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

// ---- VUGINTENTSCANWIRE (LEDGER SR515): --intent, the scan's shapes as design intent ----

/// The angular sigma (degrees) `--intent` hands doc_core when no shape carries a measured normal spread (no shape, or
/// none with a finite `spread_deg` > 0): the documented default (doc_core tests/intent_scan.rs's SIGMA_DEG). Otherwise
/// [`intent_sigma`] takes the measured one (VUGSCANSHAPESPREAD, LEDGER SR526).
const INTENT_SIGMA_DEG: f64 = 0.5;

/// `--intent`'s optional document: the next argument, unless it is a flag or one of the inputs `run` reads.
fn intent_doc(it: &mut std::iter::Peekable<impl Iterator<Item = String>>) -> Option<String> {
    let take = it.peek().is_some_and(|s| {
        let ext = Path::new(s).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        !s.starts_with('-') && !matches!(ext.as_str(), "prb" | "csv" | "raw")
    });
    if take { it.next() } else { None }
}

/// The caller's copy the SR508 clause names (doc_core takes no scan_core): a detected `Shape` as a doc_core `ShapeRef`,
/// field for field. Nothing is flipped: the plane is `normal . p = d` on both sides (geom_core's `Plane`; doc_core
/// takes |n.o - d| and the unsigned axis angle, so detect's arbitrary normal sign is carried as is), the cylinder a
/// point on its axis, the axis and the radius, the cone its apex, its axis into the opening and its half-angle in
/// radians; mm throughout; one geom_core `Vec3`. support and rms_mm are carried; fraction, max_mm and inliers have no
/// field.
fn shape_ref(s: &Shape) -> doc_core::ShapeRef {
    use doc_core::ShapeParams;
    let params = match s.kind {
        Primitive::Plane(p) => ShapeParams::Plane { normal: p.normal, d: p.d },
        Primitive::Cylinder(c) => ShapeParams::Cylinder { point: c.point, axis: c.axis, radius: c.radius },
        Primitive::Sphere(p) => ShapeParams::Sphere { centre: p.centre, radius: p.radius },
        Primitive::Cone(c) => ShapeParams::Cone { apex: c.apex, axis: c.axis, half_angle: c.half_angle },
    };
    doc_core::ShapeRef { params, support: s.support, rms_mm: s.rms_mm }
}

/// (sigma_mm, sigma_deg): the largest shape fit rms (scan_core's own residual; the worst-fitting shape sets the one
/// number every shape is judged under; 0 with no shape) and the largest shape `spread_deg` (VUGSCANSHAPESPREAD SR526:
/// scan_core's RMS normal angle over the same inliers, like for like with the rms), [`INTENT_SIGMA_DEG`] only when no
/// shape has a finite spread above 0 (doc_core refuses a sigma_deg that is not > 0).
fn intent_sigma(shapes: &[Shape]) -> (f64, f64) {
    let spread = shapes.iter().map(|s| s.spread_deg).filter(|d| d.is_finite() && *d > 0.0).fold(0.0, f64::max);
    (shapes.iter().map(|s| s.rms_mm).fold(0.0, f64::max), if spread > 0.0 { spread } else { INTENT_SIGMA_DEG })
}

/// `--intent [<doc.vug>]` (VUGINTENTSCANWIRE, LEDGER SR515): detect's shapes (the run's own, never a second detect)
/// copied by [`shape_ref`] into doc_core's `Measurement::Shapes` at [`intent_sigma`], printed as
/// `doc_core::measurements_text` with every line prefixed `intent: ` (its `shapes: n=` header is not the report's).
/// With a document: doc_core reads it (`DesignRead` refuses), `suggest` proposes against the measurement's sigma,
/// `verify_all` disposes each in order (BP-vug-7), and `Batch::text` is appended verbatim (`intent: proposed= accepted=
/// refused=`, then one `accept` / `refuse` line per proposal). A detect refusal prints `intent: refused`. Without
/// `--intent`, empty.
fn intent_road(a: &Args, detected: &Result<Vec<Shape>, FitError>) -> Result<String, Refusal> {
    let Some(doc_path) = &a.intent else { return Ok(String::new()) };
    let shapes = match detected {
        Ok(s) => s,
        Err(e) => return Ok(format!("intent: refused {e:?}\n")),
    };
    let (sigma_mm, sigma_deg) = intent_sigma(shapes);
    let m = doc_core::Measurement::Shapes { shapes: shapes.iter().map(shape_ref).collect(), sigma_mm, sigma_deg };
    let mut out = String::new();
    for l in doc_core::measurements_text(&m).lines() {
        let _ = writeln!(out, "intent: {l}");
    }
    let Some(path) = doc_path else { return Ok(out) };
    let text = String::from_utf8(read(path)?).map_err(|e| Refusal::Io { path: path.clone(), error: e.to_string() })?;
    let doc = doc_core::read_with(&text, &doc_host::DirLib::beside(std::path::Path::new(path))).map_err(Refusal::DesignRead)?; // VUGMAKEMACHINELIBFACE (SR548): `lib=<name>` beside the document
    let ps = doc_core::suggest(&doc, &m);
    out.push_str(&doc_core::verify_all(&doc, &ps, &m, doc.tol).text());
    Ok(out)
}

#[cfg(test)]
mod intent_tests {
    use super::{INTENT_SIGMA_DEG, intent_sigma, shape_ref};
    use doc_core::ShapeParams;
    use scan_core::fixture::Scene;
    use scan_core::{Cone, Cylinder, Plane, Primitive, Shape, Vec3};

    fn shape(kind: Primitive, support: u32, rms_mm: f64) -> Shape {
        Shape { kind, inliers: Vec::new(), support, fraction: 0.5, rms_mm, max_mm: 2.0 * rms_mm, spread_deg: 0.0 }
    }

    /// The fixture's cylinder (Scene::bench's r 40 post at (160, 0), axis +z) copied to doc_core and read back to 1e-9;
    /// a plane with a downward normal is carried unflipped, a cone keeps its half-angle in radians; the sigma is the
    /// largest rms.
    #[test]
    fn the_copy_round_trips_the_fixture_shapes() {
        let post = Scene::bench().cylinders[0];
        let z = Vec3::new(0.0, 0.0, 1.0);
        let c = Cylinder { point: post.base, axis: z, radius: post.radius };
        let r = shape_ref(&shape(Primitive::Cylinder(c), 1234, 0.21));
        let ShapeParams::Cylinder { point, axis, radius } = r.params else { panic!("not a cylinder: {:?}", r.params) };
        assert!((point - post.base).mag() < 1e-9, "point {point:?}");
        assert!((axis - z).mag() < 1e-9, "axis {axis:?}");
        assert!((radius - post.radius).abs() < 1e-9, "radius {radius}");
        assert_eq!((r.support, r.rms_mm, r.kind()), (1234, 0.21, "cylinder"));
        let pl = Plane { normal: Vec3::new(0.0, 0.0, -1.0), d: -100.0 };
        assert_eq!(shape_ref(&shape(Primitive::Plane(pl), 9, 0.1)).params, ShapeParams::Plane { normal: pl.normal, d: -100.0 });
        let k = Cone { apex: Vec3::new(1.0, 2.0, 3.0), axis: Vec3::new(0.0, 1.0, 0.0), half_angle: 0.3 };
        assert_eq!(
            shape_ref(&shape(Primitive::Cone(k), 7, 0.05)).params,
            ShapeParams::Cone { apex: k.apex, axis: k.axis, half_angle: 0.3 }
        );
        let ss = [shape(Primitive::Plane(pl), 9, 0.1), shape(Primitive::Cylinder(c), 1234, 0.21)];
        assert_eq!(intent_sigma(&ss), (0.21, INTENT_SIGMA_DEG));
        assert_eq!(intent_sigma(&[]), (0.0, INTENT_SIGMA_DEG));
        // VUGSCANSHAPESPREAD (SR526): the largest measured spread is the sigma_deg.
        let spread = |s: Shape, d: f64| Shape { spread_deg: d, ..s };
        let ss = [spread(shape(Primitive::Plane(pl), 9, 0.1), 0.4), spread(shape(Primitive::Cylinder(c), 1234, 0.21), 1.7)];
        assert_eq!(intent_sigma(&ss), (0.21, 1.7));
    }
}

// ---- VUGSCANGDT (LEDGER SR542): --gdt, the deviation read as ISO 1101 tolerances ----

/// `--gdt <face>=<kind>:<tol>`: the face as its `face:` line names it (split at the LAST `=`), the kind (flatness,
/// cylindricity, position) and the tolerance in mm (finite, above 0, split at the LAST `:`); anything else is usage.
fn gdt_spec(it: &mut impl Iterator<Item = String>) -> (String, scan_core::Kind, f64) {
    let s = it.next().unwrap_or_else(|| usage());
    let Some((face, rest)) = s.rsplit_once('=') else { usage() };
    let Some((kind, tol)) = rest.rsplit_once(':') else { usage() };
    let kind = scan_core::Kind::parse(kind).unwrap_or_else(|| usage());
    let tol: f64 = tol.parse().unwrap_or_else(|_| usage());
    if face.is_empty() || !(tol.is_finite() && tol > 0.0) {
        usage()
    }
    (face.to_string(), kind, tol)
}

/// `gdt:` per design face (face-index order) after `--against`'s `face:` lines: the nominal surface, the form vertices
/// `n=<kept>/<matched>` (kept outside the edge margin, the fused volume's truncation `margin`, of the vertices paired
/// with the face), flatness (a plane) or cylindricity and position (a cylinder), the nominal-relative `band=` (no verdict), and
/// a `verdict=<kind>:<tol> PASS|FAIL|UNMEASURED` per callout naming the face; then `gdt: callouts= pass= fail=`.
/// A FAIL is a printed measurement (the run exits 0, BP-vug-5); an unknown face (`GdtFace`), a kind its surface does
/// not take (`GdtKind`) and `--gdt` without `--against` (`GdtWithoutAgainst`) are refused before anything is written.
/// Without `--against`, nothing.
fn gdt_road(a: &Args, sm: &ScanMesh, dev: Option<&(Deviation, Vec<String>, scan_core::Design)>, margin: f64) -> Result<String, Refusal> {
    let Some((d, names, design)) = dev else {
        return if a.gdt.is_empty() && a.datum.is_none() { Ok(String::new()) } else { Err(Refusal::GdtWithoutAgainst) }; // VUGSCANGDTDATUMS (SR552): `--datum` needs `--against` too
    };
    let mut g = scan_core::gdt(sm, design, d, margin, Tol::DEFAULT); let frame = datum_road(a, sm, design, d, names, margin, &mut g)?; // VUGSCANGDTDATUMS (SR552): position read in the datum frame
    let mut calls: Vec<(usize, scan_core::Kind, f64)> = Vec::with_capacity(a.gdt.len());
    for (face, kind, tol) in &a.gdt {
        let Some(i) = names.iter().position(|n| n == face) else { return Err(Refusal::GdtFace { face: face.clone() }) };
        if !g.get(i).is_some_and(|f| f.accepts(*kind)) {
            return Err(Refusal::GdtKind { face: face.clone(), kind: kind.name() });
        }
        calls.push((i, *kind, *tol));
    }
    let mm = |v: Option<f64>| v.map_or_else(|| "none".to_string(), |x| format!("{x:.3}"));
    let (mut pass, mut fail) = (0usize, 0usize);
    let mut r = frame; // VUGSCANGDTDATUMS (SR552): the frame line first
    for f in &g {
        let name = names.get(f.face as usize).map(String::as_str).unwrap_or("?");
        let _ = write!(r, "gdt: {name} surface={} n={}/{}", f.nominal.name(), f.n, f.matched);
        match f.nominal {
            scan_core::Nominal::Plane(p) => {
                let _ = write!(r, " normal={} d={:.3} flatness={} band={}", v3(p.normal), p.d, mm(f.flatness_mm), mm(f.band_mm));
            }
            scan_core::Nominal::Cylinder(c) => {
                let _ = write!(
                    r,
                    " axis={} point={} r={:.3} cylindricity={} position={} band={}",
                    v3(c.axis),
                    v3(c.point),
                    c.radius,
                    mm(f.cylindricity_mm),
                    mm(f.position_mm),
                    mm(f.band_mm)
                );
            }
            scan_core::Nominal::Other => {}
        }
        r.push_str(&f.minzone_tokens()); for &(_, kind, tol) in calls.iter().filter(|c| c.0 == f.face as usize) { // VUGSCANGDTMINZONE (SR554): the min-zone token before the verdicts
            let v = match f.verdict(kind, tol) {
                scan_core::Verdict::Pass { .. } => {
                    pass += 1;
                    "PASS"
                }
                scan_core::Verdict::Fail { .. } => {
                    fail += 1;
                    "FAIL"
                }
                scan_core::Verdict::Unmeasured => "UNMEASURED",
            };
            let _ = write!(r, " verdict={}:{tol} {v}", kind.name());
        }
        r.push('\n');
    }
    let _ = writeln!(r, "gdt: callouts={} pass={pass} fail={fail}", calls.len());
    Ok(r)
}

// ---- VUGSCANGDTDATUMS (LEDGER SR552): --datum, the ISO 5459 datum frame position is read in ----

/// `--datum A=<face> B=<face> C=<face>`: exactly the three following arguments, labelled A, B, C in that order, each
/// face as its `face:` line names it (split at the FIRST `=`); anything else is usage.
fn datum_spec(it: &mut impl Iterator<Item = String>) -> [String; 3] {
    let mut out: [String; 3] = Default::default();
    for (k, label) in ["A=", "B=", "C="].iter().enumerate() {
        let s = it.next().unwrap_or_else(|| usage());
        let Some(face) = s.strip_prefix(*label) else { usage() };
        if face.is_empty() {
            usage()
        }
        out[k] = face.to_string();
    }
    out
}

/// With `--datum`: the datum faces by name (an unknown one refused as `GdtDatumFace` by label and name), the frame
/// scan_core `gdt::frame_of` builds (measured from the scan's datum faces at the aligned pose, nominal from the design;
/// a refusal as `GdtDatum` with the datum's name), every cylinder face's position replaced by `gdt::position_in`, so
/// `position=` and its verdict read in the frame; returns the `gdt: frame A= B= C= residual=` line. Without, nothing.
fn datum_road(a: &Args, sm: &ScanMesh, design: &scan_core::Design, d: &Deviation, names: &[String], margin: f64, g: &mut [scan_core::FaceGdt]) -> Result<String, Refusal> {
    let Some(datum) = &a.datum else { return Ok(String::new()) };
    let mut faces = [0u32; 3];
    for (k, face) in datum.iter().enumerate() {
        let label = scan_core::gdt::DATUM_LABELS[k];
        let Some(i) = names.iter().position(|n| n == face) else { return Err(Refusal::GdtDatumFace { label, face: face.clone() }) };
        faces[k] = i as u32;
    }
    let fr = scan_core::gdt::frame_of(sm, design, d, faces, margin, Tol::DEFAULT).map_err(|error| {
        let k = scan_core::gdt::DATUM_LABELS.iter().position(|&l| l == error.label()).unwrap_or(0);
        Refusal::GdtDatum { face: datum[k].clone(), error }
    })?;
    for f in g.iter_mut() {
        if let (scan_core::Nominal::Cylinder(nc), Some(axis)) = (f.nominal, f.axis) {
            f.position_mm = Some(scan_core::gdt::position_in(&fr, &axis, &nc, f.extent));
        }
    }
    Ok(format!("gdt: frame A={} B={} C={} residual={:.3}\n", datum[0], datum[1], datum[2], fr.residual_mm))
}
