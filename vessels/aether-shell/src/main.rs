use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::LocalSet;
use bandy::{FacetCommand, FacetView, Origin, SMessage, Synapse};

/// Receipts the shell stamps on the requests it delegates to Facet: `'A'` in the top byte, the tab's id
/// in bits 32..48 (so the answer is routed back to the tab that asked), a per-tab counter below.
const FACET_RECEIPT_TAG: u64 = (b'A' as u64) << 56;

fn is_ours(c: &FacetCommand) -> bool {
    c.receipt_id().is_some_and(|r| r >> 56 == FACET_RECEIPT_TAG >> 56)
}

/// The tab a Facet receipt was stamped by.
fn receipt_tab(receipt: u64) -> u64 {
    (receipt >> 32) & 0xFFFF
}
// use quartzite::browser::bootstrap_browser;
use aether::AetherEngine;
use aether::chrome;

/// Collapse a burst of shell messages that arrived while the engine was busy.
///
/// A single live-resize drag or a flick of the wheel can deposit dozens of
/// messages in the broadcast channel while one relayout is running; replaying
/// all of them means the viewport chases a backlog instead of the cursor.
/// Two shapes, and only two:
///
///   * **Resize: last wins.** Every `BrowserResize` in the batch except the
///     final one is dropped — intermediate geometries are pure work with no
///     observable result. The survivor keeps its original position so
///     ordering against navigation/clicks is preserved.
///   * **Scroll: sum.** Adjacent `BrowserScroll` messages merge by adding
///     their deltas — a scroll is a *displacement*, so dropping one would
///     lose distance. Non-adjacent scrolls (something happened in between)
///     are left alone.
///   * **Pointer motion: last wins among neighbours.** Adjacent
///     `BrowserMouseMove` messages collapse to the latest — a pointer is a
///     *position*, and the page's hover follows where it is, not the path
///     it took while the engine was busy (SR67). A move separated by a click
///     or anything else is kept, so the click lands on what it hovered.
///
/// Everything else passes through untouched, in order.
fn coalesce(batch: Vec<SMessage>) -> Vec<SMessage> {
    let last_resize = batch
        .iter()
        .rposition(|m| matches!(m, SMessage::BrowserResize(..)));
    let mut out: Vec<SMessage> = Vec::with_capacity(batch.len());
    for (i, msg) in batch.into_iter().enumerate() {
        match msg {
            SMessage::BrowserResize(..) if Some(i) != last_resize => continue,
            SMessage::BrowserScroll(dx, dy) => {
                if let Some(SMessage::BrowserScroll(px, py)) = out.last_mut() {
                    *px += dx;
                    *py += dy;
                } else {
                    out.push(SMessage::BrowserScroll(dx, dy));
                }
            }
            SMessage::BrowserMouseMove(x, y) => {
                if let Some(SMessage::BrowserMouseMove(px, py)) = out.last_mut() {
                    (*px, *py) = (x, y);
                } else {
                    out.push(SMessage::BrowserMouseMove(x, y));
                }
            }
            other => out.push(other),
        }
    }
    out
}

// =================================================================================================
// AETHERCHROME (SR84): one engine per tab, a coordinator that owns the chrome model
// =================================================================================================
//
// Every tab is its own engine on its own thread (64 MB stack, a current-thread runtime and LocalSet —
// exactly what the single engine thread was), because the engine's page state is thread-local (the JS
// realm, the selection, the media registry): a thread per tab is "one engine per tab" by construction,
// with no state to swap on a switch. The COORDINATOR (one thread) is the only bus subscriber of the
// browser: it owns the tab strip, the bookmarks and the download list (`aether::chrome`), routes the
// chrome's input to the active tab's channel, Facet's answers to the tab whose receipt they carry, and
// Stria's to every tab. A tab fires its frames and chrome mirrors (url, title, favicon, toolbar state,
// status line) to the bus only while it is the active one, and re-presents all of them when it becomes
// active again; what the strip needs of every tab (url, title, loading) it reports to the coordinator.

/// The coordinator → one tab's engine thread.
enum TabCmd {
    /// A bus message for this tab (input, navigation, Facet / Stria answers).
    Msg(SMessage),
    /// This tab just became the active one: paint and re-mirror everything.
    Activate,
    /// The tab was closed.
    Shutdown,
}

/// A tab → the coordinator.
enum TabReport {
    Url(u64, String),
    Title(u64, String),
    Loading(u64, bool),
    /// A navigation's response is a download: the coordinator runs it (so it outlives its tab).
    StartDownload { url: String, name: String, total: Option<u64> },
    /// A download's progress row.
    Download(chrome::downloads::Download),
}

/// The page a failed traversal or reload shows (a NEW navigation's failure is `load_error_page`, which
/// records a history entry; a traversal's must not).
fn error_html(url: &str, error: &str) -> String {
    format!(
        "<html><head><title>Error</title></head><body style=\"background-color: #f8d7da; color: #721c24; padding: 20px; font-family: sans-serif;\"><h1>Navigation Error</h1><p>Failed to load {}: {}</p></body></html>",
        url, error
    )
}

fn spawn_tab(id: u64, bus: Synapse, active: Arc<AtomicU64>, reports: UnboundedSender<TabReport>) -> UnboundedSender<TabCmd> {
    let (tx, rx) = unbounded_channel();
    // Big stack: the ECMAScript engine's parser/interpreter is recursive-descent and real-world page
    // bundles nest deeply — the 2 MB default overflows (debug frames especially).
    thread::Builder::new()
        .name(format!("aether-tab-{id}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let local = LocalSet::new();
            local.block_on(&rt, Tab::new(id, bus, active, reports, rx).run());
        })
        .expect("spawn tab engine thread");
    tx
}

struct Tab {
    id: u64,
    engine: AetherEngine,
    bus: Synapse,
    active: Arc<AtomicU64>,
    reports: UnboundedSender<TabReport>,
    rx: UnboundedReceiver<TabCmd>,
    /// Commands that arrived while a load was in flight, run after it.
    deferred: VecDeque<TabCmd>,
    last_url: Option<String>,
    last_title: Option<String>,
    /// The favicon url of the page whose icon is showing, the one a load just staged (fetched after
    /// the page, off the critical path), and the decoded icon (re-shown when the tab is re-activated).
    last_favicon: Option<String>,
    pending_favicon: Option<String>,
    favicon: std::rc::Rc<std::cell::RefCell<Option<(u32, u32, Vec<u8>)>>>,
    facet_receipts: u64,
    facet_url: Option<String>,
    loading: bool,
    last_nav: Option<(bool, bool, bool, u32)>,
    last_status: String,
}

impl Tab {
    fn new(id: u64, bus: Synapse, active: Arc<AtomicU64>, reports: UnboundedSender<TabReport>, rx: UnboundedReceiver<TabCmd>) -> Self {
        Self {
            id,
            engine: AetherEngine::new(),
            bus,
            active,
            reports,
            rx,
            deferred: VecDeque::new(),
            last_url: None,
            last_title: None,
            last_favicon: None,
            pending_favicon: None,
            favicon: Default::default(),
            facet_receipts: 0,
            facet_url: None,
            loading: false,
            last_nav: None,
            last_status: String::new(),
        }
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed) == self.id
    }

    /// Fire a chrome-facing message — only while this is the active tab.
    fn show(&self, m: SMessage) {
        if self.is_active() {
            self.bus.fire(m);
        }
    }

    fn report(&self, r: TabReport) {
        let _ = self.reports.send(r);
    }

    fn blit(&self) {
        if self.is_active() {
            let (width, height, pixels) = self.engine.present_rgba();
            self.bus.fire(SMessage::SurfaceBlit { url: "viewport".into(), width, height, pixels });
        }
    }

    /// The tab became active: a full frame, then every mirror the chrome shows for a tab.
    fn present_all(&mut self) {
        if !self.is_active() {
            return;
        }
        let (w, h) = (self.engine.width, self.engine.height);
        self.engine.damage_rects.push((0, 0, w, h));
        let _ = self.engine.render_frame();
        self.blit();
        self.bus.fire(SMessage::BrowserUrlChanged(self.engine.current_url()));
        self.bus.fire(SMessage::BrowserTitleChanged(self.engine.title.clone()));
        if let Some((width, height, rgba)) = self.favicon.borrow().clone() {
            self.bus.fire(SMessage::BrowserFaviconChanged { width, height, rgba });
        }
        self.bus.fire(SMessage::BrowserCursorChanged(self.engine.cursor().to_string()));
        self.last_nav = None;
        self.last_status = String::new();
        self.bus.fire(SMessage::BrowserStatus(String::new()));
        self.mirror_chrome();
    }

    fn set_loading(&mut self, loading: bool) {
        if self.loading != loading {
            self.loading = loading;
            self.report(TabReport::Loading(self.id, loading));
            self.mirror_chrome();
        }
    }

    async fn run(mut self) {
        let _ = self.engine.render_frame();
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(16));
        loop {
            let mut ticked = false;
            if self.deferred.is_empty() {
                tokio::select! {
                    _ = tick.tick() => ticked = true,
                    c = self.rx.recv() => match c {
                        Some(c) => self.deferred.push_back(c),
                        None => return,
                    },
                }
            }
            // One relayout can take seconds on a heavy page, and a live-resize drag keeps firing the
            // whole time: take everything already waiting and collapse the stale work out of it.
            while let Ok(c) = self.rx.try_recv() {
                self.deferred.push_back(c);
            }
            if ticked && self.engine.tick() {
                let damages = self.engine.render_frame();
                if !damages.is_empty() {
                    self.blit();
                }
            }
            let mut batch = Vec::new();
            while let Some(c) = self.deferred.pop_front() {
                match c {
                    TabCmd::Shutdown => {
                        // The page's media stops with it (a blank document resets the registry and
                        // queues the stops).
                        self.engine.load_html("about:blank", "<html></html>", true);
                        for req in self.engine.take_media_requests() {
                            self.bus.fire(req);
                        }
                        return;
                    }
                    TabCmd::Activate => self.present_all(),
                    TabCmd::Msg(m) => batch.push(m),
                }
            }
            for m in coalesce(batch) {
                self.handle(m).await;
            }
            self.after_turn();
        }
    }

    /// Await `fut` unless the person stops the load first (Esc / the stop button) or the tab closes;
    /// anything else that arrives meanwhile runs after it.
    async fn cancellable<T>(&mut self, fut: impl std::future::Future<Output = T>) -> Option<T> {
        tokio::pin!(fut);
        loop {
            tokio::select! {
                out = &mut fut => return Some(out),
                c = self.rx.recv() => match c {
                    Some(TabCmd::Msg(SMessage::BrowserNavStop)) => return None,
                    Some(TabCmd::Activate) => self.present_all(),
                    Some(c @ TabCmd::Shutdown) => {
                        self.deferred.push_back(c);
                        return None;
                    }
                    Some(c) => self.deferred.push_back(c),
                    None => {
                        self.deferred.push_back(TabCmd::Shutdown);
                        return None;
                    }
                },
            }
        }
    }

    /// A new navigation (typed URL, link, bookmark): the download probe, then the load.
    async fn navigate(&mut self, url: String) {
        self.set_loading(true);
        let probe = self.cancellable(chrome::host::probe(&url)).await;
        match probe {
            None => {}
            Some(chrome::host::Probe::Download { url, name, total }) => {
                self.report(TabReport::StartDownload { url, name, total });
            }
            Some(chrome::host::Probe::Document) => self.load(url, true).await,
        }
        self.set_loading(false);
    }

    /// Fetch and show `url`; `add_history` false for a traversal or reload.
    async fn load(&mut self, url: String, add_history: bool) {
        self.set_loading(true);
        let fetched = self.cancellable(aether::net::fetch_page(&url)).await;
        match fetched {
            None => {}
            Some(Ok(page)) => {
                self.pending_favicon = page.favicon.clone();
                self.engine.load_page(page, add_history);
            }
            Some(Err(e)) if add_history => self.engine.load_error_page(&url, &e.to_string()),
            Some(Err(e)) => self.engine.load_html(&url, &error_html(&url, &e.to_string()), false),
        }
        self.set_loading(false);
    }

    async fn handle(&mut self, msg: SMessage) {
        match msg {
            SMessage::OpenDocument { url } if FacetCommand::local_image_path(&url).is_some() => {
                self.facet_receipts += 1;
                let path = FacetCommand::local_image_path(&url).unwrap_or_default();
                self.facet_url = Some(url);
                self.bus.fire(SMessage::Facet(FacetCommand::ImageOpen {
                    receipt_id: FACET_RECEIPT_TAG | ((self.id & 0xFFFF) << 32) | (self.facet_receipts & 0xFFFF_FFFF),
                    principal: Origin::LocalUser("aether".into()),
                    path,
                }));
            }
            SMessage::Facet(FacetCommand::ImageOpened { receipt_id, handle, .. }) if receipt_id >> 56 == FACET_RECEIPT_TAG >> 56 => {
                let (w, h) = self.engine.device_size();
                self.bus.fire(SMessage::Facet(FacetCommand::ImageRender {
                    receipt_id,
                    handle,
                    width: w.min(facet::MAX_VIEWPORT),
                    height: h.min(facet::MAX_VIEWPORT),
                    view: FacetView::default(),
                }));
            }
            SMessage::Facet(FacetCommand::ImageRendered { receipt_id, width, height, rgba, .. }) if receipt_id >> 56 == FACET_RECEIPT_TAG >> 56 => {
                self.show(SMessage::SurfaceBlit { url: "viewport".into(), width, height, pixels: rgba });
                if let Some(u) = self.facet_url.take() {
                    self.show(SMessage::BrowserUrlChanged(u.clone()));
                    self.report(TabReport::Url(self.id, u));
                }
            }
            SMessage::Facet(ref c @ FacetCommand::ImageError { ref message, .. }) if is_ours(c) => {
                let url = self.facet_url.take().unwrap_or_default();
                self.engine.load_error_page(&url, message);
            }
            SMessage::OpenDocument { url } => self.navigate(url).await,
            // Back / Forward walk the tab's session history: a same-document entry (pushState, a
            // fragment) is applied in place with `popstate`; another document's is fetched.
            SMessage::BrowserNavBack => {
                if let Some(url) = self.engine.traverse(-1) {
                    self.load(url, false).await;
                }
            }
            SMessage::BrowserNavForward => {
                if let Some(url) = self.engine.traverse(1) {
                    self.load(url, false).await;
                }
            }
            SMessage::BrowserNavReload => {
                let url = self.engine.current_url();
                if !url.is_empty() {
                    self.load(url, false).await;
                }
            }
            // A stop with no load in flight has nothing to stop.
            SMessage::BrowserNavStop => {}
            // The chrome speaks device pixels; the page lays out in CSS pixels (zoom).
            SMessage::BrowserScroll(dx, dy) => {
                let (dx, dy) = self.engine.to_css(dx, dy);
                self.engine.handle_event(aether::api::events::Event::Scroll(dx, dy));
            }
            SMessage::BrowserClick(x, y) => {
                let (x, y) = self.engine.to_css(x, y);
                self.engine.handle_event(aether::api::events::Event::MouseDown(x, y));
                self.engine.handle_event(aether::api::events::Event::MouseUp(x, y));
            }
            // Stria's replies for the page's <video>/<audio>: a frame marks its box damaged; the next
            // tick paints and blits it.
            ref m @ (SMessage::MediaOpened { .. } | SMessage::MediaFrame { .. } | SMessage::MediaEnded { .. } | SMessage::MediaError { .. }) => {
                self.engine.on_media_message(m);
            }
            SMessage::BrowserResize(w, h) => self.engine.set_device_size(w, h),
            SMessage::BrowserKey(key) => self.engine.handle_event(aether::api::events::Event::KeyDown(key)),
            SMessage::BrowserText(text) => self.engine.handle_event(aether::api::events::Event::Text(text)),
            // SR67: hover, `:hover` re-style, the page's mouse events.
            SMessage::BrowserMouseMove(x, y) => {
                let (x, y) = self.engine.to_css(x, y);
                self.engine.handle_event(aether::api::events::Event::MouseMove(x, y));
            }
            // AETHERSELECT (SR69): Ctrl/Cmd+C copies the page selection (innerText serialization) to the
            // platform clipboard through the chrome; Ctrl/Cmd+A selects the document.
            SMessage::BrowserCopy => {
                if let Some(set) = self.engine.copy_selection() {
                    self.show(set);
                }
            }
            SMessage::BrowserSelectAll => self.engine.select_all(),
            // The platform clipboard's text (Ctrl/Cmd+V): inserted at the focused field's caret.
            SMessage::BrowserPaste(text) => self.engine.paste(&text),
            SMessage::BrowserZoom(dir) => {
                self.engine.zoom_step(dir);
            }
            SMessage::BrowserFind { query, forward } => {
                let (matches, current) = self.engine.find(&query, forward);
                self.show(SMessage::BrowserFindResult { query, matches, current });
            }
            SMessage::BrowserFindClose => self.engine.find_close(),
            SMessage::BrowserPageInfoAsk => {
                let url = self.engine.current_url();
                let (secure, summary) = chrome::host::page_info(&url);
                self.show(SMessage::BrowserPageInfo { url, secure, summary });
            }
            _ => {}
        }
        // A link click or form submit staged a navigation; a page's `history.back()` may have staged a
        // cross-document traversal. Run them here where awaiting is legal (the engine thread must
        // never block on the runtime itself).
        if let Some(nav) = self.engine.take_pending_nav() {
            match nav.method {
                aether::forms::HttpMethod::Get => self.navigate(nav.url).await,
                aether::forms::HttpMethod::Post => {
                    let body = nav.body.unwrap_or_default();
                    let ct = nav.content_type.as_deref().unwrap_or("application/x-www-form-urlencoded").to_string();
                    // The response document's URL is the FINAL one (a POST is usually answered
                    // 303 → a GET page).
                    self.set_loading(true);
                    let posted = self.cancellable(aether::net::post_document_typed(&nav.url, &ct, &body)).await;
                    match posted {
                        None => {}
                        Some(Ok((html, final_url))) => self.engine.load_html(&final_url, &html, true),
                        Some(Err(e)) => self.engine.load_error_page(&nav.url, &e.to_string()),
                    }
                    self.set_loading(false);
                }
            }
        }
        if let Some(url) = self.engine.take_pending_traversal() {
            self.load(url, false).await;
        }
    }

    /// The toolbar's state and the status line, each deduped.
    fn mirror_chrome(&mut self) {
        let nav = (self.engine.can_go(-1), self.engine.can_go(1), self.loading, self.engine.zoom_pct());
        if self.last_nav != Some(nav) && self.is_active() {
            self.last_nav = Some(nav);
            self.bus.fire(SMessage::BrowserNavState { can_back: nav.0, can_forward: nav.1, loading: nav.2, zoom_pct: nav.3 });
        }
        let link = self.engine.hovered_link().unwrap_or_default();
        if link != self.last_status && self.is_active() {
            self.last_status = link.clone();
            self.bus.fire(SMessage::BrowserStatus(link));
        }
    }

    /// After every turn: Stria's requests, the cursor, and the chrome mirrors.
    fn after_turn(&mut self) {
        // Media requests the turn queued (posters on load, play/pause on a click, stops on navigation)
        // go to Stria — the page's own streams, no site code — active tab or not.
        for req in self.engine.take_media_requests() {
            self.bus.fire(req);
        }
        // The hovered element's cursor changed (SR67).
        if let Some(cursor) = self.engine.take_cursor_change() {
            self.show(SMessage::BrowserCursorChanged(cursor));
        }
        // One choke point for the address bar: whatever the turn did — link click, form submit,
        // Back, Forward, Reload, typed url, pushState, or a script-driven navigation run off the tick —
        // the session history's current entry is the truth, mirrored to the chrome exactly once.
        let url = self.engine.current_url();
        if !url.is_empty() && self.last_url.as_deref() != Some(url.as_str()) {
            self.last_url = Some(url.clone());
            self.show(SMessage::BrowserUrlChanged(url.clone()));
            self.report(TabReport::Url(self.id, url));
        }
        // The window title (and the tab's label) mirror the document's `<title>`, deduped.
        if self.last_title.as_deref() != Some(self.engine.title.as_str()) {
            self.last_title = Some(self.engine.title.clone());
            self.show(SMessage::BrowserTitleChanged(self.engine.title.clone()));
            self.report(TabReport::Title(self.id, self.engine.title.clone()));
        }
        // The favicon is chrome decoration: the fetch is detached onto the LocalSet, never awaited;
        // a failure is simply no icon.
        if let Some(fav) = self.pending_favicon.take() {
            if self.last_favicon.as_deref() != Some(fav.as_str()) {
                self.last_favicon = Some(fav.clone());
                let (bus, active, id, slot) = (self.bus.clone(), self.active.clone(), self.id, self.favicon.clone());
                tokio::task::spawn_local(async move {
                    if let Some((width, height, rgba)) = aether::net::fetch_favicon(&fav).await {
                        *slot.borrow_mut() = Some((width, height, rgba.clone()));
                        if active.load(Ordering::Relaxed) == id {
                            bus.fire(SMessage::BrowserFaviconChanged { width, height, rgba });
                        }
                    }
                });
            }
        }
        self.mirror_chrome();
    }
}

/// The browser's one bus subscriber: the tab strip, bookmarks and downloads (the `aether::chrome`
/// model), and the router between the chrome and the tabs' engines.
struct Coordinator {
    bus: Synapse,
    strip: chrome::tabs::TabStrip,
    tabs: HashMap<u64, UnboundedSender<TabCmd>>,
    active: Arc<AtomicU64>,
    reports: UnboundedSender<TabReport>,
    bookmarks: chrome::bookmarks::Bookmarks,
    downloads: chrome::downloads::DownloadList,
    next_download: u64,
    last_resize: Option<(u32, u32)>,
}

impl Coordinator {
    fn send(&self, id: u64, cmd: TabCmd) {
        if let Some(tx) = self.tabs.get(&id) {
            let _ = tx.send(cmd);
        }
    }

    fn to_active(&self, m: SMessage) {
        self.send(self.strip.active(), TabCmd::Msg(m));
    }

    fn open_tab(&mut self) {
        let id = self.strip.open();
        let tx = spawn_tab(id, self.bus.clone(), self.active.clone(), self.reports.clone());
        self.tabs.insert(id, tx);
        self.activate(id);
    }

    fn activate(&mut self, id: u64) {
        self.active.store(id, Ordering::Relaxed);
        if let Some((w, h)) = self.last_resize {
            self.send(id, TabCmd::Msg(SMessage::BrowserResize(w, h)));
        }
        self.send(id, TabCmd::Activate);
        self.publish_tabs();
        self.publish_bookmarks();
    }

    fn close_tab(&mut self, id: u64) {
        let was_active = self.strip.active() == id;
        self.send(id, TabCmd::Shutdown);
        self.tabs.remove(&id);
        match self.strip.close(id) {
            Some(next) if was_active => self.activate(next),
            Some(_) => self.publish_tabs(),
            // Never a window with nothing in it: the last tab's close opens a fresh one.
            None => self.open_tab(),
        }
    }

    fn publish_tabs(&self) {
        self.bus.fire(SMessage::BrowserTabs { active: self.strip.active(), tabs: self.strip.snapshot() });
    }

    fn publish_bookmarks(&self) {
        let current = self.strip.active_tab().is_some_and(|t| self.bookmarks.contains(&t.url));
        self.bus.fire(SMessage::BrowserBookmarks { items: self.bookmarks.pairs(), current });
    }

    fn on_bus(&mut self, m: SMessage) {
        match m {
            SMessage::BrowserTabNew => self.open_tab(),
            SMessage::BrowserTabClose(id) => {
                let id = id.unwrap_or(self.strip.active());
                if self.strip.get(id).is_some() {
                    self.close_tab(id);
                }
            }
            SMessage::BrowserTabSelect(id) => {
                if id != self.strip.active() && self.strip.select(id) {
                    self.activate(id);
                }
            }
            SMessage::BrowserTabStep(n) => {
                let before = self.strip.active();
                if let Some(id) = self.strip.step(n) {
                    if id != before {
                        self.activate(id);
                    }
                }
            }
            // The address bar: URL or search, decided once, by the model.
            SMessage::BrowserGo { input, search } => {
                if let Some(r) = chrome::omnibox::resolve(&input, &search) {
                    self.to_active(SMessage::OpenDocument { url: r.url().to_string() });
                }
            }
            SMessage::BrowserBookmarkToggle => {
                if let Some(t) = self.strip.active_tab() {
                    let (url, title) = (t.url.clone(), t.title.clone());
                    self.bookmarks.toggle(&url, &title);
                    if let Err(e) = chrome::host::save_bookmarks(&self.bookmarks) {
                        eprintln!("[aether] bookmarks not saved: {e}");
                    }
                    self.publish_bookmarks();
                }
            }
            SMessage::BrowserResize(w, h) => {
                self.last_resize = Some((w, h));
                self.to_active(SMessage::BrowserResize(w, h));
            }
            // Facet's answers go back to the tab whose receipt they carry.
            SMessage::Facet(c)
                if matches!(c, FacetCommand::ImageOpened { .. } | FacetCommand::ImageRendered { .. } | FacetCommand::ImageError { .. })
                    && is_ours(&c) =>
            {
                let tab = c.receipt_id().map(receipt_tab).unwrap_or(0);
                self.send(tab, TabCmd::Msg(SMessage::Facet(c)));
            }
            // Stria's answers are keyed by media url; every tab's engine takes the ones it asked for.
            SMessage::MediaOpened { .. } | SMessage::MediaFrame { .. } | SMessage::MediaEnded { .. } | SMessage::MediaError { .. } => {
                for tx in self.tabs.values() {
                    let _ = tx.send(TabCmd::Msg(m.clone()));
                }
            }
            SMessage::OpenDocument { .. }
            | SMessage::BrowserNavBack
            | SMessage::BrowserNavForward
            | SMessage::BrowserNavReload
            | SMessage::BrowserNavStop
            | SMessage::BrowserScroll(..)
            | SMessage::BrowserClick(..)
            | SMessage::BrowserKey(_)
            | SMessage::BrowserText(_)
            | SMessage::BrowserPaste(_)
            | SMessage::BrowserMouseMove(..)
            | SMessage::BrowserCopy
            | SMessage::BrowserSelectAll
            | SMessage::BrowserZoom(_)
            | SMessage::BrowserFind { .. }
            | SMessage::BrowserFindClose
            | SMessage::BrowserPageInfoAsk => self.to_active(m),
            _ => {}
        }
    }

    fn on_report(&mut self, r: TabReport) {
        match r {
            TabReport::Url(id, url) => {
                if self.strip.update(id, None, Some(&url), None) {
                    self.publish_tabs();
                    if id == self.strip.active() {
                        self.publish_bookmarks();
                    }
                }
            }
            TabReport::Title(id, title) => {
                if self.strip.update(id, Some(&title), None, None) {
                    self.publish_tabs();
                }
            }
            TabReport::Loading(id, loading) => {
                if self.strip.update(id, None, None, Some(loading)) {
                    self.publish_tabs();
                }
            }
            TabReport::StartDownload { url, name, total } => {
                self.next_download += 1;
                let (id, reports) = (self.next_download, self.reports.clone());
                tokio::task::spawn_local(async move {
                    chrome::host::download(id, url, name, total, |d| {
                        let _ = reports.send(TabReport::Download(d.clone()));
                    })
                    .await;
                });
            }
            TabReport::Download(d) => {
                self.bus.fire(SMessage::BrowserDownload {
                    id: d.id,
                    name: d.name.clone(),
                    received: d.received,
                    total: d.total,
                    done: d.done,
                    error: d.error.clone(),
                });
                self.downloads.upsert(d);
            }
        }
    }

    async fn run(mut self, mut bus_rx: tokio::sync::broadcast::Receiver<SMessage>, mut reports_rx: UnboundedReceiver<TabReport>) {
        self.open_tab();
        loop {
            tokio::select! {
                m = bus_rx.recv() => match m {
                    Ok(m) => self.on_bus(m),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return,
                },
                Some(r) = reports_rx.recv() => self.on_report(r),
            }
        }
    }
}

fn main() {
    let synapse = Synapse::new();
    // Stria's media service on the shell's bus: it answers the engine's `MediaPoster` /
    // `PlayMedia` / ... with `MediaOpened` / `MediaFrame` / `MediaEnded` / `MediaError`
    // (AETHERVIDEO, LEDGER SR39). Kept alive for the life of the process.
    let _stria_media = stria::media_bus::MediaService::spawn(synapse.clone(), 60, true);

    // SR29: images are Facet's. The shell serves the Images handler on its own Synapse, and a
    // navigation to a local image is DELEGATED to it (open, render at the viewport size, blit)
    // instead of being fetched and laid out as a page.
    {
        let facet_rx = synapse.subscribe();
        let facet_tx = synapse.clone();
        thread::Builder::new()
            .name("facet".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                rt.block_on(facet::serve(facet_tx, facet_rx, facet::Facet::new()));
            })
            .expect("spawn facet thread");
    }

    // AETHERCHROME (SR84): the coordinator (tab strip, bookmarks, downloads; the router) and, through
    // it, one engine thread per tab. Subscribed before the window exists, so nothing the chrome
    // fires is missed.
    {
        let bus = synapse.clone();
        let bus_rx = synapse.subscribe();
        thread::Builder::new()
            .name("aether-chrome".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                let local = LocalSet::new();
                let (reports, reports_rx) = unbounded_channel();
                let coordinator = Coordinator {
                    bus,
                    strip: chrome::tabs::TabStrip::new(),
                    tabs: HashMap::new(),
                    active: Arc::new(AtomicU64::new(0)),
                    reports,
                    bookmarks: chrome::host::load_bookmarks(),
                    downloads: chrome::downloads::DownloadList::default(),
                    next_download: 0,
                    last_resize: None,
                };
                local.block_on(&rt, coordinator.run(bus_rx, reports_rx));
            })
            .expect("spawn chrome coordinator thread");
    }

    // R104: the GTK face grows the browser chrome (quartzite's tab strip, url bar, find bar, status
    // line, downloads); the other faces keep the tetra toolbar until their arms carry it.
    #[cfg(all(target_os = "linux", feature = "gtk"))]
    {
        let syn = synapse.clone();
        quartzite::Backend::new_vessel("org.unaos.aether", "Aether Browser", (800.0, 600.0), move |window| {
            quartzite::platforms::gtk::bootstrap_browser_chrome(window, syn.clone())
        })
        .run();
    }

    #[cfg(not(all(target_os = "linux", feature = "gtk")))]
    {
        let aether_ui = quartzite::tetra::TetraNode::VStack(vec![
            quartzite::tetra::TetraNode::HStack(vec![
                quartzite::tetra::TetraNode::Button {
                    id: "back".into(),
                    label: "<".into(),
                    action: bandy::SMessage::BrowserNavBack,
                },
                quartzite::tetra::TetraNode::Button {
                    id: "fwd".into(),
                    label: ">".into(),
                    action: bandy::SMessage::BrowserNavForward,
                },
                quartzite::tetra::TetraNode::Button {
                    id: "reload".into(),
                    label: "C".into(),
                    action: bandy::SMessage::BrowserNavReload,
                },
                quartzite::tetra::TetraNode::TextField {
                    id: "url".into(),
                    placeholder: "Enter URL...".into(),
                    action: quartzite::tetra::TextAction::OpenDocument,
                },
            ]),
            quartzite::tetra::TetraNode::Surface { id: "viewport".into() },
        ]);

        quartzite::Backend::new_tetra_vessel("org.unaos.aether", "Aether Browser", (800.0, 600.0), aether_ui, synapse.clone()).run();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SR67: adjacent pointer motion collapses to the last position; a click between two moves keeps
    /// both (the click lands on what the first move hovered).
    #[test]
    fn coalesce_keeps_the_last_of_adjacent_pointer_moves() {
        let out = coalesce(vec![
            SMessage::BrowserMouseMove(1.0, 1.0),
            SMessage::BrowserMouseMove(2.0, 3.0),
            SMessage::BrowserMouseMove(5.0, 8.0),
            SMessage::BrowserClick(5.0, 8.0),
            SMessage::BrowserMouseMove(9.0, 9.0),
        ]);
        let shape: Vec<String> = out.iter().map(|m| format!("{m:?}")).collect();
        assert_eq!(shape, ["BrowserMouseMove(5.0, 8.0)", "BrowserClick(5.0, 8.0)", "BrowserMouseMove(9.0, 9.0)"]);
    }

    /// Scroll still sums and resize still keeps only the last geometry alongside pointer motion.
    #[test]
    fn coalesce_pointer_motion_leaves_scroll_and_resize_rules_alone() {
        let out = coalesce(vec![
            SMessage::BrowserResize(10, 10),
            SMessage::BrowserScroll(0.0, 1.0),
            SMessage::BrowserScroll(0.0, 2.0),
            SMessage::BrowserMouseMove(4.0, 4.0),
            SMessage::BrowserResize(20, 20),
        ]);
        let shape: Vec<String> = out.iter().map(|m| format!("{m:?}")).collect();
        assert_eq!(shape, ["BrowserScroll(0.0, 3.0)", "BrowserMouseMove(4.0, 4.0)", "BrowserResize(20, 20)"]);
    }
}
