//! AETHERVIDEO unit tests: the engine against a FAKE Stria that answers on a real bandy bus
//! with test-pattern frames (`gneiss_pal::dsp::video::TestPattern`, the same renderer Stria's
//! `utp1` decoder uses), so what lands in the `<video>` box can be read back by its counter.

use bandy::{SMessage, Synapse};
use gneiss_pal::dsp::video::TestPattern;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::AetherEngine;
use crate::media::{self, State};

const W: u32 = 160;
const H: u32 = 120;
const FRAME_NS: i64 = 100_000_000; // 10 fps
const FRAMES: i64 = 4;

/// Stria's bus behaviour, minus decoding: poster → opened + frame 0; play → the remaining
/// frames then ended; seek → the frame at the target; a url containing "missing" → error.
struct FakeStria {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeStria {
    fn spawn(syn: Synapse) -> FakeStria {
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let mut rx = syn.subscribe();
        let thread = std::thread::spawn(move || {
            let mut pos: std::collections::HashMap<String, i64> = Default::default();
            let frame = |syn: &Synapse, url: &str, n: i64| {
                syn.fire(SMessage::MediaFrame {
                    url: url.to_string(),
                    pts_ns: n * FRAME_NS,
                    width: W,
                    height: H,
                    rgba: TestPattern::render(W, H, n as u32),
                    levels: vec![],
                });
            };
            while !st.load(Ordering::Acquire) {
                let msg = match rx.try_recv() {
                    Ok(m) => m,
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                };
                match msg {
                    SMessage::MediaPoster { url } | SMessage::PlayMedia { url, .. } if url.contains("missing") => {
                        syn.fire(SMessage::MediaError { url: url.clone(), error: format!("{url}: No such file or directory (os error 2)") });
                    }
                    SMessage::MediaPoster { url } => {
                        syn.fire(SMessage::MediaOpened {
                            url: url.clone(),
                            duration_ns: (FRAMES * FRAME_NS) as u64,
                            width: W,
                            height: H,
                            video: "V_UNAOS/TESTPATTERN".into(),
                            audio: String::new(),
                            real_video: true,
                            audio_clock: false,
                        });
                        frame(&syn, &url, 0);
                        pos.insert(url, 0);
                    }
                    SMessage::PlayMedia { url, .. } => {
                        let start = match pos.get(&url) {
                            Some(p) => p + 1,
                            None => {
                                syn.fire(SMessage::MediaOpened {
                                    url: url.clone(),
                                    duration_ns: (FRAMES * FRAME_NS) as u64,
                                    width: W,
                                    height: H,
                                    video: "V_UNAOS/TESTPATTERN".into(),
                                    audio: String::new(),
                                    real_video: true,
                                    audio_clock: false,
                                });
                                0
                            }
                        };
                        for n in start..FRAMES {
                            frame(&syn, &url, n);
                        }
                        pos.insert(url.clone(), FRAMES - 1);
                        syn.fire(SMessage::MediaEnded { url, presented: FRAMES as u64, dropped: 0 });
                    }
                    SMessage::MediaSeek { url, position_ns } => {
                        let n = (position_ns as i64 / FRAME_NS).min(FRAMES - 1);
                        frame(&syn, &url, n);
                        pos.insert(url, n);
                    }
                    _ => {}
                }
            }
        });
        FakeStria { stop, thread: Some(thread) }
    }
}

impl Drop for FakeStria {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The shell's job, synchronously: fire the engine's queued requests on the bus, then feed
/// Stria's replies back until `done` sees the one we wait for (5 s cap). Returns every reply.
fn pump(engine: &mut AetherEngine, syn: &Synapse, rx: &mut tokio::sync::broadcast::Receiver<SMessage>, done: impl Fn(&SMessage) -> bool) -> Vec<SMessage> {
    for req in engine.take_media_requests() {
        syn.fire(req);
    }
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(m) => {
                if matches!(m, SMessage::MediaOpened { .. } | SMessage::MediaFrame { .. } | SMessage::MediaEnded { .. } | SMessage::MediaError { .. }) {
                    engine.on_media_message(&m);
                    let stop = done(&m);
                    got.push(m);
                    if stop {
                        break;
                    }
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    got
}

fn bus() -> (Synapse, tokio::sync::broadcast::Receiver<SMessage>, FakeStria) {
    let syn = Synapse::new();
    let rx = syn.subscribe();
    let fake = FakeStria::spawn(syn.clone());
    (syn, rx, fake)
}

fn engine_with(html: &str) -> AetherEngine {
    let mut e = AetherEngine::new();
    e.load_html_styled("file:///page/index.html", html, &[], true);
    e
}

/// RGBA of the viewport rect (x, y, w, h) — the surface is BGRA.
fn crop(e: &AetherEngine, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for yy in y..y + h {
        for xx in x..x + w {
            let o = ((yy * e.width + xx) * 4) as usize;
            let p = &e.surface()[o..o + 4];
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out
}

fn video_box(e: &AetherEngine, i: usize) -> (u32, u32, u32, u32) {
    let el = &e.media_elements()[i];
    let (x, y, w, h) = e.box_of(&el.node).expect("video has a layout box");
    (x.round() as u32, y.round() as u32, w.round() as u32, h.round() as u32)
}

fn is_opened(m: &SMessage) -> bool {
    matches!(m, SMessage::MediaFrame { .. })
}

#[test]
fn layout_fires_media_poster_and_paints_the_first_frame() {
    let (syn, mut rx, _fake) = bus();
    let mut e = engine_with(r#"<html><body style="margin:0"><video src="clip.webm"></video><p>after</p></body></html>"#);
    // Before Stria answers: the poster request is queued, the box is the 300x150 default.
    let reqs = e.take_media_requests();
    assert_eq!(format!("{:?}", reqs), format!("{:?}", vec![SMessage::MediaPoster { url: "file:///page/clip.webm".into() }]));
    assert_eq!(video_box(&e, 0).2, 300);
    assert_eq!(video_box(&e, 0).3, 150);
    for r in reqs {
        syn.fire(r);
    }
    pump(&mut e, &syn, &mut rx, is_opened);
    e.render_frame();
    // MediaOpened gave the natural size: the box relaid out to 160x120 and frame 0 is in it,
    // pixel for pixel.
    let (x, y, w, h) = video_box(&e, 0);
    assert_eq!((w, h), (W, H));
    let got = crop(&e, x, y, w, h);
    assert_eq!(got, TestPattern::render(W, H, 0), "the box holds frame 0 exactly");
    assert_eq!(TestPattern::read_counter(&got, W, H), Some(0));
    let el = &e.media_elements()[0];
    assert_eq!(el.state, State::Paused);
    assert_eq!(el.natural, Some((W, H)));
    assert_eq!(el.duration_ns, (FRAMES * FRAME_NS) as u64);
    // No play request went out: a poster only.
    assert!(e.take_media_requests().is_empty());
}

#[test]
fn poster_attribute_wins_until_playback() {
    // A 2x2 PNG poster (solid red) as a data: URI — decoded synchronously at paint.
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    use base64::Engine as _;
    let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png));
    let (syn, mut rx, _fake) = bus();
    let mut e = engine_with(&format!(
        r#"<html><body style="margin:0"><video src="clip.webm" poster="{uri}" width="160" height="120"></video></body></html>"#
    ));
    pump(&mut e, &syn, &mut rx, is_opened);
    e.render_frame();
    let (x, y, w, h) = video_box(&e, 0);
    let got = crop(&e, x, y, w, h);
    // The poster (scaled by object-fit: contain into 160x120 → a 120x120 red square, centred).
    let px = |gx: u32, gy: u32| &got[((gy * w + gx) * 4) as usize..((gy * w + gx) * 4 + 3) as usize];
    assert_eq!(px(80, 60), &[255, 0, 0], "the poster paints, not frame 0");
    assert_eq!(px(5, 60), &[255, 255, 255], "contain letterboxes the square poster");
    // Play clears the show-poster flag: the frames paint from then on.
    assert!(e.media_play(0));
    pump(&mut e, &syn, &mut rx, |m| matches!(m, SMessage::MediaEnded { .. }));
    e.render_frame();
    let got = crop(&e, x, y, w, h);
    assert_eq!(TestPattern::read_counter(&got, W, H), Some((FRAMES - 1) as u32));
}

#[test]
fn play_paints_each_frame_then_ended_keeps_the_last() {
    let (syn, mut rx, _fake) = bus();
    let mut e = engine_with(r#"<html><body style="margin:0"><video src="clip.webm"></video></body></html>"#);
    pump(&mut e, &syn, &mut rx, is_opened);
    e.render_frame();
    let (x, y, w, h) = video_box(&e, 0);
    // A click on the box is the user's play.
    e.handle_event(crate::api::events::Event::MouseDown(x as f64 + 10.0, y as f64 + 10.0));
    e.handle_event(crate::api::events::Event::MouseUp(x as f64 + 10.0, y as f64 + 10.0));
    let reqs = e.take_media_requests();
    assert!(matches!(&reqs[..], [SMessage::PlayMedia { url, mime, .. }] if url == "file:///page/clip.webm" && mime == "video/webm"), "{reqs:?}");
    for r in reqs {
        syn.fire(r);
    }
    // Paint after every frame: the counter in the box follows the pts.
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let Ok(m) = rx.try_recv() else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        let ended = matches!(m, SMessage::MediaEnded { .. });
        if e.on_media_message(&m) {
            e.render_frame();
            if let SMessage::MediaFrame { pts_ns, .. } = m {
                let n = TestPattern::read_counter(&crop(&e, x, y, w, h), W, H);
                seen.push((pts_ns / FRAME_NS, n));
            }
        }
        if ended {
            break;
        }
    }
    assert_eq!(seen, (1..FRAMES).map(|n| (n, Some(n as u32))).collect::<Vec<_>>());
    assert_eq!(e.media_elements()[0].state, State::Ended);
    assert_eq!(TestPattern::read_counter(&crop(&e, x, y, w, h), W, H), Some((FRAMES - 1) as u32), "ended keeps the last frame");
    // Play after the end restarts from 0: a seek to 0 then play.
    assert!(e.media_play(0));
    let reqs = e.take_media_requests();
    assert!(matches!(&reqs[..], [SMessage::MediaSeek { position_ns: 0, .. }, SMessage::PlayMedia { .. }]), "{reqs:?}");
}

#[test]
fn click_toggles_pause_and_seek_paints_the_target() {
    let (syn, mut rx, _fake) = bus();
    let mut e = engine_with(r#"<html><body style="margin:0"><video src="clip.webm"></video></body></html>"#);
    pump(&mut e, &syn, &mut rx, is_opened);
    e.render_frame();
    let (x, y, w, h) = video_box(&e, 0);
    let click = |e: &mut AetherEngine| {
        e.handle_event(crate::api::events::Event::MouseDown(x as f64 + 5.0, y as f64 + 5.0));
        e.handle_event(crate::api::events::Event::MouseUp(x as f64 + 5.0, y as f64 + 5.0));
    };
    click(&mut e);
    assert_eq!(e.media_elements()[0].state, State::Playing);
    let _ = e.take_media_requests();
    click(&mut e);
    assert_eq!(format!("{:?}", e.take_media_requests()), format!("{:?}", vec![SMessage::MediaPause { url: "file:///page/clip.webm".into() }]));
    assert_eq!(e.media_elements()[0].state, State::Paused);
    // Seek to frame 2.
    assert!(e.media_seek(0, 2 * FRAME_NS as u64));
    pump(&mut e, &syn, &mut rx, is_opened);
    e.render_frame();
    assert_eq!(TestPattern::read_counter(&crop(&e, x, y, w, h), W, H), Some(2));
}

#[test]
fn media_error_text_paints_in_the_box() {
    let (syn, mut rx, _fake) = bus();
    let mut e = engine_with(r#"<html><body style="margin:0"><video src="missing.webm" width="320" height="100"></video></body></html>"#);
    pump(&mut e, &syn, &mut rx, |m| matches!(m, SMessage::MediaError { .. }));
    e.render_frame();
    let el = &e.media_elements()[0];
    assert!(matches!(&el.state, State::Error(t) if t.contains("No such file")), "{:?}", el.state);
    let (x, y, w, h) = video_box(&e, 0);
    let got = crop(&e, x, y, w, h);
    let dark = got.chunks(4).filter(|p| p[0] == 32 && p[1] == 32 && p[2] == 32).count();
    let light = got.chunks(4).filter(|p| p[0] > 128 && p[1] > 128 && p[2] > 128).count();
    assert!(dark > (w * h / 2) as usize, "the error box is filled");
    assert!(light > 20, "and the error text is drawn in it ({light} light px)");
}

#[test]
fn autoplay_policy_muted_runs_bare_does_not() {
    let mut e = engine_with(
        r#"<html><body>
            <video id=a src="a.webm" autoplay muted></video>
            <video id=b src="b.webm" autoplay></video>
            <audio id=c src="c.wav" autoplay></audio>
            <audio id=d src="d.wav" autoplay muted controls></audio>
        </body></html>"#,
    );
    let reqs = e.take_media_requests();
    let names: Vec<String> = reqs
        .iter()
        .map(|m| match m {
            SMessage::PlayMedia { url, .. } => format!("play {}", url.rsplit('/').next().unwrap()),
            SMessage::MediaPoster { url } => format!("poster {}", url.rsplit('/').next().unwrap()),
            SMessage::MediaMute { url, muted } => format!("mute={muted} {}", url.rsplit('/').next().unwrap()),
            other => format!("{other:?}"),
        })
        .collect();
    // AUDIOTRACK: an <audio> asks for its metadata like a <video> (Chromium's default
    // preload); a muted element's opening request is followed by MediaMute.
    assert_eq!(names, vec!["play a.webm", "mute=true a.webm", "poster b.webm", "poster c.wav", "play d.wav", "mute=true d.wav"]);
    let st: Vec<State> = e.media_elements().iter().map(|e| e.state.clone()).collect();
    assert_eq!(st, vec![State::Playing, State::Loading, State::Loading, State::Playing]);
}

/// A `<video>` whose source is a `<source>` child is still an inline replaced box sized by its
/// attributes (the oracle found it laid out as a full-width block: the child made it "contain
/// block content").
#[test]
fn video_with_source_children_keeps_its_attribute_size() {
    let e = engine_with(r#"<html><body style="margin:0"><video width="320" height="240"><source src="a.webm"><source src="b.webm"></video></body></html>"#);
    assert_eq!(video_box(&e, 0), (0, 0, 320, 240));
}

#[test]
fn source_selection_skips_types_stria_cannot_play() {
    let mut e = engine_with(
        r#"<html><body><video>
            <source src="x.ogv" type="video/ogg; codecs=theora">
            <source src="p.webm" type='video/webm; codecs="utp1"'>
            <source src="p-vp9.webm" type='video/webm; codecs="vp9"'>
            fallback text
        </video></body></html>"#,
    );
    assert_eq!(format!("{:?}", e.take_media_requests()), format!("{:?}", vec![SMessage::MediaPoster { url: "file:///page/p.webm".into() }]));
    assert!(media::can_play_type("video/webm; codecs=\"vp09.00.10.08\""));
    assert!(media::can_play_type("video/mp4; codecs=\"av01.0.04M.08, opus\""));
    assert!(!media::can_play_type("video/ogg"));
    assert!(!media::can_play_type("video/webm; codecs=\"theora\""));
    // The fallback text is not rendered: the replaced box has no children.
    let el = &e.media_elements()[0];
    let layout = e.layout_tree.as_ref().unwrap();
    let id = layout.node_map.iter().find(|(_, n)| **n == el.node).map(|(id, _)| *id).unwrap();
    assert!(layout.taffy.children(id).unwrap().is_empty());
}

#[test]
fn navigation_stops_the_previous_pages_sessions() {
    let mut e = engine_with(r#"<html><body><video src="clip.webm"></video><audio src="idle.wav" preload="none"></audio></body></html>"#);
    let _ = e.take_media_requests();
    e.load_html_styled("file:///page/next.html", "<html><body><p>next</p></body></html>", &[], true);
    // The idle (preload="none") <audio> never opened a session, so only the video stops.
    assert_eq!(format!("{:?}", e.take_media_requests()), format!("{:?}", vec![SMessage::MediaStop { url: "file:///page/clip.webm".into() }]));
    assert!(e.media_elements().is_empty());
}

#[test]
fn object_fit_geometry() {
    use media::object_fit_rect as f;
    // 4:3 picture in a 400x150 box.
    assert_eq!(f(1, (0.5, 0.5), 400.0, 150.0, 160.0, 120.0), (100.0, 0.0, 200.0, 150.0)); // contain
    assert_eq!(f(0, (0.5, 0.5), 400.0, 150.0, 160.0, 120.0), (0.0, 0.0, 400.0, 150.0)); // fill
    assert_eq!(f(2, (0.5, 0.5), 400.0, 150.0, 160.0, 120.0), (0.0, -75.0, 400.0, 300.0)); // cover
    assert_eq!(f(3, (0.5, 0.5), 400.0, 150.0, 160.0, 120.0), (120.0, 15.0, 160.0, 120.0)); // none
    assert_eq!(f(4, (0.5, 0.5), 80.0, 30.0, 160.0, 120.0), (20.0, 0.0, 40.0, 30.0)); // scale-down = contain (shrinks)
    assert_eq!(f(4, (0.5, 0.5), 400.0, 150.0, 160.0, 120.0), (120.0, 15.0, 160.0, 120.0)); // scale-down = none (fits)
    assert_eq!(f(1, (0.0, 1.0), 400.0, 150.0, 160.0, 120.0), (0.0, 0.0, 200.0, 150.0)); // left bottom
    assert_eq!(media::parse_object_position("right 25%"), (1.0, 0.25));
    assert_eq!(media::parse_object_position("top"), (0.5, 0.0));
    assert_eq!(media::parse_object_fit("Cover"), Some(2));
}

#[test]
fn object_fit_cover_paints_clipped_to_the_box() {
    let (syn, mut rx, _fake) = bus();
    // 160x120 frames into a 160x60 box with cover: scale 1, the middle 60 rows show.
    let mut e = engine_with(
        r#"<html><body style="margin:0"><video src="clip.webm" style="width:160px;height:60px;object-fit:cover"></video><div style="height:40px;background:#00ff00"></div></body></html>"#,
    );
    pump(&mut e, &syn, &mut rx, is_opened);
    e.render_frame();
    let (x, y, w, h) = video_box(&e, 0);
    assert_eq!((w, h), (160, 60));
    let got = crop(&e, x, y, w, h);
    let want = TestPattern::render(W, H, 0);
    let mid: Vec<u8> = want[(30 * W * 4) as usize..(90 * W * 4) as usize].to_vec();
    assert_eq!(got, mid, "cover crops to the centre rows, exact at scale 1");
    // Nothing spilled below the box onto the green div.
    let below = crop(&e, x, y + h, w, 1);
    assert!(below.chunks(4).all(|p| p[..3] == [0, 255, 0]), "cover clips to the content box");
}

#[test]
fn controls_strip_paints_play_then_pause() {
    let (syn, mut rx, _fake) = bus();
    let mut e = engine_with(r#"<html><body style="margin:0"><video src="clip.webm" controls></video></body></html>"#);
    pump(&mut e, &syn, &mut rx, is_opened);
    e.render_frame();
    let (x, y, w, h) = video_box(&e, 0);
    let strip_white = |e: &AetherEngine| {
        crop(e, x + 10, y + h - 32, 12, 32).chunks(4).filter(|p| p[..3] == [255, 255, 255]).count()
    };
    let play_px = strip_white(&e);
    assert!(play_px > 40, "a play triangle ({play_px} px)");
    // The top of the box is the frame, untouched by the strip.
    let top = crop(&e, x, y, w, 10);
    assert_eq!(top, TestPattern::render(W, H, 0)[..(w * 10 * 4) as usize]);
    assert!(e.media_play(0));
    e.damage_rects.push((0, 0, e.width, e.height));
    e.render_frame();
    let pause_px = strip_white(&e);
    assert_eq!(pause_px, 2 * 4 * 14, "two 4x14 pause bars");
}

// ------------------------------------------------------------------------------------------
// AUDIOTRACK (LEDGER SR45): <audio controls>, mute, http(s) media through the cache.

fn audio_opened(url: &str, duration_ns: u64) -> SMessage {
    SMessage::MediaOpened { url: url.into(), duration_ns, width: 0, height: 0, video: String::new(), audio: "wav".into(), real_video: false, audio_clock: true }
}

/// RGB at (x, y) of the engine surface (BGRA).
fn px(e: &AetherEngine, x: u32, y: u32) -> (u8, u8, u8) {
    let o = ((y * e.width + x) * 4) as usize;
    let p = &e.surface()[o..o + 4];
    (p[2], p[1], p[0])
}

#[test]
fn audio_controls_asks_for_metadata_paints_chromiums_control_and_follows_the_meters() {
    let mut e = engine_with(r#"<html><body style="margin:0"><audio controls src="a.wav"></audio></body></html>"#);
    assert_eq!(format!("{:?}", e.take_media_requests()), format!("{:?}", vec![SMessage::MediaPoster { url: "file:///page/a.wav".into() }]));
    assert_eq!(video_box(&e, 0), (0, 0, 300, 54), "Chromium's 300x54 audio control box");
    e.on_media_message(&audio_opened("file:///page/a.wav", 2_500_000_000));
    // an audio-only session's meter frame: no pixels, peak L/R per 50 ms
    e.on_media_message(&SMessage::MediaFrame { url: "file:///page/a.wav".into(), pts_ns: 1_000_000_000, width: 0, height: 0, rgba: vec![], levels: vec![[0.25, 0.5], [0.5, 0.75]] });
    let el = &e.media_elements()[0];
    assert_eq!((el.state.clone(), el.duration_ns, el.pts_ns, el.levels), (State::Paused, 2_500_000_000, 1_100_000_000, [0.5, 0.75]));
    assert!(el.frame.is_none() && el.natural.is_none(), "a meter frame is not a picture");
    e.damage_rects.push((0, 0, e.width, e.height));
    e.render_frame();
    // The pill (rgb 241,243,244), its rounded end leaving the page white in the corner, the
    // black play triangle, the timeline (played part dark, the rest grey), speaker and dots.
    assert_eq!(px(&e, 150, 8), (241, 243, 244));
    assert_eq!(px(&e, 0, 0), (255, 255, 255));
    assert_eq!(px(&e, 25, 27), (0, 0, 0), "play triangle");
    let f = 1.1 / 2.5;
    let split = 129.0 + (209.0 - 129.0) * f;
    assert_eq!(px(&e, split as u32 - 4, 27), (11, 11, 11), "played timeline");
    assert_eq!(px(&e, split as u32 + 4, 27), (88, 89, 89), "unplayed timeline");
    assert_eq!(px(&e, 236, 27), (0, 0, 0), "speaker");
    assert_eq!(px(&e, 272, 27), (0, 0, 0), "menu dot");
    // playing: the pause bars replace the triangle
    assert!(e.media_play(0));
    e.damage_rects.push((0, 0, e.width, e.height));
    e.render_frame();
    assert_eq!(px(&e, 23, 27), (0, 0, 0), "left pause bar");
    assert_eq!(px(&e, 27, 27), (241, 243, 244), "the gap between the bars");
}

#[test]
fn preload_none_waits_for_play_and_mute_follows_the_opening_request() {
    let mut e = engine_with(r#"<html><body><audio src="a.mp3" preload="none" muted></audio></body></html>"#);
    assert!(e.take_media_requests().is_empty());
    assert!(e.media_play(0));
    let r = e.take_media_requests();
    assert!(matches!(&r[..], [SMessage::PlayMedia { url, .. }, SMessage::MediaMute { url: u2, muted: true }] if url == "file:///page/a.mp3" && u2 == url), "{r:?}");
    assert!(media::can_play_type("audio/mpeg") && media::can_play_type("audio/ogg; codecs=opus") && media::can_play_type("audio/wav; codecs=1"));
}

/// A tiny HTTP/1.1 server for the cache tests: serves `body` at any path but `/missing`
/// (404); with `ranges` it answers `Range: bytes=a-b` with 206 + Content-Range, else 200.
/// Records each request's Range header ("" when none).
struct MiniHttp {
    port: u16,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}
impl MiniHttp {
    fn start(body: Vec<u8>, ranges: bool) -> MiniHttp {
        use std::io::{BufRead, BufReader, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut c) = c else { break };
                let mut rd = BufReader::new(c.try_clone().unwrap());
                let mut first = String::new();
                if rd.read_line(&mut first).is_err() {
                    continue;
                }
                let mut range = String::new();
                loop {
                    let mut h = String::new();
                    if rd.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("range:") {
                        range = v.trim().to_string();
                    }
                }
                log.lock().unwrap().push(range.clone());
                if first.contains("/missing") {
                    let _ = c.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    continue;
                }
                let n = body.len();
                let span = range.strip_prefix("bytes=").and_then(|r| r.split_once('-')).and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok().unwrap_or(n - 1))));
                match (ranges, span) {
                    (true, Some((a, _))) if a >= n => {
                        let _ = c.write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{n}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
                    }
                    (true, Some((a, b))) => {
                        let b = b.min(n - 1);
                        let _ = c.write_all(format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{b}/{n}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b + 1 - a).as_bytes());
                        let _ = c.write_all(&body[a..=b]);
                    }
                    _ => {
                        let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {n}\r\nConnection: close\r\n\r\n").as_bytes());
                        let _ = c.write_all(&body);
                    }
                }
            }
        });
        MiniHttp { port, seen }
    }
}

fn blob(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7919 % 251) as u8).collect()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("aether-media-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn fetch_uses_byte_ranges_resumes_a_partial_file_and_falls_back_to_one_get() {
    let body = blob(2500);
    let dir = scratch("fetch");
    // ranges honoured: 3 × 1000-byte requests
    let s = MiniHttp::start(body.clone(), true);
    let url = format!("http://127.0.0.1:{}/v.webm", s.port);
    let rep = media::fetch_to_cache(&url, &dir.join("a"), 1000).unwrap();
    assert_eq!((rep.bytes, rep.ranged, rep.requests, rep.resumed_from), (2500, true, 3, 0));
    assert_eq!(std::fs::read(dir.join("a")).unwrap(), body);
    assert_eq!(*s.seen.lock().unwrap(), vec!["bytes=0-999", "bytes=1000-1999", "bytes=2000-2999"]);
    // resume: a partial file of 1200 bytes → the fetch continues at byte 1200
    std::fs::write(dir.join("b.part"), &body[..1200]).unwrap();
    s.seen.lock().unwrap().clear();
    let rep = media::fetch_to_cache(&url, &dir.join("b"), 1000).unwrap();
    assert_eq!((rep.bytes, rep.ranged, rep.requests, rep.resumed_from), (2500, true, 2, 1200));
    assert_eq!(std::fs::read(dir.join("b")).unwrap(), body);
    assert_eq!(*s.seen.lock().unwrap(), vec!["bytes=1200-2199", "bytes=2200-3199"]);
    // no range support: the whole body from one 200, a stale partial file discarded
    let s2 = MiniHttp::start(body.clone(), false);
    std::fs::write(dir.join("c.part"), b"stale").unwrap();
    let rep = media::fetch_to_cache(&format!("http://127.0.0.1:{}/v.webm", s2.port), &dir.join("c"), 1000).unwrap();
    assert_eq!((rep.bytes, rep.ranged, rep.requests), (2500, false, 1));
    assert_eq!(std::fs::read(dir.join("c")).unwrap(), body);
    // a 404 is an error and leaves no cache file
    assert!(media::fetch_to_cache(&format!("http://127.0.0.1:{}/missing", s2.port), &dir.join("d"), 1000).is_err());
    assert!(!dir.join("d").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn http_media_is_cached_then_handed_to_stria_as_a_file_and_a_failure_paints() {
    let body = blob(5000);
    let s = MiniHttp::start(body.clone(), true);
    let dir = scratch("cache");
    media::set_cache_dir(&dir);
    let src = format!("http://127.0.0.1:{}/clip.webm", s.port);
    let missing = format!("http://127.0.0.1:{}/missing.webm", s.port);
    let mut e = engine_with(&format!(r#"<html><body><video src="{src}"></video><video src="{missing}"></video></body></html>"#));
    let key = format!("file://{}", media::cache_path(&src).display());
    let el = &e.media_elements()[0];
    assert_eq!((el.src.as_str(), el.url.as_str(), el.fetching), (src.as_str(), key.as_str(), true));
    assert!(media::cache_path(&src).starts_with(&dir));
    let name = media::cache_path(&src).file_name().unwrap().to_string_lossy().to_string();
    assert_eq!(name.len(), 64, "the cache file is named by the url's SHA-256");
    // nothing goes to Stria until the bytes are local
    let mut reqs = e.take_media_requests();
    let t0 = Instant::now();
    while reqs.len() < 2 && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
        reqs.extend(e.take_media_requests());
    }
    let key2 = format!("file://{}", media::cache_path(&missing).display());
    assert!(reqs.iter().any(|r| matches!(r, SMessage::MediaPoster { url } if *url == key)), "{reqs:?}");
    assert!(reqs.iter().any(|r| matches!(r, SMessage::MediaError { url, error } if *url == key2 && error.contains("404"))), "{reqs:?}");
    assert_eq!(std::fs::read(media::cache_path(&src)).unwrap(), body);
    // the error comes back over the bus like Stria's and paints in the box
    for r in &reqs {
        e.on_media_message(r);
    }
    assert!(matches!(&e.media_elements()[1].state, State::Error(t) if t.contains("404")));
    // a second page on the same url finds the cache: its poster goes out at once
    // (the registry is per engine thread: loading it stops the first page's sessions first)
    let e2 = &mut engine_with(&format!(r#"<html><body><video src="{src}"></video></body></html>"#));
    let r2 = e2.take_media_requests();
    assert_eq!(format!("{:?}", r2.last()), format!("{:?}", Some(SMessage::MediaPoster { url: key.clone() })), "{r2:?}");
    assert!(!e2.media_elements()[0].fetching);
    let _ = std::fs::remove_dir_all(&dir);
}
