//! AETHERMACCHROME (SR205): the browser's side tab column on AppKit. The strip's MODEL is the shell's one
//! `aether::chrome::tabs::TabStrip` (the `Coordinator`); this face paints what `SMessage::BrowserTabs` says through
//! [`crate::widgets::chrome_rows::side_rows`] (the model's `TabStrip::rows` over the wire) and fires the row's verb
//! ([`crate::widgets::chrome_rows::row_verb`]: `BrowserTabSelect` / `BrowserTabClose` / `BrowserTabGroupToggle` /
//! `BrowserTabNew`) through the existing [`super::button::bootstrap_button`]. A vertical `NSStackView`
//! `chrome_rows::SIDE_W` points wide: the find toggle on top, then one row per tab (the active one bordered), a header
//! per group, and `+` last. Every `BrowserTabs` rebuilds the rows (the url bar's subscription idiom: its own thread, a
//! current-thread runtime, the GCD hop to the main queue).
//!
//! STRIP2 (SR219): both strips, as the GTK face has them, from chrome_rows' one copy of each rule. Where the tabs stand
//! is Principia's `aether.tabs.layout` (`chrome_rows::stored_layout`, read at build and on its `PrefChanged`): the
//! side column or the TOP strip (a horizontal `NSStackView` under the toolbar walking `chrome_rows::top_rows`, its
//! `+` outside the scroller); only one shows. The side column scrolls (an `NSScrollView` over a flipped list) and
//! collapses to the `chrome_rows::RAIL_W` icon rail (its «/» toggle: tiles only). Each tab's slot shows its favicon
//! (`BrowserTabFavicon` through `chrome_rows::favicon_rgba` into an `NSImage` over `window_title::bitmap_rep`, the
//! one RGBA bitmap-rep builder) or the letter tile (`chrome_rows::tile_face`). Every cell is a [`UnaTabRow`]: a tab's
//! tile DRAGS (an `NSDraggingSession` carrying `chrome_rows::drag_payload`) and a cell takes a drop
//! (`TabWire::drop_verb`: on a tab its place, on a group's header or chip the group, on `+` the end); a right-click
//! opens the group verbs (`chrome_rows::tab_menu` / `group_menu` as an `NSMenu`, each item's target a
//! `bootstrap_button` it retains). The wire and the favicon images live in main-thread thread-locals, written only
//! inside the GCD main hop. The keys (Cmd+T among them) are the Tab menu's (`super::install_tab_keys`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, define_class, msg_send, ClassType, DeclaredClass};
use objc2_app_kit::{NSScrollView, NSStackView, NSView};
use objc2_foundation::{MainThreadMarker, NSArray, NSPoint, NSRect, NSSize, NSString};

use bandy::{BrowserTabRow, SMessage, Synapse};

use crate::widgets::chrome_rows::{self, DropOn, MenuRow, SideRow, TabWire, Tile};

/// `NSEventModifierFlagCommand`.
pub(crate) const CMD: usize = 1 << 20;
/// `NSEventModifierFlagShift` / `NSEventModifierFlagControl`.
pub(crate) const SHIFT: usize = 1 << 17;
pub(crate) const CONTROL: usize = 1 << 18;
/// The drag's pasteboard type (`NSPasteboardTypeString`) and `NSDragOperationMove`.
const PLAIN_TEXT: &str = "public.utf8-plain-text";
const DRAG_MOVE: usize = 16;
/// How far (points, x + y) the mouse moves on a tile before a press becomes a drag.
const DRAG_SLOP: f64 = 4.0;

fn zero() -> NSRect {
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
}

/// Lay `s` out as a chrome stack: `vertical` (else horizontal), Fill along the axis, children leading-aligned.
fn setup_stack(s: &NSStackView, vertical: bool) {
    unsafe {
        let _: () = msg_send![s, setOrientation: if vertical { 1isize } else { 0isize }];
        // NSStackViewDistributionFill
        let _: () = msg_send![s, setDistribution: 0isize];
        if vertical {
            // NSLayoutAttributeLeading = 5
            let _: () = msg_send![s, setAlignment: 5isize];
        }
        let _: () = msg_send![s, setSpacing: 2.0f64];
    }
}

/// A fresh `NSStackView`: `vertical` (else horizontal), Fill along the axis, children leading-aligned.
pub(crate) fn stack(vertical: bool) -> Retained<NSStackView> {
    unsafe {
        let s: Allocated<NSStackView> = msg_send![NSStackView::class(), alloc];
        let s: Retained<NSStackView> = msg_send![s, initWithFrame: zero()];
        setup_stack(&s, vertical);
        s
    }
}

/// `view` holds its size along `axis` (0 horizontal, 1 vertical): hugging and compression resistance high, so the
/// page beside it takes the slack.
pub(crate) fn hug(view: &NSView, axis: isize) {
    unsafe {
        let _: () = msg_send![view, setContentHuggingPriority: 750.0f32, forOrientation: axis];
        let _: () = msg_send![view, setContentCompressionResistancePriority: 750.0f32, forOrientation: axis];
    }
}

/// `view` yields along `axis`: hugging 1, so it takes its stack's slack.
fn yield_axis(view: &NSView, axis: isize) {
    unsafe {
        let _: () = msg_send![view, setContentHuggingPriority: 1.0f32, forOrientation: axis];
    }
}

/// Give `button` a key equivalent (`key` with `mask`, e.g. "f" + [`CMD`]).
pub(crate) fn key_equivalent(button: &NSView, key: &str, mask: usize) {
    unsafe {
        let k = NSString::from_str(key);
        let _: () = msg_send![button, setKeyEquivalent: &*k];
        let _: () = msg_send![button, setKeyEquivalentModifierMask: mask];
    }
}

/// Activate `a == b` between two layout anchors.
fn equal_anchor(a: &AnyObject, b: &AnyObject) {
    unsafe {
        let c: Retained<AnyObject> = msg_send![a, constraintEqualToAnchor: b];
        let _: () = msg_send![&*c, setActive: Bool::YES];
    }
}

/// Fix `view`'s `dimension` ("w" width, else height) to `pt` points; the constraint is returned (live: its constant
/// can change).
fn fix(view: &NSView, width: bool, pt: f64) -> Retained<AnyObject> {
    unsafe {
        let dim: Retained<AnyObject> = if width { msg_send![view, widthAnchor] } else { msg_send![view, heightAnchor] };
        let c: Retained<AnyObject> = msg_send![&*dim, constraintEqualToConstant: pt];
        let _: () = msg_send![&*c, setActive: Bool::YES];
        c
    }
}

/// An NSMenuItem titled `title` (key equivalent `key` with `mask`) whose action is `target`'s `onAction:` — a
/// `bootstrap_button` / `bootstrap_button_with` button. The item holds the target as its `representedObject`, so the
/// target lives exactly as long as the item (`target` is a weak reference).
pub(crate) fn menu_item(title: &str, key: &str, mask: usize, target: &NSView) -> Retained<AnyObject> {
    unsafe {
        let item: *mut AnyObject = msg_send![class!(NSMenuItem), alloc];
        let t = NSString::from_str(title);
        let k = NSString::from_str(key);
        let item: *mut AnyObject = msg_send![item, initWithTitle: &*t, action: Some(objc2::sel!(onAction:)), keyEquivalent: &*k];
        let item = Retained::from_raw(item).expect("NSMenuItem");
        let _: () = msg_send![&*item, setKeyEquivalentModifierMask: mask];
        let _: () = msg_send![&*item, setTarget: target];
        let _: () = msg_send![&*item, setRepresentedObject: target];
        item
    }
}

/// An empty `NSMenu` titled `title`, its items always enabled.
pub(crate) fn new_menu(title: &str) -> Retained<AnyObject> {
    unsafe {
        let m: *mut AnyObject = msg_send![class!(NSMenu), alloc];
        let t = NSString::from_str(title);
        let m: *mut AnyObject = msg_send![m, initWithTitle: &*t];
        let m = Retained::from_raw(m).expect("NSMenu");
        let _: () = msg_send![&*m, setAutoenablesItems: Bool::NO];
        m
    }
}

/// A right-click menu from chrome_rows' rows: each verb an item; the `None` row the group's rename entry (an item
/// whose view is a text field firing `BrowserTabGroupRename` on Enter).
fn menu_of(rows: Vec<MenuRow>, synapse: &Synapse) -> Retained<AnyObject> {
    let menu = new_menu("");
    for (label, verb, gid) in rows {
        let item = match verb {
            Some(m) => {
                let target = super::button::bootstrap_button(&label, m, synapse.clone());
                menu_item(&label, "", 0, &target)
            }
            None => {
                let syn = synapse.clone();
                let field = super::text_field::bootstrap_text_field_with(
                    &label,
                    Box::new(move |name: String| syn.fire(SMessage::BrowserTabGroupRename { group: gid, name })),
                );
                unsafe {
                    let item: *mut AnyObject = msg_send![class!(NSMenuItem), alloc];
                    let t = NSString::from_str(&label);
                    let k = NSString::from_str("");
                    let item: *mut AnyObject = msg_send![item, initWithTitle: &*t, action: None::<objc2::runtime::Sel>, keyEquivalent: &*k];
                    let item = Retained::from_raw(item).expect("NSMenuItem");
                    let _: () = msg_send![&*item, setView: &*field];
                    item
                }
            }
        };
        unsafe {
            let _: () = msg_send![&*menu, addItem: &*item];
        }
    }
    menu
}

/// `view`'s right-click menu.
fn set_menu(view: &NSView, menu: &AnyObject) {
    unsafe {
        let _: () = msg_send![view, setMenu: menu];
    }
}

// ---- UnaTabRow: the strips' one view class (a cell, a tile, a list) ----------------------------------------------

pub struct TabRowIvars {
    /// The tab this view drags (a tile; its hit test then takes every click inside it), or none.
    drag: Cell<Option<u64>>,
    /// The verb a click on a tile fires (a press that moves past [`DRAG_SLOP`] drags instead).
    click: RefCell<Option<SMessage>>,
    /// Where a tab dropped on this view lands (a registered drop target), or none.
    drop: Cell<Option<DropOn>>,
    synapse: RefCell<Option<Synapse>>,
    /// A list in a scroller: flipped, so its rows start at the top.
    flipped: Cell<bool>,
    /// The press (window point) and whether it became a drag.
    down: Cell<Option<(f64, f64)>>,
    dragging: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSStackView))]
    #[name = "UnaTabRow"]
    #[ivars = TabRowIvars]
    pub struct UnaTabRow;

    impl UnaTabRow {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(TabRowIvars {
                drag: Cell::new(None),
                click: RefCell::new(None),
                drop: Cell::new(None),
                synapse: RefCell::new(None),
                flipped: Cell::new(false),
                down: Cell::new(None),
                dragging: Cell::new(false),
            });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> Bool {
            Bool::new(self.ivars().flipped.get())
        }

        /// A tile takes every click inside it (its image or letter child never does).
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            let hit: *mut NSView = unsafe { msg_send![super(self), hitTest: point] };
            if !hit.is_null() && self.ivars().drag.get().is_some() {
                return self as *const Self as *mut NSView;
            }
            hit
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &AnyObject) {
            if self.ivars().drag.get().is_none() {
                let _: () = unsafe { msg_send![super(self), mouseDown: event] };
                return;
            }
            let p: NSPoint = unsafe { msg_send![event, locationInWindow] };
            self.ivars().down.set(Some((p.x, p.y)));
            self.ivars().dragging.set(false);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &AnyObject) {
            let (Some(id), Some((x, y))) = (self.ivars().drag.get(), self.ivars().down.get()) else {
                let _: () = unsafe { msg_send![super(self), mouseDragged: event] };
                return;
            };
            if self.ivars().dragging.get() {
                return;
            }
            let p: NSPoint = unsafe { msg_send![event, locationInWindow] };
            if (p.x - x).abs() + (p.y - y).abs() < DRAG_SLOP {
                return;
            }
            self.ivars().dragging.set(true);
            self.begin_drag(id, event);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &AnyObject) {
            if self.ivars().drag.get().is_none() {
                let _: () = unsafe { msg_send![super(self), mouseUp: event] };
                return;
            }
            let pressed = self.ivars().down.take().is_some();
            let dragged = self.ivars().dragging.replace(false);
            if pressed && !dragged {
                let verb = self.ivars().click.borrow().clone();
                let syn = self.ivars().synapse.borrow().clone();
                if let (Some(m), Some(s)) = (verb, syn) {
                    s.fire(m);
                }
            }
        }

        /// NSDraggingSource: a tab only moves.
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_mask(&self, _session: &AnyObject, _context: isize) -> usize {
            DRAG_MOVE
        }

        /// NSDraggingDestination: a drop target takes a dragged tab.
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, _info: &AnyObject) -> usize {
            if self.ivars().drop.get().is_some() { DRAG_MOVE } else { 0 }
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, _info: &AnyObject) -> usize {
            if self.ivars().drop.get().is_some() { DRAG_MOVE } else { 0 }
        }

        /// The drop: the payload's tab id, then chrome_rows' `TabWire::drop_verb` over the strip now.
        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, info: &AnyObject) -> Bool {
            let Some(on) = self.ivars().drop.get() else { return Bool::NO };
            let payload: Option<String> = unsafe {
                let pb: *mut AnyObject = msg_send![info, draggingPasteboard];
                if pb.is_null() {
                    None
                } else {
                    let ty = NSString::from_str(PLAIN_TEXT);
                    let s: Option<Retained<NSString>> = msg_send![pb, stringForType: &*ty];
                    s.map(|s| s.to_string())
                }
            };
            let Some(id) = payload.as_deref().and_then(chrome_rows::drag_id) else { return Bool::NO };
            let verb = LAST.with(|l| l.borrow().drop_verb(id, on));
            let syn = self.ivars().synapse.borrow().clone();
            match (verb, syn) {
                (Some(m), Some(s)) => {
                    s.fire(m);
                    Bool::YES
                }
                _ => Bool::NO,
            }
        }
    }
);

impl UnaTabRow {
    /// A fresh row: a chrome stack (`vertical`, else horizontal) that drags nothing and takes no drop.
    fn new(vertical: bool, synapse: &Synapse) -> Retained<Self> {
        unsafe {
            let r: Allocated<Self> = msg_send![Self::class(), alloc];
            let r: Retained<Self> = msg_send![r, initWithFrame: zero()];
            setup_stack(&r, vertical);
            *r.ivars().synapse.borrow_mut() = Some(synapse.clone());
            r
        }
    }

    /// This view takes a dropped tab, landing it `on`.
    fn accept_drops(&self, on: DropOn) {
        self.ivars().drop.set(Some(on));
        unsafe {
            let ty = NSString::from_str(PLAIN_TEXT);
            let types = NSArray::from_retained_slice(&[ty]);
            let _: () = msg_send![self, registerForDraggedTypes: &*types];
        }
    }

    /// Begin dragging tab `id`: the payload as plain text, the tile's own picture under the pointer.
    fn begin_drag(&self, id: u64, event: &AnyObject) {
        unsafe {
            let text = NSString::from_str(&chrome_rows::drag_payload(id));
            let item: *mut AnyObject = msg_send![class!(NSDraggingItem), alloc];
            let item: *mut AnyObject = msg_send![item, initWithPasteboardWriter: &*text];
            let Some(item) = Retained::<AnyObject>::from_raw(item) else { return };
            let bounds: NSRect = msg_send![self, bounds];
            let rep: *mut AnyObject = msg_send![self, bitmapImageRepForCachingDisplayInRect: bounds];
            let img: *mut AnyObject = msg_send![class!(NSImage), alloc];
            let img: *mut AnyObject = msg_send![img, initWithSize: bounds.size];
            let img = Retained::<AnyObject>::from_raw(img);
            if let (false, Some(img)) = (rep.is_null(), img.as_ref()) {
                let _: () = msg_send![self, cacheDisplayInRect: bounds, toBitmapImageRep: rep];
                let _: () = msg_send![&**img, addRepresentation: rep];
            }
            let _: () = msg_send![&*item, setDraggingFrame: bounds, contents: img.as_deref()];
            let items = NSArray::from_retained_slice(&[item]);
            let _: *mut AnyObject = msg_send![self, beginDraggingSessionWithItems: &*items, event: event, source: self];
        }
    }
}

fn as_view(v: Retained<UnaTabRow>) -> Retained<NSView> {
    unsafe { Retained::cast_unchecked::<NSView>(v) }
}

// ---- the wire and the favicons (main thread only) -----------------------------------------------------------------

/// The views both strips fill, held on the main thread.
struct Views {
    top: Retained<UnaTabRow>,
    side: Retained<UnaTabRow>,
    top_strip: Retained<NSView>,
    side_col: Retained<NSView>,
    width: Retained<AnyObject>,
    toggle: Retained<NSView>,
    synapse: Synapse,
}

thread_local! {
    /// The strip as the shell last sent it, each tab's favicon image, the rail and the layout in force (true: side).
    static LAST: RefCell<TabWire> = RefCell::new(TabWire::default());
    static ICONS: RefCell<HashMap<u64, Retained<AnyObject>>> = RefCell::new(HashMap::new());
    static RAIL: Cell<bool> = const { Cell::new(false) };
    static SIDE: Cell<bool> = const { Cell::new(false) };
    static VIEWS: RefCell<Option<Views>> = const { RefCell::new(None) };
}

/// The strip now (the tab search lists it).
pub(crate) fn wire() -> TabWire {
    LAST.with(|l| l.borrow().clone())
}

/// Tab `id`'s favicon image, if the engine sent one.
pub(crate) fn tile_image(id: u64) -> Option<Retained<AnyObject>> {
    ICONS.with(|i| i.borrow().get(&id).cloned())
}

/// The strip showing now and whether it is the side column (the tab search's anchor).
pub(crate) fn search_anchor() -> Option<(Retained<NSView>, bool)> {
    let side = SIDE.with(|s| s.get());
    VIEWS.with(|v| v.borrow().as_ref().map(|v| (if side { v.side_col.clone() } else { v.top_strip.clone() }, side)))
}

/// A favicon tile's `NSImage` (`FAVICON_PX` points) over the one bitmap-rep builder.
fn icon_image(px: &[u8], size: u32) -> Option<Retained<AnyObject>> {
    let rep = super::window_title::bitmap_rep(px, size)?;
    let pt = chrome_rows::FAVICON_PX as f64;
    unsafe {
        let img: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let img: *mut AnyObject = msg_send![img, initWithSize: NSSize::new(pt, pt)];
        let img = Retained::from_raw(img)?;
        let _: () = msg_send![&*img, addRepresentation: &*rep];
        Some(img)
    }
}

/// Take a strip-shaped message into the thread-locals; true when the strips should redraw. Main thread only.
fn take(m: &SMessage) -> bool {
    match m {
        SMessage::BrowserTabs { tabs, .. } => {
            ICONS.with(|i| i.borrow_mut().retain(|id, _| tabs.iter().any(|t| t.id == *id)));
            LAST.with(|l| l.borrow_mut().take(m))
        }
        SMessage::BrowserTabFavicon { id, size, rgba } => {
            match chrome_rows::favicon_rgba(*size, rgba).and_then(|px| icon_image(px, *size)) {
                Some(img) => ICONS.with(|i| i.borrow_mut().insert(*id, img)),
                None => ICONS.with(|i| i.borrow_mut().remove(id)),
            };
            true
        }
        _ => false,
    }
}

// ---- the cells ------------------------------------------------------------------------------------------------------

/// A tab's favicon slot, `d` points square: the favicon or the letter tile; it drags the tab, a click selects it.
fn tile_view(t: &BrowserTabRow, d: u32, synapse: &Synapse) -> Retained<NSView> {
    let tile = UnaTabRow::new(false, synapse);
    tile.ivars().drag.set(Some(t.id));
    *tile.ivars().click.borrow_mut() = Some(SMessage::BrowserTabSelect(t.id));
    let img = tile_image(t.id);
    let child: Retained<NSView> = match (chrome_rows::tile_face(t, img.is_some()), img) {
        (Tile::Icon, Some(img)) => unsafe { msg_send![class!(NSImageView), imageViewWithImage: &*img] },
        (Tile::Letter(l), _) => unsafe { Retained::cast_unchecked::<NSView>(super::find_bar::label(&l)) },
        (Tile::Icon, None) => unsafe { Retained::cast_unchecked::<NSView>(super::find_bar::label(&t.initial)) },
    };
    unsafe {
        let _: () = msg_send![&tile, addArrangedSubview: &*child];
        // NSLayoutAttributeCenterY = 12 across a horizontal stack.
        let _: () = msg_send![&tile, setAlignment: 12isize];
    }
    let _ = fix(&tile, true, d as f64);
    let _ = fix(&tile, false, d as f64);
    hug(&tile, 0);
    as_view(tile)
}

/// One cell of either strip (`top`: the top strip; `rail`: the side column collapsed to icons).
fn cell_view(row: &SideRow, top: bool, rail: bool, synapse: &Synapse) -> Retained<NSView> {
    let s = wire();
    let button = |label: &str, msg: SMessage| super::button::bootstrap_button(label, msg, synapse.clone());
    let cell = UnaTabRow::new(false, synapse);
    match row {
        SideRow::Group { id, name, count, collapsed } => {
            let text = if rail { (if *collapsed { "▸" } else { "▾" }).to_string() } else { chrome_rows::group_label(name, *count, *collapsed) };
            let b = button(&text, chrome_rows::row_verb(row, false));
            let menu = menu_of(chrome_rows::group_menu(&s, *id), synapse);
            set_menu(&b, &menu);
            set_menu(&cell, &menu);
            cell.accept_drops(DropOn::Group(*id));
            unsafe {
                let _: () = msg_send![&cell, addArrangedSubview: &*b];
            }
        }
        SideRow::Tab { id, label, active, grouped } => {
            let Some(t) = s.tabs.iter().find(|t| t.id == *id) else { return as_view(cell) };
            let tile = tile_view(t, if top { chrome_rows::TILE_TOP } else { chrome_rows::TILE_SIDE }, synapse);
            let menu = menu_of(chrome_rows::tab_menu(&s, *id), synapse);
            set_menu(&cell, &menu);
            cell.accept_drops(DropOn::Tab(*id));
            unsafe {
                let _: () = msg_send![&cell, addArrangedSubview: &*tile];
            }
            if !rail {
                let indent = if *grouped && !top { "  " } else { "" };
                let max = if top { chrome_rows::TOP_LABEL_MAX } else { chrome_rows::SIDE_LABEL_MAX };
                let select = button(&format!("{indent}{}", chrome_rows::clip(label, max)), chrome_rows::row_verb(row, false));
                set_menu(&select, &menu);
                let close = button("×", chrome_rows::row_verb(row, true));
                unsafe {
                    let _: () = msg_send![&select, setBordered: Bool::new(*active)];
                    let _: () = msg_send![&close, setBordered: Bool::NO];
                    let tip = NSString::from_str(if t.url.is_empty() { &t.label } else { &t.url });
                    let _: () = msg_send![&select, setToolTip: &*tip];
                }
                if !top {
                    // The title takes the row's width, the close box holds.
                    yield_axis(&select, 0);
                }
                hug(&close, 0);
                unsafe {
                    let _: () = msg_send![&cell, addArrangedSubview: &*select];
                    let _: () = msg_send![&cell, addArrangedSubview: &*close];
                }
            }
        }
        SideRow::New => {
            let plus = button("+", chrome_rows::row_verb(row, false));
            cell.accept_drops(DropOn::End);
            unsafe {
                let _: () = msg_send![&cell, addArrangedSubview: &*plus];
            }
        }
    }
    as_view(cell)
}

/// Replace `list`'s cells with `rows`. Main thread only.
fn fill(list: &NSStackView, rows: &[SideRow], top: bool, rail: bool, synapse: &Synapse) {
    unsafe {
        let old: Retained<NSArray<NSView>> = msg_send![list, arrangedSubviews];
        for v in old.to_vec() {
            let _: () = msg_send![list, removeArrangedSubview: &*v];
            let _: () = msg_send![&v, removeFromSuperview];
        }
        for row in rows {
            let v = cell_view(row, top, rail, synapse);
            let _: () = msg_send![list, addArrangedSubview: &*v];
        }
    }
}

/// Redraw both strips from the wire. Main thread only.
fn refill() {
    let s = wire();
    let rail = RAIL.with(|r| r.get());
    VIEWS.with(|v| {
        if let Some(v) = v.borrow().as_ref() {
            fill(&v.side, &chrome_rows::side_rows(s.active, &s.tabs, &s.groups), false, rail, &v.synapse);
            fill(&v.top, &chrome_rows::top_rows(s.active, &s.tabs, &s.groups), true, false, &v.synapse);
        }
    });
}

/// Show the side column (`side`) or the top strip. Main thread only.
fn apply_layout(side: bool) {
    SIDE.with(|s| s.set(side));
    VIEWS.with(|v| {
        if let Some(v) = v.borrow().as_ref() {
            unsafe {
                let _: () = msg_send![&v.top_strip, setHidden: Bool::new(side)];
                let _: () = msg_send![&v.side_col, setHidden: Bool::new(!side)];
            }
        }
    });
}

/// The side column's «/» toggle: the icon rail (`RAIL_W`, tiles only) or the open column (`SIDE_W`).
fn toggle_rail() {
    let now = RAIL.with(|r| {
        r.set(!r.get());
        r.get()
    });
    VIEWS.with(|v| {
        if let Some(v) = v.borrow().as_ref() {
            let w = if now { chrome_rows::RAIL_W } else { chrome_rows::SIDE_W };
            unsafe {
                let _: () = msg_send![&*v.width, setConstant: w as f64];
                let t = NSString::from_str(if now { "»" } else { "«" });
                let _: () = msg_send![&v.toggle, setTitle: &*t];
            }
        }
    });
    refill();
}

/// `doc` in an `NSScrollView` scrolling `vertical`ly (else horizontally), pinned to its clip view's leading and top
/// edges and its cross-axis size.
fn scroller(doc: &UnaTabRow, vertical: bool) -> Retained<NSView> {
    unsafe {
        let sc: Allocated<NSScrollView> = msg_send![NSScrollView::class(), alloc];
        let sc: Retained<NSScrollView> = msg_send![sc, initWithFrame: zero()];
        let _: () = msg_send![&sc, setHasVerticalScroller: Bool::new(vertical)];
        let _: () = msg_send![&sc, setHasHorizontalScroller: Bool::new(!vertical)];
        let _: () = msg_send![&sc, setAutohidesScrollers: Bool::YES];
        let _: () = msg_send![&sc, setDrawsBackground: Bool::NO];
        let _: () = msg_send![doc, setTranslatesAutoresizingMaskIntoConstraints: Bool::NO];
        let _: () = msg_send![&sc, setDocumentView: doc];
        let clip: Retained<NSView> = msg_send![&sc, contentView];
        let a: Retained<AnyObject> = msg_send![doc, leadingAnchor];
        let b: Retained<AnyObject> = msg_send![&clip, leadingAnchor];
        equal_anchor(&a, &b);
        let a: Retained<AnyObject> = msg_send![doc, topAnchor];
        let b: Retained<AnyObject> = msg_send![&clip, topAnchor];
        equal_anchor(&a, &b);
        let (a, b): (Retained<AnyObject>, Retained<AnyObject>) = if vertical {
            (msg_send![doc, widthAnchor], msg_send![&clip, widthAnchor])
        } else {
            (msg_send![doc, heightAnchor], msg_send![&clip, heightAnchor])
        };
        equal_anchor(&a, &b);
        Retained::cast_unchecked::<NSView>(sc)
    }
}

/// The two strips; the window shows one (the layout).
pub struct TabStrips {
    /// The top strip (under the toolbar).
    pub top: Retained<NSView>,
    /// The side column (beside the page), carrying `find_toggle`.
    pub side: Retained<NSView>,
}

/// Both strips from every `BrowserTabs` / `BrowserTabFavicon`, the layout from Principia's row and its changes.
pub fn bootstrap_tab_strips(synapse: Synapse, find_toggle: Retained<NSView>) -> TabStrips {
    let _mtm = MainThreadMarker::new().expect("bootstrap_tab_strips must run on the main thread");

    // The side column: the rail toggle, the find toggle, the scrolling list.
    let column = stack(true);
    let toggle = super::button::bootstrap_button_with("«", Box::new(toggle_rail));
    let side = UnaTabRow::new(true, &synapse);
    side.ivars().flipped.set(true);
    let side_scroll = scroller(&side, true);
    yield_axis(&side_scroll, 1);
    unsafe {
        let _: () = msg_send![&column, addArrangedSubview: &*toggle];
        let _: () = msg_send![&column, addArrangedSubview: &*find_toggle];
        let _: () = msg_send![&column, addArrangedSubview: &*side_scroll];
        // NSLayoutAttributeWidth = 7: the scroller fills the column's width.
        let _: () = msg_send![&column, setAlignment: 7isize];
    }
    let width = fix(&column, true, chrome_rows::SIDE_W as f64);
    hug(&column, 0);

    // The top strip: the scrolling cells, then `+` outside the scroller.
    let top_row = stack(false);
    let top = UnaTabRow::new(false, &synapse);
    let top_scroll = scroller(&top, false);
    yield_axis(&top_scroll, 0);
    let plus = cell_view(&SideRow::New, true, false, &synapse);
    unsafe {
        let _: () = msg_send![&top_row, addArrangedSubview: &*top_scroll];
        let _: () = msg_send![&top_row, addArrangedSubview: &*plus];
    }
    let _ = fix(&top_row, false, (chrome_rows::ROW_H + 4) as f64);
    hug(&top_row, 1);

    let (top_strip, side_col) = unsafe { (Retained::cast_unchecked::<NSView>(top_row), Retained::cast_unchecked::<NSView>(column)) };
    VIEWS.with(|v| {
        *v.borrow_mut() = Some(Views {
            top,
            side,
            top_strip: top_strip.clone(),
            side_col: side_col.clone(),
            width,
            toggle,
            synapse: synapse.clone(),
        })
    });
    refill();
    apply_layout(chrome_rows::stored_layout());

    let mut rx = synapse.subscribe();
    std::thread::Builder::new()
        .name("tab-strips".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tab strips subscription runtime");
            rt.block_on(async move {
                loop {
                    match rx.recv().await {
                        Ok(m @ (SMessage::BrowserTabs { .. } | SMessage::BrowserTabFavicon { .. })) => {
                            dispatch2::DispatchQueue::main().exec_async(move || {
                                if take(&m) {
                                    refill();
                                }
                            });
                        }
                        Ok(m) if chrome_rows::is_layout_change(&m) => {
                            let side = chrome_rows::stored_layout();
                            dispatch2::DispatchQueue::main().exec_async(move || apply_layout(side));
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        })
        .expect("spawn tab-strips thread");

    TabStrips { top: top_strip, side: side_col }
}
