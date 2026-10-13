//! AETHERWPT3 (LEDGER SR138): constructable stylesheets and `adoptedStyleSheets` (CSSOM §6.1 — `StyleSheet`,
//! `CSSStyleSheet` with its constructor, `replace()` / `replaceSync()`, `insertRule()` / `deleteRule()` /
//! `addRule()` / `removeRule()`, `cssRules`; §6.1.2 `DocumentOrShadowRoot.adoptedStyleSheets` as an
//! ObservableArray; §6.4 the CSS rules — `CSSRule`, `CSSStyleRule` (with its declaration block), the grouping
//! rules `CSSMediaRule` / `CSSSupportsRule` / `CSSLayerBlockRule` / `CSSContainerRule`, `CSSLayerStatementRule`,
//! `CSSFontFaceRule`, `CSSKeyframesRule`, `CSSNamespaceRule` and `CSSImportRule` — serialized per §6.4.4).
//!
//! The rules are parsed by css_core (`__css_parse`: CSS Syntax §5.3.3 "parse a list of rules" / §5.3.5 "parse a
//! rule", a style rule kept only when its prelude is a valid selector list); the sheet's serialized rules are
//! what the cascade reads: the document's adopted sheets go to `crate::css::set_adopted` (`__css_adopt`) after
//! every change that can alter them, and the DOM version moves so layout follows.

use super::idl::*;
use css_core::parser::{parse_block_contents, parse_component_values, parse_one_rule, parse_rule_list_cvs, trim_ws, BlockItem};
use css_core::selectors::{parse_selector_list, Namespaces, ParseContext};
use css_core::serialize::to_css;
use css_core::{Declaration, Rule, CV};
use js_core::vm::*;

fn json_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Component values as CSS text with whitespace runs collapsed and the ends trimmed.
fn text(v: &[CV]) -> String {
    let raw = to_css(trim_ws(v));
    let mut out = String::with_capacity(raw.len());
    let mut ws = false;
    for c in raw.chars() {
        if c.is_ascii_whitespace() {
            ws = true;
            continue;
        }
        if ws && !out.is_empty() {
            out.push(' ');
        }
        ws = false;
        out.push(c);
    }
    out
}

fn decls_json(decls: &[Declaration], out: &mut String) {
    out.push('[');
    for (i, d) in decls.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let name = if d.name.starts_with("--") { d.name.clone() } else { d.name.to_ascii_lowercase() };
        let value = if name.starts_with("--") {
            to_css(trim_ws(&d.value))
        } else {
            super::cssom::normalize_value(&name, &d.value).unwrap_or_else(|| text(&d.value))
        };
        out.push('[');
        json_str(&name, out);
        out.push(',');
        json_str(&value, out);
        out.push_str(if d.important { ",true]" } else { ",false]" });
    }
    out.push(']');
}

fn block_decls(block: &[CV]) -> Vec<Declaration> {
    parse_block_contents(block)
        .into_iter()
        .filter_map(|r| match r {
            Ok(BlockItem::Declaration(d)) => Some(d),
            _ => None,
        })
        .collect()
}

fn rules_json(rules: Vec<Rule>, out: &mut String) {
    out.push('[');
    let mut first = true;
    for r in rules {
        let mut one = String::new();
        if describe(r, &mut one) {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&one);
        }
    }
    out.push(']');
}

fn child_rules(block: &[CV]) -> Vec<Rule> {
    parse_rule_list_cvs(block, false).into_iter().filter_map(|r| r.ok()).collect()
}

/// One rule as a descriptor object for the prelude; false when the rule is invalid (dropped).
fn describe(r: Rule, out: &mut String) -> bool {
    match r {
        Rule::Qualified(q) => {
            let ns = Namespaces::default();
            if parse_selector_list(&q.prelude, &ParseContext::new(&ns)).is_err() {
                return false;
            }
            out.push_str("{\"t\":\"style\",\"sel\":");
            json_str(&text(&q.prelude), out);
            out.push_str(",\"decls\":");
            decls_json(&block_decls(&q.block), out);
            out.push('}');
            true
        }
        Rule::At(a) => {
            let name = a.name.to_ascii_lowercase();
            let prelude = text(&a.prelude);
            match (name.as_str(), &a.block) {
                ("media" | "supports" | "container", Some(b)) => {
                    out.push_str(&format!("{{\"t\":\"{name}\",\"cond\":"));
                    json_str(&prelude, out);
                    out.push_str(",\"rules\":");
                    rules_json(child_rules(b), out);
                    out.push('}');
                }
                ("layer", Some(b)) => {
                    out.push_str("{\"t\":\"layer-block\",\"name\":");
                    json_str(&prelude, out);
                    out.push_str(",\"rules\":");
                    rules_json(child_rules(b), out);
                    out.push('}');
                }
                ("layer", None) => {
                    if prelude.is_empty() {
                        return false;
                    }
                    out.push_str("{\"t\":\"layer-statement\",\"names\":");
                    let names: Vec<String> = prelude.split(',').map(|n| n.trim().to_string()).collect();
                    out.push('[');
                    for (i, n) in names.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        json_str(n, out);
                    }
                    out.push_str("]}");
                }
                ("font-face", Some(b)) => {
                    out.push_str("{\"t\":\"font-face\",\"decls\":");
                    decls_json(&block_decls(b), out);
                    out.push('}');
                }
                ("keyframes", Some(b)) => {
                    out.push_str("{\"t\":\"keyframes\",\"name\":");
                    json_str(&prelude, out);
                    out.push_str(",\"text\":");
                    json_str(&format!("@keyframes {prelude} {{ {} }}", text(b)), out);
                    out.push('}');
                }
                ("import", None) => {
                    out.push_str("{\"t\":\"import\",\"text\":");
                    json_str(&format!("@import {prelude};"), out);
                    out.push('}');
                }
                ("namespace", None) => {
                    out.push_str("{\"t\":\"namespace\",\"text\":");
                    json_str(&format!("@namespace {prelude};"), out);
                    out.push('}');
                }
                ("charset", _) => return false,
                (_, Some(b)) => {
                    out.push_str("{\"t\":\"other\",\"text\":");
                    let p = if prelude.is_empty() { String::new() } else { format!(" {prelude}") };
                    json_str(&format!("@{name}{p} {{ {} }}", text(b)), out);
                    out.push('}');
                }
                (_, None) => {
                    out.push_str("{\"t\":\"other\",\"text\":");
                    let p = if prelude.is_empty() { String::new() } else { format!(" {prelude}") };
                    json_str(&format!("@{name}{p};"), out);
                    out.push('}');
                }
            }
            true
        }
    }
}

/// `__css_parse(text, "sheet" | "rule" | "decls")`: a JSON list of rule descriptors (CSS Syntax "parse a list of
/// rules"), one descriptor or null ("parse a rule": a syntax error), or a declaration list.
fn css_parse(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let input = string(vm, &arg(vm, ctx, 0))?;
    let mode = string(vm, &arg(vm, ctx, 1))?;
    let mut out = String::new();
    match mode.as_str() {
        "rule" => match parse_one_rule(&input) {
            Ok(r) => {
                if !describe(r, &mut out) {
                    return Ok(Value::Null);
                }
            }
            Err(_) => return Ok(Value::Null),
        },
        "decls" => decls_json(&block_decls(&parse_component_values(&input)), &mut out),
        _ => {
            let cvs = parse_component_values(&input);
            rules_json(parse_rule_list_cvs(&cvs, true).into_iter().filter_map(|r| r.ok()).collect(), &mut out);
        }
    }
    Ok(s(&out))
}

/// `__css_adopt(texts)`: the document's adopted sheets, serialized, into the cascade.
fn css_adopt(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let list = arg(vm, ctx, 0);
    let mut texts = Vec::new();
    if let Value::Object(o) = list {
        let n = vm.get(o, &PropertyKey::from_str("length"))?;
        let n = vm.to_number(&n)? as usize;
        for i in 0..n {
            let v = vm.get(o, &PropertyKey::Index(i as u32))?;
            texts.push(string(vm, &v)?);
        }
    }
    let doc = super::page(|p| p.doc.clone());
    crate::css::set_adopted(doc, texts);
    super::touch();
    Ok(Value::Undefined)
}

pub fn install(vm: &mut Vm) {
    crate::css::set_adopted(None, Vec::new());
    let g = vm.realm().global;
    for (name, len, f) in [("__css_parse", 2u32, css_parse as NativeFn), ("__css_adopt", 1, css_adopt as NativeFn)] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("stylesheets-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] stylesheets prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    'use strict';
    var G = globalThis;
    var parse = G.__css_parse, adopt = G.__css_adopt;
    delete G.__css_parse;
    delete G.__css_adopt;
    var TOKEN = {};
    var iface = function (n, c) { Object.defineProperty(G, n, { value: c, writable: true, enumerable: false, configurable: true }); };
    var str = function (v) {
        if (typeof v === 'symbol') { throw new TypeError('Cannot convert a Symbol value to a string'); }
        return String(v);
    };
    var dom = function (name, msg) { return new DOMException(msg, name); };
    var need = function (n, min, i, op) {
        if (n < min) { throw new TypeError("Failed to execute '" + op + "' on '" + i + "': " + min + ' argument' + (min > 1 ? 's' : '') + ' required, but only ' + n + ' present.'); }
    };
    var ulong = function (v) { var n = Number(v); if (!isFinite(n)) { return 0; } n = Math.trunc(n) % 4294967296; return n < 0 ? n + 4294967296 : n; };

    // ---- MediaList (CSSOM §4.1) ---------------------------------------------------------------------
    var ML = new WeakMap();
    var splitMedia = function (t) { return String(t).split(',').map(function (x) { return x.trim().replace(/\s+/g, ' ').toLowerCase(); }).filter(function (x) { return x !== ''; }); };
    class MediaList {
        constructor(tok, owner) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } ML.set(this, { list: [], owner: owner }); }
        get mediaText() { return ML.get(this).list.join(', '); }
        set mediaText(v) { var s = ML.get(this); s.list = v === null ? [] : splitMedia(v); changed(s.owner); }
        get length() { return ML.get(this).list.length; }
        item(i) { var l = ML.get(this).list; i = ulong(i); return i < l.length ? l[i] : null; }
        appendMedium(m) { var s = ML.get(this); m = splitMedia(m)[0]; if (m && s.list.indexOf(m) < 0) { s.list.push(m); changed(s.owner); } }
        deleteMedium(m) {
            var s = ML.get(this); m = splitMedia(m)[0] || '';
            var i = s.list.indexOf(m);
            if (i < 0) { throw dom('NotFoundError', "Failed to execute 'deleteMedium' on 'MediaList': Failed to delete '" + m + "'."); }
            s.list.splice(i, 1); changed(s.owner);
        }
        toString() { return this.mediaText; }
    }

    // ---- the rule objects (CSSOM §6.4) --------------------------------------------------------------
    var RS = new WeakMap();
    var rs = function (o) { var s = RS.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    class CSSRule {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } }
        get type() { return rs(this).type; }
        get cssText() { return serialize(this); }
        set cssText(v) {}
        get parentRule() { return rs(this).parentRule; }
        get parentStyleSheet() { return rs(this).sheet; }
    }
    [['STYLE_RULE', 1], ['CHARSET_RULE', 2], ['IMPORT_RULE', 3], ['MEDIA_RULE', 4], ['FONT_FACE_RULE', 5], ['PAGE_RULE', 6],
     ['KEYFRAMES_RULE', 7], ['KEYFRAME_RULE', 8], ['MARGIN_RULE', 9], ['NAMESPACE_RULE', 10], ['COUNTER_STYLE_RULE', 11],
     ['SUPPORTS_RULE', 12], ['FONT_FEATURE_VALUES_RULE', 14]].forEach(function (p) {
        Object.defineProperty(CSSRule, p[0], { value: p[1], enumerable: true });
        Object.defineProperty(CSSRule.prototype, p[0], { value: p[1], enumerable: true });
    });

    // A rule's declaration block (CSSOM §6.6 CSSStyleDeclaration, for a rule: no owner node).
    var DS = new WeakMap();
    var declText = function (decls) {
        return decls.map(function (d) { return d[0] + ': ' + d[1] + (d[2] ? ' !important' : '') + ';'; }).join(' ');
    };
    class CSSRuleDeclarations {
        constructor(tok, rule) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } DS.set(this, rule); }
        get cssText() { return declText(rs(DS.get(this)).decls); }
        set cssText(v) { var r = rs(DS.get(this)); r.decls = JSON.parse(parse(str(v), 'decls')); changed(r.sheet); }
        get length() { return rs(DS.get(this)).decls.length; }
        item(i) { var d = rs(DS.get(this)).decls; i = ulong(i); return i < d.length ? d[i][0] : ''; }
        getPropertyValue(p) { p = str(p); var n = p.indexOf('--') === 0 ? p : p.toLowerCase(); var d = rs(DS.get(this)).decls.filter(function (x) { return x[0] === n; })[0]; return d ? d[1] : ''; }
        getPropertyPriority(p) { p = str(p).toLowerCase(); var d = rs(DS.get(this)).decls.filter(function (x) { return x[0] === p; })[0]; return d && d[2] ? 'important' : ''; }
        setProperty(p, v, prio) {
            var r = rs(DS.get(this));
            p = str(p); v = v === null || v === undefined ? '' : str(v);
            var name = p.indexOf('--') === 0 ? p : p.toLowerCase();
            if (v === '') { this.removeProperty(name); return; }
            var pr = prio === undefined || prio === null ? '' : str(prio).toLowerCase();
            if (pr !== '' && pr !== 'important') { return; }
            var parsed = JSON.parse(parse(name + ':' + v + (pr ? ' !important' : ''), 'decls'));
            if (parsed.length !== 1) { return; }
            var i = r.decls.findIndex(function (x) { return x[0] === name; });
            if (i >= 0) { r.decls[i] = parsed[0]; } else { r.decls.push(parsed[0]); }
            changed(r.sheet);
        }
        removeProperty(p) {
            var r = rs(DS.get(this));
            p = str(p); var name = p.indexOf('--') === 0 ? p : p.toLowerCase();
            var i = r.decls.findIndex(function (x) { return x[0] === name; });
            if (i < 0) { return ''; }
            var old = r.decls[i][1];
            r.decls.splice(i, 1);
            changed(r.sheet);
            return old;
        }
        get parentRule() { return DS.get(this); }
    }
    class CSSStyleRule extends CSSRule {
        get selectorText() { return rs(this).sel; }
        set selectorText(v) {
            var r = rs(this);
            var d = parse(str(v) + ' {}', 'rule');
            var o = d === null ? null : JSON.parse(d);
            if (o && o.t === 'style') { r.sel = o.sel; changed(r.sheet); }
        }
        get style() { var r = rs(this); if (!r.styleObj) { r.styleObj = new CSSRuleDeclarations(TOKEN, this); } return r.styleObj; }
        set style(v) { this.style.cssText = v; }
    }
    class CSSGroupingRule extends CSSRule {
        get cssRules() { return ruleList(rs(this)); }
        insertRule(rule, index) {
            need(arguments.length, 1, 'CSSGroupingRule', 'insertRule');
            var r = rs(this);
            return insertInto(r, str(rule), index === undefined ? 0 : ulong(index), true);
        }
        deleteRule(index) {
            need(arguments.length, 1, 'CSSGroupingRule', 'deleteRule');
            deleteFrom(rs(this), ulong(index));
        }
    }
    class CSSConditionRule extends CSSGroupingRule {
        get conditionText() { return rs(this).cond; }
    }
    class CSSMediaRule extends CSSConditionRule {
        get media() { var r = rs(this); if (!r.mediaObj) { r.mediaObj = new MediaList(TOKEN, r.sheet); ML.get(r.mediaObj).list = splitMedia(r.cond); } return r.mediaObj; }
        get matches() { try { return G.matchMedia(rs(this).cond).matches; } catch (e) { return false; } }
    }
    class CSSSupportsRule extends CSSConditionRule {}
    class CSSContainerRule extends CSSConditionRule {}
    class CSSLayerBlockRule extends CSSGroupingRule { get name() { return rs(this).name; } }
    class CSSLayerStatementRule extends CSSRule { get nameList() { return Object.freeze(rs(this).names.slice()); } }
    class CSSFontFaceRule extends CSSRule {
        get style() { var r = rs(this); if (!r.styleObj) { r.styleObj = new CSSRuleDeclarations(TOKEN, this); } return r.styleObj; }
    }
    class CSSKeyframesRule extends CSSRule { get name() { return rs(this).name; } }
    class CSSNamespaceRule extends CSSRule {}
    class CSSImportRule extends CSSRule {}

    var KIND = {
        'style': [CSSStyleRule, 1], 'media': [CSSMediaRule, 4], 'supports': [CSSSupportsRule, 12], 'container': [CSSContainerRule, 0],
        'layer-block': [CSSLayerBlockRule, 0], 'layer-statement': [CSSLayerStatementRule, 0], 'font-face': [CSSFontFaceRule, 5],
        'keyframes': [CSSKeyframesRule, 7], 'namespace': [CSSNamespaceRule, 10], 'import': [CSSImportRule, 3], 'other': [CSSRule, 0],
    };
    // A rule object from a descriptor (`__css_parse`).
    var make = function (d, sheet, parentRule) {
        var k = KIND[d.t] || KIND.other;
        var o = Object.create(k[0].prototype);
        var rec = { type: k[1], kind: d.t, sheet: sheet, parentRule: parentRule, sel: d.sel, decls: d.decls || [], cond: d.cond, name: d.name, names: d.names, text: d.text, rules: [] };
        rec.self = o;
        RS.set(o, rec);
        if (d.rules) { rec.rules = d.rules.map(function (c) { return make(c, sheet, o); }); }
        return o;
    };
    // CSSOM §6.4.4 "serialize a CSS rule".
    var block = function (rules) {
        var s = ' {';
        rules.forEach(function (r) { s += '\n  ' + serialize(r).split('\n').join('\n  '); });
        return s + '\n}';
    };
    var serialize = function (rule) {
        var r = rs(rule);
        switch (r.kind) {
            case 'style': { var dt = declText(r.decls); return r.sel + ' {' + (dt ? ' ' + dt : '') + ' }'; }
            case 'font-face': { var ft = declText(r.decls); return '@font-face {' + (ft ? ' ' + ft : '') + ' }'; }
            case 'media': return '@media ' + r.cond + block(r.rules);
            case 'supports': return '@supports ' + r.cond + block(r.rules);
            case 'container': return '@container ' + r.cond + block(r.rules);
            case 'layer-block': return '@layer' + (r.name ? ' ' + r.name : '') + block(r.rules);
            case 'layer-statement': return '@layer ' + r.names.join(', ') + ';';
            default: return r.text;
        }
    };
    // CSSRuleList (CSSOM §6.3): a live view of a rule list.
    var LS = new WeakMap();
    class CSSRuleList {
        constructor(tok, owner) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } LS.set(this, owner); }
        get length() { return LS.get(this).rules.length; }
        item(i) { var l = LS.get(this).rules; i = ulong(i); return i < l.length ? l[i] : null; }
    }
    var listProxy = function (owner) {
        var l = new CSSRuleList(TOKEN, owner);
        return new Proxy(l, {
            get: function (t, k, recv) {
                if (typeof k === 'string' && /^(0|[1-9][0-9]*)$/.test(k)) { return owner.rules[+k]; }
                var v = Reflect.get(t, k, t);
                return typeof v === 'function' ? v.bind(t) : v;
            },
            has: function (t, k) { return (typeof k === 'string' && /^(0|[1-9][0-9]*)$/.test(k) && +k < owner.rules.length) || Reflect.has(t, k); },
            getPrototypeOf: function () { return CSSRuleList.prototype; },
        });
    };
    var ruleList = function (owner) { if (!owner.listObj) { owner.listObj = listProxy(owner); } return owner.listObj; };
    // CSSOM §6.4.2 "insert a CSS rule" into `owner` (a sheet or grouping rule record).
    var insertInto = function (owner, text, index, nested) {
        var d = parse(text, 'rule');
        if (d === null) { throw dom('SyntaxError', "Failed to execute 'insertRule': Failed to parse the rule '" + text + "'."); }
        if (index > owner.rules.length) { throw dom('IndexSizeError', "Failed to execute 'insertRule': The index provided (" + index + ") is larger than the maximum index (" + owner.rules.length + ")."); }
        d = JSON.parse(d);
        if (d.t === 'import' && (nested || owner.constructed)) { throw dom('SyntaxError', "Failed to execute 'insertRule': Can't insert @import rules into a constructed stylesheet."); }
        if (nested && d.t === 'namespace') { throw dom('HierarchyRequestError', "Failed to execute 'insertRule': @namespace rules cannot be nested."); }
        var sheet = owner.sheetObj || owner.sheet;
        var rule = make(d, sheet, nested ? owner.self : null);
        owner.rules.splice(index, 0, rule);
        changed(sheet);
        return index;
    };
    var deleteFrom = function (owner, index) {
        if (index >= owner.rules.length) { throw dom('IndexSizeError', "Failed to execute 'deleteRule': The index provided (" + index + ") is outside the range [0, " + owner.rules.length + ")."); }
        var r = owner.rules.splice(index, 1)[0];
        var s = RS.get(r);
        if (s) { s.sheet = null; s.parentRule = null; }
        changed(owner.sheetObj || owner.sheet);
    };

    // ---- StyleSheet / CSSStyleSheet (CSSOM §6.1) ----------------------------------------------------
    var SS = new WeakMap();
    var ss = function (o) { var s = SS.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    class StyleSheet {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } }
        get type() { ss(this); return 'text/css'; }
        get href() { ss(this); return null; }
        get ownerNode() { ss(this); return null; }
        get parentStyleSheet() { ss(this); return null; }
        get title() { ss(this); return null; }
        get media() { return ss(this).media; }
        set media(v) { this.media.mediaText = v; }
        get disabled() { return ss(this).disabled; }
        set disabled(v) { var s = ss(this); s.disabled = !!v; changed(this); }
    }
    class CSSStyleSheet extends StyleSheet {
        // CSSOM §6.1.2 "create a constructed CSSStyleSheet".
        constructor(options) {
            super(TOKEN);
            var o = options === undefined || options === null ? {} : options;
            if (typeof o !== 'object' && typeof o !== 'function') { throw new TypeError("Failed to construct 'CSSStyleSheet': The provided value is not of type 'CSSStyleSheetInit'."); }
            var baseURL = o.baseURL, disabled = o.disabled, media = o.media;
            var base = String(G.document.baseURI);
            if (baseURL !== undefined) {
                try { base = new URL(str(baseURL), G.document.baseURI).href; } catch (e) { throw dom('NotAllowedError', "Failed to construct 'CSSStyleSheet': Invalid baseURL."); }
            }
            var rec = { rules: [], constructed: true, disallow: false, disabled: !!disabled, base: base, document: G.document, adopters: [], sheetObj: this };
            rec.media = new MediaList(TOKEN, this);
            if (media !== undefined) {
                ML.get(rec.media).list = (typeof media === 'object' && media !== null && ML.has(media)) ? ML.get(media).list.slice() : splitMedia(str(media));
            }
            SS.set(this, rec);
        }
        get ownerRule() { ss(this); return null; }
        get cssRules() { return ruleList(ss(this)); }
        get rules() { return this.cssRules; }
        insertRule(rule, index) {
            need(arguments.length, 1, 'CSSStyleSheet', 'insertRule');
            var s = ss(this);
            rule = str(rule);
            if (s.disallow) { throw dom('NotAllowedError', "Failed to execute 'insertRule' on 'CSSStyleSheet': The sheet is being modified by replace()."); }
            return insertInto(s, rule, index === undefined ? 0 : ulong(index), false);
        }
        deleteRule(index) {
            need(arguments.length, 1, 'CSSStyleSheet', 'deleteRule');
            var s = ss(this);
            if (s.disallow) { throw dom('NotAllowedError', "Failed to execute 'deleteRule' on 'CSSStyleSheet': The sheet is being modified by replace()."); }
            deleteFrom(s, ulong(index));
        }
        addRule(selector, style, index) {
            var s = ss(this);
            var text = (selector === undefined ? 'undefined' : str(selector)) + ' { ' + (style === undefined ? 'undefined' : str(style)) + ' }';
            var i = index === undefined ? s.rules.length : ulong(index);
            this.insertRule(text, i);
            return -1;
        }
        removeRule(index) { this.deleteRule(index === undefined ? 0 : index); }
        // §6.1.2 replace(): the rules swap in a task; @import rules are dropped.
        replace(text) {
            var self = this, s;
            try { s = ss(this); } catch (e) { return Promise.reject(e); }
            text = str(text);
            if (!s.constructed) { return Promise.reject(dom('NotAllowedError', "Failed to execute 'replace' on 'CSSStyleSheet': Can't call replace on non-constructed CSSStyleSheets.")); }
            if (s.disallow) { return Promise.reject(dom('NotAllowedError', "Failed to execute 'replace' on 'CSSStyleSheet': Can't call replace on a sheet already being replaced.")); }
            s.disallow = true;
            return new Promise(function (resolve) {
                setTimeout(function () {
                    setRules(s, text);
                    s.disallow = false;
                    changed(self);
                    resolve(self);
                }, 0);
            });
        }
        replaceSync(text) {
            need(arguments.length, 1, 'CSSStyleSheet', 'replaceSync');
            var s = ss(this);
            text = str(text);
            if (!s.constructed) { throw dom('NotAllowedError', "Failed to execute 'replaceSync' on 'CSSStyleSheet': Can't call replaceSync on non-constructed CSSStyleSheets."); }
            if (s.disallow) { throw dom('NotAllowedError', "Failed to execute 'replaceSync' on 'CSSStyleSheet': Can't call replaceSync on a sheet being replaced."); }
            setRules(s, text);
            changed(this);
        }
    }
    var setRules = function (s, text) {
        var list = JSON.parse(parse(text, 'sheet')).filter(function (d) { return d.t !== 'import'; });
        s.rules.forEach(function (r) { var x = RS.get(r); if (x) { x.sheet = null; } });
        s.rules = list.map(function (d) { return make(d, s.sheetObj, null); });
    };
    // The sheet's text as the cascade reads it: its rules, inside its media when it has one.
    var sheetText = function (sheet) {
        var s = SS.get(sheet);
        if (!s || s.disabled) { return ''; }
        var body = s.rules.map(serialize).join('\n');
        var m = ML.get(s.media).list;
        return m.length ? '@media ' + m.join(', ') + ' {\n' + body + '\n}' : body;
    };

    // ---- DocumentOrShadowRoot.adoptedStyleSheets (CSSOM §6.1.2, an ObservableArray) ---------------------
    var AD = new WeakMap();
    var docOf = function (owner) { return owner.nodeType === 9 ? owner : owner.ownerDocument; };
    var sync = function () {
        var list = AD.get(G.document);
        adopt(list ? list.items.map(sheetText) : []);
    };
    var changed = function (sheet) {
        var s = sheet && SS.get(sheet);
        if (s && s.adopters.indexOf(G.document) >= 0) { sync(); }
    };
    var check = function (owner, v) {
        if (!(v instanceof CSSStyleSheet) || !SS.has(v)) { throw new TypeError("Failed to set the 'adoptedStyleSheets' property: Failed to convert value to 'CSSStyleSheet'."); }
        var s = SS.get(v);
        if (!s.constructed || s.document !== docOf(owner)) {
            throw dom('NotAllowedError', "Failed to set the 'adoptedStyleSheets' property: Sharing constructed stylesheets in multiple documents is not allowed");
        }
    };
    var recompute = function (owner, list) {
        // Every sheet knows the owners that adopt it (for `changed`).
        list.all.forEach(function (sh) { var s = SS.get(sh); var i = s.adopters.indexOf(owner); if (i >= 0) { s.adopters.splice(i, 1); } });
        list.items.forEach(function (sh) { var s = SS.get(sh); if (s.adopters.indexOf(owner) < 0) { s.adopters.push(owner); } });
        list.all = list.items.slice();
        if (owner === G.document) { sync(); }
    };
    var observable = function (owner) {
        var list = AD.get(owner);
        if (list) { return list; }
        list = { items: [], all: [] };
        var arr = [];
        list.proxy = new Proxy(arr, {
            get: function (t, k) {
                if (k === 'length') { return list.items.length; }
                if (typeof k === 'string' && /^(0|[1-9][0-9]*)$/.test(k)) { return list.items[+k]; }
                return Reflect.get(t, k);
            },
            set: function (t, k, v) {
                if (k === 'length') {
                    var n = Number(v);
                    if (n !== n >>> 0) { throw new RangeError('Invalid array length'); }
                    if (n > list.items.length) { return false; }
                    list.items.length = n;
                    recompute(owner, list);
                    return true;
                }
                if (typeof k === 'string' && /^(0|[1-9][0-9]*)$/.test(k)) {
                    var i = +k;
                    if (i > list.items.length) { return false; }
                    check(owner, v);
                    list.items[i] = v;
                    recompute(owner, list);
                    return true;
                }
                return Reflect.set(t, k, v);
            },
            deleteProperty: function (t, k) {
                if (typeof k === 'string' && /^(0|[1-9][0-9]*)$/.test(k)) {
                    if (+k !== list.items.length - 1) { return false; }
                    list.items.pop();
                    recompute(owner, list);
                    return true;
                }
                return Reflect.deleteProperty(t, k);
            },
            has: function (t, k) {
                if (typeof k === 'string' && /^(0|[1-9][0-9]*)$/.test(k)) { return +k < list.items.length; }
                return Reflect.has(t, k);
            },
            ownKeys: function () { return list.items.map(function (_, i) { return String(i); }).concat(['length']); },
            getOwnPropertyDescriptor: function (t, k) {
                if (k === 'length') { return { value: list.items.length, writable: true, enumerable: false, configurable: false }; }
                if (typeof k === 'string' && /^(0|[1-9][0-9]*)$/.test(k) && +k < list.items.length) { return { value: list.items[+k], writable: true, enumerable: true, configurable: true }; }
                return undefined;
            },
        });
        AD.set(owner, list);
        return list;
    };
    var adoptedAccessor = function (proto) {
        if (!proto) { return; }
        Object.defineProperty(proto, 'adoptedStyleSheets', {
            get: function () { return observable(this).proxy; },
            set: function (v) {
                if (v === null || (typeof v !== 'object' && typeof v !== 'function') || typeof v[Symbol.iterator] !== 'function') {
                    throw new TypeError("Failed to set the 'adoptedStyleSheets' property: The provided value cannot be converted to a sequence.");
                }
                var owner = this, items = [];
                for (var x of v) { check(owner, x); items.push(x); }
                var list = observable(owner);
                list.items = items;
                recompute(owner, list);
            },
            enumerable: true, configurable: true,
        });
    };
    adoptedAccessor(G.Document && G.Document.prototype);
    adoptedAccessor(G.ShadowRoot && G.ShadowRoot.prototype);

    [StyleSheet, CSSStyleSheet, MediaList, CSSRule, CSSRuleList, CSSStyleRule, CSSGroupingRule, CSSConditionRule, CSSMediaRule,
     CSSSupportsRule, CSSContainerRule, CSSLayerBlockRule, CSSLayerStatementRule, CSSFontFaceRule, CSSKeyframesRule, CSSNamespaceRule,
     CSSImportRule].forEach(function (c) {
        Object.defineProperty(c.prototype, Symbol.toStringTag, { value: c.name, configurable: true });
        iface(c.name, c);
    });
})();
"#;
