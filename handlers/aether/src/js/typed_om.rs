//! AETHERAPIS (LEDGER SR145): CSS Typed OM Level 1 — §3 `StylePropertyMapReadOnly` / `StylePropertyMap`
//! (`Element.attributeStyleMap` over the element's style attribute, `Element.computedStyleMap()` over
//! `getComputedStyle`, `CSSStyleRule.styleMap`; `get` / `getAll` / `has` / `size` / iteration; `set` /
//! `append` / `delete` / `clear` with the property's grammar — a value whose numeric type the property does not
//! take, a keyword outside its grammar, an unsupported value associated with another property, a list for a
//! non-list property: TypeError; a custom property reads and writes a `CSSUnparsedValue`); §4 the values:
//! `CSSStyleValue` (`parse` / `parseAll`), `CSSKeywordValue`, `CSSUnparsedValue` / `CSSVariableReferenceValue`,
//! §5 `CSSNumericValue` (`add` / `sub` / `mul` / `div` / `min` / `max` / `equals` / `to` / `toSum` / `type` /
//! `parse`), `CSSUnitValue` (every unit of §5.1 with the factory functions on `CSS`), the `CSSMathValue` family
//! (`CSSMathSum` / `CSSMathProduct` / `CSSMathNegate` / `CSSMathInvert` / `CSSMathMin` / `CSSMathMax` /
//! `CSSMathClamp` with `CSSNumericArray`), §5.6 the numeric type algebra (add / multiply / invert with the
//! percent hint), §6.7 serialization (`calc()` with nested parentheses, `min()` / `max()` / `clamp()`), §6
//! `CSSTransformValue` with `CSSTranslate` / `CSSRotate` / `CSSScale` / `CSSSkew` / `CSSSkewX` / `CSSSkewY` /
//! `CSSPerspective` / `CSSMatrixComponent` (`is2D`, `toMatrix()`), `CSSImageValue`; §7 reification of a
//! declared value — `<number>` / `<dimension>` / `<percentage>` / `calc()` / `min()` / `max()` / `clamp()`
//! into numeric values, an identifier into a keyword, a transform list into its components, `url()` into an
//! image, `var()` into an unparsed value, anything else into a base `CSSStyleValue` bound to its property — and
//! of a computed value (relative lengths resolved to px, times to s, angles to deg; an out-of-range number
//! serialized as `calc()` so the declaration stays valid).
//!
//! CHARTER: Aether — binding.
//!
//! Everything is the prelude below over the CSSOM this binding already has (`element.style`, `getComputedStyle`,
//! `CSSStyleRule.style`): the property grammar table (`PROPS`) is the one the tests of
//! `css/css-typed-om/the-stylepropertymap/properties` exercise, in the Typed OM's own terms (which numeric base
//! types a property takes, its keywords, whether it is list-valued, whether it is non-negative or integer-valued).
//! Not reified (the gap list in docs/dev/evidence/aetherwpt/WPT.md §AETHERAPIS): colors (`CSSColorValue`),
//! `<position>`, shorthands (a base `CSSStyleValue`), and `calc()` simplification beyond same-unit sums.

use js_core::vm::*;

pub fn install(vm: &mut Vm) {
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("typed-om-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] typed om prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var hide = function (o, name, v) {
        Object.defineProperty(o, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var def = function (proto, name, f) {
        Object.defineProperty(proto, name, { value: f, writable: true, configurable: true, enumerable: true });
    };
    var accessor = function (proto, name, get, set) {
        Object.defineProperty(proto, name, { get: get, set: set, configurable: true, enumerable: true });
    };
    var TOKEN = {};
    var CSS_WIDE = ['initial', 'inherit', 'unset', 'revert', 'revert-layer'];
    // ---------- units and numeric types (§5.1, §5.6) ----------
    var UNITS = {
        number: '', percent: 'percent',
        em: 'length', ex: 'length', ch: 'length', ic: 'length', rem: 'length', lh: 'length', rlh: 'length', vw: 'length', vh: 'length', vi: 'length', vb: 'length', vmin: 'length', vmax: 'length',
        svw: 'length', svh: 'length', svi: 'length', svb: 'length', svmin: 'length', svmax: 'length', lvw: 'length', lvh: 'length', lvi: 'length', lvb: 'length', lvmin: 'length', lvmax: 'length',
        dvw: 'length', dvh: 'length', dvi: 'length', dvb: 'length', dvmin: 'length', dvmax: 'length', cqw: 'length', cqh: 'length', cqi: 'length', cqb: 'length', cqmin: 'length', cqmax: 'length',
        cap: 'length', rcap: 'length', rex: 'length', rch: 'length', ric: 'length',
        cm: 'length', mm: 'length', q: 'length', 'in': 'length', pt: 'length', pc: 'length', px: 'length',
        deg: 'angle', grad: 'angle', rad: 'angle', turn: 'angle',
        s: 'time', ms: 'time',
        hz: 'frequency', khz: 'frequency',
        dpi: 'resolution', dpcm: 'resolution', dppx: 'resolution', x: 'resolution',
        fr: 'flex'
    };
    var CANONICAL = { length: 'px', angle: 'deg', time: 's', frequency: 'hz', resolution: 'dppx', flex: 'fr' };
    // value in canonical unit = value * FACTOR[unit] (absolute units only)
    var FACTOR = { px: 1, cm: 96 / 2.54, mm: 96 / 25.4, q: 96 / 2.54 / 40, 'in': 96, pt: 96 / 72, pc: 16,
        deg: 1, grad: 0.9, rad: 180 / Math.PI, turn: 360, s: 1, ms: 0.001, hz: 1, khz: 1000, dppx: 1, x: 1, dpi: 1 / 96, dpcm: 2.54 / 96, fr: 1, number: 1, percent: 1 };
    var BASES = ['length', 'angle', 'time', 'frequency', 'resolution', 'flex', 'percent'];
    var unitOk = function (u) { return Object.prototype.hasOwnProperty.call(UNITS, u); };
    var typeOfUnit = function (u) {
        var t = {};
        var b = UNITS[u];
        if (b) { t[b] = 1; }
        return t;
    };
    var typeKeys = function (t) { return BASES.filter(function (b) { return t[b] !== undefined && t[b] !== 0; }); };
    var applyHint = function (t, hint) {
        var r = Object.assign({}, t);
        if (hint === undefined || hint === null) { return r; }
        if (r.percent) { r[hint] = (r[hint] || 0) + r.percent; delete r.percent; }
        else if (r[hint] === undefined) { r[hint] = 0; }
        r.percentHint = hint;
        return r;
    };
    var sameExp = function (a, b) {
        for (var i = 0; i < BASES.length; i++) { if ((a[BASES[i]] || 0) !== (b[BASES[i]] || 0)) { return false; } }
        return true;
    };
    // "add two types": null on failure
    var addTypes = function (t1, t2) {
        var a = Object.assign({}, t1), b = Object.assign({}, t2);
        if (a.percentHint !== undefined && b.percentHint !== undefined && a.percentHint !== b.percentHint) { return null; }
        if (a.percentHint !== undefined && b.percentHint === undefined) { b = applyHint(b, a.percentHint); }
        if (b.percentHint !== undefined && a.percentHint === undefined) { a = applyHint(a, b.percentHint); }
        var ka = typeKeys(a), kb = typeKeys(b);
        if (sameExp(a, b)) {
            var r = {};
            ka.forEach(function (k) { r[k] = a[k]; });
            if (a.percentHint !== undefined) { r.percentHint = a.percentHint; }
            return r;
        }
        if ((a.percent && ka.length > 1) || (b.percent && kb.length > 1) || (a.percent && kb.length > 0 && !b.percent) || (b.percent && ka.length > 0 && !a.percent)) {
            // one side has percent: try each base type as a hint
            for (var i = 0; i < BASES.length; i++) {
                var base = BASES[i];
                if (base === 'percent') { continue; }
                var a2 = applyHint(a, base), b2 = applyHint(b, base);
                if (sameExp(a2, b2)) {
                    var r2 = {};
                    typeKeys(a2).forEach(function (k) { r2[k] = a2[k]; });
                    r2.percentHint = base;
                    return r2;
                }
            }
        }
        return null;
    };
    var mulTypes = function (t1, t2) {
        var a = Object.assign({}, t1), b = Object.assign({}, t2);
        if (a.percentHint !== undefined && b.percentHint !== undefined && a.percentHint !== b.percentHint) { return null; }
        if (a.percentHint !== undefined && b.percentHint === undefined) { b = applyHint(b, a.percentHint); }
        if (b.percentHint !== undefined && a.percentHint === undefined) { a = applyHint(a, b.percentHint); }
        var r = {};
        BASES.forEach(function (k) { var v = (a[k] || 0) + (b[k] || 0); if (v !== 0) { r[k] = v; } });
        if (a.percentHint !== undefined) { r.percentHint = a.percentHint; }
        return r;
    };
    var invertType = function (t) {
        var r = {};
        BASES.forEach(function (k) { if (t[k]) { r[k] = -t[k]; } });
        if (t.percentHint !== undefined) { r.percentHint = t.percentHint; }
        return r;
    };
    var typeToDict = function (t) {
        var d = {};
        BASES.forEach(function (k) { if (t[k]) { d[k] = t[k]; } });
        if (t.percentHint !== undefined) { d.percentHint = t.percentHint; }
        return d;
    };
    // ---------- number serialization ----------
    var numStr = function (n) {
        if (Object.is(n, -0)) { return '-0'; }
        if (!isFinite(n)) { return isNaN(n) ? 'NaN' : (n > 0 ? 'infinity' : '-infinity'); }
        var s = String(n);
        if (/e/.test(s)) { s = n.toFixed(20).replace(/\.?0+$/, ''); }
        return s;
    };
    // ---------- the value classes ----------
    var ASSOC = new WeakMap();   // base CSSStyleValue -> { text, property }
    class CSSStyleValue {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } }
        toString() { var a = ASSOC.get(this); return a ? a.text : ''; }
        static parse(property, cssText) { return parseValue(property, cssText, false); }
        static parseAll(property, cssText) { return parseValue(property, cssText, true); }
    }
    var baseValue = function (text, property) {
        var v = new CSSStyleValue(TOKEN);
        ASSOC.set(v, { text: text, property: property === undefined ? null : property });
        return v;
    };
    class CSSKeywordValue extends CSSStyleValue {
        constructor(value) {
            super(TOKEN);
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'CSSKeywordValue': 1 argument required, but only 0 present."); }
            var v = String(value);
            if (v === '') { throw new TypeError("Failed to construct 'CSSKeywordValue': The value must not be empty."); }
            this.__value = v;
        }
        get value() { return this.__value; }
        set value(v) {
            var s = String(v);
            if (s === '') { throw new TypeError("Failed to set the 'value' property on 'CSSKeywordValue': The value must not be empty."); }
            this.__value = s;
        }
        toString() { return this.__value; }
    }
    // §5 numeric values
    class CSSNumericValue extends CSSStyleValue {
        constructor(tok) { super(tok); }
        type() { return typeToDict(numType(this)); }
        add() { return arith('add', this, rectifyAll(arguments)); }
        sub() { return arith('add', this, rectifyAll(arguments).map(negate)); }
        mul() { return arith('mul', this, rectifyAll(arguments)); }
        div() { return arith('mul', this, rectifyAll(arguments).map(invert)); }
        min() { return arith('min', this, rectifyAll(arguments)); }
        max() { return arith('max', this, rectifyAll(arguments)); }
        equals() {
            var vals = rectifyAll(arguments);
            for (var i = 0; i < vals.length; i++) { if (!numEquals(this, vals[i])) { return false; } }
            return true;
        }
        to(unit) {
            unit = String(unit).toLowerCase();
            if (!unitOk(unit)) { throw new SyntaxError("Failed to execute 'to' on 'CSSNumericValue': Invalid unit."); }
            var sum = createSum(this);
            if (sum === null || sum.length !== 1) { throw new TypeError("Failed to execute 'to' on 'CSSNumericValue': Cannot convert to a single unit."); }
            var item = sum[0];
            var r = convertTerm(item, unit);
            if (r === null) { throw new TypeError("Failed to execute 'to' on 'CSSNumericValue': Incompatible units."); }
            return r;
        }
        toSum() {
            var units = Array.prototype.map.call(arguments, function (u) { return String(u).toLowerCase(); });
            for (var i = 0; i < units.length; i++) { if (!unitOk(units[i])) { throw new SyntaxError("Failed to execute 'toSum' on 'CSSNumericValue': Invalid unit."); } }
            var sum = createSum(this);
            if (sum === null) { throw new TypeError("Failed to execute 'toSum' on 'CSSNumericValue': The value is not a sum."); }
            var terms = sum.map(function (item) { return { value: item.value, unit: item.unit }; });
            if (units.length === 0) {
                terms.sort(function (a, b) { return a.unit < b.unit ? -1 : (a.unit > b.unit ? 1 : 0); });
                return ctor(CSSMathSum, terms.map(function (t) { return new CSSUnitValue(t.value, t.unit); }));
            }
            var out = [];
            units.forEach(function (u) {
                var acc = null;
                terms = terms.filter(function (t) {
                    var c = convertTerm(t, u);
                    if (c === null) { return true; }
                    acc = acc === null ? c.value : acc + c.value;
                    return false;
                });
                out.push(new CSSUnitValue(acc === null ? 0 : acc, u));
            });
            if (terms.length > 0) { throw new TypeError("Failed to execute 'toSum' on 'CSSNumericValue': Incompatible units."); }
            return new (Function.prototype.bind.apply(CSSMathSum, [null].concat(out)))();
        }
        static parse(cssText) {
            var v = parseNumeric(String(cssText));
            if (v === null) { throw new DOMException("Failed to execute 'parse' on 'CSSNumericValue': Invalid numeric value", 'SyntaxError'); }
            return v;
        }
    }
    class CSSUnitValue extends CSSNumericValue {
        constructor(value, unit) {
            super(TOKEN);
            if (arguments.length < 2) { throw new TypeError("Failed to construct 'CSSUnitValue': 2 arguments required, but only " + arguments.length + " present."); }
            var u = String(unit).toLowerCase();
            if (!unitOk(u)) { throw new TypeError("Failed to construct 'CSSUnitValue': Invalid unit: " + unit); }
            var n = Number(value);
            if (typeof value !== 'number' && (value === null || typeof value === 'object' || typeof value === 'symbol')) { n = Number(value); }
            if (isNaN(n) && typeof value !== 'number') { throw new TypeError("Failed to construct 'CSSUnitValue': The provided double value is non-finite."); }
            this.__value = n;
            this.__unit = u;
        }
        get value() { return this.__value; }
        set value(v) { this.__value = Number(v); }
        get unit() { return this.__unit; }
        toString() { return serializeNumeric(this, false, false); }
    }
    class CSSNumericArray {
        constructor(tok, list) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            for (var i = 0; i < list.length; i++) { Object.defineProperty(this, i, { value: list[i], enumerable: true, configurable: false, writable: false }); }
            Object.defineProperty(this, 'length', { value: list.length, enumerable: false });
            this.__list = list.slice();
        }
        [Symbol.iterator]() { return this.__list[Symbol.iterator](); }
        entries() { return this.__list.entries(); }
        keys() { return this.__list.keys(); }
        values() { return this.__list.values(); }
        forEach(f, thisArg) { this.__list.forEach(f, thisArg); }
    }
    class CSSMathValue extends CSSNumericValue {
        constructor(tok) { super(tok); }
        get operator() { return this.__operator; }
        toString() { return serializeNumeric(this, false, false); }
    }
    var mathList = function (name, args, combine) {
        var vals = rectifyAll(args);
        if (vals.length === 0) { throw new TypeError("Failed to construct '" + name + "': Arguments can't be empty"); }
        var t = numType(vals[0]);
        for (var i = 1; i < vals.length; i++) {
            t = combine(t, numType(vals[i]));
            if (t === null) { throw new TypeError("Failed to construct '" + name + "': Incompatible types"); }
        }
        return { values: vals, type: t };
    };
    class CSSMathSum extends CSSMathValue {
        constructor() {
            super(TOKEN);
            var r = mathList('CSSMathSum', arguments, addTypes);
            this.__operator = 'sum'; this.__values = new CSSNumericArray(TOKEN, r.values); this.__type = r.type;
        }
        get values() { return this.__values; }
    }
    class CSSMathProduct extends CSSMathValue {
        constructor() {
            super(TOKEN);
            var r = mathList('CSSMathProduct', arguments, mulTypes);
            this.__operator = 'product'; this.__values = new CSSNumericArray(TOKEN, r.values); this.__type = r.type;
        }
        get values() { return this.__values; }
    }
    class CSSMathMin extends CSSMathValue {
        constructor() {
            super(TOKEN);
            var r = mathList('CSSMathMin', arguments, addTypes);
            this.__operator = 'min'; this.__values = new CSSNumericArray(TOKEN, r.values); this.__type = r.type;
        }
        get values() { return this.__values; }
    }
    class CSSMathMax extends CSSMathValue {
        constructor() {
            super(TOKEN);
            var r = mathList('CSSMathMax', arguments, addTypes);
            this.__operator = 'max'; this.__values = new CSSNumericArray(TOKEN, r.values); this.__type = r.type;
        }
        get values() { return this.__values; }
    }
    class CSSMathClamp extends CSSMathValue {
        constructor(lower, value, upper) {
            super(TOKEN);
            if (arguments.length < 3) { throw new TypeError("Failed to construct 'CSSMathClamp': 3 arguments required, but only " + arguments.length + " present."); }
            var r = mathList('CSSMathClamp', [lower, value, upper], addTypes);
            this.__operator = 'clamp'; this.__lower = r.values[0]; this.__value = r.values[1]; this.__upper = r.values[2]; this.__type = r.type;
        }
        get lower() { return this.__lower; }
        get value() { return this.__value; }
        get upper() { return this.__upper; }
    }
    class CSSMathNegate extends CSSMathValue {
        constructor(arg) {
            super(TOKEN);
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'CSSMathNegate': 1 argument required, but only 0 present."); }
            var v = rectify(arg);
            this.__operator = 'negate'; this.__value = v; this.__type = numType(v);
        }
        get value() { return this.__value; }
    }
    class CSSMathInvert extends CSSMathValue {
        constructor(arg) {
            super(TOKEN);
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'CSSMathInvert': 1 argument required, but only 0 present."); }
            var v = rectify(arg);
            this.__operator = 'invert'; this.__value = v; this.__type = invertType(numType(v));
        }
        get value() { return this.__value; }
    }
    var numType = function (v) {
        if (v instanceof CSSUnitValue) { return typeOfUnit(v.unit); }
        if (v instanceof CSSMathValue) { return v.__type; }
        return {};
    };
    var rectify = function (v) {
        if (typeof v === 'number') { return new CSSUnitValue(v, 'number'); }
        if (v instanceof CSSNumericValue) { return v; }
        throw new TypeError('The value is not a CSSNumericValue');
    };
    var rectifyAll = function (args) { return Array.prototype.map.call(args, rectify); };
    var negate = function (v) {
        if (v instanceof CSSUnitValue) { return new CSSUnitValue(-v.value, v.unit); }
        if (v instanceof CSSMathNegate) { return v.value; }
        return new CSSMathNegate(v);
    };
    var invert = function (v) {
        if (v instanceof CSSUnitValue && v.unit === 'number') {
            if (v.value === 0) { throw new RangeError('Division by zero'); }
            return new CSSUnitValue(1 / v.value, 'number');
        }
        if (v instanceof CSSMathInvert) { return v.value; }
        return new CSSMathInvert(v);
    };
    var ctor = function (C, list) { return new (Function.prototype.bind.apply(C, [null].concat(list)))(); };
    var arith = function (op, self, vals) {
        var values;
        if (op === 'add' && self instanceof CSSMathSum) { values = Array.from(self.values).concat(vals); }
        else if (op === 'mul' && self instanceof CSSMathProduct) { values = Array.from(self.values).concat(vals); }
        else if (op === 'min' && self instanceof CSSMathMin) { values = Array.from(self.values).concat(vals); }
        else if (op === 'max' && self instanceof CSSMathMax) { values = Array.from(self.values).concat(vals); }
        else { values = [self].concat(vals); }
        var combine = op === 'mul' ? mulTypes : addTypes;
        var t = numType(values[0]);
        for (var i = 1; i < values.length; i++) {
            t = combine(t, numType(values[i]));
            if (t === null) { throw new TypeError('Incompatible types'); }
        }
        var allUnit = values.every(function (v) { return v instanceof CSSUnitValue; });
        if (allUnit) {
            var unit = values[0].unit;
            var same = values.every(function (v) { return v.unit === unit; });
            if (op === 'add' && same) { return new CSSUnitValue(values.reduce(function (a, v) { return a + v.value; }, 0), unit); }
            if (op === 'min' && same) { return new CSSUnitValue(Math.min.apply(null, values.map(function (v) { return v.value; })), unit); }
            if (op === 'max' && same) { return new CSSUnitValue(Math.max.apply(null, values.map(function (v) { return v.value; })), unit); }
            if (op === 'mul') {
                var nonNumber = values.filter(function (v) { return v.unit !== 'number'; });
                if (nonNumber.length <= 1) {
                    var u = nonNumber.length ? nonNumber[0].unit : 'number';
                    return new CSSUnitValue(values.reduce(function (a, v) { return a * v.value; }, 1), u);
                }
            }
        }
        var C = op === 'add' ? CSSMathSum : (op === 'mul' ? CSSMathProduct : (op === 'min' ? CSSMathMin : CSSMathMax));
        return ctor(C, values);
    };
    var numEquals = function (a, b) {
        if (a.constructor !== b.constructor) { return false; }
        if (a instanceof CSSUnitValue) { return a.value === b.value && a.unit === b.unit; }
        if (a instanceof CSSMathNegate || a instanceof CSSMathInvert) { return numEquals(a.value, b.value); }
        if (a instanceof CSSMathClamp) { return numEquals(a.lower, b.lower) && numEquals(a.value, b.value) && numEquals(a.upper, b.upper); }
        if (a instanceof CSSMathValue) {
            var av = Array.from(a.values), bv = Array.from(b.values);
            if (av.length !== bv.length) { return false; }
            for (var i = 0; i < av.length; i++) { if (!numEquals(av[i], bv[i])) { return false; } }
            return true;
        }
        return false;
    };
    // "create a sum value": [{value, unit}] with units as a sorted unit map; null when not representable
    var unitMapKey = function (m) { return Object.keys(m).sort().map(function (k) { return k + '^' + m[k]; }).join('*'); };
    var createSum = function (v) {
        var terms = createSumTerms(v);
        if (terms === null) { return null; }
        // merge same unit maps
        var out = [];
        terms.forEach(function (t) {
            var key = unitMapKey(t.map);
            var hit = out.find(function (o) { return o.key === key; });
            if (hit) { hit.value += t.value; } else { out.push({ key: key, value: t.value, map: t.map }); }
        });
        var result = [];
        for (var i = 0; i < out.length; i++) {
            var keys = Object.keys(out[i].map);
            var unit;
            if (keys.length === 0) { unit = 'number'; }
            else if (keys.length === 1 && out[i].map[keys[0]] === 1) { unit = keys[0]; }
            else { return null; }
            result.push({ value: out[i].value, unit: unit });
        }
        return result;
    };
    var toCanonical = function (uv) {
        var u = uv.unit;
        if (u === 'number' || u === 'percent') { return { value: uv.value, unit: u }; }
        var base = UNITS[u];
        if (FACTOR[u] === undefined) { return { value: uv.value, unit: u }; }
        return { value: uv.value * FACTOR[u], unit: CANONICAL[base] };
    };
    var createSumTerms = function (v) {
        if (v instanceof CSSUnitValue) {
            var c = toCanonical(v);
            var map = {};
            if (c.unit !== 'number') { map[c.unit] = 1; }
            return [{ value: c.value, map: map }];
        }
        if (v instanceof CSSMathSum) {
            var all = [];
            var items = Array.from(v.values);
            for (var i = 0; i < items.length; i++) {
                var s = createSumTerms(items[i]);
                if (s === null) { return null; }
                all = all.concat(s);
            }
            return all;
        }
        if (v instanceof CSSMathNegate) {
            var n = createSumTerms(v.value);
            if (n === null) { return null; }
            return n.map(function (t) { return { value: -t.value, map: t.map }; });
        }
        if (v instanceof CSSMathInvert) {
            var inv = createSumTerms(v.value);
            if (inv === null || inv.length !== 1) { return null; }
            var m2 = {};
            Object.keys(inv[0].map).forEach(function (k) { m2[k] = -inv[0].map[k]; });
            return [{ value: 1 / inv[0].value, map: m2 }];
        }
        if (v instanceof CSSMathProduct) {
            var acc = [{ value: 1, map: {} }];
            var ps = Array.from(v.values);
            for (var j = 0; j < ps.length; j++) {
                var p = createSumTerms(ps[j]);
                if (p === null) { return null; }
                var next = [];
                acc.forEach(function (a) {
                    p.forEach(function (b) {
                        var m = Object.assign({}, a.map);
                        Object.keys(b.map).forEach(function (k) { m[k] = (m[k] || 0) + b.map[k]; if (m[k] === 0) { delete m[k]; } });
                        next.push({ value: a.value * b.value, map: m });
                    });
                });
                acc = next;
            }
            return acc;
        }
        if (v instanceof CSSMathMin || v instanceof CSSMathMax) {
            var cands = Array.from(v.values).map(createSumTerms);
            if (cands.some(function (c) { return c === null || c.length !== 1; })) { return null; }
            var key0 = unitMapKey(cands[0][0].map);
            if (cands.some(function (c) { return unitMapKey(c[0].map) !== key0; })) { return null; }
            var vals = cands.map(function (c) { return c[0].value; });
            return [{ value: v instanceof CSSMathMin ? Math.min.apply(null, vals) : Math.max.apply(null, vals), map: cands[0][0].map }];
        }
        if (v instanceof CSSMathClamp) {
            var cl = [v.lower, v.value, v.upper].map(createSumTerms);
            if (cl.some(function (c) { return c === null || c.length !== 1; })) { return null; }
            var k0 = unitMapKey(cl[0][0].map);
            if (cl.some(function (c) { return unitMapKey(c[0].map) !== k0; })) { return null; }
            return [{ value: Math.max(cl[0][0].value, Math.min(cl[1][0].value, cl[2][0].value)), map: cl[0][0].map }];
        }
        return null;
    };
    var convertTerm = function (term, unit) {
        var from = term.unit, to = unit;
        if (from === to) { return new CSSUnitValue(term.value, to); }
        if (from === 'number' || to === 'number' || from === 'percent' || to === 'percent') { return null; }
        if (UNITS[from] !== UNITS[to]) { return null; }
        if (FACTOR[from] === undefined || FACTOR[to] === undefined) { return null; }
        return new CSSUnitValue(term.value * FACTOR[from] / FACTOR[to], to);
    };
    // §6.7.2 serialization of numeric values
    var serializeNumeric = function (v, nested, parenless) {
        if (v instanceof CSSUnitValue) {
            var n = numStr(v.value);
            if (v.unit === 'number') { return n; }
            if (v.unit === 'percent') { return n + '%'; }
            return n + v.unit;
        }
        var s = '';
        if (v instanceof CSSMathSum || v instanceof CSSMathProduct) {
            var isSum = v instanceof CSSMathSum;
            if (!nested) { s += 'calc('; } else if (!parenless) { s += '('; }
            var items = Array.from(v.values);
            s += serializeNumeric(items[0], true, true);
            for (var i = 1; i < items.length; i++) {
                var it = items[i];
                if (isSum) {
                    if (it instanceof CSSMathNegate) { s += ' - ' + serializeNumeric(it.value, true, false); }
                    else { s += ' + ' + serializeNumeric(it, true, false); }
                } else {
                    if (it instanceof CSSMathInvert) { s += ' / ' + serializeNumeric(it.value, true, false); }
                    else { s += ' * ' + serializeNumeric(it, true, false); }
                }
            }
            if (!nested || !parenless) { s += ')'; }
            return s;
        }
        if (v instanceof CSSMathNegate) {
            if (!nested) { s += 'calc('; }
            s += '-' + serializeNumeric(v.value, true, false);
            if (!nested) { s += ')'; }
            return s;
        }
        if (v instanceof CSSMathInvert) {
            if (!nested) { s += 'calc('; }
            s += '1 / ' + serializeNumeric(v.value, true, false);
            if (!nested) { s += ')'; }
            return s;
        }
        if (v instanceof CSSMathMin || v instanceof CSSMathMax) {
            return (v instanceof CSSMathMin ? 'min(' : 'max(') + Array.from(v.values).map(function (x) { return serializeNumeric(x, true, true); }).join(', ') + ')';
        }
        if (v instanceof CSSMathClamp) {
            return 'clamp(' + [v.lower, v.value, v.upper].map(function (x) { return serializeNumeric(x, true, true); }).join(', ') + ')';
        }
        return '';
    };
    // ---------- a small CSS tokenizer (CSS Syntax §4) for reification ----------
    var tokenize = function (text) {
        var toks = [];
        var i = 0, n = text.length;
        var isNameStart = function (c) { return /[A-Za-z_\u0080-￿-]/.test(c); };
        var isName = function (c) { return /[A-Za-z0-9_\u0080-￿-]/.test(c); };
        var isDigit = function (c) { return c >= '0' && c <= '9'; };
        while (i < n) {
            var c = text[i];
            if (/\s/.test(c)) { var j = i; while (j < n && /\s/.test(text[j])) { j++; } toks.push({ t: 'ws', v: text.slice(i, j) }); i = j; continue; }
            if (c === '/' && text[i + 1] === '*') { var e = text.indexOf('*/', i + 2); i = e < 0 ? n : e + 2; continue; }
            if (c === '"' || c === "'") {
                var q = c, k = i + 1, str = '';
                while (k < n && text[k] !== q) { if (text[k] === '\\' && k + 1 < n) { str += text[k + 1]; k += 2; } else { str += text[k]; k++; } }
                toks.push({ t: 'string', v: str, raw: text.slice(i, Math.min(k + 1, n)) }); i = k + 1; continue;
            }
            var numMatch = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?/.exec(text.slice(i));
            if (numMatch && (isDigit(c) || ((c === '+' || c === '-') && (isDigit(text[i + 1]) || (text[i + 1] === '.' && isDigit(text[i + 2])))) || (c === '.' && isDigit(text[i + 1])))) {
                var numText = numMatch[0];
                var after = i + numText.length;
                var val = parseFloat(numText);
                if (text[after] === '%') { toks.push({ t: 'percentage', v: val, raw: numText + '%' }); i = after + 1; continue; }
                var um = /^[A-Za-z_-][A-Za-z0-9_-]*/.exec(text.slice(after));
                if (um && um[0] !== '-' && !(um[0].charAt(0) === '-' && um[0].length > 1 && !isNameStart(um[0].charAt(1)) && !(um[0].charAt(1) === '-'))) {
                    toks.push({ t: 'dimension', v: val, unit: um[0], raw: numText + um[0] }); i = after + um[0].length; continue;
                }
                toks.push({ t: 'number', v: val, raw: numText, isInt: !/[.eE]/.test(numText) }); i = after; continue;
            }
            if (isNameStart(c) || (c === '-' && (isNameStart(text[i + 1] || '') || text[i + 1] === '-')) || c === '\\') {
                var m = i, name = '';
                while (m < n && (isName(text[m]) || text[m] === '\\')) { if (text[m] === '\\' && m + 1 < n) { name += text[m + 1]; m += 2; } else { name += text[m]; m++; } }
                if (text[m] === '(') {
                    if (name.toLowerCase() === 'url') {
                        var depth = 1, p = m + 1;
                        while (p < n && depth > 0) { if (text[p] === '(') { depth++; } else if (text[p] === ')') { depth--; } p++; }
                        toks.push({ t: 'url', v: text.slice(m + 1, p - 1).trim(), raw: text.slice(i, p) }); i = p; continue;
                    }
                    toks.push({ t: 'function', v: name }); i = m + 1; continue;
                }
                toks.push({ t: 'ident', v: name }); i = m; continue;
            }
            if (c === '#') { var h = i + 1; while (h < n && isName(text[h])) { h++; } toks.push({ t: 'hash', v: text.slice(i + 1, h), raw: text.slice(i, h) }); i = h; continue; }
            toks.push({ t: 'delim', v: c }); i++;
        }
        return toks;
    };
    // group function calls into trees: {t:'function', v:name, args:[tokens...]}
    var group = function (toks) {
        var pos = 0;
        var parseList = function (closer) {
            var out = [];
            while (pos < toks.length) {
                var tk = toks[pos];
                if (closer && tk.t === 'delim' && tk.v === closer) { pos++; return out; }
                if (tk.t === 'function') { pos++; out.push({ t: 'function', v: tk.v, args: parseList(')') }); continue; }
                if (tk.t === 'delim' && tk.v === '(') { pos++; out.push({ t: 'block', args: parseList(')') }); continue; }
                if (tk.t === 'delim' && (tk.v === ')' ) && !closer) { pos++; continue; }
                out.push(tk); pos++;
            }
            return out;
        };
        return parseList(null);
    };
    var splitTop = function (items, sep) {
        var out = [], cur = [];
        items.forEach(function (it) {
            if (it.t === 'delim' && it.v === sep) { out.push(cur); cur = []; } else { cur.push(it); }
        });
        out.push(cur);
        return out;
    };
    var trimWs = function (items) {
        var a = 0, b = items.length;
        while (a < b && items[a].t === 'ws') { a++; }
        while (b > a && items[b - 1].t === 'ws') { b--; }
        return items.slice(a, b);
    };
    var serializeItems = function (items) {
        return items.map(function (it) {
            if (it.t === 'ws') { return ' '; }
            if (it.t === 'function') { return it.v + '(' + serializeItems(it.args) + ')'; }
            if (it.t === 'block') { return '(' + serializeItems(it.args) + ')'; }
            if (it.raw !== undefined) { return it.raw; }
            if (it.t === 'string') { return '"' + it.v + '"'; }
            return String(it.v);
        }).join('');
    };
    // math expression -> CSSNumericValue (null when not numeric)
    var MATH_FN = ['calc', 'min', 'max', 'clamp'];
    var parseMathTree = function (items, inCalc) {
        items = trimWs(items);
        if (items.length === 0) { return null; }
        if (items.length === 1) {
            var it = items[0];
            if (it.t === 'number') { return new CSSUnitValue(it.v, 'number'); }
            if (it.t === 'percentage') { return new CSSUnitValue(it.v, 'percent'); }
            if (it.t === 'dimension') { var u = it.unit.toLowerCase(); return unitOk(u) && u !== 'number' && u !== 'percent' ? new CSSUnitValue(it.v, u) : null; }
            if (it.t === 'block') { return parseMathTree(it.args, true); }
            if (it.t === 'function') {
                var fn = it.v.toLowerCase();
                if (fn === 'calc') { return parseMathTree(it.args, true); }
                if (fn === 'min' || fn === 'max' || fn === 'clamp') {
                    var parts = splitTop(it.args, ',').map(function (p) { return parseMathTree(p, true); });
                    if (parts.some(function (p) { return p === null; })) { return null; }
                    try {
                        if (fn === 'clamp') { return parts.length === 3 ? new CSSMathClamp(parts[0], parts[1], parts[2]) : null; }
                        return ctor(fn === 'min' ? CSSMathMin : CSSMathMax, parts);
                    } catch (e) { return null; }
                }
                return null;
            }
            if (it.t === 'ident' && inCalc) {
                var l = it.v.toLowerCase();
                if (l === 'pi') { return new CSSUnitValue(Math.PI, 'number'); }
                if (l === 'e') { return new CSSUnitValue(Math.E, 'number'); }
                if (l === 'infinity') { return new CSSUnitValue(Infinity, 'number'); }
                if (l === '-infinity') { return new CSSUnitValue(-Infinity, 'number'); }
                if (l === 'nan') { return new CSSUnitValue(NaN, 'number'); }
                return null;
            }
            return null;
        }
        if (!inCalc) { return null; }
        // sum of products: split on top-level + / - surrounded by whitespace
        var terms = [], cur = [], sign = 1, signs = [];
        for (var i = 0; i < items.length; i++) {
            var tk = items[i];
            var prevWs = i > 0 && items[i - 1].t === 'ws', nextWs = i + 1 < items.length && items[i + 1].t === 'ws';
            if (tk.t === 'delim' && (tk.v === '+' || tk.v === '-') && prevWs && nextWs && cur.length) {
                terms.push(cur); signs.push(sign); cur = []; sign = tk.v === '-' ? -1 : 1; continue;
            }
            if (tk.t === 'number' && i > 0 && prevWs && cur.length && (tk.raw.charAt(0) === '+' || tk.raw.charAt(0) === '-') && false) { continue; }
            cur.push(tk);
        }
        terms.push(cur); signs.push(sign);
        if (terms.length === 1) { return parseProduct(terms[0]); }
        var vals = [];
        for (var j = 0; j < terms.length; j++) {
            var v = parseProduct(terms[j]);
            if (v === null) { return null; }
            vals.push(signs[j] < 0 ? (v instanceof CSSUnitValue ? new CSSUnitValue(-v.value, v.unit) : new CSSMathNegate(v)) : v);
        }
        try { return ctor(CSSMathSum, vals); } catch (e) { return null; }
    };
    var parseProduct = function (items) {
        items = trimWs(items);
        var factors = [], cur = [], ops = [], op = '*';
        for (var i = 0; i < items.length; i++) {
            var tk = items[i];
            if (tk.t === 'delim' && (tk.v === '*' || tk.v === '/')) { factors.push(cur); ops.push(op); cur = []; op = tk.v; continue; }
            cur.push(tk);
        }
        factors.push(cur); ops.push(op);
        if (factors.length === 1) { return parseMathTree(factors[0], true); }
        var vals = [];
        for (var j = 0; j < factors.length; j++) {
            var v = parseMathTree(factors[j], true);
            if (v === null) { return null; }
            if (ops[j] === '/') {
                if (v instanceof CSSUnitValue && v.unit === 'number') { if (v.value === 0) { return null; } v = new CSSUnitValue(1 / v.value, 'number'); }
                else { v = new CSSMathInvert(v); }
            }
            vals.push(v);
        }
        try { return ctor(CSSMathProduct, vals); } catch (e) { return null; }
    };
    var parseNumeric = function (text) {
        var items = group(tokenize(text));
        items = trimWs(items);
        if (items.length !== 1) { return null; }
        return parseMathTree(items, false);
    };
    // ---------- unparsed values (var()) ----------
    class CSSVariableReferenceValue {
        constructor(variable, fallback) {
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'CSSVariableReferenceValue': 1 argument required, but only 0 present."); }
            var v = String(variable);
            if (v.indexOf('--') !== 0) { throw new TypeError("Failed to construct 'CSSVariableReferenceValue': Invalid custom property name"); }
            if (fallback !== undefined && fallback !== null && !(fallback instanceof CSSUnparsedValue)) { throw new TypeError("Failed to construct 'CSSVariableReferenceValue': parameter 2 is not of type 'CSSUnparsedValue'."); }
            this.__variable = v;
            this.__fallback = fallback === undefined || fallback === null ? null : fallback;
        }
        get variable() { return this.__variable; }
        set variable(v) {
            var s = String(v);
            if (s.indexOf('--') !== 0) { throw new TypeError("Failed to set the 'variable' property on 'CSSVariableReferenceValue': Invalid custom property name"); }
            this.__variable = s;
        }
        get fallback() { return this.__fallback; }
        toString() { return 'var(' + this.__variable + (this.__fallback ? ',' + this.__fallback.toString() : '') + ')'; }
    }
    var UNPARSED = new WeakMap();
    var unparsedHandler = {
        get: function (t, k, r) {
            if (typeof k === 'string' && /^\d+$/.test(k)) { var l = UNPARSED.get(t); return l[Number(k)]; }
            return Reflect.get(t, k, r);
        },
        set: function (t, k, v, r) {
            if (typeof k === 'string' && /^\d+$/.test(k)) {
                var l = UNPARSED.get(t);
                var i = Number(k);
                if (i > l.length) { throw new RangeError('Index out of range'); }
                if (typeof v !== 'string' && !(v instanceof CSSVariableReferenceValue)) { throw new TypeError('Invalid member'); }
                l[i] = v;
                return true;
            }
            return Reflect.set(t, k, v, r);
        },
        has: function (t, k) {
            if (typeof k === 'string' && /^\d+$/.test(k)) { return Number(k) < UNPARSED.get(t).length; }
            return Reflect.has(t, k);
        },
        ownKeys: function (t) {
            var l = UNPARSED.get(t);
            var keys = [];
            for (var i = 0; i < l.length; i++) { keys.push(String(i)); }
            return keys.concat(Reflect.ownKeys(t));
        },
        getOwnPropertyDescriptor: function (t, k) {
            if (typeof k === 'string' && /^\d+$/.test(k)) {
                var l = UNPARSED.get(t);
                if (Number(k) < l.length) { return { value: l[Number(k)], writable: true, enumerable: true, configurable: true }; }
                return undefined;
            }
            return Reflect.getOwnPropertyDescriptor(t, k);
        }
    };
    class CSSUnparsedValue extends CSSStyleValue {
        constructor(members) {
            super(TOKEN);
            var list = members === undefined ? [] : Array.from(members, function (m) {
                if (typeof m === 'string') { return m; }
                if (m instanceof CSSVariableReferenceValue) { return m; }
                return String(m);
            });
            UNPARSED.set(this, list);
            var p = new Proxy(this, unparsedHandler);
            UNPARSED.set(p, list);
            return p;
        }
        get length() { return UNPARSED.get(this).length; }
        toString() { return UNPARSED.get(this).map(function (m) { return typeof m === 'string' ? m : m.toString(); }).join(''); }
        [Symbol.iterator]() { return UNPARSED.get(this)[Symbol.iterator](); }
        entries() { return UNPARSED.get(this).entries(); }
        keys() { return UNPARSED.get(this).keys(); }
        values() { return UNPARSED.get(this).values(); }
        forEach(f, thisArg) { UNPARSED.get(this).forEach(f, thisArg); }
    }
    // reify a value containing var(): tokens -> CSSUnparsedValue
    var reifyUnparsed = function (items) {
        var members = [];
        var buf = '';
        var flush = function () { if (buf !== '') { members.push(buf); buf = ''; } };
        items.forEach(function (it) {
            if (it.t === 'function' && it.v.toLowerCase() === 'var') {
                flush();
                var parts = splitTop(it.args, ',');
                var name = serializeItems(trimWs(parts[0])).trim();
                var fb = null;
                if (parts.length > 1) {
                    var rest = [];
                    for (var i = 1; i < parts.length; i++) { if (i > 1) { rest.push({ t: 'delim', v: ',' }); } rest = rest.concat(parts[i]); }
                    fb = reifyUnparsed(rest);
                }
                members.push(new CSSVariableReferenceValue(name, fb));
            } else if (it.t === 'function' || it.t === 'block') {
                var inner = reifyUnparsed(it.args);
                var innerList = UNPARSED.get(inner);
                buf += (it.t === 'function' ? it.v + '(' : '(');
                innerList.forEach(function (m) { if (typeof m === 'string') { buf += m; } else { flush(); members.push(m); } });
                buf += ')';
            } else {
                buf += (it.t === 'ws' ? it.v : (it.raw !== undefined ? it.raw : (it.t === 'string' ? '"' + it.v + '"' : String(it.v))));
            }
        });
        flush();
        return new CSSUnparsedValue(members);
    };
    var hasVar = function (items) {
        return items.some(function (it) {
            if (it.t === 'function' && it.v.toLowerCase() === 'var') { return true; }
            if (it.t === 'function' || it.t === 'block') { return hasVar(it.args); }
            return false;
        });
    };
    // ---------- images ----------
    class CSSImageValue extends CSSStyleValue {
        constructor(tok, text) { super(tok); this.__text = text; }
        toString() { return this.__text; }
    }
    // ---------- transforms (§6) ----------
    var mat4 = function () { return [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]; };
    var mul4 = function (a, b) {
        var r = new Array(16);
        for (var row = 0; row < 4; row++) { for (var col = 0; col < 4; col++) {
            var s = 0;
            for (var k = 0; k < 4; k++) { s += a[k * 4 + row] * b[col * 4 + k]; }
            r[col * 4 + row] = s;
        } }
        return r;
    };
    var toDOMMatrix = function (m, is2D) {
        if (typeof DOMMatrix !== 'function') { throw new TypeError('DOMMatrix is not available'); }
        if (is2D) { return new DOMMatrix([m[0], m[1], m[4], m[5], m[12], m[13]]); }
        return new DOMMatrix(m);
    };
    var lengthPx = function (v, what) {
        if (!(v instanceof CSSNumericValue)) { throw new TypeError(what + ' is not a CSSNumericValue'); }
        return v.to('px').value;
    };
    var angleDeg = function (v) { return v.to('deg').value; };
    var numberOf = function (v) { return v.to('number').value; };
    var rectifyNumberish = function (v, name) {
        if (typeof v === 'number') { return new CSSUnitValue(v, 'number'); }
        if (v instanceof CSSNumericValue) { return v; }
        throw new TypeError("Failed to construct '" + name + "': The provided value is not of type '(CSSNumericValue or double)'.");
    };
    var requireType = function (v, base, name, what) {
        var t = numType(v);
        var keys = typeKeys(t);
        var ok = base === 'number' ? keys.length === 0 : (keys.length === 1 && t[base] === 1);
        if (base === 'length-percentage') { ok = (keys.length === 1 && (t.length === 1 || t.percent === 1)) || (keys.length === 2 && t.length === 1 && t.percent === 1 && t.percentHint === 'length'); }
        if (!ok) { throw new TypeError("Failed to construct '" + name + "': " + what + " must be a " + base); }
    };
    class CSSTransformComponent {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } this.__is2D = true; }
        get is2D() { return this.__is2D; }
        set is2D(v) { this.__is2D = !!v; }
        toString() { return ''; }
        toMatrix() { return toDOMMatrix(mat4(), true); }
    }
    class CSSTranslate extends CSSTransformComponent {
        constructor(x, y, z) {
            super(TOKEN);
            if (arguments.length < 2) { throw new TypeError("Failed to construct 'CSSTranslate': 2 arguments required, but only " + arguments.length + " present."); }
            [x, y].forEach(function (v) { if (!(v instanceof CSSNumericValue)) { throw new TypeError("Failed to construct 'CSSTranslate': parameter is not of type 'CSSNumericValue'."); } requireType(v, 'length-percentage', 'CSSTranslate', 'x/y'); });
            if (z !== undefined) { if (!(z instanceof CSSNumericValue)) { throw new TypeError("Failed to construct 'CSSTranslate': parameter 3 is not of type 'CSSNumericValue'."); } requireType(z, 'length', 'CSSTranslate', 'z'); }
            this.__x = x; this.__y = y; this.__z = z === undefined ? new CSSUnitValue(0, 'px') : z;
            this.__is2D = z === undefined;
        }
        get x() { return this.__x; } set x(v) { requireType(v, 'length-percentage', 'CSSTranslate', 'x'); this.__x = v; }
        get y() { return this.__y; } set y(v) { requireType(v, 'length-percentage', 'CSSTranslate', 'y'); this.__y = v; }
        get z() { return this.__z; } set z(v) { requireType(v, 'length', 'CSSTranslate', 'z'); this.__z = v; }
        toString() { return this.is2D ? 'translate(' + this.__x + ', ' + this.__y + ')' : 'translate3d(' + this.__x + ', ' + this.__y + ', ' + this.__z + ')'; }
        toMatrix() {
            var m = mat4();
            m[12] = lengthPx(this.__x, 'x'); m[13] = lengthPx(this.__y, 'y'); m[14] = this.is2D ? 0 : lengthPx(this.__z, 'z');
            return toDOMMatrix(m, this.is2D);
        }
    }
    class CSSRotate extends CSSTransformComponent {
        constructor(a, b, c, d) {
            super(TOKEN);
            if (arguments.length === 1) {
                if (!(a instanceof CSSNumericValue)) { throw new TypeError("Failed to construct 'CSSRotate': parameter 1 is not of type 'CSSNumericValue'."); }
                requireType(a, 'angle', 'CSSRotate', 'angle');
                this.__x = new CSSUnitValue(0, 'number'); this.__y = new CSSUnitValue(0, 'number'); this.__z = new CSSUnitValue(1, 'number'); this.__angle = a; this.__is2D = true;
            } else if (arguments.length >= 4) {
                var x = rectifyNumberish(a, 'CSSRotate'), y = rectifyNumberish(b, 'CSSRotate'), z = rectifyNumberish(c, 'CSSRotate');
                [x, y, z].forEach(function (v) { requireType(v, 'number', 'CSSRotate', 'x/y/z'); });
                if (!(d instanceof CSSNumericValue)) { throw new TypeError("Failed to construct 'CSSRotate': parameter 4 is not of type 'CSSNumericValue'."); }
                requireType(d, 'angle', 'CSSRotate', 'angle');
                this.__x = x; this.__y = y; this.__z = z; this.__angle = d; this.__is2D = false;
            } else { throw new TypeError("Failed to construct 'CSSRotate': Invalid arguments"); }
        }
        get x() { return this.__x; } set x(v) { this.__x = rectifyNumberish(v, 'CSSRotate'); }
        get y() { return this.__y; } set y(v) { this.__y = rectifyNumberish(v, 'CSSRotate'); }
        get z() { return this.__z; } set z(v) { this.__z = rectifyNumberish(v, 'CSSRotate'); }
        get angle() { return this.__angle; } set angle(v) { requireType(v, 'angle', 'CSSRotate', 'angle'); this.__angle = v; }
        toString() { return this.is2D ? 'rotate(' + this.__angle + ')' : 'rotate3d(' + this.__x + ', ' + this.__y + ', ' + this.__z + ', ' + this.__angle + ')'; }
        toMatrix() {
            var deg = angleDeg(this.__angle), rad = deg * Math.PI / 180;
            var x = numberOf(this.__x), y = numberOf(this.__y), z = numberOf(this.__z);
            if (this.is2D) { x = 0; y = 0; z = 1; }
            var len = Math.sqrt(x * x + y * y + z * z);
            if (len === 0) { return toDOMMatrix(mat4(), this.is2D); }
            x /= len; y /= len; z /= len;
            var s = Math.sin(rad), c = Math.cos(rad), t = 1 - c;
            var m = mat4();
            m[0] = c + x * x * t; m[1] = y * x * t + z * s; m[2] = z * x * t - y * s;
            m[4] = x * y * t - z * s; m[5] = c + y * y * t; m[6] = z * y * t + x * s;
            m[8] = x * z * t + y * s; m[9] = y * z * t - x * s; m[10] = c + z * z * t;
            return toDOMMatrix(m, this.is2D);
        }
    }
    class CSSScale extends CSSTransformComponent {
        constructor(x, y, z) {
            super(TOKEN);
            if (arguments.length < 2) { throw new TypeError("Failed to construct 'CSSScale': 2 arguments required, but only " + arguments.length + " present."); }
            x = rectifyNumberish(x, 'CSSScale'); y = rectifyNumberish(y, 'CSSScale');
            requireType(x, 'number', 'CSSScale', 'x'); requireType(y, 'number', 'CSSScale', 'y');
            if (z !== undefined) { z = rectifyNumberish(z, 'CSSScale'); requireType(z, 'number', 'CSSScale', 'z'); }
            this.__x = x; this.__y = y; this.__z = z === undefined ? new CSSUnitValue(1, 'number') : z; this.__is2D = z === undefined;
        }
        get x() { return this.__x; } set x(v) { v = rectifyNumberish(v, 'CSSScale'); requireType(v, 'number', 'CSSScale', 'x'); this.__x = v; }
        get y() { return this.__y; } set y(v) { v = rectifyNumberish(v, 'CSSScale'); requireType(v, 'number', 'CSSScale', 'y'); this.__y = v; }
        get z() { return this.__z; } set z(v) { v = rectifyNumberish(v, 'CSSScale'); requireType(v, 'number', 'CSSScale', 'z'); this.__z = v; }
        toString() { return this.is2D ? 'scale(' + this.__x + ', ' + this.__y + ')' : 'scale3d(' + this.__x + ', ' + this.__y + ', ' + this.__z + ')'; }
        toMatrix() { var m = mat4(); m[0] = numberOf(this.__x); m[5] = numberOf(this.__y); m[10] = this.is2D ? 1 : numberOf(this.__z); return toDOMMatrix(m, this.is2D); }
    }
    class CSSSkew extends CSSTransformComponent {
        constructor(ax, ay) {
            super(TOKEN);
            if (arguments.length < 2) { throw new TypeError("Failed to construct 'CSSSkew': 2 arguments required, but only " + arguments.length + " present."); }
            [ax, ay].forEach(function (v) { if (!(v instanceof CSSNumericValue)) { throw new TypeError("Failed to construct 'CSSSkew': parameter is not of type 'CSSNumericValue'."); } requireType(v, 'angle', 'CSSSkew', 'ax/ay'); });
            this.__ax = ax; this.__ay = ay;
        }
        get ax() { return this.__ax; } set ax(v) { requireType(v, 'angle', 'CSSSkew', 'ax'); this.__ax = v; }
        get ay() { return this.__ay; } set ay(v) { requireType(v, 'angle', 'CSSSkew', 'ay'); this.__ay = v; }
        get is2D() { return true; } set is2D(v) {}
        toString() { return 'skew(' + this.__ax + ', ' + this.__ay + ')'; }
        toMatrix() { var m = mat4(); m[4] = Math.tan(angleDeg(this.__ax) * Math.PI / 180); m[1] = Math.tan(angleDeg(this.__ay) * Math.PI / 180); return toDOMMatrix(m, true); }
    }
    class CSSSkewX extends CSSTransformComponent {
        constructor(ax) {
            super(TOKEN);
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'CSSSkewX': 1 argument required, but only 0 present."); }
            if (!(ax instanceof CSSNumericValue)) { throw new TypeError("Failed to construct 'CSSSkewX': parameter 1 is not of type 'CSSNumericValue'."); }
            requireType(ax, 'angle', 'CSSSkewX', 'ax');
            this.__ax = ax;
        }
        get ax() { return this.__ax; } set ax(v) { requireType(v, 'angle', 'CSSSkewX', 'ax'); this.__ax = v; }
        get is2D() { return true; } set is2D(v) {}
        toString() { return 'skewX(' + this.__ax + ')'; }
        toMatrix() { var m = mat4(); m[4] = Math.tan(angleDeg(this.__ax) * Math.PI / 180); return toDOMMatrix(m, true); }
    }
    class CSSSkewY extends CSSTransformComponent {
        constructor(ay) {
            super(TOKEN);
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'CSSSkewY': 1 argument required, but only 0 present."); }
            if (!(ay instanceof CSSNumericValue)) { throw new TypeError("Failed to construct 'CSSSkewY': parameter 1 is not of type 'CSSNumericValue'."); }
            requireType(ay, 'angle', 'CSSSkewY', 'ay');
            this.__ay = ay;
        }
        get ay() { return this.__ay; } set ay(v) { requireType(v, 'angle', 'CSSSkewY', 'ay'); this.__ay = v; }
        get is2D() { return true; } set is2D(v) {}
        toString() { return 'skewY(' + this.__ay + ')'; }
        toMatrix() { var m = mat4(); m[1] = Math.tan(angleDeg(this.__ay) * Math.PI / 180); return toDOMMatrix(m, true); }
    }
    class CSSPerspective extends CSSTransformComponent {
        constructor(length) {
            super(TOKEN);
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'CSSPerspective': 1 argument required, but only 0 present."); }
            this.__length = CSSPerspective.check(length);
            this.__is2D = false;
        }
        static check(v) {
            if (typeof v === 'string') { if (v.toLowerCase() === 'none') { return new CSSKeywordValue('none'); } throw new TypeError("Failed to construct 'CSSPerspective': Invalid keyword"); }
            if (v instanceof CSSKeywordValue) { if (v.value.toLowerCase() === 'none') { return v; } throw new TypeError("Failed to construct 'CSSPerspective': Invalid keyword"); }
            if (!(v instanceof CSSNumericValue)) { throw new TypeError("Failed to construct 'CSSPerspective': parameter 1 is not of type '(CSSNumericValue or CSSKeywordValue or string)'."); }
            requireType(v, 'length', 'CSSPerspective', 'length');
            return v;
        }
        get length() { return this.__length; } set length(v) { this.__length = CSSPerspective.check(v); }
        get is2D() { return false; } set is2D(v) {}
        toString() { return 'perspective(' + this.__length + ')'; }
        toMatrix() {
            var m = mat4();
            if (this.__length instanceof CSSNumericValue) { var d = lengthPx(this.__length, 'length'); if (d !== 0) { m[11] = -1 / d; } }
            return toDOMMatrix(m, false);
        }
    }
    class CSSMatrixComponent extends CSSTransformComponent {
        constructor(matrix, options) {
            super(TOKEN);
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'CSSMatrixComponent': 1 argument required, but only 0 present."); }
            if (typeof DOMMatrixReadOnly !== 'function' || !(matrix instanceof DOMMatrixReadOnly)) { throw new TypeError("Failed to construct 'CSSMatrixComponent': parameter 1 is not of type 'DOMMatrixReadOnly'."); }
            this.__matrix = new DOMMatrix(matrix.toFloat64Array ? Array.from(matrix.toFloat64Array()) : [matrix.m11, matrix.m12, matrix.m13, matrix.m14, matrix.m21, matrix.m22, matrix.m23, matrix.m24, matrix.m31, matrix.m32, matrix.m33, matrix.m34, matrix.m41, matrix.m42, matrix.m43, matrix.m44]);
            this.__is2D = options && options.is2D !== undefined ? !!options.is2D : matrix.is2D;
        }
        get matrix() { return this.__matrix; } set matrix(v) { this.__matrix = v; }
        toString() {
            var m = this.__matrix;
            if (this.is2D) { return 'matrix(' + [m.a, m.b, m.c, m.d, m.e, m.f].map(numStr).join(', ') + ')'; }
            return 'matrix3d(' + [m.m11, m.m12, m.m13, m.m14, m.m21, m.m22, m.m23, m.m24, m.m31, m.m32, m.m33, m.m34, m.m41, m.m42, m.m43, m.m44].map(numStr).join(', ') + ')';
        }
        toMatrix() {
            var m = this.__matrix;
            var arr = [m.m11, m.m12, m.m13, m.m14, m.m21, m.m22, m.m23, m.m24, m.m31, m.m32, m.m33, m.m34, m.m41, m.m42, m.m43, m.m44];
            return toDOMMatrix(arr, this.is2D);
        }
    }
    var TRANSFORMS = new WeakMap();
    var transformHandler = {
        get: function (t, k, r) {
            if (typeof k === 'string' && /^\d+$/.test(k)) { return TRANSFORMS.get(t)[Number(k)]; }
            return Reflect.get(t, k, r);
        },
        set: function (t, k, v, r) {
            if (typeof k === 'string' && /^\d+$/.test(k)) {
                var l = TRANSFORMS.get(t);
                var i = Number(k);
                if (i > l.length) { throw new RangeError('Index out of range'); }
                if (!(v instanceof CSSTransformComponent)) { throw new TypeError('Invalid component'); }
                l[i] = v;
                return true;
            }
            return Reflect.set(t, k, v, r);
        },
        has: function (t, k) { if (typeof k === 'string' && /^\d+$/.test(k)) { return Number(k) < TRANSFORMS.get(t).length; } return Reflect.has(t, k); },
        ownKeys: function (t) { var l = TRANSFORMS.get(t); var keys = []; for (var i = 0; i < l.length; i++) { keys.push(String(i)); } return keys.concat(Reflect.ownKeys(t)); },
        getOwnPropertyDescriptor: function (t, k) {
            if (typeof k === 'string' && /^\d+$/.test(k)) { var l = TRANSFORMS.get(t); if (Number(k) < l.length) { return { value: l[Number(k)], writable: true, enumerable: true, configurable: true }; } return undefined; }
            return Reflect.getOwnPropertyDescriptor(t, k);
        }
    };
    class CSSTransformValue extends CSSStyleValue {
        constructor(transforms) {
            super(TOKEN);
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'CSSTransformValue': 1 argument required, but only 0 present."); }
            var list = Array.from(transforms);
            if (list.length === 0) { throw new TypeError("Failed to construct 'CSSTransformValue': The sequence must not be empty"); }
            list.forEach(function (c) { if (!(c instanceof CSSTransformComponent)) { throw new TypeError("Failed to construct 'CSSTransformValue': parameter is not of type 'CSSTransformComponent'."); } });
            TRANSFORMS.set(this, list);
            var p = new Proxy(this, transformHandler);
            TRANSFORMS.set(p, list);
            return p;
        }
        get length() { return TRANSFORMS.get(this).length; }
        get is2D() { return TRANSFORMS.get(this).every(function (c) { return c.is2D; }); }
        toString() { return TRANSFORMS.get(this).map(function (c) { return c.toString(); }).join(' '); }
        toMatrix() {
            var list = TRANSFORMS.get(this);
            var m = mat4();
            list.forEach(function (c) {
                var cm = c.toMatrix();
                var arr = [cm.m11, cm.m12, cm.m13, cm.m14, cm.m21, cm.m22, cm.m23, cm.m24, cm.m31, cm.m32, cm.m33, cm.m34, cm.m41, cm.m42, cm.m43, cm.m44];
                m = mul4(m, arr);
            });
            return toDOMMatrix(m, this.is2D);
        }
        [Symbol.iterator]() { return TRANSFORMS.get(this)[Symbol.iterator](); }
        entries() { return TRANSFORMS.get(this).entries(); }
        keys() { return TRANSFORMS.get(this).keys(); }
        values() { return TRANSFORMS.get(this).values(); }
        forEach(f, thisArg) { TRANSFORMS.get(this).forEach(f, thisArg); }
    }
    // reify a transform function list (null when not a transform list)
    var reifyTransform = function (items) {
        items = trimWs(items);
        var comps = [];
        for (var i = 0; i < items.length; i++) {
            var it = items[i];
            if (it.t === 'ws') { continue; }
            if (it.t !== 'function') { return null; }
            var fn = it.v.toLowerCase();
            var args = splitTop(it.args, ',').map(function (a) { return parseMathTree(a, false); });
            if (args.some(function (a) { return a === null; }) && fn !== 'perspective') { return null; }
            var zero = function () { return new CSSUnitValue(0, 'px'); };
            try {
                switch (fn) {
                    case 'translate': comps.push(new CSSTranslate(args[0], args.length > 1 ? args[1] : zero())); break;
                    case 'translatex': comps.push(new CSSTranslate(args[0], zero())); break;
                    case 'translatey': comps.push(new CSSTranslate(zero(), args[0])); break;
                    case 'translatez': comps.push(new CSSTranslate(zero(), zero(), args[0])); break;
                    case 'translate3d': comps.push(new CSSTranslate(args[0], args[1], args[2])); break;
                    case 'scale': comps.push(new CSSScale(args[0], args.length > 1 ? args[1] : args[0])); break;
                    case 'scalex': comps.push(new CSSScale(args[0], 1)); break;
                    case 'scaley': comps.push(new CSSScale(1, args[0])); break;
                    case 'scalez': comps.push(new CSSScale(1, 1, args[0])); break;
                    case 'scale3d': comps.push(new CSSScale(args[0], args[1], args[2])); break;
                    case 'rotate': comps.push(new CSSRotate(args[0])); break;
                    case 'rotatex': comps.push(new CSSRotate(1, 0, 0, args[0])); break;
                    case 'rotatey': comps.push(new CSSRotate(0, 1, 0, args[0])); break;
                    case 'rotatez': comps.push(new CSSRotate(0, 0, 1, args[0])); break;
                    case 'rotate3d': comps.push(new CSSRotate(args[0], args[1], args[2], args[3])); break;
                    case 'skew': comps.push(new CSSSkew(args[0], args.length > 1 ? args[1] : new CSSUnitValue(0, 'deg'))); break;
                    case 'skewx': comps.push(new CSSSkewX(args[0])); break;
                    case 'skewy': comps.push(new CSSSkewY(args[0])); break;
                    case 'perspective': {
                        var a0 = trimWs(it.args);
                        if (a0.length === 1 && a0[0].t === 'ident' && a0[0].v.toLowerCase() === 'none') { comps.push(new CSSPerspective('none')); }
                        else if (args[0]) { comps.push(new CSSPerspective(args[0])); } else { return null; }
                        break;
                    }
                    case 'matrix': {
                        if (args.length !== 6) { return null; }
                        var nums = args.map(numberOf);
                        comps.push(new CSSMatrixComponent(new DOMMatrixReadOnly(nums), { is2D: true }));
                        break;
                    }
                    case 'matrix3d': {
                        if (args.length !== 16) { return null; }
                        comps.push(new CSSMatrixComponent(new DOMMatrixReadOnly(args.map(numberOf)), { is2D: false }));
                        break;
                    }
                    default: return null;
                }
            } catch (e) { return null; }
        }
        if (comps.length === 0) { return null; }
        return new CSSTransformValue(comps);
    };
    // ---------- the property grammar table ----------
    // codes: L length, P percentage, N number, I integer, A angle, T time, F flex, X transform, G image, C color, R position,
    //        U unsupported (any other value reifies as a base CSSStyleValue), * list-valued, + non-negative.
    // keywords follow '|'.
    var LENGTH_KW = 'auto min-content max-content fit-content';
    var SIDES_KW = 'auto';
    var BORDER_STYLE = 'none hidden dotted dashed solid double groove ridge inset outset';
    var BORDER_WIDTH = 'thin medium thick';
    var BLEND = 'normal multiply screen overlay darken lighten color-dodge color-burn hard-light soft-light difference exclusion hue saturation color luminosity';
    var EASING = 'linear ease ease-in ease-out ease-in-out step-start step-end';
    var BOX = 'border-box padding-box content-box';
    var GEOM_BOX = 'border-box padding-box content-box fill-box stroke-box view-box margin-box';
    var PROPS = {
        'accent-color': 'C|auto', 'alignment-baseline': '|baseline text-bottom alphabetic ideographic middle central mathematical text-top hanging',
        'all': '|', 'anchor-scope': '|none all', 'animation-delay': 'T*|', 'animation-direction': '*|normal reverse alternate alternate-reverse',
        'animation-duration': 'T*+|auto', 'animation-fill-mode': '*|none forwards backwards both', 'animation-iteration-count': 'N*+|infinite',
        'animation-name': 'U*|none', 'animation-play-state': '*|running paused', 'animation-timing-function': 'U*|' + EASING, 'animation': 'U|',
        'backdrop-filter': 'U|none', 'backface-visibility': '|visible hidden', 'background-attachment': '*|scroll fixed local',
        'background-blend-mode': '*|' + BLEND, 'background-clip': '*|' + BOX + ' text', 'background-color': 'C|', 'background-image': 'G*|none',
        'background-origin': '*|' + BOX, 'background-position': 'R*|', 'background-position-x': 'LP*|left center right', 'background-position-y': 'LP*|top center bottom',
        'background-repeat': 'U*|repeat repeat-x repeat-y no-repeat space round', 'background-size': 'LP*+|auto cover contain', 'background': 'U|',
        'baseline-shift': 'LP|sub super', 'block-size': 'LP+|' + LENGTH_KW, 'border-collapse': '|collapse separate', 'border-color': 'C|',
        'border-image-outset': 'LN+|', 'border-image-repeat': '|stretch repeat round space', 'border-image-slice': 'NP+|fill', 'border-image-source': 'G|none',
        'border-image-width': 'LPN+|auto', 'border-radius': 'LP+|', 'border-style': '|' + BORDER_STYLE, 'border-width': 'L+|' + BORDER_WIDTH,
        'border-top-width': 'L+|' + BORDER_WIDTH, 'border-right-width': 'L+|' + BORDER_WIDTH, 'border-bottom-width': 'L+|' + BORDER_WIDTH, 'border-left-width': 'L+|' + BORDER_WIDTH,
        'border-top-style': '|' + BORDER_STYLE, 'border-right-style': '|' + BORDER_STYLE, 'border-bottom-style': '|' + BORDER_STYLE, 'border-left-style': '|' + BORDER_STYLE,
        'border-top-color': 'C|', 'border-right-color': 'C|', 'border-bottom-color': 'C|', 'border-left-color': 'C|',
        'border-top-left-radius': 'LP+|', 'border-top-right-radius': 'LP+|', 'border-bottom-right-radius': 'LP+|', 'border-bottom-left-radius': 'LP+|',
        'border': 'U|', 'border-top': 'U|', 'border-right': 'U|', 'border-bottom': 'U|', 'border-left': 'U|', 'border-image': 'U|',
        'bottom': 'LP|auto', 'top': 'LP|auto', 'left': 'LP|auto', 'right': 'LP|auto', 'inset': 'LP|auto',
        'box-shadow': 'U*|none', 'box-sizing': '|content-box border-box', 'break-after': '|auto avoid always all avoid-page page left right recto verso avoid-column column avoid-region region',
        'break-before': '|auto avoid always all avoid-page page left right recto verso avoid-column column avoid-region region', 'break-inside': '|auto avoid avoid-page avoid-column avoid-region',
        'caption-side': '|top bottom', 'caret-color': 'C|auto', 'clear': '|none left right both inline-start inline-end', 'clip-path': 'U|none ' + GEOM_BOX,
        'clip-rule': '|nonzero evenodd', 'clip': 'U|auto', 'color-interpolation': '|auto srgb linearrgb', 'color': 'C|', 'column-count': 'I+|auto',
        'column-rule-color': 'C|', 'column-rule-style': '|' + BORDER_STYLE, 'column-rule-width': 'L+|' + BORDER_WIDTH, 'column-span': '|none all',
        'column-width': 'L+|auto', 'columns': 'U|', 'contain': 'U|none strict content size inline-size layout style paint', 'container-name': 'U|none',
        'container-type': '|normal size inline-size', 'content': 'U|normal none', 'counter-increment': 'U|none', 'counter-reset': 'U|none', 'counter-set': 'U|none',
        'cursor': 'U|auto default none context-menu help pointer progress wait cell crosshair text vertical-text alias copy move no-drop not-allowed grab grabbing e-resize n-resize ne-resize nw-resize s-resize se-resize sw-resize w-resize ew-resize ns-resize nesw-resize nwse-resize col-resize row-resize all-scroll zoom-in zoom-out',
        'd': 'U|none', 'direction': '|ltr rtl', 'display': '|none block inline inline-block flex inline-flex grid inline-grid flow-root contents list-item table inline-table table-row table-row-group table-header-group table-footer-group table-column table-column-group table-cell table-caption ruby ruby-base ruby-text',
        'dominant-baseline': '|auto text-bottom alphabetic ideographic middle central mathematical hanging text-top', 'empty-cells': '|show hide',
        'fill-opacity': 'NP|', 'fill-rule': '|nonzero evenodd', 'fill': 'U|none', 'filter': 'U|none', 'flex-basis': 'LP+|auto content ' + LENGTH_KW,
        'flex-direction': '|row row-reverse column column-reverse', 'flex-flow': 'U|', 'flex-grow': 'N+|', 'flex-shrink': 'N+|', 'flex-wrap': '|nowrap wrap wrap-reverse', 'flex': 'U|none auto',
        'float': '|left right none inline-start inline-end', 'flood-color': 'C|', 'flood-opacity': 'NP|', 'font-family': 'U*|serif sans-serif monospace cursive fantasy system-ui',
        'font-feature-settings': 'U*|normal', 'font-kerning': '|auto normal none', 'font-language-override': 'U|normal', 'font-optical-sizing': '|auto none', 'font-palette': 'U|normal light dark',
        'font-size-adjust': 'N+|none', 'font-size': 'LP+|xx-small x-small small medium large x-large xx-large xxx-large larger smaller', 'font-stretch': 'P+|normal ultra-condensed extra-condensed condensed semi-condensed semi-expanded expanded extra-expanded ultra-expanded',
        'font-style': 'U|normal italic oblique', 'font-synthesis': 'U|none weight style small-caps', 'font-variant-alternates': 'U|normal historical-forms', 'font-variant-caps': '|normal small-caps all-small-caps petite-caps all-petite-caps unicase titling-caps',
        'font-variant-east-asian': 'U|normal jis78 jis83 jis90 jis04 simplified traditional full-width proportional-width ruby', 'font-variant-emoji': '|normal text emoji unicode',
        'font-variant-ligatures': 'U|normal none common-ligatures no-common-ligatures discretionary-ligatures no-discretionary-ligatures historical-ligatures no-historical-ligatures contextual no-contextual',
        'font-variant-numeric': 'U|normal lining-nums oldstyle-nums proportional-nums tabular-nums diagonal-fractions stacked-fractions ordinal slashed-zero', 'font-variant': 'U|normal none small-caps',
        'font-variation-settings': 'U*|normal', 'font-weight': 'N+|normal bold bolder lighter', 'font': 'U|', 'gap': 'U|', 'row-gap': 'LP+|normal', 'column-gap': 'LP+|normal',
        'grid-area': 'U|auto', 'grid-auto-columns': 'LPF*+|auto min-content max-content', 'grid-auto-rows': 'LPF*+|auto min-content max-content', 'grid-auto-flow': 'U|row column dense',
        'grid-column-start': 'U|auto', 'grid-column-end': 'U|auto', 'grid-row-start': 'U|auto', 'grid-row-end': 'U|auto', 'grid-column': 'U|', 'grid-row': 'U|', 'grid-gap': 'U|',
        'grid-template-areas': 'U|none', 'grid-template-columns': 'U|none', 'grid-template-rows': 'U|none', 'grid-template': 'U|none', 'grid': 'U|none',
        'height': 'LP+|' + LENGTH_KW, 'width': 'LP+|' + LENGTH_KW, 'min-height': 'LP+|' + LENGTH_KW, 'min-width': 'LP+|' + LENGTH_KW, 'max-height': 'LP+|none ' + LENGTH_KW, 'max-width': 'LP+|none ' + LENGTH_KW,
        'inline-size': 'LP+|' + LENGTH_KW, 'min-block-size': 'LP+|' + LENGTH_KW, 'min-inline-size': 'LP+|' + LENGTH_KW, 'max-block-size': 'LP+|none ' + LENGTH_KW, 'max-inline-size': 'LP+|none ' + LENGTH_KW,
        'hyphens': '|none manual auto', 'image-rendering': '|auto smooth high-quality crisp-edges pixelated optimizespeed optimizequality', 'isolation': '|auto isolate',
        'letter-spacing': 'L|normal', 'lighting-color': 'C|', 'line-break': '|auto loose normal strict anywhere', 'line-height': 'LPN+|normal',
        'list-style-image': 'G|none', 'list-style-position': '|inside outside', 'list-style-type': 'U|none disc circle square decimal', 'list-style': 'U|',
        'margin': 'U|', 'margin-top': 'LP|auto', 'margin-right': 'LP|auto', 'margin-bottom': 'LP|auto', 'margin-left': 'LP|auto',
        'margin-block-start': 'LP|auto', 'margin-block-end': 'LP|auto', 'margin-inline-start': 'LP|auto', 'margin-inline-end': 'LP|auto',
        'padding': 'U|', 'padding-top': 'LP+|', 'padding-right': 'LP+|', 'padding-bottom': 'LP+|', 'padding-left': 'LP+|',
        'padding-block-start': 'LP+|', 'padding-block-end': 'LP+|', 'padding-inline-start': 'LP+|', 'padding-inline-end': 'LP+|',
        'inset-block-start': 'LP|auto', 'inset-block-end': 'LP|auto', 'inset-inline-start': 'LP|auto', 'inset-inline-end': 'LP|auto',
        'marker': 'U|none', 'marker-start': 'U|none', 'marker-mid': 'U|none', 'marker-end': 'U|none', 'mask-image': 'G*|none', 'mask-type': '|luminance alpha', 'mask': 'U|none',
        'mix-blend-mode': '|' + BLEND, 'object-fit': '|fill contain cover none scale-down', 'object-position': 'R|', 'offset-anchor': 'R|auto', 'offset-distance': 'LP|',
        'offset-path': 'U|none', 'offset-position': 'R|auto normal', 'offset-rotate': 'A|auto reverse', 'offset': 'U|', 'opacity': 'NP|', 'order': 'I|', 'orphans': 'I+|',
        'outline-color': 'C|invert', 'outline-offset': 'L|', 'outline-style': '|auto ' + BORDER_STYLE, 'outline-width': 'L+|' + BORDER_WIDTH, 'outline': 'U|',
        'overflow-anchor': '|auto none', 'overflow-clip-margin': 'L+|', 'overflow-wrap': '|normal break-word anywhere', 'overflow': '|visible hidden clip scroll auto',
        'overflow-x': '|visible hidden clip scroll auto', 'overflow-y': '|visible hidden clip scroll auto', 'overscroll-behavior': '|auto contain none',
        'overscroll-behavior-x': '|auto contain none', 'overscroll-behavior-y': '|auto contain none', 'page': 'U|auto', 'paint-order': 'U|normal fill stroke markers',
        'perspective-origin': 'R|', 'perspective': 'L+|none', 'pointer-events': '|auto none visiblepainted visiblefill visiblestroke visible painted fill stroke all bounding-box',
        'position-visibility': 'U|always anchors-valid anchors-visible no-overflow', 'position': '|static relative absolute sticky fixed', 'quotes': 'U|none auto',
        'resize': '|none both horizontal vertical block inline', 'scroll-behavior': '|auto smooth', 'scroll-margin': 'U|', 'scroll-margin-top': 'L|', 'scroll-margin-right': 'L|', 'scroll-margin-bottom': 'L|', 'scroll-margin-left': 'L|',
        'scroll-padding': 'U|auto', 'scroll-padding-top': 'LP+|auto', 'scroll-padding-right': 'LP+|auto', 'scroll-padding-bottom': 'LP+|auto', 'scroll-padding-left': 'LP+|auto',
        'scroll-snap-align': 'U|none start end center', 'scroll-snap-stop': '|normal always', 'scroll-snap-type': 'U|none x y block inline both mandatory proximity',
        'scrollbar-gutter': 'U|auto stable both-edges', 'scrollbar-width': '|auto thin none', 'shape-image-threshold': 'N|', 'shape-margin': 'LP+|', 'shape-outside': 'U|none',
        'shape-rendering': '|auto optimizespeed crispedges geometricprecision', 'speak': '|auto never always', 'stop-color': 'C|', 'stop-opacity': 'NP|', 'stroke-dasharray': 'U|none',
        'stroke-dashoffset': 'LP|', 'stroke-linecap': '|butt round square', 'stroke-linejoin': '|miter round bevel', 'stroke-miterlimit': 'N+|', 'stroke-opacity': 'NP|', 'stroke-width': 'LP+|', 'stroke': 'U|none',
        'tab-size': 'LN+|', 'table-layout': '|auto fixed', 'text-align-last': '|auto start end left right center justify', 'text-align': '|start end left right center justify match-parent',
        'text-anchor': '|start middle end', 'text-autospace': 'U|normal no-autospace ideograph-alpha ideograph-numeric', 'text-box-trim': '|none trim-start trim-end trim-both',
        'text-combine-upright': 'U|none all', 'text-decoration-color': 'C|', 'text-decoration-line': 'U|none underline overline line-through blink spelling-error grammar-error',
        'text-decoration-skip-ink': '|auto none all', 'text-decoration-skip': 'U|none objects spaces edges box-decoration', 'text-decoration-style': '|solid double dotted dashed wavy',
        'text-decoration-thickness': 'LP|auto from-font', 'text-decoration': 'U|', 'text-emphasis-color': 'C|', 'text-emphasis-position': 'U|over under left right', 'text-indent': 'LP|',
        'text-justify': '|auto none inter-word inter-character', 'text-orientation': '|mixed upright sideways', 'text-overflow': '|clip ellipsis', 'text-rendering': '|auto optimizespeed optimizelegibility geometricprecision',
        'text-shadow': 'U*|none', 'text-size-adjust': 'P+|none auto', 'text-transform': 'U|none capitalize uppercase lowercase full-width full-size-kana', 'text-underline-offset': 'LP|auto',
        'text-underline-position': 'U|auto under left right', 'touch-action': 'U|auto none pan-x pan-left pan-right pan-y pan-up pan-down pinch-zoom manipulation',
        'transform-box': '|' + GEOM_BOX, 'transform-style': '|flat preserve-3d', 'transform': 'X|none', 'transform-origin': 'R|', 'translate': 'U|none', 'rotate': 'U|none', 'scale': 'U|none',
        'transition-delay': 'T*|', 'transition-duration': 'T*+|', 'transition-property': 'U*|none all', 'transition-timing-function': 'U*|' + EASING, 'transition': 'U|',
        'unicode-bidi': '|normal embed isolate bidi-override isolate-override plaintext', 'user-select': '|auto text none contain all', 'vector-effect': '|none non-scaling-stroke',
        'vertical-align': 'LP|baseline sub super text-top text-bottom middle top bottom', 'visibility': '|visible hidden collapse',
        'white-space': 'U|normal pre nowrap pre-wrap pre-line break-spaces', 'widows': 'I+|', 'will-change': 'U|auto', 'word-break': '|normal break-all keep-all break-word',
        'word-spacing': 'LP|normal', 'word-wrap': '|normal break-word anywhere', 'writing-mode': '|horizontal-tb vertical-rl vertical-lr sideways-rl sideways-lr', 'z-index': 'I|auto',
        'zoom': 'NP+|normal', 'align-content': 'U|normal', 'align-items': 'U|normal', 'align-self': 'U|auto', 'justify-content': 'U|normal', 'justify-items': 'U|normal', 'justify-self': 'U|auto',
        'aspect-ratio': 'U|auto', 'appearance': 'U|none auto'
    };
    var grammar = function (prop) {
        var g = PROPS[prop];
        if (g === undefined) { return null; }
        var bar = g.indexOf('|');
        var codes = g.slice(0, bar), kws = g.slice(bar + 1);
        return { codes: codes, list: codes.indexOf('*') >= 0, nonneg: codes.indexOf('+') >= 0, keywords: kws === '' ? [] : kws.split(' '), any: codes.indexOf('U') >= 0 };
    };
    var typeAccepted = function (g, t) {
        var keys = typeKeys(t);
        var c = g.codes;
        if (keys.length === 0) { return c.indexOf('N') >= 0 || c.indexOf('I') >= 0; }
        if (keys.length === 1) {
            var k = keys[0];
            if (t[k] !== 1) { return false; }
            if (k === 'length') { return c.indexOf('L') >= 0; }
            if (k === 'percent') { return c.indexOf('P') >= 0; }
            if (k === 'angle') { return c.indexOf('A') >= 0; }
            if (k === 'time') { return c.indexOf('T') >= 0; }
            if (k === 'flex') { return c.indexOf('F') >= 0; }
            return false;
        }
        if (keys.length === 2 && t.length === 1 && t.percent === 1) { return c.indexOf('L') >= 0 && c.indexOf('P') >= 0; }
        return false;
    };
    var isCustom = function (p) { return p.indexOf('--') === 0; };
    // serialize a value for a declaration, applying range wrapping (an out-of-range number becomes calc())
    var serializeFor = function (prop, g, v) {
        if (v instanceof CSSUnitValue && !isCustom(prop)) {
            var neg = v.value < 0 && g.nonneg;
            var nonInt = g.codes.indexOf('I') >= 0 && g.codes.indexOf('N') < 0 && v.unit === 'number' && !Number.isInteger(v.value);
            if (neg || nonInt) { return 'calc(' + v.toString() + ')'; }
        }
        return v.toString();
    };
    // §3.x "check a value for the property": the serialized text, or throws TypeError
    var checkValue = function (prop, g, v, listCtx) {
        if (typeof v === 'string') {
            var items = trimWs(group(tokenize(v)));
            if (items.length === 0) { throw new TypeError('Invalid value: empty'); }
            if (!g.list && splitTop(items, ',').length > 1 && !isCustom(prop)) { throw new TypeError('Invalid value: a list for a non-list property'); }
            if (isCustom(prop) || g.any || hasVar(items)) { return v; }
            var parts = g.list ? splitTop(items, ',') : [items];
            for (var i = 0; i < parts.length; i++) {
                if (reifyOne(prop, g, trimWs(parts[i]), false) === null) { throw new TypeError("Invalid value '" + v + "' for property '" + prop + "'"); }
            }
            return v;
        }
        if (!(v instanceof CSSStyleValue)) { throw new TypeError('The provided value is not a CSSStyleValue or a string'); }
        if (isCustom(prop)) {
            if (v instanceof CSSUnparsedValue) { return v.toString(); }
            throw new TypeError('A custom property takes a CSSUnparsedValue');
        }
        if (v instanceof CSSUnparsedValue) { return v.toString(); }
        if (v instanceof CSSKeywordValue) {
            var kw = v.value.toLowerCase();
            if (CSS_WIDE.indexOf(kw) >= 0 || g.keywords.indexOf(kw) >= 0 || (g.any && /^[A-Za-z_-][A-Za-z0-9_-]*$/.test(v.value))) { return v.toString(); }
            throw new TypeError("Invalid keyword '" + v.value + "' for property '" + prop + "'");
        }
        if (v instanceof CSSNumericValue) {
            if (!typeAccepted(g, numType(v))) { throw new TypeError("Invalid numeric type for property '" + prop + "'"); }
            return serializeFor(prop, g, v);
        }
        if (v instanceof CSSTransformValue) {
            if (g.codes.indexOf('X') < 0) { throw new TypeError("A transform is not valid for property '" + prop + "'"); }
            return v.toString();
        }
        if (v instanceof CSSImageValue) {
            if (g.codes.indexOf('G') < 0) { throw new TypeError("An image is not valid for property '" + prop + "'"); }
            return v.toString();
        }
        var assoc = ASSOC.get(v);
        if (assoc) {
            if (assoc.property !== null && assoc.property !== prop) { throw new TypeError("The value is associated with property '" + assoc.property + "'"); }
            return assoc.text;
        }
        throw new TypeError('Invalid value');
    };
    // reify one declared component value for prop (items: grouped tokens, no leading/trailing ws); null = invalid
    var reifyOne = function (prop, g, items, computed) {
        if (items.length === 0) { return null; }
        var text = serializeItems(items).trim();
        if (items.length === 1 && items[0].t === 'ident') {
            var kw = items[0].v.toLowerCase();
            if (CSS_WIDE.indexOf(kw) >= 0) { return new CSSKeywordValue(kw); }
            if (g.keywords.indexOf(kw) >= 0) { return new CSSKeywordValue(items[0].v); }
            if (g.any) { return baseValue(text, prop); }
            return null;
        }
        var num = parseMathTree(items, false);
        if (num !== null) {
            if (typeAccepted(g, numType(num))) { return computed ? computeNumeric(prop, g, num) : num; }
            if (g.any) { return baseValue(text, prop); }
            return null;
        }
        if (g.codes.indexOf('X') >= 0) {
            var tv = reifyTransform(items);
            if (tv !== null) { return tv; }
        }
        if (g.codes.indexOf('G') >= 0 && items.length === 1 && (items[0].t === 'url' || (items[0].t === 'function' && /gradient$/i.test(items[0].v)))) {
            return new CSSImageValue(TOKEN, text);
        }
        if (g.codes.indexOf('C') >= 0) { return baseValue(text, prop); }
        if (g.codes.indexOf('R') >= 0) { return baseValue(text, prop); }
        if (g.any) { return baseValue(text, prop); }
        // a value the grammar table does not name but the engine accepted: unsupported, bound to the property
        return baseValue(text, prop);
    };
    var computeEl = null;
    var fontSizeOf = function () { try { return computeEl ? parseFloat(getComputedStyle(computeEl).getPropertyValue('font-size')) || 16 : 16; } catch (e) { return 16; } };
    var relativeToPx = function (uv) {
        var u = uv.unit, v = uv.value;
        var fs = fontSizeOf();
        switch (u) {
            case 'em': case 'lh': return v * fs;
            case 'rem': case 'rlh': return v * 16;
            case 'ex': case 'ch': case 'ic': case 'cap': case 'rex': case 'rch': case 'ric': case 'rcap': return v * fs * 0.5;
            case 'vw': case 'svw': case 'lvw': case 'dvw': case 'vi': case 'svi': case 'lvi': case 'dvi': case 'cqw': case 'cqi': return v * window.innerWidth / 100;
            case 'vh': case 'svh': case 'lvh': case 'dvh': case 'vb': case 'svb': case 'lvb': case 'dvb': case 'cqh': case 'cqb': return v * window.innerHeight / 100;
            case 'vmin': case 'svmin': case 'lvmin': case 'dvmin': case 'cqmin': return v * Math.min(window.innerWidth, window.innerHeight) / 100;
            case 'vmax': case 'svmax': case 'lvmax': case 'dvmax': case 'cqmax': return v * Math.max(window.innerWidth, window.innerHeight) / 100;
            default: return FACTOR[u] !== undefined ? v * FACTOR[u] : v;
        }
    };
    // the computed value of a numeric: absolute canonical units, a resolved calc where every term resolves
    var computeNumeric = function (prop, g, v) {
        var absolute = function (x) {
            if (x instanceof CSSUnitValue) {
                var b = UNITS[x.unit];
                if (x.unit === 'number' || x.unit === 'percent') { return x; }
                if (b === 'length') { return new CSSUnitValue(relativeToPx(x), 'px'); }
                return new CSSUnitValue(x.value * FACTOR[x.unit], CANONICAL[b]);
            }
            if (x instanceof CSSMathNegate) { return new CSSMathNegate(absolute(x.value)); }
            if (x instanceof CSSMathInvert) { return new CSSMathInvert(absolute(x.value)); }
            if (x instanceof CSSMathClamp) { return new CSSMathClamp(absolute(x.lower), absolute(x.value), absolute(x.upper)); }
            if (x instanceof CSSMathValue) { return ctor(x.constructor, Array.from(x.values).map(absolute)); }
            return x;
        };
        var a = absolute(v);
        var sum = createSum(a);
        var out = a;
        if (sum !== null && sum.length === 1) { out = new CSSUnitValue(sum[0].value, sum[0].unit); }
        if (out instanceof CSSUnitValue) {
            var val = out.value;
            if (g.nonneg && val < 0) { val = 0; }
            if (g.codes.indexOf('I') >= 0 && g.codes.indexOf('N') < 0 && out.unit === 'number') { val = Math.round(val); }
            out = new CSSUnitValue(val, out.unit);
        }
        return out;
    };
    // reify a declaration's value text for prop; returns a list of values (list-valued) or [value]; [] when none
    var reifyText = function (prop, text, computed) {
        if (text === null || text === undefined) { return []; }
        var t = String(text).trim();
        if (t === '') { return []; }
        if (isCustom(prop)) { return [reifyUnparsed(group(tokenize(text)))]; }
        var g = grammar(prop);
        if (g === null) { return [baseValue(t, prop)]; }
        var items = trimWs(group(tokenize(t)));
        if (items.length === 1 && items[0].t === 'ident' && CSS_WIDE.indexOf(items[0].v.toLowerCase()) >= 0) { return [new CSSKeywordValue(items[0].v.toLowerCase())]; }
        if (hasVar(items)) { return [reifyUnparsed(items)]; }
        if (g.any && g.codes.length <= 2 && g.keywords.length === 0) { return [baseValue(t, prop)]; }
        var parts = g.list ? splitTop(items, ',') : [items];
        var out = [];
        for (var i = 0; i < parts.length; i++) {
            var v = reifyOne(prop, g, trimWs(parts[i]), computed);
            out.push(v === null ? baseValue(serializeItems(trimWs(parts[i])).trim(), prop) : v);
        }
        return out;
    };
    var parseValue = function (property, cssText, all) {
        var prop = String(property);
        if (!isCustom(prop)) { prop = prop.toLowerCase(); if (grammar(prop) === null) { throw new TypeError("Failed to execute 'parse' on 'CSSStyleValue': Invalid propertyName: " + property); } }
        var text = String(cssText);
        var items = trimWs(group(tokenize(text)));
        if (items.length === 0) { throw new TypeError("Failed to execute 'parse' on 'CSSStyleValue': Invalid value"); }
        if (!isCustom(prop)) {
            var g = grammar(prop);
            if (!hasVar(items) && !g.any && !(items.length === 1 && items[0].t === 'ident' && CSS_WIDE.indexOf(items[0].v.toLowerCase()) >= 0)) {
                var parts = g.list ? splitTop(items, ',') : [items];
                if (!g.list && parts.length > 1) { throw new TypeError("Failed to execute 'parse' on 'CSSStyleValue': Invalid value"); }
                for (var i = 0; i < parts.length; i++) {
                    var r = reifyOne(prop, g, trimWs(parts[i]), false);
                    if (r === null || (ASSOC.get(r) && !g.any && g.codes.indexOf('C') < 0 && g.codes.indexOf('R') < 0)) { throw new TypeError("Failed to execute 'parse' on 'CSSStyleValue': Invalid value: " + cssText); }
                }
            }
        }
        var list = reifyText(prop, text, false);
        return all ? list : list[0];
    };
    // ---------- StylePropertyMap (§3) ----------
    var MAPS = new WeakMap();
    var declOf = function (map) {
        var m = MAPS.get(map);
        if (!m) { throw new TypeError('Illegal invocation'); }
        return m;
    };
    var normalizeProp = function (p, forMap) {
        var prop = String(p);
        if (isCustom(prop)) { return prop; }
        prop = prop.toLowerCase();
        if (grammar(prop) === null) { throw new TypeError("Invalid propertyName: " + p); }
        return prop;
    };
    var declText = function (m, prop) {
        if (m.kind === 'computed') {
            computeEl = m.el;
            var cs = getComputedStyle(m.el);
            var v = cs.getPropertyValue(prop);
            if ((v === '' || v === undefined) && !isCustom(prop)) {
                var g0 = grammar(prop);
                if (g0 !== null && !m.el.isConnected) { return ''; }
                v = cs.getPropertyValue(prop) || '';
            }
            return v;
        }
        return m.style.getPropertyValue(prop);
    };
    var listFor = function (map, prop, computed) {
        var m = declOf(map);
        var text = declText(m, prop);
        if (m.kind === 'computed' && (text === '' || text === undefined)) {
            var g = grammar(prop);
            if (g && g.keywords.length === 0 && g.codes.indexOf('L') >= 0) { text = '0px'; }
        }
        return reifyText(prop, text, m.kind === 'computed');
    };
    class StylePropertyMapReadOnly {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } }
        get(property) {
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'get' on 'StylePropertyMapReadOnly': 1 argument required, but only 0 present."); }
            var prop = normalizeProp(property);
            var list = listFor(this, prop);
            return list.length ? list[0] : undefined;
        }
        getAll(property) {
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'getAll' on 'StylePropertyMapReadOnly': 1 argument required, but only 0 present."); }
            var prop = normalizeProp(property);
            return listFor(this, prop);
        }
        has(property) {
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'has' on 'StylePropertyMapReadOnly': 1 argument required, but only 0 present."); }
            var prop = normalizeProp(property);
            return listFor(this, prop).length > 0;
        }
        get size() { return this.__entries().length; }
        __entries() {
            var m = declOf(this);
            var names = [];
            if (m.kind === 'computed') {
                var cs = getComputedStyle(m.el);
                for (var i = 0; i < cs.length; i++) { names.push(cs[i]); }
                Object.keys(PROPS).forEach(function (p) { if (names.indexOf(p) < 0 && !grammar(p).any && grammar(p).keywords.length + grammar(p).codes.length > 0) { names.push(p); } });
                names = names.filter(function (p) { return grammar(p) !== null || isCustom(p); });
            } else {
                for (var j = 0; j < m.style.length; j++) { names.push(m.style[j]); }
            }
            var out = [];
            var self = this;
            names.forEach(function (p) {
                var list = listFor(self, p);
                if (list.length) { out.push([p, list]); }
            });
            out.sort(function (a, b) { var ca = isCustom(a[0]), cb = isCustom(b[0]); if (ca !== cb) { return ca ? 1 : -1; } return a[0] < b[0] ? -1 : (a[0] > b[0] ? 1 : 0); });
            return out;
        }
        entries() { return this.__entries()[Symbol.iterator](); }
        keys() { return this.__entries().map(function (e) { return e[0]; })[Symbol.iterator](); }
        values() { return this.__entries().map(function (e) { return e[1]; })[Symbol.iterator](); }
        forEach(f, thisArg) { var self = this; this.__entries().forEach(function (e) { f.call(thisArg, e[1], e[0], self); }); }
        [Symbol.iterator]() { return this.entries(); }
    }
    Object.defineProperty(StylePropertyMapReadOnly.prototype, '__entries', { enumerable: false });
    class StylePropertyMap extends StylePropertyMapReadOnly {
        constructor(tok) { super(tok); }
        set(property) {
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'set' on 'StylePropertyMap': 1 argument required, but only 0 present."); }
            var m = declOf(this);
            var prop = normalizeProp(property);
            var values = Array.prototype.slice.call(arguments, 1);
            var g = isCustom(prop) ? { codes: '', list: false, nonneg: false, keywords: [], any: true } : grammar(prop);
            if (values.length === 0) { throw new TypeError("Failed to execute 'set' on 'StylePropertyMap': no values"); }
            if (!g.list && values.length > 1 && !isCustom(prop)) { throw new TypeError("Failed to execute 'set' on 'StylePropertyMap': The property '" + prop + "' is not list-valued"); }
            if (values.length > 1) {
                var unparsed = values.some(function (v) { return v instanceof CSSUnparsedValue || (typeof v === 'string' && hasVar(group(tokenize(v)))); });
                if (unparsed) { throw new TypeError("Failed to execute 'set' on 'StylePropertyMap': A CSSUnparsedValue or var() cannot be mixed with other values"); }
            }
            var texts = values.map(function (v) { return checkValue(prop, g, v, g.list); });
            m.style.setProperty(prop, texts.join(', '));
        }
        append(property) {
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'append' on 'StylePropertyMap': 1 argument required, but only 0 present."); }
            var m = declOf(this);
            var prop = normalizeProp(property);
            if (isCustom(prop)) { throw new TypeError("Failed to execute 'append' on 'StylePropertyMap': Cannot append to a custom property"); }
            var g = grammar(prop);
            if (!g.list) { throw new TypeError("Failed to execute 'append' on 'StylePropertyMap': The property '" + prop + "' is not list-valued"); }
            var values = Array.prototype.slice.call(arguments, 1);
            if (values.some(function (v) { return v instanceof CSSUnparsedValue || (typeof v === 'string' && hasVar(group(tokenize(v)))); })) { throw new TypeError("Failed to execute 'append' on 'StylePropertyMap': Cannot append a CSSUnparsedValue"); }
            var current = m.style.getPropertyValue(prop);
            if (hasVar(group(tokenize(current)))) { throw new TypeError("Failed to execute 'append' on 'StylePropertyMap': The current value holds var()"); }
            var texts = values.map(function (v) { return checkValue(prop, g, v, true); });
            var all = (current === '' ? [] : [current]).concat(texts);
            m.style.setProperty(prop, all.join(', '));
        }
        delete(property) {
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'delete' on 'StylePropertyMap': 1 argument required, but only 0 present."); }
            var m = declOf(this);
            var prop = normalizeProp(property);
            m.style.removeProperty(prop);
        }
        clear() {
            var m = declOf(this);
            var names = [];
            for (var i = 0; i < m.style.length; i++) { names.push(m.style[i]); }
            names.forEach(function (p) { m.style.removeProperty(p); });
        }
    }
    var ATTR_MAPS = new WeakMap();
    accessor(Element.prototype, 'attributeStyleMap', function () {
        var m = ATTR_MAPS.get(this);
        if (!m) {
            m = new StylePropertyMap(TOKEN);
            MAPS.set(m, { kind: 'inline', style: this.style, el: this });
            ATTR_MAPS.set(this, m);
        }
        return m;
    });
    var COMPUTED_MAPS = new WeakMap();
    def(Element.prototype, 'computedStyleMap', function computedStyleMap() {
        var m = COMPUTED_MAPS.get(this);
        if (!m) {
            m = new StylePropertyMapReadOnly(TOKEN);
            MAPS.set(m, { kind: 'computed', el: this });
            COMPUTED_MAPS.set(this, m);
        }
        return m;
    });
    if (typeof CSSStyleRule === 'function') {
        var RULE_MAPS = new WeakMap();
        accessor(CSSStyleRule.prototype, 'styleMap', function () {
            var m = RULE_MAPS.get(this);
            if (!m) {
                m = new StylePropertyMap(TOKEN);
                MAPS.set(m, { kind: 'rule', style: this.style, el: null });
                RULE_MAPS.set(this, m);
            }
            return m;
        });
    }
    // ---------- the CSS namespace factories (§5.1) ----------
    var CSSNS = typeof globalThis.CSS === 'object' && globalThis.CSS !== null ? globalThis.CSS : {};
    if (globalThis.CSS !== CSSNS) { hide(globalThis, 'CSS', CSSNS); }
    Object.keys(UNITS).forEach(function (u) {
        var name = u === 'q' ? 'Q' : (u === 'hz' ? 'Hz' : (u === 'khz' ? 'kHz' : u));
        if (u === 'x') { return; }
        Object.defineProperty(CSSNS, name, { value: function (v) { return new CSSUnitValue(Number(v), u); }, writable: true, configurable: true, enumerable: true });
    });
    // ---------- expose ----------
    [CSSStyleValue, CSSKeywordValue, CSSNumericValue, CSSUnitValue, CSSMathValue, CSSMathSum, CSSMathProduct, CSSMathNegate, CSSMathInvert, CSSMathMin, CSSMathMax, CSSMathClamp, CSSNumericArray,
     CSSUnparsedValue, CSSVariableReferenceValue, CSSImageValue, CSSTransformValue, CSSTransformComponent, CSSTranslate, CSSRotate, CSSScale, CSSSkew, CSSSkewX, CSSSkewY, CSSPerspective, CSSMatrixComponent,
     StylePropertyMapReadOnly, StylePropertyMap].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        hide(globalThis, C.name, C);
    });
})();
"#;
