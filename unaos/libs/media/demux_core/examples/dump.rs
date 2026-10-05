//! `cargo run -p demux_core --example dump -- <file>`: print tracks and the packet table.
fn main() {
    let path = std::env::args().nth(1).expect("usage: dump <file>");
    let mut d = demux_core::Demuxer::open(std::fs::read(&path).unwrap()).unwrap();
    println!("format {:?} duration_ns {} declared {:?}", d.format(), d.duration_ns(), d.declared_duration_ns());
    for t in d.tracks() {
        println!(
            "track {} {:?} {:?} '{}' {}x{} rate {} ch {} tb {}/{} samples {} span_ns {} cfg {}B delay {}",
            t.id, t.kind, t.codec, t.codec_name, t.width, t.height, t.sample_rate, t.channels,
            t.timebase.num, t.timebase.den, t.sample_count, t.duration_ns, t.config.len(), t.codec_delay_ns
        );
    }
    let verbose = std::env::args().nth(2).is_some();
    while let Some(p) = d.next_packet() {
        if verbose {
            let tb = d.tracks()[d.track_index(p.track).unwrap()].timebase;
            println!("pkt t{} pts {} ({} ns) dts {} dur {} key {} size {}", p.track, p.pts, tb.to_ns(p.pts), p.dts, p.duration, p.keyframe, p.data.len());
        }
    }
}
