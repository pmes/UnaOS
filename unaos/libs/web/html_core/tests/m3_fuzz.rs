//! M3 — mutation fuzz: 1000 mutants of the html5lib tree-construction inputs (plus the EYES pages, which are
//! always available) through the document parser, the fragment parser in a rotating context, the serializer,
//! and a re-parse of the serialization. Gate: no panic, and every arena keeps consistent parent/child/sibling
//! links. Deterministic (xorshift64*, fixed seed) so a failure reproduces.

mod common;

use common::{crate_dir, fetch, parse_dat, vectors};
use html_core::serialize::{outer_html, SerializeOpts};
use html_core::{parse_document, parse_fragment, Document, Namespace, NodeId, ParseOpts, QuirksMode};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

const SNIPPETS: &[&str] = &[
    "<", "</", ">", "/>", "<!--", "-->", "<!", "<![CDATA[", "]]>", "<?x ", "&", "&amp", "&#x", "&#1114112;", "&notin",
    "\0", "\r\n", "\"", "'", "=", "<table>", "</table>", "<tr>", "<td>", "<caption>", "<colgroup><col>", "<tbody>",
    "<svg>", "</svg>", "<math>", "<mi>", "<annotation-xml encoding=text/html>", "<foreignObject>", "<desc>",
    "<template>", "</template>", "<select>", "<option>", "<optgroup>", "<selectedcontent>", "<button>", "</p>",
    "<p>", "<a>", "</a>", "<b>", "</b>", "<i>", "<nobr>", "<frameset>", "<frame>", "<body>", "</body>", "<html>",
    "</html>", "<head>", "<noscript>", "<script>", "</script>", "<!--<script>", "<style>", "<textarea>", "<pre>\n",
    "<plaintext>", "<xmp>", "<iframe>", "<form>", "</form>", "<li>", "<dd>", "<h1>", "</h2>", "<ruby><rt>",
    "<image>", "<input type=hidden>", "<hr>", "<br/>", "</br>", "<!DOCTYPE html>", "<!doctype x PUBLIC \"a\">",
    "<font color=red>", "<listing>", "<applet>", "<marquee>", "<object>", "<math><mtext><svg>", "\u{FFFD}", "日本",
];

/// Walk every node reachable from the document and check the links are mutually consistent.
fn check_links(doc: &Document) {
    for i in 0..doc.nodes.len() {
        let id = NodeId(i);
        let n = doc.node(id);
        if let Some(fc) = n.first_child {
            assert_eq!(doc.node(fc).parent, Some(id));
            assert_eq!(doc.node(fc).prev_sibling, None);
        }
        if let Some(lc) = n.last_child {
            assert_eq!(doc.node(lc).parent, Some(id));
            assert_eq!(doc.node(lc).next_sibling, None);
        }
        if let Some(ns) = n.next_sibling {
            assert_eq!(doc.node(ns).prev_sibling, Some(id));
            assert_eq!(doc.node(ns).parent, n.parent);
        }
        assert_eq!(n.first_child.is_none(), n.last_child.is_none());
    }
}

fn mutate(rng: &mut Rng, seed: &str) -> String {
    let mut chars: Vec<char> = seed.chars().collect();
    let ops = 1 + rng.below(6);
    for _ in 0..ops {
        let len = chars.len();
        match rng.below(5) {
            0 if len > 0 => {
                // delete a range
                let a = rng.below(len);
                let b = (a + 1 + rng.below(16)).min(len);
                chars.drain(a..b);
            }
            1 if len > 0 => {
                // duplicate a range
                let a = rng.below(len);
                let b = (a + 1 + rng.below(64)).min(len);
                let dup: Vec<char> = chars[a..b].to_vec();
                let at = rng.below(chars.len() + 1);
                chars.splice(at..at, dup);
            }
            2 if len > 1 => {
                // swap two characters
                let (a, b) = (rng.below(len), rng.below(len));
                chars.swap(a, b);
            }
            3 => {
                // random code point
                let c = char::from_u32(rng.below(0x3000) as u32).unwrap_or('x');
                let at = rng.below(len + 1);
                chars.insert(at, c);
            }
            _ => {
                let s = SNIPPETS[rng.below(SNIPPETS.len())];
                let at = rng.below(len + 1);
                chars.splice(at..at, s.chars());
            }
        }
    }
    chars.into_iter().collect()
}

#[test]
fn fuzz_1000_mutants_no_panic() {
    let mut seeds: Vec<String> = Vec::new();
    for (url, sha) in vectors("tree") {
        match fetch(&url, &sha) {
            Some(t) => seeds.extend(parse_dat(&t).into_iter().map(|d| d.data)),
            None => break,
        }
    }
    let html5lib_seeds = seeds.len();
    let eyes = crate_dir().join("tests/data/eyes");
    for e in std::fs::read_dir(&eyes).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "html") && !p.to_string_lossy().ends_with(".chromium.html") {
            seeds.push(std::fs::read_to_string(p).unwrap());
        }
    }
    let contexts: &[(Namespace, &str)] = &[
        (Namespace::Html, "body"),
        (Namespace::Html, "td"),
        (Namespace::Html, "select"),
        (Namespace::Html, "template"),
        (Namespace::Html, "textarea"),
        (Namespace::Svg, "svg"),
        (Namespace::MathMl, "mi"),
        (Namespace::Html, "html"),
    ];
    let mut rng = Rng(0x48544D4C434F5245); // "HTMLCORE"
    let mut panics = Vec::new();
    for i in 0..1000 {
        let seed = &seeds[rng.below(seeds.len())];
        let input = mutate(&mut rng, seed);
        let ctx = contexts[i % contexts.len()];
        let pi = rng.below(4) == 0;
        let r = std::panic::catch_unwind(|| {
            let opts = ParseOpts { processing_instructions: pi, ..ParseOpts::default() };
            let doc = parse_document(&input, opts);
            check_links(&doc);
            let root = doc.document_element().expect("document element");
            let s = outer_html(&doc, root, SerializeOpts::default());
            let again = parse_document(&s, opts);
            check_links(&again);
            let (fdoc, frag) = parse_fragment(ctx.0, ctx.1, Vec::new(), &input, QuirksMode::NoQuirks, opts);
            check_links(&fdoc);
            let _ = outer_html(&fdoc, frag, SerializeOpts::default());
        });
        if r.is_err() {
            panics.push(format!("mutant #{i} ctx={ctx:?} input={input:?}"));
        }
    }
    println!(
        "fuzz: 1000 mutants of {} seeds ({html5lib_seeds} html5lib + {} EYES), panics: {}",
        seeds.len(),
        seeds.len() - html5lib_seeds,
        panics.len()
    );
    for p in &panics {
        println!("PANIC {p}");
    }
    assert!(panics.is_empty());
}
