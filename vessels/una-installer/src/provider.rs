//! CHARTER: Vein — the UnaOS_Installer provider face (vessel module; seam shared-core gneiss_pal::api ModelProvider via vein::provider::ProviderSlot)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALLERPROVIDER (LEDGER SR424): the landing goes to the person's own AI. Peter 2026-10-27: "we will need a way for
// people to connect to their ai of choice". The landing face (INSTALLERLANDING SR415) lands `PROMPT.md` + `landing.md`
// under docs/dev/evidence/landing-<stamp>/; this face SENDS them. It BUILDS NO PROVIDER and KEEPS NO KEY STORE: the
// provider is Vein's `ProviderSlot::from_lookup_with` (Principia's `vein` preferences, Holocron's `vein/claude.api_key`
// first, the env var the preference names as the fallback), the one call is `ModelProvider::generate` (its retry,
// `send_with_backoff`, rides inside), and the pick is two Principia `PrefSet`s (`menu_choice_prefs`) through
// `Principia::process_impulse`, as Vein's Settings dropdown writes them.
//
// The key: Vein never stores one (the preference names an env var; Holocron has custody). A key typed here is HELD in
// this process only — never written, never printed — and answers exactly the env var `ProviderConfig::key_env` names,
// through the slot's own env closure. Quartzite has no secret field (LANDING-SECRETFIELD owed), so a raw line is this
// face's only when the Key button armed it or it has a provider key's shape (`sk-ant-`, `AIza`): a pasted key never
// reaches the vessel's `input ignored` print, nor the landing face as a card password (main routes card, provider,
// landing).
//
// Rung 1 sends PROMPT.md and landing.md's census (about 1k tokens); the boot logs are NAMED, not sent.
//
// Wire:
//   [installer] provider name=<claude|gemini|claudecode|none> key=<set|missing> send=<ok|refused|-> why=<-|no-key|no-landing|network|provider-error> reply_kb=<n> ms=<n>

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use bandy::{PrefValue, PrincipiaCommand, SMessage};
use gneiss_pal::api::provider::{CLAUDE_DEFAULT_MODEL, GEMINI_DEFAULT_MODEL};
use gneiss_pal::api::{
    ChatRequest, ModelProvider, PREF_NS, Part, ProviderConfig, ProviderError, ProviderKind, StopReason, Thread,
    menu_choice_prefs,
};
use holocron::holocron_core::keysource::KeySource;
use quartzite::tetra::{ConsoleLine, ConsoleTetra, TetraNode, TextAction};
use vein::provider::{ProviderSlot, is_vein_pref_change};

/// The fixed system line every send carries: what the model is reading and the repo's landing rules.
pub const SYSTEM: &str = "You are reading an UnaOS landing for its operator: the user turn is the landing's PROMPT.md (the standing landing-report recipe) followed by its landing.md (the verdict census of the boot logs the UnaOS_Installer read off the card). You cannot open files: the boot logs PROMPT.md names are on the operator's disk, not in this message. Work from landing.md's census, follow PROMPT.md's recipe as far as the census reaches, mark every step that needs a log's text as 'needs the log', and never mark a ledger row flown: that is the seat's, from a quoted log line (the repo's CLAUDE.md and docs/dev/LAWS.md).";

/// The separator between PROMPT.md and landing.md in the user turn.
const SEP: &str = "\n\n---\n\n";

/// The user turn's parts: PROMPT.md, then landing.md.
pub fn parts(prompt_md: &str, landing_md: &str) -> Vec<Part> {
    vec![Part::text(format!("{prompt_md}{SEP}{landing_md}"))]
}

/// The newest landing under `<checkout>/docs/dev/evidence` (the `landing-<yyyymmdd-hhmm>[-k]` names sort by time).
pub fn newest_landing(checkout: &Path) -> Option<PathBuf> {
    let evidence = checkout.join("docs/dev/evidence");
    std::fs::read_dir(&evidence)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("landing-") && e.path().is_dir())
        .map(|e| e.path())
        .max()
}

/// Where the reply lands: `REPLY.md` beside PROMPT.md, `REPLY-2.md`, `REPLY-3.md` … when taken.
pub fn reply_path(dir: &Path) -> PathBuf {
    let mut p = dir.join("REPLY.md");
    let mut k = 2;
    while p.exists() {
        p = dir.join(format!("REPLY-{k}.md"));
        k += 1;
    }
    p
}

/// A provider error as the wire's `why=`.
pub fn why_of(e: &ProviderError) -> &'static str {
    match e {
        ProviderError::Config(_) => "no-key",
        ProviderError::Retryable { status: 0, .. } => "network",
        _ => "provider-error",
    }
}

/// `[installer] provider name= key= send= why= reply_kb= ms=`.
pub fn line(name: &str, key_set: bool, send: &str, why: &str, reply_kb: u64, ms: u128) -> String {
    format!(
        "[installer] provider name={name} key={} send={send} why={why} reply_kb={reply_kb} ms={ms}",
        if key_set { "set" } else { "missing" }
    )
}

fn say(l: String) -> String {
    println!("{l}");
    l
}

/// A raw line with a provider key's shape (Anthropic `sk-ant-`, Google `AIza`).
pub fn key_shaped(t: &str) -> bool {
    !t.contains(char::is_whitespace) && (t.starts_with("sk-ant-") || t.starts_with("AIza"))
}

/// The send's verdict: the wire line and the reply's path when one landed.
enum Msg {
    Done(String, Option<PathBuf>),
}

/// The send on its own thread: ONE `generate`, the stop reason checked before the text, the reply written.
fn job(p: Arc<dyn ModelProvider>, req: ChatRequest, dir: PathBuf) -> (String, Option<PathBuf>) {
    let t0 = Instant::now();
    let name = p.name().to_string();
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(_) => return (say(line(&name, true, "refused", "provider-error", 0, 0)), None),
    };
    let r = rt.block_on(p.generate(&req));
    let ms = t0.elapsed().as_millis();
    let resp = match r {
        Ok(resp) => resp,
        Err(e) => return (say(line(&name, true, "refused", why_of(&e), 0, ms)), None),
    };
    let cut = match resp.stop {
        StopReason::EndTurn => "",
        StopReason::MaxTokens => "_The reply was cut at the output cap (vein.max_tokens)._\n\n",
        StopReason::Refusal { .. } | StopReason::Other(_) => {
            return (say(line(&name, true, "refused", "provider-error", 0, ms)), None);
        }
    };
    let body = format!("# Reply — {} / {}\n\n{cut}{}\n", name, p.model(), resp.text);
    let path = reply_path(&dir);
    if std::fs::write(&path, &body).is_err() {
        return (say(line(&name, true, "refused", "no-landing", 0, ms)), None);
    }
    (say(line(&name, true, "ok", "-", (body.len() as u64).div_ceil(1024), ms)), Some(path))
}

/// The provider face's state. ONE owner: the vessel's serve task (no lock). The send runs on its own thread, which
/// owns the provider and the request and reports over a channel this struct drains. The typed key is held only here,
/// never written, never printed, and dropped by the next pick.
pub struct Provider {
    pub checkout: PathBuf,
    /// The HOME Principia writes the pick under.
    pub home: PathBuf,
    /// Principia's settings folder Vein's slot reads.
    pub prefs: PathBuf,
    /// The last reply's path.
    pub reply: Option<PathBuf>,
    pub verdict: String,
    /// The Key button armed the next raw line as the key.
    pub arm_key: bool,
    /// The host's env and Holocron answer for the key too (false in tests).
    host_env: bool,
    key: Option<String>,
    rx: Option<Receiver<Msg>>,
}

impl Provider {
    /// Principia's standard settings folder, the host env and Holocron; landing under `checkout` when given.
    pub fn probe_in(checkout: Option<PathBuf>) -> Self {
        let prefs = principia::default_prefs_path();
        let home = prefs.parent().map(Path::to_path_buf).unwrap_or_default();
        Self::with(checkout.unwrap_or_else(crate::landing::default_checkout), home, prefs, true)
    }

    /// A HOME of its own, no host env, no Holocron (tests).
    pub fn probe_at(checkout: PathBuf, home: PathBuf) -> Self {
        let prefs = principia::Principia::with_home(&home).prefs().path().to_path_buf();
        Self::with(checkout, home, prefs, false)
    }

    fn with(checkout: PathBuf, home: PathBuf, prefs: PathBuf, host_env: bool) -> Self {
        Provider {
            checkout,
            home,
            prefs,
            reply: None,
            verdict: String::from("pick Claude or Gemini, give the key, then Send the newest landing"),
            arm_key: false,
            host_env,
            key: None,
            rx: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Vein's slot over Principia's `vein` preferences; a held key answers the configured key env var.
    fn slot(&self) -> ProviderSlot {
        let store = principia::PrefStore::load(&self.prefs).ok();
        let get = |k: &str| store.as_ref().and_then(|s| s.get(PREF_NS, k));
        let var = ProviderConfig::from_prefs(get).ok().and_then(|c| c.key_env().map(str::to_string));
        let host_env = self.host_env;
        let key = self.key.clone();
        let env = |k: &str| match (&key, &var) {
            (Some(held), Some(v)) if v == k => Some(held.clone()),
            _ if host_env => std::env::var(k).ok(),
            _ => None,
        };
        let held = self.key.is_some();
        ProviderSlot::from_lookup_with(get, env, || {
            if held || !host_env {
                KeySource::Fallback
            } else {
                holocron::client::claude_api_key(holocron::client::default_socket().as_deref())
            }
        })
    }

    /// The configured provider's name (`none` when the preferences do not parse).
    fn name(&self) -> String {
        let store = principia::PrefStore::load(&self.prefs).ok();
        ProviderConfig::from_prefs(|k| store.as_ref().and_then(|s| s.get(PREF_NS, k)))
            .map(|c| c.kind.as_str().to_string())
            .unwrap_or_else(|_| String::from("none"))
    }

    /// The state line (`send=-`).
    pub fn status(&self) -> String {
        line(&self.name(), self.slot().provider().is_ok(), "-", "-", 0, 0)
    }

    /// Drain the send's verdict; true when anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let Ok(Msg::Done(l, path)) = rx.try_recv() else { return false };
        self.rx = None;
        self.verdict = l;
        if path.is_some() {
            self.reply = path;
        }
        true
    }

    /// `provider pick <claude|gemini>`: `vein.provider` + `vein.model` through Principia (and `vein.gemini.auth =
    /// api_key` for Gemini when no Vertex project is set: the mode the key field serves). The held key is dropped.
    fn pick(&mut self, kind: ProviderKind) {
        self.key = None;
        self.arm_key = false;
        let model = match kind {
            ProviderKind::Gemini => GEMINI_DEFAULT_MODEL,
            _ => CLAUDE_DEFAULT_MODEL,
        };
        let Some(writes) = menu_choice_prefs(&format!("{model} ({})", kind.as_str())) else { return };
        let mut principia = principia::Principia::with_home(&self.home);
        let mut sets: Vec<(&str, PrefValue)> = writes.to_vec();
        if kind == ProviderKind::Gemini && principia.prefs().get(PREF_NS, "gemini.project").is_none() {
            sets.push(("gemini.auth", PrefValue::Str(String::from("api_key"))));
        }
        for (key, value) in sets {
            let reply = principia.process_impulse(&SMessage::Principia(PrincipiaCommand::PrefSet {
                ns: PREF_NS.to_string(),
                key: key.to_string(),
                value,
            }));
            if !reply.as_ref().is_some_and(is_vein_pref_change) {
                self.verdict = format!("Principia refused vein.{key}");
                return;
            }
        }
        self.verdict = say(self.status());
    }

    /// Hold `k` as the key for this session (never written, never printed).
    fn hold_key(&mut self, k: &str) {
        self.key = Some(k.to_string());
        self.arm_key = false;
        self.verdict = say(self.status());
    }

    /// `provider send [<landing dir>]`: the slot's provider, PROMPT.md + landing.md under [`SYSTEM`], on its thread.
    pub fn start_send(&mut self, arg: &str) {
        if self.busy() {
            return;
        }
        let name = self.name();
        let dir = if arg.is_empty() { newest_landing(&self.checkout) } else { Some(PathBuf::from(arg)) };
        let read = |d: &Path, f: &str| std::fs::read_to_string(d.join(f)).ok();
        let Some((dir, prompt, landing)) =
            dir.and_then(|d| Some((read(&d, "PROMPT.md")?, read(&d, "landing.md")?)).map(|(p, l)| (d, p, l)))
        else {
            let key_set = self.slot().provider().is_ok();
            self.verdict = say(line(&name, key_set, "refused", "no-landing", 0, 0));
            return;
        };
        let slot = self.slot();
        let p = match slot.provider() {
            Ok(p) => p,
            Err(_) => {
                self.verdict = say(line(&name, false, "refused", "no-key", 0, 0));
                return;
            }
        };
        let req = slot.turn(Some(SYSTEM.to_string()), &Thread::new(), parts(&prompt, &landing));
        self.start_with(p, req, dir);
    }

    /// The send's tail: ONE `generate` of `req` on `p` on its own thread, the reply into `dir` (the test seam too).
    pub fn start_with(&mut self, p: Arc<dyn ModelProvider>, req: ChatRequest, dir: PathBuf) {
        let (tx, rx) = channel();
        self.verdict = format!("sending {} to {} ...", dir.display(), p.name());
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let (l, path) = job(p, req, dir);
            let _ = tx.send(Msg::Done(l, path));
        });
    }
}

/// Drain the send's verdict into `provider` (the serve task calls it beside `card::poll`).
pub fn poll(provider: &mut Provider) {
    provider.poll();
}

/// The pane INSTALLERVESSEL's window shows (a fresh probe).
pub fn face() -> TetraNode {
    face_of(&Provider::probe_in(None))
}

/// The pane for a given state: the status, the landing that Send takes, the picks, the key field, Key and Send. Every
/// control speaks `SMessage::ConsoleInput`, routed to [`on_input`]. No line carries the key.
pub fn face_of(p: &Provider) -> TetraNode {
    let line = |level: &str, text: String| ConsoleLine { level: level.to_string(), source: String::from("provider"), text };
    let mut lines = vec![line("info", p.status())];
    match newest_landing(&p.checkout) {
        Some(d) => lines.push(line("info", format!("Send takes {}", d.display()))),
        None => lines.push(line("warn", String::from("no landing yet — Read a card first"))),
    }
    if p.arm_key {
        lines.push(line("warn", String::from("type the key in the field: held for this session, never written, never shown")));
    }
    if let Some(r) = &p.reply {
        lines.push(line("info", format!("reply in {}", r.display())));
    }
    lines.push(line("info", p.verdict.clone()));
    let button = |id: &str, label: &str, cmd: &str| TetraNode::Button { id: id.into(), label: label.into(), action: SMessage::ConsoleInput(cmd.into()) };
    TetraNode::VStack(vec![
        TetraNode::Console(ConsoleTetra { lines, ..Default::default() }),
        TetraNode::HStack(vec![
            button("provider.pick.claude", "Claude", "provider pick claude"),
            button("provider.pick.gemini", "Gemini", "provider pick gemini"),
        ]),
        TetraNode::TextField { id: String::from("provider.key"), placeholder: String::from("API key (held for this session, never written)"), action: TextAction::ConsoleInput },
        TetraNode::HStack(vec![button("provider.key", "Key", "provider key"), button("provider.send", "Send", "provider send")]),
    ])
}

/// Route one `SMessage::ConsoleInput` line: `provider pick <claude|gemini>`, `provider key [<key>]`, `provider send
/// [<landing dir>]`, `provider status`, or a raw line the Key button armed / with a key's shape (the key). False = not
/// the provider face's line.
pub fn on_input(p: &mut Provider, line: &str) -> bool {
    p.poll();
    let t = line.trim();
    let Some(rest) = t.strip_prefix("provider ").map(str::trim).or((t == "provider").then_some("")) else {
        if !t.is_empty() && (p.arm_key || key_shaped(t)) {
            p.hold_key(t);
            return true;
        }
        return false;
    };
    let (verb, arg) = rest.split_once(' ').map(|(v, a)| (v, a.trim())).unwrap_or((rest, ""));
    match verb {
        "pick" if !p.busy() => match arg {
            "claude" => p.pick(ProviderKind::Claude),
            "gemini" => p.pick(ProviderKind::Gemini),
            _ => p.verdict = String::from("pick claude or gemini"),
        },
        "key" if arg.is_empty() => {
            p.arm_key = true;
            p.verdict = String::from("type the key in the field");
        }
        "key" => p.hold_key(arg),
        "send" if !p.busy() => p.start_send(arg),
        "status" => p.verdict = say(p.status()),
        _ if p.busy() => p.verdict = String::from("busy sending — wait for the reply"),
        _ => p.verdict = format!("unknown provider verb `{verb}` (pick / key / send / status)"),
    }
    true
}

// ── tests ───────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use gneiss_pal::api::{BoxFuture, ChatResponse, DeltaStream, Usage};
    use std::fs;
    use std::sync::Mutex;

    const PROMPT: &str = "# Landing prompt — docs/dev/evidence/landing-20261027-2340\n\n1. Read `CLAUDE.md` first.\n";
    const LANDING: &str = "# Landing — image.img\n\n- verdicts: 3 PASS / 1 FAIL / 0 SKIP\n";
    const SECRETS: [&str; 3] = ["AIzaTEST-SECRET-0001", "sk-ant-TEST-SECRET-0002", "plain-TEST-SECRET-0003"];

    /// A provider that never leaves the process: it records the request and answers `answer`.
    struct Fake {
        seen: Mutex<Vec<ChatRequest>>,
        answer: Result<ChatResponse, ProviderError>,
    }

    impl Fake {
        fn answering(answer: Result<ChatResponse, ProviderError>) -> Arc<Fake> {
            Arc::new(Fake { seen: Mutex::new(Vec::new()), answer })
        }
        fn text(t: &str, stop: StopReason) -> Result<ChatResponse, ProviderError> {
            Ok(ChatResponse { text: t.to_string(), stop, usage: Usage::default() })
        }
    }

    impl ModelProvider for Fake {
        fn name(&self) -> &str {
            "fake"
        }
        fn model(&self) -> &str {
            "fake-1"
        }
        fn generate<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, Result<ChatResponse, ProviderError>> {
            self.seen.lock().unwrap().push(req.clone());
            let a = self.answer.clone();
            Box::pin(async move { a })
        }
        fn stream<'a>(&'a self, _req: &'a ChatRequest) -> BoxFuture<'a, Result<DeltaStream<'a>, ProviderError>> {
            Box::pin(async { Err(ProviderError::Unsupported(String::from("fake"))) })
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("installerprovider-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A checkout with two landings (the newer one is Send's) and a HOME of its own.
    fn laid(name: &str) -> (Provider, PathBuf) {
        let root = scratch(name);
        let checkout = root.join("checkout");
        for (stamp, prompt) in [("landing-20261026-0101", "# old prompt\n"), ("landing-20261027-2340", PROMPT)] {
            let d = checkout.join("docs/dev/evidence").join(stamp);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("PROMPT.md"), prompt).unwrap();
            fs::write(d.join("landing.md"), LANDING).unwrap();
        }
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        (Provider::probe_at(checkout, home), root)
    }

    fn settle(p: &mut Provider) {
        for _ in 0..1000 {
            if !p.busy() {
                return;
            }
            p.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the send never answered");
    }

    fn user_text(req: &ChatRequest) -> String {
        req.messages
            .iter()
            .flat_map(|m| m.parts.iter())
            .map(|p| match p {
                Part::Text { text } => text.clone(),
                Part::FileData { .. } => String::new(),
            })
            .collect()
    }

    fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() { files_under(&p, out) } else { out.push(p) }
        }
    }

    #[test]
    fn provider_send_carries_the_prompt_and_lands_the_reply() {
        let (mut p, root) = laid("send");
        let dir = newest_landing(&p.checkout).unwrap();
        assert!(dir.ends_with("landing-20261027-2340"), "{}", dir.display());
        let fake = Fake::answering(Fake::text("Boot 1: 3 PASS / 1 FAIL; the FAIL needs the log.", StopReason::EndTurn));
        let req = ChatRequest::single(Some(SYSTEM.to_string()), parts(PROMPT, LANDING));
        p.start_with(fake.clone(), req, dir.clone());
        settle(&mut p);
        let seen = fake.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "ONE generate per send");
        assert_eq!(seen[0].system.as_deref(), Some(SYSTEM));
        assert_eq!(user_text(&seen[0]), format!("{PROMPT}{SEP}{LANDING}"));
        assert!(p.verdict.starts_with("[installer] provider name=fake key=set send=ok why=- reply_kb=1 ms="), "{}", p.verdict);
        assert_eq!(p.reply.as_deref(), Some(dir.join("REPLY.md").as_path()));
        let reply = fs::read_to_string(dir.join("REPLY.md")).unwrap();
        assert!(reply.starts_with("# Reply — fake / fake-1\n\n") && reply.contains("the FAIL needs the log."), "{reply}");

        // A second send never overwrites the first reply; a MaxTokens cut is said in the file.
        let fake = Fake::answering(Fake::text("cut", StopReason::MaxTokens));
        p.start_with(fake, ChatRequest::single(Some(SYSTEM.to_string()), parts(PROMPT, LANDING)), dir.clone());
        settle(&mut p);
        assert_eq!(p.reply.as_deref(), Some(dir.join("REPLY-2.md").as_path()));
        assert!(fs::read_to_string(dir.join("REPLY-2.md")).unwrap().contains("output cap"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn provider_refusals_by_name() {
        assert_eq!(why_of(&ProviderError::Config(String::from("set X"))), "no-key");
        assert_eq!(why_of(&ProviderError::Retryable { status: 0, message: String::new() }), "network");
        assert_eq!(why_of(&ProviderError::Retryable { status: 529, message: String::new() }), "provider-error");
        assert_eq!(why_of(&ProviderError::Request { status: 401, message: String::new() }), "provider-error");

        let (mut p, root) = laid("refuse");
        let dir = newest_landing(&p.checkout).unwrap();
        let cases = [
            (Err(ProviderError::Retryable { status: 0, message: String::from("refused") }), "network"),
            (Fake::text("no", StopReason::Refusal { category: None }), "provider-error"),
            (Err(ProviderError::Request { status: 401, message: String::from("bad key") }), "provider-error"),
        ];
        for (answer, why) in cases {
            p.start_with(Fake::answering(answer), ChatRequest::single(None, parts(PROMPT, LANDING)), dir.clone());
            settle(&mut p);
            assert!(p.verdict.contains(&format!("send=refused why={why} reply_kb=0")), "{}", p.verdict);
        }
        assert!(!dir.join("REPLY.md").exists(), "a refused send writes nothing");

        // No key anywhere (no host env, no Holocron, nothing typed): the live path refuses before any thread.
        assert!(on_input(&mut p, "provider send"));
        assert_eq!(p.verdict, "[installer] provider name=claude key=missing send=refused why=no-key reply_kb=0 ms=0");
        assert!(!p.busy());
        // A dir without PROMPT.md / landing.md.
        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(on_input(&mut p, &format!("provider send {}", empty.display())));
        assert!(p.verdict.contains("send=refused why=no-landing"), "{}", p.verdict);
        let bare = Provider::probe_at(root.join("nowhere"), root.join("home"));
        assert_eq!(newest_landing(&bare.checkout), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn provider_pick_goes_through_principia_and_the_key_is_never_shown() {
        let (mut p, root) = laid("pick");
        assert!(p.status().starts_with("[installer] provider name=claude key=missing send=- why=-"), "{}", p.status());
        assert!(on_input(&mut p, "provider pick gemini"));
        let store = principia::PrefStore::load(&p.prefs).unwrap();
        assert_eq!(store.get(PREF_NS, "provider"), Some(PrefValue::Str(String::from("gemini"))));
        assert_eq!(store.get(PREF_NS, "model"), Some(PrefValue::Str(GEMINI_DEFAULT_MODEL.to_string())));
        assert_eq!(store.get(PREF_NS, "gemini.auth"), Some(PrefValue::Str(String::from("api_key"))));
        assert_eq!(p.verdict, "[installer] provider name=gemini key=missing send=- why=- reply_kb=0 ms=0");

        let mut shown = Vec::new();
        assert!(on_input(&mut p, &format!("provider key {}", SECRETS[0])));
        assert_eq!(p.verdict, "[installer] provider name=gemini key=set send=- why=- reply_kb=0 ms=0");
        shown.push(p.verdict.clone());

        // The pick drops the held key; a pasted key-shaped raw line is the key.
        assert!(on_input(&mut p, "provider pick claude"));
        assert!(p.verdict.contains("name=claude key=missing"), "{}", p.verdict);
        assert!(on_input(&mut p, SECRETS[1]));
        assert!(p.verdict.contains("name=claude key=set"), "{}", p.verdict);
        shown.push(p.verdict.clone());

        // Any other raw line is the key only when the Key button armed it, and only once.
        assert!(!on_input(&mut p, "hello"));
        assert!(on_input(&mut p, "provider key"));
        assert!(p.arm_key);
        shown.push(p.verdict.clone());
        assert!(on_input(&mut p, SECRETS[2]));
        assert!(!p.arm_key);
        assert!(!on_input(&mut p, "landing read"));
        assert!(!on_input(&mut p, "card pick sdb"));
        shown.push(p.verdict.clone());
        shown.push(p.status());

        let TetraNode::VStack(kids) = face_of(&p) else { panic!("face is a VStack") };
        assert_eq!(kids.len(), 4);
        let TetraNode::Console(con) = &kids[0] else { panic!("status lines") };
        shown.extend(con.lines.iter().map(|l| l.text.clone()));
        assert!(matches!(&kids[2], TetraNode::TextField { id, .. } if id == "provider.key"));

        let mut files = Vec::new();
        files_under(&root, &mut files);
        assert!(files.iter().any(|f| f.starts_with(&p.prefs)), "the pick was persisted under the HOME");
        for f in &files {
            shown.push(String::from_utf8_lossy(&fs::read(f).unwrap()).into_owned());
        }
        for s in SECRETS {
            for text in &shown {
                assert!(!text.contains(s), "the key appeared: {text}");
            }
        }
        let _ = fs::remove_dir_all(root);
    }
}
