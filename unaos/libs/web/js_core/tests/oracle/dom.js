// A tiny DOM for the js_core oracle host: just enough of Node / Element / Text / Document for the oracle pages,
// with HTML serialization matching the HTML Standard's fragment serializer (what Chromium's --dump-dom prints).
(function (global) {
  "use strict";
  var VOID = { area: 1, base: 1, br: 1, col: 1, embed: 1, hr: 1, img: 1, input: 1, link: 1, meta: 1, source: 1, track: 1, wbr: 1 };
  var RAW = { script: 1, style: 1 };
  function Node() {}
  Node.prototype.appendChild = function (c) { return this.insertBefore(c, null); };
  Node.prototype.insertBefore = function (c, ref) {
    if (c.parentNode) c.parentNode.removeChild(c);
    var i = ref === null || ref === undefined ? this.childNodes.length : this.childNodes.indexOf(ref);
    if (i < 0) throw new Error("NotFoundError");
    this.childNodes.splice(i, 0, c);
    c.parentNode = this;
    return c;
  };
  Node.prototype.removeChild = function (c) {
    var i = this.childNodes.indexOf(c);
    if (i < 0) throw new Error("NotFoundError");
    this.childNodes.splice(i, 1);
    c.parentNode = null;
    return c;
  };
  Object.defineProperty(Node.prototype, "firstChild", { get: function () { return this.childNodes[0] || null; } });
  Object.defineProperty(Node.prototype, "lastChild", { get: function () { return this.childNodes[this.childNodes.length - 1] || null; } });
  Object.defineProperty(Node.prototype, "textContent", {
    get: function () { return this.childNodes.map(function (c) { return c.textContent; }).join(""); },
    set: function (v) { this.childNodes.forEach(function (c) { c.parentNode = null; }); this.childNodes = []; if (String(v) !== "") this.appendChild(new Text(String(v))); }
  });
  function Text(data) { this.data = data; this.parentNode = null; this.childNodes = []; this.nodeType = 3; }
  Text.prototype = Object.create(Node.prototype);
  Object.defineProperty(Text.prototype, "textContent", { get: function () { return this.data; }, set: function (v) { this.data = String(v); } });
  function Element(tag) { this.tagName = tag.toUpperCase(); this.localName = tag; this.attrs = []; this.childNodes = []; this.parentNode = null; this.nodeType = 1; }
  Element.prototype = Object.create(Node.prototype);
  Element.prototype.getAttribute = function (n) { for (var i = 0; i < this.attrs.length; i++) if (this.attrs[i][0] === n) return this.attrs[i][1]; return null; };
  Element.prototype.setAttribute = function (n, v) {
    n = String(n).toLowerCase(); v = String(v);
    for (var i = 0; i < this.attrs.length; i++) if (this.attrs[i][0] === n) { this.attrs[i][1] = v; return; }
    this.attrs.push([n, v]);
  };
  Element.prototype.removeAttribute = function (n) { this.attrs = this.attrs.filter(function (a) { return a[0] !== n; }); };
  Object.defineProperty(Element.prototype, "id", { get: function () { return this.getAttribute("id") || ""; }, set: function (v) { this.setAttribute("id", v); } });
  Object.defineProperty(Element.prototype, "className", { get: function () { return this.getAttribute("class") || ""; }, set: function (v) { this.setAttribute("class", v); } });
  Object.defineProperty(Element.prototype, "children", { get: function () { return this.childNodes.filter(function (c) { return c.nodeType === 1; }); } });
  function walk(n, f) { n.childNodes.forEach(function (c) { if (c.nodeType === 1) { f(c); walk(c, f); } }); }
  function matches(e, sel) {
    if (sel[0] === "#") return e.id === sel.slice(1);
    if (sel[0] === ".") return (" " + e.className + " ").indexOf(" " + sel.slice(1) + " ") >= 0;
    return e.localName === sel.toLowerCase();
  }
  Element.prototype.getElementsByTagName = function (t) { var out = []; walk(this, function (e) { if (e.localName === t.toLowerCase()) out.push(e); }); return out; };
  Element.prototype.querySelectorAll = function (sel) { var out = []; walk(this, function (e) { if (matches(e, sel)) out.push(e); }); return out; };
  Element.prototype.querySelector = function (sel) { return this.querySelectorAll(sel)[0] || null; };
  function escText(s) { return s.replace(/&/g, "&amp;").replace(/ /g, "&nbsp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"); }
  function escAttr(s) { return s.replace(/&/g, "&amp;").replace(/ /g, "&nbsp;").replace(/"/g, "&quot;"); }
  function serialize(n, rawParent) {
    if (n.nodeType === 3) return rawParent ? n.data : escText(n.data);
    var s = "<" + n.localName;
    n.attrs.forEach(function (a) { s += " " + a[0] + '="' + escAttr(a[1]) + '"'; });
    s += ">";
    if (VOID[n.localName]) return s;
    var raw = !!RAW[n.localName];
    n.childNodes.forEach(function (c) { s += serialize(c, raw); });
    return s + "</" + n.localName + ">";
  }
  Object.defineProperty(Element.prototype, "outerHTML", { get: function () { return serialize(this, false); } });
  Object.defineProperty(Element.prototype, "innerHTML", { get: function () { var raw = !!RAW[this.localName]; return this.childNodes.map(function (c) { return serialize(c, raw); }).join(""); } });
  function build(t) {
    if (typeof t === "string") return new Text(t);
    var e = new Element(t[0]);
    t[1].forEach(function (a) { e.attrs.push([a[0], a[1]]); });
    t[2].forEach(function (c) { e.appendChild(build(c)); });
    return e;
  }
  var document = {
    createElement: function (t) { return new Element(String(t).toLowerCase()); },
    createTextNode: function (d) { return new Text(String(d)); },
    getElementById: function (id) { var r = null; walk({ childNodes: [this.documentElement] }, function (e) { if (r === null && e.id === id) r = e; }); return r; },
    querySelector: function (sel) { return matches(this.documentElement, sel) ? this.documentElement : this.documentElement.querySelector(sel); },
    querySelectorAll: function (sel) { return this.documentElement.querySelectorAll(sel); },
    getElementsByTagName: function (t) { return this.documentElement.getElementsByTagName(t); }
  };
  Object.defineProperty(document, "body", { get: function () { return this.documentElement.querySelector("body"); } });
  Object.defineProperty(document, "head", { get: function () { return this.documentElement.querySelector("head"); } });
  global.document = document;
  global.__domBuild = function (tree) { document.documentElement = build(tree); };
  global.__domDump = function () { return document.documentElement.outerHTML; };
})(globalThis);
