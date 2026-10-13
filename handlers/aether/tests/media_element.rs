//! AETHERMEDIA (LEDGER SR107): one test per element rule — HTMLMediaElement's readyState /
//! networkState machine and its events (HTML §4.8.11), `loop`, seeking, `volume` / `muted` /
//! `playbackRate` on the wire, `<source>` selection by type, and the image side's `srcset` /
//! `sizes` / `<picture>` / `loading=lazy` selection.
//!
//! Stria is played by hand: its replies (`MediaOpened`, `MediaFrame`, `MediaEnded`, `MediaError`)
//! are fed to the engine in the order Stria sends them, so every event sequence below is
//! deterministic. No binary fixture: the frames are solid 4x4 RGBA, the pages are inline.

use aether::AetherEngine;
use aether::dom::NodeRef;
use aether::media::{self, State};
use bandy::SMessage;

const PAGE: &str = "file:///page/index.html";

fn engine_with(html: &str) -> AetherEngine {
    let mut e = AetherEngine::new();
    e.load_html_styled(PAGE, html, &[], true);
    e
}

fn node(e: &AetherEngine, i: usize) -> NodeRef {
    e.media_elements()[i].node.clone()
}

fn opened(url: &str, duration_ns: u64, w: u32, h: u32) -> SMessage {
    SMessage::MediaOpened {
        url: url.into(),
        duration_ns,
        width: w,
        height: h,
        video: if w > 0 { "V_VP8".into() } else { String::new() },
        audio: "A_VORBIS".into(),
        real_video: w > 0,
        audio_clock: true,
    }
}

fn frame(url: &str, pts_ns: i64) -> SMessage {
    SMessage::MediaFrame { url: url.into(), pts_ns, width: 4, height: 4, rgba: vec![200; 64], levels: vec![] }
}

fn meter(url: &str, pts_ns: i64) -> SMessage {
    SMessage::MediaFrame { url: url.into(), pts_ns, width: 0, height: 0, rgba: vec![], levels: vec![[0.5, 0.25]] }
}

fn names(reqs: &[SMessage]) -> Vec<String> {
    reqs.iter()
        .map(|m| match m {
            SMessage::PlayMedia { url, .. } => format!("play {}", url.rsplit('/').next().unwrap()),
            SMessage::MediaPoster { url } => format!("poster {}", url.rsplit('/').next().unwrap()),
            SMessage::MediaSeek { url, position_ns } => format!("seek {} {position_ns}", url.rsplit('/').next().unwrap()),
            SMessage::MediaPause { url } => format!("pause {}", url.rsplit('/').next().unwrap()),
            SMessage::MediaResume { url } => format!("resume {}", url.rsplit('/').next().unwrap()),
            SMessage::MediaMute { url, muted } => format!("mute={muted} {}", url.rsplit('/').next().unwrap()),
            SMessage::MediaVolume { url, volume } => format!("volume={volume} {}", url.rsplit('/').next().unwrap()),
            SMessage::MediaRate { url, rate } => format!("rate={rate} {}", url.rsplit('/').next().unwrap()),
            other => format!("{other:?}"),
        })
        .collect()
}

/// Rule: readyState climbs HAVE_NOTHING → HAVE_METADATA → HAVE_ENOUGH_DATA with HTML's events in
/// order; a play fires `play` then `playing` at the first picture; `timeupdate` every 250 ms of
/// media time; at the end a `loop` element seeks to 0 and plays on (no `ended`, no `pause`).
#[test]
fn video_ready_state_events_and_loop() {
    let mut e = engine_with(r#"<html><body><video src="clip.webm" loop></video></body></html>"#);
    let url = "file:///page/clip.webm";
    let v = node(&e, 0);
    assert_eq!(names(&e.take_media_requests()), vec!["poster clip.webm"]);
    assert_eq!(media::events(&v), vec!["loadstart"]);
    assert_eq!((media::ready_state(&v), media::network_state(&v)), (media::HAVE_NOTHING, media::NETWORK_LOADING));

    e.on_media_message(&opened(url, 1_000_000_000, 4, 4));
    assert_eq!(media::ready_state(&v), media::HAVE_METADATA);
    e.on_media_message(&frame(url, 0));
    assert_eq!(media::ready_state(&v), media::HAVE_ENOUGH_DATA);
    assert_eq!(media::network_state(&v), media::NETWORK_IDLE);
    assert_eq!(
        media::events(&v),
        vec!["loadstart", "durationchange", "resize", "loadedmetadata", "loadeddata", "canplay", "canplaythrough"]
    );

    // the user's click
    assert!(media::toggle(&v));
    assert_eq!(names(&e.take_media_requests()), vec!["play clip.webm"]);
    for n in 1..10 {
        e.on_media_message(&frame(url, n * 100_000_000));
    }
    let ev = media::events(&v);
    assert_eq!(&ev[7..], &["play", "playing", "timeupdate", "timeupdate", "timeupdate"], "{ev:?}");
    assert!((media::current_time(&v) - 0.9).abs() < 1e-9);

    // the end, looping: seek to 0 and play on
    e.on_media_message(&SMessage::MediaEnded { url: url.into(), presented: 10, dropped: 0 });
    assert_eq!(names(&e.take_media_requests()), vec!["seek clip.webm 0", "play clip.webm"]);
    assert!(media::seeking(&v));
    assert_eq!(media::current_time(&v), 0.0);
    e.on_media_message(&frame(url, 0));
    assert!(!media::seeking(&v));
    let ev = media::events(&v);
    assert_eq!(&ev[12..], &["seeking", "timeupdate", "seeked"], "{ev:?}");
    assert!(!ev.contains(&"ended") && !ev.contains(&"pause"));
    assert_eq!(e.media_elements()[0].state, State::Playing);
}

/// Rule: without `loop` the end fires `timeupdate`, `pause`, `ended`; a seek reads the target at
/// once (`seeking`) and lands with `timeupdate` + `seeked`; a source Stria cannot open before
/// metadata is MEDIA_ERR_SRC_NOT_SUPPORTED (4) with networkState NO_SOURCE.
#[test]
fn video_end_seek_and_error() {
    let mut e = engine_with(r#"<html><body><video src="a.webm"></video><video src="missing.webm"></video></body></html>"#);
    let (a, b) = (node(&e, 0), node(&e, 1));
    let url = "file:///page/a.webm";
    let _ = e.take_media_requests();
    e.on_media_message(&opened(url, 500_000_000, 4, 4));
    e.on_media_message(&frame(url, 0));
    assert!(media::toggle(&a));
    e.on_media_message(&frame(url, 100_000_000));
    e.on_media_message(&SMessage::MediaEnded { url: url.into(), presented: 2, dropped: 0 });
    let ev = media::events(&a);
    assert_eq!(&ev[7..], &["play", "playing", "timeupdate", "pause", "ended"], "{ev:?}");
    assert_eq!(e.media_elements()[0].state, State::Ended);

    assert!(e.media_seek(0, 300_000_000));
    assert!(media::seeking(&a));
    assert!((media::current_time(&a) - 0.3).abs() < 1e-9, "currentTime is the target while seeking");
    e.on_media_message(&frame(url, 300_000_000));
    assert!(!media::seeking(&a));
    assert_eq!(&media::events(&a)[12..], &["seeking", "timeupdate", "seeked"]);

    e.on_media_message(&SMessage::MediaError { url: "file:///page/missing.webm".into(), error: "No such file".into() });
    assert_eq!(media::error_code(&b), 4);
    assert_eq!(media::network_state(&b), media::NETWORK_NO_SOURCE);
    assert_eq!(media::events(&b), vec!["loadstart", "error"]);
}

/// Rule: an audio-only stream is HAVE_ENOUGH_DATA at `MediaOpened` (no picture to wait for);
/// volume goes to Stria as `MediaVolume` (mute stays its own verb); out-of-range volume is
/// refused; playbackRate goes as `MediaRate` and 0 leaves the element playing; `preload=none`
/// suspends after `loadstart`, and a volume set before it opens rides its opening request.
#[test]
fn audio_volume_rate_and_preload() {
    let mut e = engine_with(r#"<html><body><audio src="s.wav" controls></audio><audio src="n.wav" preload="none"></audio></body></html>"#);
    let (s, n) = (node(&e, 0), node(&e, 1));
    let url = "file:///page/s.wav";
    assert_eq!(names(&e.take_media_requests()), vec!["poster s.wav"]);
    assert_eq!(media::events(&n), vec!["loadstart", "suspend"]);
    assert_eq!(media::network_state(&n), media::NETWORK_IDLE);

    e.on_media_message(&opened(url, 2_000_000_000, 0, 0));
    assert_eq!(media::ready_state(&s), media::HAVE_ENOUGH_DATA);
    assert_eq!(
        media::events(&s),
        vec!["loadstart", "durationchange", "loadedmetadata", "loadeddata", "canplay", "canplaythrough"]
    );

    assert_eq!(media::set_volume(&s, 0.0), Ok(true));
    assert_eq!(names(&e.take_media_requests()), vec!["volume=0 s.wav"]);
    assert_eq!(media::set_volume(&s, 0.5), Ok(true));
    assert_eq!(names(&e.take_media_requests()), vec!["volume=0.5 s.wav"]);
    assert_eq!(media::volume(&s), 0.5);
    assert_eq!(media::set_volume(&s, 1.5), Err(()));

    assert!(media::toggle(&s));
    assert_eq!(names(&e.take_media_requests()), vec!["play s.wav"]);
    e.on_media_message(&meter(url, 0));
    assert!(media::set_playback_rate(&s, 0.0));
    assert_eq!(names(&e.take_media_requests()), vec!["rate=0 s.wav"]);
    assert_eq!(e.media_elements()[0].state, State::Playing, "rate 0 is not paused");
    assert!(media::set_playback_rate(&s, 1.0));
    assert_eq!(names(&e.take_media_requests()), vec!["rate=1 s.wav"]);
    assert_eq!(media::playback_rate(&s), 1.0);
    let ev = media::events(&s);
    assert_eq!(&ev[6..], &["volumechange", "volumechange", "play", "playing", "ratechange", "ratechange"], "{ev:?}");

    // preload=none: the volume waits in the element and goes out after the opening request
    assert_eq!(media::set_volume(&n, 0.25), Ok(true));
    assert!(e.take_media_requests().is_empty(), "no session yet");
    assert!(media::toggle(&n));
    assert_eq!(names(&e.take_media_requests()), vec!["play n.wav", "volume=0.25 n.wav"]);
}

/// Rule: resource selection takes the first `<source>` whose `type` this engine can play; when
/// none can, nothing is asked of Stria and networkState is NO_SOURCE.
#[test]
fn source_selection_by_type() {
    let mut e = engine_with(
        r#"<html><body>
            <video id=a><source src="x.ogv" type="video/ogg"><source src="y.webm" type='video/webm; codecs="vp8, vorbis"'><source src="z.mp4"></video>
            <video id=b><source src="only.ogv" type="video/ogg; codecs=theora"></video>
        </body></html>"#,
    );
    assert_eq!(names(&e.take_media_requests()), vec!["poster y.webm"]);
    assert_eq!(e.media_elements().len(), 1);
    let doc = e.document.clone().unwrap();
    let b = doc.select("#b").unwrap().next().unwrap().as_node().clone();
    assert_eq!(media::network_state(&b), media::NETWORK_NO_SOURCE);
    assert_eq!(media::ready_state(&b), media::HAVE_NOTHING);
}

/// Rule (images): `srcset` parses per HTML (a comma inside a url is part of it), `sizes` turns
/// width descriptors into densities, `<picture>` takes its first `<source>` whose media matches
/// and whose type decodes (AVIF does, through av1_core; JPEG XL does not), else the img;
/// `loading=lazy` loads within 1250 px of the viewport.
#[test]
fn img_srcset_sizes_picture_and_lazy() {
    let set = aether::images::parse_srcset("a.png 1x, b,c.png 2x, bad.png 2q, d.png 400w");
    let urls: Vec<&str> = set.iter().map(|c| c.url.as_str()).collect();
    assert_eq!(urls, vec!["a.png", "b,c.png", "d.png"]);
    assert_eq!(aether::images::pick_srcset("/a-400.png 400w, /a-1200.png 1200w, /a-900.png 900w").as_deref(), Some("/a-900.png"));
    assert_eq!(aether::images::source_size(Some("(max-width: 600px) 300px, 50vw"), (800.0, 600.0)), 400.0);
    assert_eq!(aether::images::source_size(Some("(min-width: 600px) 300px, 50vw"), (800.0, 600.0)), 300.0);

    let doc = aether::dom::parse_html(
        r#"<html><body>
            <picture id=p>
              <source srcset="a.jxl" type="image/jxl">
              <source srcset="n.png" media="(max-width: 100px)">
              <source srcset="w-400.png 400w, w-1000.png 1000w" sizes="50vw">
              <img src="f.png">
            </picture>
            <picture id=q><source srcset="x.avif" type="image/avif"><img src="fallback.png"></picture>
            <picture id=r><source srcset="y.jxl" type="image/jxl"><img src="fallback.png" srcset="hi.png 2x"></picture>
            <img id=l src="lazy.png" loading="lazy">
        </body></html>"#,
    );
    let img = |sel: &str| doc.select(sel).unwrap().next().unwrap().as_node().clone();
    assert_eq!(aether::images::img_source(&img("#p img"), (800.0, 600.0)).as_deref(), Some("w-400.png"));
    assert_eq!(aether::images::img_source(&img("#q img"), (800.0, 600.0)).as_deref(), Some("x.avif"));
    assert_eq!(aether::images::img_source(&img("#r img"), (800.0, 600.0)).as_deref(), Some("fallback.png"));
    // an AVIF file is recognised by its ftyp brand (and decoded by av1_core, not the image crate)
    let mut ftyp = vec![0, 0, 0, 0x1c];
    ftyp.extend_from_slice(b"ftypmif1\0\0\0\0mif1avifmiaf");
    assert!(aether::images::is_avif(&ftyp));
    assert!(!aether::images::is_avif(b"\x89PNG\r\n\x1a\n0000000000"));
    let lazy = img("#l");
    assert!(aether::images::is_lazy(&lazy.as_element().unwrap().attributes.borrow()));
    assert!(aether::images::lazy_should_load(1800.0, 100.0, 0.0, 600.0));
    assert!(!aether::images::lazy_should_load(1900.0, 100.0, 0.0, 600.0));
}
