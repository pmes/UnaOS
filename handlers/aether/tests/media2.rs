//! AETHERMEDIA2 (LEDGER SR134): one test per rule — Media Source Extensions (MediaSource /
//! SourceBuffer over a fragmented MP4 built here, the journal Stria's feed reads, readyState and
//! `waiting` from `buffered`, endOfStream to `ended`), the MSE type gate and its exceptions,
//! canPlayType with VP9 real, `<track>` WebVTT cues with `cuechange` and the caption box, the autoplay
//! policy (sticky activation, NotAllowedError, the play affordance), `preload=auto`,
//! playbackRate's range, `requestVideoFrameCallback`, and the Picture-in-Picture / captureStream
//! refusals.
//!
//! Stria is played by hand as in media_element.rs: its replies are fed to the engine in order, so
//! every sequence is deterministic. The MP4 is `demux_core::build::mp4` with `fragment` set (the
//! test-pattern builder Stria's tests use), cut at its box boundaries.

use aether::AetherEngine;
use aether::dom::NodeRef;
use aether::media;
use bandy::SMessage;
use gneiss_pal::dsp::demux::build::{self, Mp4Options};
use gneiss_pal::dsp::demux::fragment::{MediaSourceModel, Op};
use std::path::PathBuf;

fn settle(e: &mut AetherEngine) {
    for _ in 0..6 {
        if let Some(js) = e.js_engine.as_mut() {
            aether::event_loop::settle(js, 50);
        }
        e.tick();
    }
}

fn load(url: &str, html: &str) -> AetherEngine {
    let mut e = AetherEngine::new();
    e.load_html(url, html, true);
    settle(&mut e);
    e
}

fn eval(e: &mut AetherEngine, src: &str) -> String {
    let js = e.js_engine.as_mut().expect("page has a script engine");
    js.eval_string(src).unwrap_or_else(|err| format!("THROW {err}"))
}

fn node(e: &AetherEngine, i: usize) -> NodeRef {
    e.media_elements()[i].node.clone()
}

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("aethermedia2").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 2 s, 25 fps, 160x120 test-pattern fragmented MP4 (a keyframe and a fragment per second), cut
/// into its initialization segment and media segments.
fn fmp4() -> (Vec<u8>, Vec<Vec<u8>>) {
    let file = build::mp4(&[build::test_pattern_track(1, 160, 120, 25, 50, 25)], &Mp4Options { fragment: 25, ..Default::default() });
    let (mut init, mut media) = (Vec::new(), Vec::<Vec<u8>>::new());
    let mut p = 0;
    while p + 8 <= file.len() {
        let size = u32::from_be_bytes([file[p], file[p + 1], file[p + 2], file[p + 3]]) as usize;
        let bx = &file[p..p + size];
        match &file[p + 4..p + 8] {
            b"ftyp" | b"moov" => init.extend_from_slice(bx),
            b"moof" => media.push(bx.to_vec()),
            b"mdat" => media.last_mut().unwrap().extend_from_slice(bx),
            _ => {}
        }
        p += size;
    }
    (init, media)
}

fn js_bytes(name: &str, b: &[u8]) -> String {
    format!("var {name} = new Uint8Array([{}]);", b.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","))
}

fn frame(url: &str, pts_ns: i64) -> SMessage {
    SMessage::MediaFrame { url: url.into(), pts_ns, width: 4, height: 4, rgba: vec![200; 64], levels: vec![] }
}

fn opened(url: &str, duration_ns: u64) -> SMessage {
    SMessage::MediaOpened { url: url.into(), duration_ns, width: 160, height: 120, video: "utp1".into(), audio: String::new(), real_video: true, audio_clock: false }
}

/// Rule (MSE §2.4.1, §3.2, §3.5): a MediaSource attached through `URL.createObjectURL` opens
/// (`sourceopen`); `appendBuffer` runs updatestart → update → updateend with `updating` true then
/// false; the init segment brings HAVE_METADATA and the duration; a media segment brings `buffered`
/// [0, 1) and HAVE_ENOUGH_DATA; the journal Stria reads replays to the same `buffered`; at the
/// buffered end readyState drops and `waiting` fires; endOfStream makes the MediaSource "ended" and
/// Stria's `MediaEnded` ends the element.
#[test]
fn mse_appends_fragmented_mp4_and_the_journal_matches() {
    let (init, media) = fmp4();
    let html = format!(
        r#"<html><body><video id=v></video><script>
{} {} {}
window.log = [];
var v = document.getElementById('v');
var ms = new MediaSource();
window.ms = ms;
ms.onsourceopen = function () {{
    log.push('sourceopen:' + ms.readyState);
    var sb = ms.addSourceBuffer('video/mp4; codecs="utp1"');
    window.sb = sb;
    ['updatestart', 'update', 'updateend', 'error'].forEach(function (t) {{
        sb.addEventListener(t, function () {{ log.push(t + ':' + sb.updating); }});
    }});
    sb.appendBuffer(INIT);
    log.push('appending:' + sb.updating);
}};
ms.addEventListener('sourceended', function () {{ log.push('sourceended'); }});
v.src = URL.createObjectURL(ms);
log.push('url:' + v.src.slice(0, 5) + ':' + ms.readyState);
</script></body></html>"#,
        js_bytes("INIT", &init),
        js_bytes("SEG0", &media[0]),
        js_bytes("SEG1", &media[1])
    );
    let mut e = load("file:///page/mse.html", &html);
    assert_eq!(
        eval(&mut e, "log.join(' ')"),
        "url:blob::closed sourceopen:open appending:true updatestart:true update:false updateend:false"
    );
    let v = node(&e, 0);
    assert_eq!(media::ready_state(&v), media::HAVE_METADATA);
    assert_eq!(eval(&mut e, "String(ms.duration) + ' ' + v.duration + ' ' + sb.buffered.length"), "2 2 0");

    eval(&mut e, "sb.appendBuffer(SEG0)");
    settle(&mut e);
    assert_eq!(eval(&mut e, "sb.buffered.start(0) + '-' + sb.buffered.end(0) + ' ' + v.buffered.end(0)"), "0-1 1");
    assert_eq!(media::ready_state(&v), media::HAVE_ENOUGH_DATA);
    let ev = media::events(&v);
    assert_eq!(&ev[..], &["loadstart", "durationchange", "loadedmetadata", "loadeddata", "canplay", "canplaythrough"], "{ev:?}");

    // The bus key and the journal: Stria's feed applies these records to the same model.
    let reqs = e.take_media_requests();
    let key = reqs
        .iter()
        .find_map(|m| match m {
            SMessage::MediaPoster { url } => Some(url.clone()),
            _ => None,
        })
        .expect("MediaPoster for the MediaSource session");
    let path = key.strip_prefix("mediasource:").expect("mediasource: key");
    let journal = std::fs::read(path).unwrap();
    let mut model = MediaSourceModel::new();
    let mut p = 0;
    while let Ok(Some((op, used))) = Op::decode(&journal[p..]) {
        model.apply(&op);
        p += used;
    }
    assert_eq!(p, journal.len(), "whole records only");
    assert_eq!(model.buffered(), vec![(0, 1_000_000_000)]);

    // Stria plays: frames up to the buffered end; readyState falls and `waiting` fires.
    assert!(e.media_play(0));
    e.on_media_message(&opened(&key, 2_000_000_000));
    for n in 0..25 {
        e.on_media_message(&frame(&key, n * 40_000_000));
    }
    let ev = media::events(&v);
    assert!(ev.contains(&"playing") && ev.ends_with(&["waiting"]), "{ev:?}");
    assert!(media::ready_state(&v) <= media::HAVE_CURRENT_DATA);

    // the rest, then endOfStream: ready again, ended at Stria's MediaEnded
    eval(&mut e, "sb.onupdateend = function () { if (!sb.updating && ms.readyState === 'open' && sb.buffered.end(0) >= 2) { ms.endOfStream(); } }; sb.appendBuffer(SEG1);");
    settle(&mut e);
    assert_eq!(eval(&mut e, "ms.readyState + ' ' + ms.duration + ' ' + log.slice(-1)[0]"), "ended 2 sourceended");
    assert_eq!(media::ready_state(&v), media::HAVE_ENOUGH_DATA);
    for n in 25..50 {
        e.on_media_message(&frame(&key, n * 40_000_000));
    }
    e.on_media_message(&SMessage::MediaEnded { url: key.clone(), presented: 50, dropped: 0 });
    assert!(media::events(&v).ends_with(&["timeupdate", "pause", "ended"]), "{:?}", media::events(&v));
}

/// Rule (MSE §2.2 isTypeSupported, §2.4 exceptions, §3.5.3 append error): the type gate admits the
/// byte streams and codecs Stria decodes and refuses the stand-in codecs; addSourceBuffer before
/// "open" is InvalidStateError, an unsupported type NotSupportedError; appendBuffer while updating is
/// InvalidStateError; bytes that are not a segment fire `error` + `updateend` and end the stream.
#[test]
fn mse_type_gate_exceptions_and_append_error() {
    let mut e = load("file:///page/mse2.html", "<html><body><video id=v></video><script>window.log=[];</script></body></html>");
    assert_eq!(
        eval(&mut e, r#"[MediaSource.isTypeSupported('video/webm; codecs="vp9"'), MediaSource.isTypeSupported('video/webm; codecs="vp09.00.10.08"'), MediaSource.isTypeSupported('audio/webm; codecs="opus"'), MediaSource.isTypeSupported('video/mp4; codecs="av01.0.05M.08"'), MediaSource.isTypeSupported('video/mp4; codecs="avc1.42E01E"'), MediaSource.isTypeSupported('video/ogg'), MediaSource.isTypeSupported('video/x-matroska')].join()"#),
        "true,true,true,true,false,false,false"
    );
    assert_eq!(
        eval(&mut e, "var ms = new MediaSource(); try { ms.addSourceBuffer('video/webm'); 'no' } catch (x) { x.name + ' ' + ms.readyState }"),
        "InvalidStateError closed"
    );
    eval(&mut e, "document.getElementById('v').src = URL.createObjectURL(ms);");
    settle(&mut e);
    assert_eq!(eval(&mut e, "ms.readyState"), "open");
    assert_eq!(eval(&mut e, "try { ms.addSourceBuffer('video/mp4; codecs=\"avc1.42E01E\"'); 'no' } catch (x) { x.name }"), "NotSupportedError");
    assert_eq!(eval(&mut e, "try { ms.endOfStream('bogus'); 'no' } catch (x) { x.name }"), "TypeError");
    assert_eq!(
        eval(&mut e, "var sb = ms.addSourceBuffer('video/webm; codecs=\"vp9\"'); sb.onerror = function () { log.push('error:' + sb.updating); }; sb.onupdateend = function () { log.push('updateend'); }; ms.onsourceended = function () { log.push('sourceended'); }; sb.appendBuffer(new Uint8Array([0x1A, 0x45, 0xDF, 0xA3, 0x80, 0x18, 0x53, 0x80, 0x67, 0x81, 0x00, 0x1F, 0x43, 0xB6, 0x75, 0x80])); try { sb.appendBuffer(new Uint8Array(4)); 'no' } catch (x) { x.name + ' ' + sb.updating }"),
        "InvalidStateError true"
    );
    settle(&mut e);
    assert_eq!(eval(&mut e, "log.join(' ') + ' ' + ms.readyState"), "error:false updateend sourceended ended");
    assert_eq!(eval(&mut e, "document.getElementById('v').error.code"), "4", "decode error before metadata: src not supported");
}

/// Rule (HTML canPlayType; VP9CORE SR116): VP9 is decoded (probably); H.264 / HEVC play as the labelled
/// stand-in (maybe); no `codecs` is maybe; Ogg / Theora is not playable ("").
#[test]
fn can_play_type_reports_vp9_real_and_stand_ins_maybe() {
    let mut e = load("file:///page/cpt.html", "<html><body><video id=v></video></body></html>");
    assert_eq!(
        eval(&mut e, r#"var v = document.getElementById('v'); [v.canPlayType('video/webm; codecs="vp9, opus"'), v.canPlayType('video/mp4; codecs="vp09.00.10.08"'), v.canPlayType('video/mp4; codecs="avc1.42E01E"'), v.canPlayType('video/webm'), v.canPlayType('video/ogg; codecs="theora"')].join('|')"#),
        "probably|probably|maybe|maybe|"
    );
}

/// Rule (HTML §4.8.11.11, WebVTT): a `default` subtitles `<track>` is showing and loads its WebVTT
/// (`load`, readyState LOADED, its cues); as the clock moves into a cue it becomes active —
/// `cuechange` at the TextTrack and the `<track>`, `enter` at the cue — and the caption box paints its
/// text; leaving it clears the box. `addTextTrack` + `VTTCue` work the same way.
#[test]
fn webvtt_track_cues_cuechange_and_caption_box() {
    let dir = scratch("vtt");
    std::fs::write(dir.join("subs.vtt"), "WEBVTT\n\nNOTE a comment\n\none\n00:00.500 --> 00:01.500 line:90%\n<v Una>Hello &amp; welcome</v>\n\n00:02.000 --> 00:03.000\nSecond\nline two\n").unwrap();
    let url = format!("file://{}/index.html", dir.display());
    let html = r#"<html><body><video src="clip.webm"><track id=t kind=subtitles src="subs.vtt" srclang=en label=English default></video><script>
window.log = [];
var v = document.querySelector('video');
</script></body></html>"#;
    let mut e = load(&url, html);
    let v = node(&e, 0);
    let key = e.media_elements()[0].url.clone();
    assert_eq!(
        eval(&mut e, "var tt = v.textTracks[0]; tt.oncuechange = function () { log.push('cuechange:' + tt.activeCues.length); }; document.getElementById('t').addEventListener('cuechange', function () { log.push('track-cuechange'); }); [v.textTracks.length, tt.kind, tt.label, tt.language, tt.mode, document.getElementById('t').readyState, tt.cues.length, tt.cues[0].id, tt.cues[0].startTime, tt.cues[1].text].join('|')"),
        "1|subtitles|English|en|showing|2|2|one|0.5|Second\nline two"
    );
    eval(&mut e, "tt.cues[0].onenter = function () { log.push('enter'); };");
    e.on_media_message(&SMessage::MediaOpened { url: key.clone(), duration_ns: 4_000_000_000, width: 4, height: 4, video: "V_VP8".into(), audio: String::new(), real_video: true, audio_clock: false });
    e.on_media_message(&frame(&key, 600_000_000));
    settle(&mut e);
    assert_eq!(eval(&mut e, "log.join(' ')"), "enter cuechange:1 track-cuechange");
    assert_eq!(media::paint_for(&v).unwrap().captions, vec!["Hello & welcome".to_string()]);
    e.on_media_message(&frame(&key, 1_700_000_000));
    assert!(media::paint_for(&v).unwrap().captions.is_empty());

    // addTextTrack + VTTCue
    assert_eq!(
        eval(&mut e, "var t2 = v.addTextTrack('captions', 'Mine', 'de'); t2.mode = 'showing'; var c = new VTTCue(1.6, 1.9, 'Hallo'); t2.addCue(c); [v.textTracks.length, t2.mode, t2.cues.length, c.track === t2].join('|')"),
        "2|showing|1|true"
    );
    e.on_media_message(&frame(&key, 1_750_000_000));
    assert_eq!(media::paint_for(&v).unwrap().captions, vec!["Hallo".to_string()]);
    assert_eq!(eval(&mut e, "try { t2.removeCue(new VTTCue(0, 1, 'x')); 'no' } catch (x) { x.name }"), "NotFoundError");
}

/// Rule (HTML §6.4.1 sticky activation; Chromium's autoplay policy): bare `autoplay` does not play
/// before a user activation and the element paints the play affordance; script `play()` on an audible
/// element rejects with NotAllowedError; muted `autoplay` plays; after an activation `play()` resolves
/// and the affordance goes.
#[test]
fn autoplay_needs_sticky_activation() {
    let mut e = load("file:///page/ap.html", r#"<html><body><video src="a.webm" autoplay></video><video src="b.webm" autoplay muted></video><script>window.log = [];</script></body></html>"#);
    let (a, b) = (node(&e, 0), node(&e, 1));
    assert!(!media::events(&a).contains(&"play"));
    assert!(media::paint_for(&a).unwrap().blocked, "the play affordance");
    assert!(media::events(&b).contains(&"play"), "muted autoplay plays");
    eval(&mut e, "document.querySelector('video').play().then(function () { log.push('resolved'); }, function (x) { log.push(x.name); });");
    settle(&mut e);
    assert_eq!(eval(&mut e, "log.join(' ')"), "NotAllowedError");
    media::note_user_activation();
    eval(&mut e, "document.querySelector('video').play().then(function () { log.push('resolved'); }, function (x) { log.push(x.name); });");
    settle(&mut e);
    assert_eq!(eval(&mut e, "log.join(' ')"), "NotAllowedError resolved");
    assert!(!media::paint_for(&a).unwrap().blocked);
}

/// Rule (HTML preload): `preload=auto` — once the whole resource is local, `progress` then `suspend`
/// and networkState IDLE.
#[test]
fn preload_auto_fires_progress_then_suspend() {
    let mut e = load("file:///page/pl.html", r#"<html><body><video src="a.webm" preload="auto"></video></body></html>"#);
    let a = node(&e, 0);
    let url = e.media_elements()[0].url.clone();
    e.on_media_message(&SMessage::MediaOpened { url: url.clone(), duration_ns: 1_000_000_000, width: 4, height: 4, video: "V_VP9".into(), audio: String::new(), real_video: true, audio_clock: false });
    e.on_media_message(&frame(&url, 0));
    let ev = media::events(&a);
    assert!(ev.ends_with(&["canplaythrough", "progress", "suspend"]), "{ev:?}");
    assert_eq!(media::network_state(&a), media::NETWORK_IDLE);
}

/// Rules: playbackRate 2 goes to Stria (`MediaRate`), 20 is NotSupportedError (outside Chromium's
/// 1/16..16); `requestVideoFrameCallback` runs after the next presented picture with its metadata;
/// `requestPictureInPicture` rejects and `captureStream` throws NotSupportedError;
/// `document.pictureInPictureEnabled` is false.
#[test]
fn rate_range_video_frame_callback_and_refusals() {
    let mut e = load("file:///page/r.html", r#"<html><body><video src="a.webm"></video><script>window.log = []; var v = document.querySelector('video');</script></body></html>"#);
    let url = e.media_elements()[0].url.clone();
    let _ = e.take_media_requests();
    e.on_media_message(&SMessage::MediaOpened { url: url.clone(), duration_ns: 1_000_000_000, width: 4, height: 4, video: "V_VP9".into(), audio: String::new(), real_video: true, audio_clock: false });
    assert_eq!(eval(&mut e, "v.playbackRate = 2; try { v.playbackRate = 20; 'no' } catch (x) { x.name + ' ' + v.playbackRate }"), "NotSupportedError 2");
    assert!(e.take_media_requests().iter().any(|m| matches!(m, SMessage::MediaRate { rate, .. } if *rate == 2.0)));

    eval(&mut e, "v.requestVideoFrameCallback(function (now, md) { log.push(md.presentedFrames + ':' + md.width + 'x' + md.height + ':' + md.mediaTime); });");
    if let Some(js) = e.js_engine.as_mut() {
        js.drain_raf_once();
    }
    assert_eq!(eval(&mut e, "log.length"), "0", "no new picture yet");
    e.on_media_message(&frame(&url, 40_000_000));
    for _ in 0..3 {
        if let Some(js) = e.js_engine.as_mut() {
            js.drain_raf_once();
        }
    }
    assert_eq!(eval(&mut e, "log.join(' ')"), "1:4x4:0.04");

    eval(&mut e, "v.requestPictureInPicture().catch(function (x) { log.push('pip:' + x.name); }); try { v.captureStream(); } catch (x) { log.push('capture:' + x.name); } log.push('enabled:' + document.pictureInPictureEnabled);");
    settle(&mut e);
    assert_eq!(eval(&mut e, "log.slice(1).sort().join(' ')"), "capture:NotSupportedError enabled:false pip:NotSupportedError");
}
