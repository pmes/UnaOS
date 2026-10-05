//! A small arena DOM implementing css_core's `Element` trait (the test stand-in for html_core).
#![allow(dead_code)]
use super::json::Json;
use css_core::matching::{html_state, Element};
use css_core::selectors::ElementState;

#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub ns: String,
    /// (namespace, local name, value)
    pub attrs: Vec<(String, String, String)>,
    pub has_text: bool,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub index: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Dom {
    pub nodes: Vec<Node>,
    /// Node 0 is the document element (false: a detached subtree).
    pub in_document: bool,
    /// The id the URL fragment names (`:target`).
    pub target: Option<String>,
    /// Ids of elements in user-action states (for cascade tests); empty by default.
    pub hover: Vec<usize>,
}

impl Dom {
    /// From the generator's JSON element tree `{n, ns, a: [[ns, name, value]], t, c: [...]}`.
    pub fn from_json(j: &Json) -> Dom {
        let mut d = Dom { in_document: true, ..Default::default() };
        d.add(j, None);
        d
    }

    fn add(&mut self, j: &Json, parent: Option<usize>) -> usize {
        let i = self.nodes.len();
        let attrs = j
            .get("a")
            .map(|a| a.arr().iter().map(|x| (x.arr()[0].str().to_string(), x.arr()[1].str().to_string(), x.arr()[2].str().to_string())).collect())
            .unwrap_or_default();
        let index = parent.map(|p| self.nodes[p].children.len()).unwrap_or(0);
        self.nodes.push(Node {
            name: j.get("n").map(|n| n.str().to_string()).unwrap_or_default(),
            ns: j.get("ns").map(|n| n.str().to_string()).unwrap_or_default(),
            attrs,
            has_text: matches!(j.get("t"), Some(Json::Bool(true))),
            parent,
            children: Vec::new(),
            index,
        });
        if let Some(p) = parent {
            self.nodes[p].children.push(i);
        }
        if let Some(c) = j.get("c") {
            for ch in c.arr() {
                self.add(ch, Some(i));
            }
        }
        i
    }

    /// Append a new element under `parent` (or as the root).
    pub fn push(&mut self, parent: Option<usize>, name: &str, ns: &str, attrs: Vec<(String, String, String)>) -> usize {
        let i = self.nodes.len();
        let index = parent.map(|p| self.nodes[p].children.len()).unwrap_or(0);
        self.nodes.push(Node { name: name.into(), ns: ns.into(), attrs, has_text: false, parent, children: vec![], index });
        if let Some(p) = parent {
            self.nodes[p].children.push(i);
        }
        i
    }

    /// Deep-copy the subtree at `src` of `from` under `parent` of `self`; `mark` adds an attribute to each copy.
    pub fn copy_subtree(&mut self, from: &Dom, src: usize, parent: Option<usize>, mark: Option<&str>) -> usize {
        let n = &from.nodes[src];
        let mut attrs = n.attrs.clone();
        if let Some(m) = mark {
            attrs.push((String::new(), m.to_string(), String::new()));
        }
        let i = self.push(parent, &n.name.clone(), &n.ns.clone(), attrs);
        self.nodes[i].has_text = n.has_text;
        for &c in &from.nodes[src].children {
            self.copy_subtree(from, c, Some(i), mark);
        }
        i
    }

    pub fn el(&self, i: usize) -> El<'_> {
        El { dom: self, i }
    }

    pub fn id_of(&self, i: usize) -> Option<&str> {
        self.nodes[i].attrs.iter().find(|(ns, n, _)| ns.is_empty() && n == "id").map(|(_, _, v)| v.as_str())
    }

    pub fn by_id(&self, id: &str) -> Option<usize> {
        self.preorder(0).into_iter().find(|&i| self.id_of(i) == Some(id))
    }

    pub fn by_name(&self, name: &str) -> Option<usize> {
        self.preorder(0).into_iter().find(|&i| self.nodes[i].name == name)
    }

    /// Tree-order indices of the subtree at `root` (root included).
    pub fn preorder(&self, root: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut st = vec![root];
        while let Some(i) = st.pop() {
            out.push(i);
            for &c in self.nodes[i].children.iter().rev() {
                st.push(c);
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug)]
pub struct El<'a> {
    pub dom: &'a Dom,
    pub i: usize,
}

impl PartialEq for El<'_> {
    fn eq(&self, o: &Self) -> bool {
        std::ptr::eq(self.dom, o.dom) && self.i == o.i
    }
}

impl<'a> Element for El<'a> {
    fn local_name(&self) -> &str {
        &self.dom.nodes[self.i].name
    }
    fn namespace_url(&self) -> &str {
        &self.dom.nodes[self.i].ns
    }
    fn each_attr(&self, f: &mut dyn FnMut(&str, &str, &str) -> bool) -> bool {
        self.dom.nodes[self.i].attrs.iter().any(|(ns, n, v)| f(ns, n, v))
    }
    fn parent_element(&self) -> Option<Self> {
        self.dom.nodes[self.i].parent.map(|p| El { dom: self.dom, i: p })
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        let n = &self.dom.nodes[self.i];
        let p = n.parent?;
        if n.index == 0 { None } else { Some(El { dom: self.dom, i: self.dom.nodes[p].children[n.index - 1] }) }
    }
    fn next_sibling_element(&self) -> Option<Self> {
        let n = &self.dom.nodes[self.i];
        let p = n.parent?;
        self.dom.nodes[p].children.get(n.index + 1).map(|&i| El { dom: self.dom, i })
    }
    fn first_child_element(&self) -> Option<Self> {
        self.dom.nodes[self.i].children.first().map(|&i| El { dom: self.dom, i })
    }
    fn is_empty(&self) -> bool {
        let n = &self.dom.nodes[self.i];
        n.children.is_empty() && !n.has_text
    }
    fn is_root(&self) -> bool {
        self.dom.in_document && self.dom.nodes[self.i].parent.is_none()
    }
    fn state(&self, s: ElementState) -> bool {
        match s {
            ElementState::Target => self.dom.in_document && self.dom.target.is_some() && self.dom.id_of(self.i) == self.dom.target.as_deref(),
            ElementState::Hover => self.dom.hover.contains(&self.i),
            _ => html_state(self, s),
        }
    }
}
