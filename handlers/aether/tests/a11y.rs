//! AETHERA11Y (SR119): one test per accessibility rule over fixture pages — roles (HTML-AAM implicit,
//! WAI-ARIA explicit), the accessible name and description (accname 1.2), hidden subtrees, landmarks,
//! states, sequential focus navigation (HTML §6.6.3), keyboard activation and arrow keys, the focus ring,
//! the shell's reading verbs, the accessibility media features and the devtools panel. No network, no
//! WPT (R107): every page is loaded with `load_html`.

use std::sync::OnceLock;

use aether::api::events::Event;
use aether::chrome::devtools::{self, Panel};
use aether::dom::a11y::{self, focus, keyboard, name::NameFrom, nav, prefs::A11yPrefs, AxNode, AxTree, Tristate};
use aether::dom::NodeRef;
use aether::AetherEngine;

const URL: &str = "https://a11y.test/page.html";

fn init() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("aether-a11y-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        aether::storage::set_root_dir(Some(dir));
    });
}

fn page(html: &str) -> AetherEngine {
    init();
    let mut e = AetherEngine::new();
    e.handle_event(Event::Resize(400, 300));
    e.load_html(URL, html, true);
    e
}

fn dom(e: &AetherEngine, id: &str) -> NodeRef {
    e.document
        .as_ref()
        .unwrap()
        .document_node()
        .descendants()
        .find(|n| n.as_element().is_some_and(|el| el.attributes.borrow().get("id") == Some(id)))
        .unwrap_or_else(|| panic!("no #{id}"))
}

fn ax<'t>(t: &'t AxTree, e: &AetherEngine, id: &str) -> &'t AxNode {
    let n = dom(e, id);
    t.find_dom(n.id().0).unwrap_or_else(|| panic!("#{id} not in the accessibility tree:\n{}", t.lines().join("\n")))
}

fn absent(t: &AxTree, e: &AetherEngine, id: &str) -> bool {
    t.find_dom(dom(e, id).id().0).is_none()
}

fn focused_id(e: &AetherEngine) -> Option<String> {
    e.focused_node.as_ref().and_then(|n| n.as_element()).and_then(|el| el.attributes.borrow().get("id").map(str::to_string))
}

fn attr_of(e: &AetherEngine, id: &str, name: &str) -> Option<String> {
    dom(e, id).as_element().and_then(|el| el.attributes.borrow().get(name).map(str::to_string))
}

/// HTML-AAM §3.4: the implicit role of each element (the coverage table in
/// docs/dev/evidence/aethera11y/COVERAGE.md is this fixture).
#[test]
fn implicit_roles_follow_html_aam() {
    let e = page(
        r#"<!doctype html><html><head><title>Roles</title></head><body>
<header id="hdr">Site</header>
<nav id="nav"><a id="lnk" href="/x">Home</a> <a id="nohref">plain</a></nav>
<main id="main">
  <article id="art"><header id="arthdr">byline</header><h2 id="h2">Post</h2><p id="p">Para</p></article>
  <section id="secnamed" aria-label="Region">x</section>
  <aside id="aside">side</aside>
  <form id="frm" aria-label="Order">
    <input id="txt" type="text"><input id="srch" type="search"><input id="chk" type="checkbox">
    <input id="rad" type="radio"><input id="rng" type="range"><input id="num" type="number">
    <input id="sub" type="submit"><input id="em" type="email" list="dl"><textarea id="ta"></textarea>
    <select id="cmb"><option id="opt">One</option></select>
    <select id="lb" multiple><option>A</option></select>
    <button id="btn">Go</button>
  </form>
  <ul id="ul"><li id="li">item</li></ul>
  <table id="tbl"><tr id="tr"><th id="th">H</th><td id="td">D</td></tr></table>
  <img id="img" alt="Logo" style="display:inline-block;width:10px;height:10px">
  <hr id="hr"><progress id="prog" value="3" max="10"></progress><meter id="mtr" value="0.5"></meter>
  <output id="out">5</output>
  <figure id="fig"><figcaption>Cap</figcaption></figure>
  <fieldset id="fs"><legend>Legend</legend></fieldset>
  <dialog id="dlg" open>Hi</dialog>
  <blockquote id="bq">q</blockquote>
  <h1 id="h1">Top</h1>
  <details id="det" open><summary id="sum">More</summary>body</details>
</main>
<footer id="ftr">foot</footer>
</body></html>"#,
    );
    let t = a11y::tree(&e);
    assert_eq!(t.root().unwrap().role, "RootWebArea");
    assert_eq!(t.root().unwrap().name, "Roles");
    for (id, role) in [
        ("hdr", "banner"), ("nav", "navigation"), ("lnk", "link"), ("main", "main"), ("art", "article"),
        ("h2", "heading"), ("p", "paragraph"), ("secnamed", "region"), ("aside", "complementary"), ("frm", "form"),
        ("txt", "textbox"), ("srch", "searchbox"), ("chk", "checkbox"), ("rad", "radio"), ("rng", "slider"),
        ("num", "spinbutton"), ("sub", "button"), ("em", "combobox"), ("ta", "textbox"), ("cmb", "combobox"),
        ("opt", "option"), ("lb", "listbox"), ("btn", "button"), ("ul", "list"), ("li", "listitem"), ("tbl", "table"),
        ("tr", "row"), ("th", "columnheader"), ("td", "cell"), ("img", "img"), ("hr", "separator"),
        ("prog", "progressbar"), ("mtr", "meter"), ("out", "status"), ("fig", "figure"), ("fs", "group"),
        ("dlg", "dialog"), ("bq", "blockquote"), ("h1", "heading"), ("det", "group"), ("sum", "button"),
        ("ftr", "contentinfo"),
    ] {
        assert_eq!(ax(&t, &e, id).role, role, "#{id}");
    }
    // A header inside an article is not the banner; an <a> without href and an unnamed section are
    // generic (flattened out of the tree).
    assert!(absent(&t, &e, "arthdr"), "scoped header is generic");
    assert!(absent(&t, &e, "nohref"));
    assert_eq!(ax(&t, &e, "h1").level, Some(1));
    assert_eq!(ax(&t, &e, "h2").level, Some(2));
}

/// WAI-ARIA 1.2 §5.4 / §9.1: the first valid concrete token wins, abstract roles are refused, and a
/// presentational role yields to focusability and global properties.
#[test]
fn explicit_roles_and_presentation_conflict() {
    let e = page(
        r#"<body>
<div id="a" role="bogus switch checkbox" aria-checked="true">s</div>
<div id="b" role="widget button">abstract skipped</div>
<div id="c" role="landmark">only abstract</div>
<h3 id="d" role="presentation">plain</h3>
<button id="f" role="none">still a button</button>
<h3 id="g" role="none" aria-label="labelled">kept</h3>
<ul role="presentation"><li id="h">inherits presentation</li></ul>
</body>"#,
    );
    let t = a11y::tree(&e);
    assert_eq!(ax(&t, &e, "a").role, "switch");
    assert_eq!(ax(&t, &e, "a").states.checked, Some(Tristate::True));
    assert_eq!(ax(&t, &e, "b").role, "button");
    assert!(absent(&t, &e, "c"), "an abstract role is ignored: the div stays generic");
    assert!(absent(&t, &e, "d"), "presentation removes the heading semantics");
    assert_eq!(ax(&t, &e, "f").role, "button", "focusable: presentation ignored");
    assert_eq!(ax(&t, &e, "g").role, "heading", "a global aria-* property: presentation ignored");
    assert!(absent(&t, &e, "h"), "a required owned element inherits presentation");
}

/// accname 1.2 steps 2B, 2C, 2D: aria-labelledby beats aria-label beats the host language's label.
#[test]
fn name_precedence_labelledby_label_native() {
    let e = page(
        r#"<body>
<span id="l1">First</span><span id="l2" hidden>Hidden part</span>
<input id="a" aria-labelledby="l1 l2" aria-label="ignored">
<label for="b">Label for</label><input id="b" aria-label="Aria wins">
<label for="c">Explicit</label><input id="c">
<label>Wrapping <input id="d"></label>
<button id="e" aria-labelledby="e x" aria-label="Self">Body</button><span id="x">Extra</span>
</body>"#,
    );
    let t = a11y::tree(&e);
    let a = ax(&t, &e, "a");
    assert_eq!((a.name.as_str(), a.name_from), ("First Hidden part", NameFrom::LabelledBy), "a hidden referenced node still names");
    assert_eq!((ax(&t, &e, "b").name.as_str(), ax(&t, &e, "b").name_from), ("Aria wins", NameFrom::AriaLabel));
    assert_eq!((ax(&t, &e, "c").name.as_str(), ax(&t, &e, "c").name_from), ("Explicit", NameFrom::Label));
    assert_eq!(ax(&t, &e, "d").name, "Wrapping");
    assert_eq!(ax(&t, &e, "e").name, "Self Extra", "a self-reference names by the node's own aria-label");
}

/// HTML-AAM §4.1 step 2D: alt, legend, caption, figcaption, button values and defaults, option labels.
#[test]
fn name_from_host_language() {
    let e = page(
        r#"<body>
<img id="i" alt="A cat" style="display:inline-block;width:4px;height:4px">
<fieldset id="fs"><legend>Shipping</legend><input></fieldset>
<table id="tb"><caption>Prices</caption><tr><td>1</td></tr></table>
<figure id="fg"><img alt="" style="display:block;width:4px;height:4px"><figcaption>Figure one</figcaption></figure>
<input id="s" type="submit"><input id="r" type="reset"><input id="v" type="button" value="Press">
<input id="im" type="image" alt="Search" style="width:4px;height:4px">
<select id="sel"><option id="o" label="Short">A long option text</option></select>
</body>"#,
    );
    let t = a11y::tree(&e);
    for (id, name, from) in [
        ("i", "A cat", NameFrom::Alt),
        ("fs", "Shipping", NameFrom::Legend),
        ("tb", "Prices", NameFrom::Caption),
        ("fg", "Figure one", NameFrom::Figcaption),
        ("s", "Submit", NameFrom::Value),
        ("r", "Reset", NameFrom::Value),
        ("v", "Press", NameFrom::Value),
        ("im", "Search", NameFrom::Alt),
        ("o", "Short", NameFrom::Value),
    ] {
        let n = ax(&t, &e, id);
        assert_eq!((n.name.as_str(), n.name_from), (name, from), "#{id}");
    }
}

/// accname 1.2 steps 2E and 2F: name from content (block children spaced, hidden ones skipped) and an
/// embedded control contributing its value.
#[test]
fn name_from_content_and_embedded_control() {
    let e = page(
        r#"<body>
<a id="a" href="/y"><span>Read</span> <b>more</b><span style="display:none">secret</span></a>
<button id="b"><div>Line one</div><div>Line two</div></button>
<label for="c">Quantity <input id="q" value="3"> items</label><input id="c" type="checkbox">
<h2 id="h"><img alt="Logo" style="display:inline-block;width:4px;height:4px"> Home</h2>
<div id="d" role="link" tabindex="0">Click <span aria-hidden="true">icon</span>here</div>
</body>"#,
    );
    let t = a11y::tree(&e);
    assert_eq!((ax(&t, &e, "a").name.as_str(), ax(&t, &e, "a").name_from), ("Read more", NameFrom::Contents));
    assert_eq!(ax(&t, &e, "b").name, "Line one Line two");
    assert_eq!(ax(&t, &e, "c").name, "Quantity 3 items", "the textbox inside the label gives its value");
    assert_eq!(ax(&t, &e, "h").name, "Logo Home");
    assert_eq!(ax(&t, &e, "d").name, "Click here");
}

/// accname 1.2 step 2I and HTML-AAM: title as the last resort, placeholder after it, and descriptions.
#[test]
fn title_placeholder_and_description() {
    let e = page(
        r#"<body>
<input id="a" title="Tooltip name">
<input id="b" placeholder="Search…">
<button id="c" title="Saves the file">Save</button>
<span id="hint">Eight characters minimum</span>
<input id="d" aria-label="Password" aria-describedby="hint">
<button id="f" aria-description="Opens a dialog">Share</button>
</body>"#,
    );
    let t = a11y::tree(&e);
    assert_eq!((ax(&t, &e, "a").name.as_str(), ax(&t, &e, "a").name_from), ("Tooltip name", NameFrom::Title));
    assert_eq!((ax(&t, &e, "b").name.as_str(), ax(&t, &e, "b").name_from), ("Search…", NameFrom::Placeholder));
    assert_eq!(ax(&t, &e, "c").name, "Save");
    assert_eq!(ax(&t, &e, "c").description, "Saves the file", "an unused title describes");
    assert_eq!(ax(&t, &e, "d").description, "Eight characters minimum");
    assert_eq!(ax(&t, &e, "f").description, "Opens a dialog");
    assert_eq!(ax(&t, &e, "a").description, "", "a title used as the name does not describe too");
}

/// WAI-ARIA 1.2 §7.1: hidden subtrees are pruned; a visible child of a visibility:hidden parent stays.
#[test]
fn hidden_subtrees_are_pruned() {
    let e = page(
        r#"<head><style>.gone { display: none } .ghost { visibility: hidden } .back { visibility: visible }</style></head><body>
<button id="a" hidden>hidden attr</button>
<div aria-hidden="true"><button id="b">aria-hidden</button></div>
<div class="gone"><button id="c">display none</button></div>
<div class="ghost"><button id="d">invisible</button><button id="e" class="back">visible again</button></div>
<div inert><button id="f">inert</button></div>
<button id="g">shown</button>
</body>"#,
    );
    let t = a11y::tree(&e);
    for id in ["a", "b", "c", "d", "f"] {
        assert!(absent(&t, &e, id), "#{id} must be pruned:\n{}", t.lines().join("\n"));
    }
    assert_eq!(ax(&t, &e, "e").name, "visible again");
    assert_eq!(ax(&t, &e, "g").name, "shown");
}

/// WAI-ARIA 1.2 §5.3.4: landmarks in order; a region and a form are landmarks only when named.
#[test]
fn landmarks_are_listed_in_order() {
    let e = page(
        r#"<body>
<header>Top</header><nav aria-label="Primary">n</nav>
<main><section aria-label="News">s</section><section>anon</section><form>unnamed</form>
<form aria-label="Newsletter"><input></form><search>q</search></main>
<aside>a</aside><footer>f</footer>
</body>"#,
    );
    let ls: Vec<(&str, String)> = nav::landmarks(&e).into_iter().map(|l| (l.role, l.name)).collect();
    assert_eq!(
        ls,
        vec![
            ("banner", String::new()),
            ("navigation", "Primary".into()),
            ("main", String::new()),
            ("region", "News".into()),
            ("form", "Newsletter".into()),
            ("search", String::new()),
            ("complementary", String::new()),
            ("contentinfo", String::new()),
        ]
    );
}

/// WAI-ARIA 1.2 §6.6/§6.7 and HTML-AAM §3.5: states and properties from aria-* and native semantics.
#[test]
fn states_and_properties() {
    let e = page(
        r#"<body>
<input id="a" type="checkbox" checked required>
<div id="b" role="checkbox" aria-checked="mixed" tabindex="0">tri</div>
<button id="c" aria-pressed="true" aria-expanded="false" aria-haspopup="true">Menu</button>
<button id="d" disabled>Off</button>
<fieldset disabled><input id="e"></fieldset>
<input id="f" type="range" min="10" max="20" value="15">
<textarea id="g" readonly>t</textarea>
<details id="det"><summary id="h">S</summary>x</details>
<select id="i" multiple><option id="j" selected>A</option><option id="k">B</option></select>
<div id="l" role="alert">Saved</div>
<a id="m" href="/" aria-current="page">Here</a>
<input id="n" aria-invalid="true" value="bad">
</body>"#,
    );
    let t = a11y::tree(&e);
    let a = &ax(&t, &e, "a").states;
    assert_eq!((a.checked, a.required, a.focusable), (Some(Tristate::True), true, true));
    assert_eq!(ax(&t, &e, "b").states.checked, Some(Tristate::Mixed));
    let c = &ax(&t, &e, "c").states;
    assert_eq!((c.pressed, c.expanded, c.haspopup.as_deref()), (Some(Tristate::True), Some(false), Some("menu")));
    assert!(ax(&t, &e, "d").states.disabled && !ax(&t, &e, "d").states.focusable);
    assert!(ax(&t, &e, "e").states.disabled, "a disabled fieldset disables its controls");
    let f = &ax(&t, &e, "f").states;
    assert_eq!((f.value_min, f.value_max, f.value_now), (Some(10.0), Some(20.0), Some(15.0)));
    let g = &ax(&t, &e, "g").states;
    assert!(g.readonly && g.multiline);
    assert_eq!(ax(&t, &e, "h").states.expanded, Some(false), "a closed details' summary is collapsed");
    assert!(ax(&t, &e, "i").states.multiselectable);
    assert_eq!(ax(&t, &e, "j").states.selected, Some(true));
    assert_eq!(ax(&t, &e, "k").states.selected, Some(false));
    assert_eq!(ax(&t, &e, "l").states.live.as_deref(), Some("assertive"));
    assert_eq!(ax(&t, &e, "m").states.current.as_deref(), Some("page"));
    assert!(ax(&t, &e, "n").states.invalid);
}

/// HTML §6.6.3: positive tabindex first (ascending, tree order among equals), then tabindex 0 and the
/// default-focusable in tree order; negative, disabled, hidden and inert elements are not stops.
#[test]
fn tab_order_follows_tabindex() {
    let mut e = page(
        r#"<body>
<button id="z">zero-default</button>
<div id="p2" tabindex="2">two</div>
<a id="l" href="/x">link</a>
<div id="p1" tabindex="1">one</div>
<div id="n" tabindex="-1">negative</div>
<button id="dis" disabled>disabled</button>
<button id="hid" style="display:none">hidden</button>
<div inert><button id="ine">inert</button></div>
<div id="p1b" tabindex=" +1x">one again</div>
<span id="ce" contenteditable>edit</span>
<a id="nohref">not focusable</a>
</body>"#,
    );
    let mut seen = Vec::new();
    for _ in 0..7 {
        assert!(keyboard::handle_key(&mut e, "Tab"));
        seen.push(focused_id(&e).unwrap());
    }
    assert_eq!(seen, ["p1", "p1b", "p2", "z", "l", "ce", "p1"], "wraps to the first stop");
    assert_eq!(focus::parse_integer(" +1x"), Some(1));
    assert_eq!(focus::parse_integer("x1"), None);
}

/// Shift+Tab (and GTK's ISO_Left_Tab) walk the order backwards; a Tab from an element that is not a
/// stop continues from its tree position.
#[test]
fn shift_tab_goes_backwards() {
    let mut e = page(r#"<body><button id="a">a</button><div id="mid" tabindex="-1">m</div><button id="b">b</button><button id="c">c</button></body>"#);
    assert!(keyboard::handle_key(&mut e, "Shift+Tab"));
    assert_eq!(focused_id(&e).as_deref(), Some("c"), "from nothing, Shift+Tab lands on the last stop");
    assert!(keyboard::handle_key(&mut e, "ISO_Left_Tab"));
    assert_eq!(focused_id(&e).as_deref(), Some("b"));
    let mid = dom(&e, "mid");
    focus::focus(&mut e, Some(mid), false);
    assert!(keyboard::handle_key(&mut e, "Tab"));
    assert_eq!(focused_id(&e).as_deref(), Some("b"), "the next stop after #mid in tree order");
    assert!(!keyboard::handle_key(&mut e, "Ctrl+Tab"), "Ctrl+Tab is the shell's, not the page's");
}

/// HTML §4.10.5.1.18: a radio group is one Tab stop (its checked radio); arrows move within the group,
/// checking as they go, and wrap.
#[test]
fn radio_group_is_one_stop_and_arrows_check() {
    let mut e = page(
        r#"<body><button id="before">before</button>
<input type="radio" name="size" id="s"><input type="radio" name="size" id="m" checked><input type="radio" name="size" id="l">
<button id="after">after</button></body>"#,
    );
    keyboard::handle_key(&mut e, "Tab");
    keyboard::handle_key(&mut e, "Tab");
    assert_eq!(focused_id(&e).as_deref(), Some("m"), "the checked radio is the group's stop");
    assert!(keyboard::handle_key(&mut e, "Down"));
    assert_eq!(focused_id(&e).as_deref(), Some("l"));
    let t = a11y::tree(&e);
    assert_eq!(ax(&t, &e, "l").states.checked, Some(Tristate::True), "arrowing checks");
    assert_eq!(ax(&t, &e, "m").states.checked, Some(Tristate::False));
    assert!(keyboard::handle_key(&mut e, "Right"));
    assert_eq!(focused_id(&e).as_deref(), Some("s"), "wraps to the first radio");
    keyboard::handle_key(&mut e, "Tab");
    assert_eq!(focused_id(&e).as_deref(), Some("after"), "Tab leaves the group in one step");
    keyboard::handle_key(&mut e, "Shift+Tab");
    assert_eq!(focused_id(&e).as_deref(), Some("s"), "back into the group at its checked radio");
}

/// WAI-ARIA APG listbox: Up/Down/Home/End move the active option; selection follows focus in a
/// single-select listbox, both for roving focus and for aria-activedescendant.
#[test]
fn listbox_arrows_move_selection() {
    let mut e = page(
        r#"<body>
<ul id="lb" role="listbox" aria-label="Fruit">
  <li id="o1" role="option" tabindex="0" aria-selected="true">Apple</li>
  <li id="o2" role="option" tabindex="-1">Banana</li>
  <li id="o3" role="option" tabindex="-1" aria-disabled="true">Cherry</li>
  <li id="o4" role="option" tabindex="-1">Damson</li>
</ul>
<div id="ad" role="listbox" tabindex="0" aria-activedescendant="a1">
  <div id="a1" role="option">One</div><div id="a2" role="option">Two</div>
</div></body>"#,
    );
    keyboard::handle_key(&mut e, "Tab");
    assert_eq!(focused_id(&e).as_deref(), Some("o1"));
    assert!(keyboard::handle_key(&mut e, "Down"));
    assert_eq!(focused_id(&e).as_deref(), Some("o2"));
    assert_eq!(attr_of(&e, "o2", "aria-selected").as_deref(), Some("true"));
    assert_eq!(attr_of(&e, "o1", "aria-selected").as_deref(), Some("false"));
    assert!(keyboard::handle_key(&mut e, "Down"));
    assert_eq!(focused_id(&e).as_deref(), Some("o4"), "a disabled option is skipped");
    assert!(keyboard::handle_key(&mut e, "Home"));
    assert_eq!(focused_id(&e).as_deref(), Some("o1"));
    let ad = dom(&e, "ad");
    focus::focus(&mut e, Some(ad), true);
    assert!(keyboard::handle_key(&mut e, "Down"));
    assert_eq!(focused_id(&e).as_deref(), Some("ad"), "the listbox keeps focus");
    assert_eq!(attr_of(&e, "ad", "aria-activedescendant").as_deref(), Some("a2"));
    assert_eq!(attr_of(&e, "a2", "aria-selected").as_deref(), Some("true"));
}

/// Enter on a focused link follows it (the trusted click's activation behavior); Enter on a summary
/// toggles its details.
#[test]
fn enter_activates_links_and_summaries() {
    let mut e = page(r#"<body><a id="a" href="/next.html">Next</a><details id="d"><summary id="s">More</summary>body</details></body>"#);
    keyboard::handle_key(&mut e, "Tab");
    assert_eq!(focused_id(&e).as_deref(), Some("a"));
    assert!(keyboard::handle_key(&mut e, "Return"));
    e.tick();
    let nav = e.take_pending_nav().expect("Enter on a link navigates");
    assert_eq!(nav.url, "https://a11y.test/next.html");
    keyboard::handle_key(&mut e, "Tab");
    assert_eq!(focused_id(&e).as_deref(), Some("s"));
    assert!(keyboard::handle_key(&mut e, "space"));
    e.tick();
    assert!(attr_of(&e, "d", "open").is_some(), "Space on the summary opens the details");
    assert!(!keyboard::handle_key(&mut e, "a"), "a printable key is not a navigation key");
}

/// Space and Return on a keyboard-focused native button reach its click through the engine's own
/// activation (the key path a face sends), after Tab moved focus there.
#[test]
fn space_activates_a_tabbed_button() {
    let mut e = page(r#"<body><button id="b" onclick="this.setAttribute('data-hit', (+this.getAttribute('data-hit') || 0) + 1)">Go</button></body>"#);
    keyboard::handle_key(&mut e, "Tab");
    assert_eq!(focused_id(&e).as_deref(), Some("b"));
    assert!(!keyboard::handle_key(&mut e, "space"), "native button activation is the engine's");
    e.handle_event(Event::KeyDown("space".into()));
    assert_eq!(attr_of(&e, "b", "data-hit").as_deref(), Some("1"));
    e.handle_event(Event::KeyDown("Return".into()));
    assert_eq!(attr_of(&e, "b", "data-hit").as_deref(), Some("2"));
}

/// HTML §6.6.4: a keyboard focus move is the script's too — document.activeElement, blur/focus and
/// focusout/focusin with relatedTarget — and :focus-visible matches.
#[test]
fn focus_moves_fire_events_and_match_focus_visible() {
    let mut e = page(
        r#"<head><style>button:focus-visible { color: rgb(1, 2, 3); }</style></head><body>
<button id="a">a</button><button id="b">b</button>
<script>
  var log = [];
  for (const id of ['a', 'b']) {
    const el = document.getElementById(id);
    for (const t of ['focus', 'blur', 'focusin', 'focusout'])
      el.addEventListener(t, ev => { log.push(t + ':' + id + ':' + (ev.relatedTarget ? ev.relatedTarget.id : '-')); document.body.setAttribute('data-log', log.join(' ')); });
  }
</script></body>"#,
    );
    keyboard::handle_key(&mut e, "Tab");
    keyboard::handle_key(&mut e, "Tab");
    let log = e.document.as_ref().unwrap().document_node().descendants().find(|n| n.as_element().is_some_and(|el| &*el.name.local == "body")).unwrap();
    let log = log.as_element().unwrap().attributes.borrow().get("data-log").map(str::to_string).unwrap_or_default();
    assert_eq!(log, "focus:a:- focusin:a:- blur:a:b focusout:a:b focus:b:a focusin:b:a");
    let b = dom(&e, "b");
    let ins = devtools::elements::inspect(&e, b.id().0).unwrap();
    let color = ins.computed.iter().find(|(k, _)| *k == "color").map(|(_, v)| v.clone());
    assert_eq!(color.as_deref(), Some("rgb(1, 2, 3)"), ":focus-visible after a keyboard move");
}

/// The painter rings the keyboard-focused element from the theme (2px #101010 outside the border box,
/// a white halo outside that), not after a pointer focus, and wider under prefers-contrast: more.
#[test]
fn focus_ring_paints_from_the_theme() {
    let mut e = page(r#"<body style="margin:0"><button id="b" style="margin:20px">Go</button></body>"#);
    keyboard::handle_key(&mut e, "Tab");
    e.render_frame();
    let (x, y, _w, h) = e.box_of(&dom(&e, "b")).unwrap();
    let px = |e: &AetherEngine, px: u32, py: u32| {
        let i = ((py * e.width + px) * 4) as usize;
        (e.surface[i + 2], e.surface[i + 1], e.surface[i])
    };
    let mid = (y + h / 2.0) as u32;
    assert_eq!(px(&e, x as u32 - 1, mid), (16, 16, 16), "ring band left of the border box");
    assert_eq!(px(&e, x as u32 - 2, mid), (16, 16, 16));
    assert_eq!(px(&e, x as u32 - 3, mid), (255, 255, 255), "the halo");
    // A pointer interaction: :focus-visible no longer matches, the ring goes.
    aether::css::note_focus_modality(true);
    e.damage_rects.push((0, 0, e.width, e.height));
    e.render_frame();
    assert_ne!(px(&e, x as u32 - 1, mid), (16, 16, 16), "no ring after a pointer focus");
    // More contrast: a 3px black ring.
    let more = A11yPrefs::from_prefs(|k| (k == "accessibility.contrast").then(|| "more".to_string()));
    aether::render::focusring::set_theme(more.focus_ring());
    keyboard::handle_key(&mut e, "Shift+Tab");
    e.render_frame();
    let (x, y, _w, h) = e.box_of(&dom(&e, "b")).unwrap();
    let mid = (y + h / 2.0) as u32;
    assert_eq!(px(&e, x as u32 - 3, mid), (0, 0, 0), "3px under prefers-contrast: more");
    aether::render::focusring::set_theme(Default::default());
}

/// The shell's verbs: skip to content lands on <main>; heading navigation walks headings (one level or
/// any) from the reading position, wrapping; landmark navigation likewise.
#[test]
fn skip_to_content_and_heading_navigation() {
    let mut e = page(
        r#"<body><nav><a id="n1" href="/a">A</a><a href="/b">B</a></nav>
<main id="main"><h1 id="t">Title</h1><h2 id="s1">One</h2><p>text</p><h3 id="s1a">Deep</h3><h2 id="s2">Two</h2>
<button id="inmain">act</button></main><footer id="f">f</footer></body>"#,
    );
    let main = dom(&e, "main").id().0;
    assert_eq!(nav::skip_to_content(&mut e), Some(main));
    assert_eq!(e.focused_node, None, "main is not focusable: it becomes the starting point");
    keyboard::handle_key(&mut e, "Tab");
    assert_eq!(focused_id(&e).as_deref(), Some("inmain"), "Tab continues from the skip target");
    let hs: Vec<(u32, String)> = nav::headings(&e).into_iter().map(|h| (h.level, h.name)).collect();
    assert_eq!(hs, vec![(1, "Title".into()), (2, "One".into()), (3, "Deep".into()), (2, "Two".into())]);
    focus::focus(&mut e, None, true);
    focus::set_starting_point(None);
    assert_eq!(nav::next_heading(&mut e, None, false), Some(dom(&e, "t").id().0));
    assert_eq!(nav::next_heading(&mut e, Some(2), false), Some(dom(&e, "s1").id().0));
    assert_eq!(nav::next_heading(&mut e, Some(2), false), Some(dom(&e, "s2").id().0));
    assert_eq!(nav::next_heading(&mut e, Some(2), false), Some(dom(&e, "s1").id().0), "wraps");
    assert_eq!(nav::next_heading(&mut e, None, true), Some(dom(&e, "t").id().0), "backwards");
    // From the heading inside <main>, the next landmark is the footer; back from there is <main>.
    assert_eq!(nav::next_landmark(&mut e, false), Some(dom(&e, "f").id().0));
    assert_eq!(nav::next_landmark(&mut e, true), Some(main));
}

/// The page's own skip link is used when there is no <main>.
#[test]
fn skip_link_target_without_main() {
    let mut e = page(r##"<body><a id="skip" href="#content">Skip to content</a><a href="/x">nav</a><div id="content"><button id="first">first</button></div></body>"##);
    assert_eq!(nav::skip_to_content(&mut e), Some(dom(&e, "content").id().0));
    keyboard::handle_key(&mut e, "Tab");
    assert_eq!(focused_id(&e).as_deref(), Some("first"));
}

/// Principia's accessibility rows drive prefers-reduced-motion, prefers-contrast and forced-colors.
#[test]
fn accessibility_media_features_follow_principia() {
    let css = r#"<head><style>
#m { color: rgb(0, 0, 0); } @media (prefers-reduced-motion: reduce) { #m { color: rgb(1, 1, 1); } }
#c { color: rgb(0, 0, 0); } @media (prefers-contrast: more) { #c { color: rgb(2, 2, 2); } }
#f { color: rgb(0, 0, 0); } @media (forced-colors: active) { #f { color: rgb(3, 3, 3); } }
</style></head><body><p id="m">m</p><p id="c">c</p><p id="f">f</p></body>"#;
    let mut e = page(css);
    let color = |e: &AetherEngine, id: &str| {
        let ins = devtools::elements::inspect(e, dom(e, id).id().0).unwrap();
        ins.computed.iter().find(|(k, _)| *k == "color").map(|(_, v)| v.clone()).unwrap()
    };
    assert_eq!(color(&e, "m"), "rgb(0, 0, 0)");
    let store = [("accessibility.reduce_motion", "true"), ("accessibility.contrast", "more"), ("accessibility.forced_colors", "true")];
    let p = A11yPrefs::from_prefs(|k| store.iter().find(|(key, _)| *key == k).map(|(_, v)| v.to_string()));
    assert!(p.reduce_motion && p.forced_colors);
    p.apply(Some(&mut e));
    assert_eq!(A11yPrefs::current(), p);
    assert_eq!(color(&e, "m"), "rgb(1, 1, 1)");
    assert_eq!(color(&e, "c"), "rgb(2, 2, 2)");
    assert_eq!(color(&e, "f"), "rgb(3, 3, 3)");
    A11yPrefs::default().apply(Some(&mut e));
    assert_eq!(color(&e, "m"), "rgb(0, 0, 0)", "unset again: no preference");
}

/// The devtools Accessibility panel renders the tree and the selected node's computed properties.
#[test]
fn devtools_accessibility_panel() {
    let e = page(r#"<head><title>Panel</title></head><body><main><h1>Hi</h1><label for="q">Query</label><input id="q" required></main></body>"#);
    let mut tools = devtools::DevTools::new();
    tools.panel = Panel::Accessibility;
    tools.selected = Some(dom(&e, "q").id().0);
    let lines = tools.render(&e);
    assert!(lines[0].ends_with("[Accessibility]"), "{}", lines[0]);
    assert!(lines.iter().any(|l| l == "RootWebArea \"Panel\" focusable focused"), "{lines:#?}");
    assert!(lines.iter().any(|l| l.trim_start() == "heading \"Hi\""), "{lines:#?}");
    assert!(lines.iter().any(|l| l == "name: \"Query\" (from label)"), "{lines:#?}");
    assert!(lines.iter().any(|l| l == "tab order: 1"), "{lines:#?}");
    let ins = devtools::accessibility::inspect(&e, dom(&e, "q").id().0).unwrap();
    assert_eq!((ins.role, ins.tab_stop), ("textbox", Some(1)));
    assert!(ins.states.contains(&"required".to_string()));
}
