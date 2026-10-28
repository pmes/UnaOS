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

//! CHARTER: vessel — `una-installer`, the UnaOS_Installer vessel (INSTALLERVESSEL, LEDGER SR413; docs/ROADMAP.md §1c).
//!
//! Like every vessel this binary is wiring and lifecycle only: it binds Aulë (the build: `Aule::forge_in`), Amber Bytes
//! (the card face, `card`, INSTALLERCARD SR414) and Quartzite (one tetra tree through `Backend::new_tetra_vessel`, the
//! stria-view shape) on one Bandy Synapse. Two panes: Install UnaOS (`card::face()`) and Install apps (the catalog,
//! Build / Install / Run per vessel). Without a backend (Linux without `--features gtk`) it is the headless face: the
//! census line and the catalog list.
//!
//! ```text
//! una-installer [--checkout DIR]                         # the window (headless: census + list)
//! una-installer [--checkout DIR] census | list
//! una-installer [--checkout DIR] about|build|install|run <crate>
//! ```
//! The wire: `[installer] host os= arch= rustup= toolkit= apps_dir= checkout=` once per start;
//! `[installer] installed vessel= path= built=1|0 why=`, `[installer] refused vessel= why=`,
//! `[installer] run vessel= path= pid=`.

mod card;
mod catalog;
mod host; mod landing; mod provider; // INSTALLERLANDING (SR415): the landing face — the card's boot logs read back into docs/dev/evidence (LINE-NEUTRAL); INSTALLERPROVIDER (SR424): the provider face — the landing sent to the person's own AI

use bandy::{SMessage, Synapse, telemetry};
use catalog::{CATALOG, Face, Vessel};
use host::{Host, Os, Plan};
use quartzite::tetra::TetraNode;
use std::path::PathBuf;

/// The bus target the apps pane's buttons address.
const TARGET: &str = "installer";

/// How often the serve task drains the card write's progress (`card::poll`) when the bus is quiet.
const CARD_TICK: std::time::Duration = std::time::Duration::from_millis(100);

struct Cli {
    checkout: Option<PathBuf>,
    verb: Option<String>,
}

const USAGE: &str = "usage: una-installer [--checkout DIR] [census | list | about|build|install|run <crate>]";

fn cli() -> Result<Cli, String> {
    let mut args = std::env::args().skip(1);
    let mut c = Cli { checkout: None, verb: None };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--checkout" => c.checkout = Some(PathBuf::from(args.next().ok_or("--checkout needs a directory")?)),
            "census" | "list" if c.verb.is_none() => c.verb = Some(a),
            "about" | "build" | "install" | "run" if c.verb.is_none() => {
                let krate = args.next().ok_or_else(|| format!("{a} needs a crate name"))?;
                c.verb = Some(format!("{a}:{krate}"));
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(c)
}

/// One wire line: stdout and the log.
fn say(line: &str) {
    println!("{line}");
    log::info!("{line}");
}

fn main() {
    if let Some(rc) = card::open_device_verb(std::env::args().skip(1)) { std::process::exit(rc); } // INSTALLERAUTHOPEN (SR422): the hidden root helper verb (pkexec), before the census or the log touch stdout
    let c = match cli() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("una-installer: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    if gneiss_pal::paths::UnaPaths::awaken().is_ok() {
        telemetry::ignite(gneiss_pal::paths::UnaPaths::root().join("logs"));
    }
    let host = Host::probe(c.checkout.as_deref());
    say(&host.census());

    let windowed = cfg!(any(target_os = "macos", all(target_os = "linux", feature = "gtk")));
    match c.verb.as_deref() {
        Some("census") => {}
        Some("list") => list(&host),
        Some(v) => std::process::exit(if verb(&host, v) { 0 } else { 1 }),
        None if !windowed => list(&host),
        None => window(host),
    }
}

/// The catalog as text: one line per vessel with its face on this host.
fn list(host: &Host) {
    for v in CATALOG.iter() {
        let installed = host.apps.as_ref().map(|a| a.bin.join(v.krate).is_file()).unwrap_or(false);
        let face = v.run_line(host.os).unwrap_or_else(|| format!("no face on {}", host.os.name()));
        println!("{:<10} {:<13} {:<20} installed={} {face}", v.name, v.krate, v.about, installed as u8);
    }
}

/// THE window: the Install UnaOS pane beside the Install apps pane.
fn tree() -> TetraNode {
    let os = Os::this();
    let header = |id: &str, label: &str| TetraNode::Button {
        id: id.into(),
        label: label.into(),
        action: SMessage::Input { target: TARGET.into(), text: "census".into() },
    };
    let rows = CATALOG.iter().map(|v| {
        let button = |verb: &str, label: &str| TetraNode::Button {
            id: format!("{verb}.{}", v.krate),
            label: label.into(),
            action: SMessage::Input { target: TARGET.into(), text: format!("{verb}:{}", v.krate) },
        };
        let none = if v.face(os) == Face::None { " (no face here)" } else { "" };
        TetraNode::HStack(vec![
            button("about", &format!("{} — {}{none}", v.name, v.about)),
            button("build", "Build"),
            button("install", "Install"),
            button("run", "Run"),
        ])
    });
    TetraNode::HStack(vec![
        TetraNode::VStack(vec![header("installer.unaos", "Install UnaOS"), card::face(), landing::face(), provider::face()]), // INSTALLERLANDING (SR415): the landing pane under the card pane; INSTALLERPROVIDER (SR424): the provider pane under it
        TetraNode::VStack(std::iter::once(header("installer.apps", "Install apps")).chain(rows).collect()),
    ])
}

fn window(host: Host) {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let synapse = Synapse::new();
    rt.spawn(serve(synapse.subscribe(), host));
    quartzite::Backend::new_tetra_vessel("org.unaos.installer", "UnaOS Installer", (760.0, 520.0), tree(), synapse.clone())
        .run();
    drop(rt);
}

/// The one subscriber: it owns the host snapshot and the card face's state (no lock). The apps pane's verbs run on
/// the blocking pool (a build takes minutes); every `ConsoleInput` line is the card face's (`card::on_input`), and
/// `card::poll` drains the card write after every message and on a quiet tick.
async fn serve(mut rx: tokio::sync::broadcast::Receiver<SMessage>, host: Host) {
    use tokio::sync::broadcast::error::RecvError;
    let host = std::sync::Arc::new(host);
    let mut card = card::Card::probe(); let mut landing = landing::Landing::probe_in(host.checkout.clone()); let mut provider = provider::Provider::probe_in(host.checkout.clone()); // INSTALLERLANDING (SR415): the landing face's state, landing under the probed checkout (LINE-NEUTRAL); INSTALLERPROVIDER (SR424): the provider face's state
    let mut tick = tokio::time::interval(CARD_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(SMessage::Input { target, text }) if target == TARGET => {
                    let host = host.clone();
                    tokio::task::spawn_blocking(move || verb(&host, &text));
                }
                Ok(SMessage::ConsoleInput(line)) => {
                    if !card::on_input(&mut card, &line) && !provider::on_input(&mut provider, &line) && !landing::on_input(&mut landing, &line) { // INSTALLERLANDING (SR415): the card face first, then the landing face; INSTALLERPROVIDER (SR424): the provider face before landing, so a pasted key never reaches the unafs mount as a card password
                        say(&format!("[installer] input ignored bytes={}", line.len())); // LANDINGSECRETFIELD (SR432): an untaken line may be a secret field's: its length, never its text
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            _ = tick.tick() => {}
        }
        card::poll(&mut card); landing::poll(&mut landing); provider::poll(&mut provider); // INSTALLERLANDING (SR415): drain the landing read beside the card write (LINE-NEUTRAL); INSTALLERPROVIDER (SR424): and the provider's reply
    }
}

/// One verb, `census` or `<about|build|install|run>:<crate>`; true when it did what it names.
fn verb(host: &Host, text: &str) -> bool {
    let (name, krate) = text.split_once(':').unwrap_or((text, ""));
    if name == "census" {
        say(&host.census());
        return true;
    }
    let Some(v) = catalog::find(krate) else {
        say(&format!("[installer] refused vessel={krate} why=unknown-vessel"));
        return false;
    };
    match name {
        "about" => {
            let line = v.run_line(host.os).unwrap_or_else(|| "-".into());
            say(&format!("[installer] vessel={} name={} dir={} about={} run={line}", v.krate, v.name, v.dir, v.about));
            true
        }
        "build" => match host::plan(host, v, false) {
            Ok(p) => forge(host, v, &p),
            Err(why) => refuse(v, why),
        },
        "install" => install(host, v),
        "run" => run(host, v),
        _ => {
            say(&format!("[installer] refused verb={name} why=unknown-verb"));
            false
        }
    }
}

fn refuse(v: &Vessel, why: &str) -> bool {
    say(&format!("[installer] refused vessel={} why={why}", v.krate));
    false
}

/// Build through Aulë when the plan builds (its lines stream to stdout); true when there was nothing to build or
/// the build succeeded.
fn forge(host: &Host, v: &Vessel, p: &Plan) -> bool {
    let (Some(args), Some(checkout)) = (&p.build, &host.checkout) else {
        return true;
    };
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let printer = std::thread::spawn(move || {
        for line in rx {
            println!("{line}");
        }
    });
    let status = aule::Aule::forge_in(checkout, &argv, tx);
    let _ = printer.join();
    match status {
        Ok(s) if s.success() => true,
        Ok(_) => refuse(v, "build-failed"),
        Err(_) => refuse(v, "build-spawn"),
    }
}

/// Build (when the toolchain is here), copy the release binary into the apps dir, and on Linux write its desktop
/// entry.
fn install(host: &Host, v: &Vessel) -> bool {
    let prebuilt = host.checkout.as_ref().is_some_and(|c| c.join("target/release").join(v.krate).is_file());
    let p = match host::plan(host, v, prebuilt) {
        Ok(p) => p,
        Err(why) => return refuse(v, why),
    };
    if !forge(host, v, &p) {
        return false;
    }
    if copy_in(v, &p).is_err() {
        return refuse(v, "copy-failed");
    }
    say(&format!(
        "[installer] installed vessel={} path={} built={} why={}",
        v.krate,
        p.dest.display(),
        p.build.is_some() as u8,
        p.why.unwrap_or("-")
    ));
    true
}

fn copy_in(v: &Vessel, p: &Plan) -> std::io::Result<()> {
    if let Some(dir) = p.dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::copy(&p.from, &p.dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p.dest, std::fs::Permissions::from_mode(0o755))?;
    }
    if let Some(entry) = &p.desktop {
        if let Some(dir) = entry.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(entry, host::desktop_entry(v, &p.dest))?;
    }
    Ok(())
}

/// Spawn the installed binary.
fn run(host: &Host, v: &Vessel) -> bool {
    let dest = match &host.apps {
        Ok(a) => a.bin.join(v.krate),
        Err(why) => return refuse(v, why),
    };
    if !dest.is_file() {
        say(&format!("[installer] run vessel={} path={} why=not-installed", v.krate, dest.display()));
        return false;
    }
    match std::process::Command::new(&dest).spawn() {
        Ok(child) => {
            say(&format!("[installer] run vessel={} path={} pid={}", v.krate, dest.display(), child.id()));
            true
        }
        Err(_) => refuse(v, "spawn-failed"),
    }
}

/// THE host test module (INSTALLERVESSEL SR413): the catalog resolves to real workspace crates, the census line
/// parses, and the apps-dir rule and the install plan hold per OS.
#[cfg(test)]
mod tests {
    use crate::catalog::{self, CATALOG, Face};
    use crate::host::{AppsDir, CENSUS_KEYS, Host, Os, Toolkit, apps_dir, desktop_entry, parse_census, plan};
    use std::path::{Path, PathBuf};

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The root workspace's `members = [ ... ]` entries.
    fn members() -> Vec<String> {
        let text = std::fs::read_to_string(root().join("Cargo.toml")).expect("root Cargo.toml");
        let start = text.find("members = [").expect("members list") + "members = [".len();
        let body = &text[start..start + text[start..].find("\n]").expect("members list end")];
        body.lines()
            .filter_map(|l| l.trim().strip_prefix('"'))
            .filter_map(|l| l.split('"').next())
            .map(str::to_string)
            .collect()
    }

    /// The lines of a manifest's `[section]` up to the next header.
    fn section<'a>(manifest: &'a str, header: &str) -> Vec<&'a str> {
        manifest.lines().skip_while(|l| l.trim() != header).skip(1).take_while(|l| !l.trim_start().starts_with('[')).collect()
    }

    #[test]
    fn installer_catalog_rows_are_workspace_vessels() {
        let members = members();
        assert!(members.iter().any(|m| m == "vessels/una-installer"), "the installer is not a member: {members:?}");
        for v in CATALOG.iter() {
            assert!(members.iter().any(|m| m == v.dir), "{} is not a workspace member", v.dir);
            let manifest = std::fs::read_to_string(root().join(v.dir).join("Cargo.toml")).expect("vessel manifest");
            let name = format!("name = \"{}\"", v.krate);
            assert!(section(&manifest, "[package]").iter().any(|l| l.trim() == name), "{} does not name {}", v.dir, v.krate);
            assert!(!manifest.contains("[[bin]]"), "{}: a [[bin]] rename breaks binary = crate", v.dir);
            let features = section(&manifest, "[features]");
            for face in [v.macos, v.linux] {
                if let Face::Feature(f) = face {
                    let key = format!("{f} =");
                    assert!(features.iter().any(|l| l.trim_start().starts_with(&key)), "{}: no feature `{f}`", v.dir);
                }
            }
            assert!(catalog::find(v.krate).is_some());
        }
    }

    #[test]
    fn installer_census_line_parses() {
        let probed = Host::probe(Some(&root()));
        let line = probed.census();
        let kv = parse_census(&line).unwrap_or_else(|| panic!("census does not parse: {line}"));
        assert_eq!(kv.iter().map(|(k, _)| *k).collect::<Vec<_>>(), CENSUS_KEYS);
        assert_eq!(kv[0].1, Os::this().name());
        assert_eq!(kv[1].1, std::env::consts::ARCH);
        assert!(kv[2].1 == "0" || kv[2].1 == "1", "rustup={}", kv[2].1);
        assert_ne!(kv[5].1, "none", "the test's own checkout was not found: {line}");

        let home = Path::new("/Users/A User");
        let synth = Host {
            os: Os::MacOs,
            arch: "aarch64",
            rustup: true,
            toolkit: Toolkit::AppKit,
            apps: apps_dir(Os::MacOs, home),
            checkout: Some(home.join("src/UnaOS")),
        };
        let s = synth.census();
        assert_eq!(
            s,
            "[installer] host os=macos arch=aarch64 rustup=1 toolkit=appkit apps_dir=/Users/A User/Applications checkout=/Users/A User/src/UnaOS"
        );
        let kv = parse_census(&s).expect("synthetic census");
        assert_eq!(kv[3], ("toolkit", "appkit"));
        assert_eq!(kv[4], ("apps_dir", "/Users/A User/Applications"));
        assert_eq!(kv[5], ("checkout", "/Users/A User/src/UnaOS"));
        assert!(parse_census("[installer] host os=linux arch=x86_64").is_none());
        assert!(parse_census("[other] host os=linux").is_none());
    }

    fn host_of(os: Os, rustup: bool, toolkit: Toolkit) -> Host {
        Host { os, arch: "x86_64", rustup, toolkit, apps: apps_dir(os, Path::new("/home/u")), checkout: Some(PathBuf::from("/src/UnaOS")) }
    }

    #[test]
    fn installer_apps_dir_and_plan_per_os() {
        assert_eq!(apps_dir(Os::MacOs, Path::new("/Users/u")), Ok(AppsDir { bin: "/Users/u/Applications".into(), desktop: None }));
        assert_eq!(
            apps_dir(Os::Linux, Path::new("/home/u")),
            Ok(AppsDir { bin: "/home/u/.local/bin".into(), desktop: Some("/home/u/.local/share/applications".into()) })
        );
        assert_eq!(apps_dir(Os::Windows, Path::new("C:/Users/u")), Err("windows-unbuilt"));

        let aether = catalog::find("aether-shell").unwrap();
        let pulse = catalog::find("pulse").unwrap();
        let una = catalog::find("una").unwrap();

        assert_eq!(plan(&host_of(Os::Windows, true, Toolkit::None), aether, true), Err("windows-unbuilt"));
        assert_eq!(plan(&host_of(Os::Linux, true, Toolkit::Gtk4), pulse, true), Err("no-face"));
        assert_eq!(plan(&host_of(Os::Linux, true, Toolkit::Qt6), aether, false), Err("no-gtk4-dev"));
        let p = plan(&host_of(Os::Linux, true, Toolkit::Gtk4Qt6), aether, false).unwrap();
        assert_eq!(p.build.as_ref().unwrap().join(" "), "build -p aether-shell --features gtk --release");
        assert_eq!(p.from, PathBuf::from("/src/UnaOS/target/release/aether-shell"));
        assert_eq!(p.dest, PathBuf::from("/home/u/.local/bin/aether-shell"));
        assert_eq!(p.desktop, Some(PathBuf::from("/home/u/.local/share/applications/aether-shell.desktop")));

        assert_eq!(plan(&host_of(Os::MacOs, false, Toolkit::AppKit), pulse, false), Err("no-rustup"));
        let p = plan(&host_of(Os::MacOs, false, Toolkit::AppKit), pulse, true).unwrap();
        assert_eq!((p.build.clone(), p.why), (None, Some("no-rustup")));
        assert_eq!(p.dest, PathBuf::from("/home/u/Applications/pulse"));
        assert_eq!(p.desktop, None);
        assert_eq!(plan(&host_of(Os::MacOs, true, Toolkit::None), una, false), Err("no-xcode-clt"));
        let p = plan(&host_of(Os::MacOs, true, Toolkit::AppKit), una, false).unwrap();
        assert_eq!(p.build.unwrap().join(" "), "build -p una --release");
        let mut lost = host_of(Os::MacOs, true, Toolkit::AppKit);
        lost.checkout = None;
        assert_eq!(plan(&lost, una, true), Err("no-checkout"));

        let entry = desktop_entry(aether, Path::new("/home/u/.local/bin/aether-shell"));
        assert!(entry.starts_with("[Desktop Entry]\nType=Application\n"), "{entry}");
        assert!(entry.contains("\nName=Aether\n") && entry.contains("\nExec=/home/u/.local/bin/aether-shell\n"), "{entry}");
        assert_eq!(aether.run_line(Os::MacOs).unwrap(), "cargo run -p aether-shell --features macos --release");
    }
}
