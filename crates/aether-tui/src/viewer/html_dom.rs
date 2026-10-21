//! html5ever parsing under a deadline and a node budget, so hostile markup fails fast.
//! Mis-nested formatting tags make the parser clone elements, so input size alone
//! bounds neither its time nor the tree it builds.
use anyhow::{Result, bail, ensure};
use html2text::{Handle, RcDom};
use html5ever::{
    Attribute, ExpandedName, ParseOpts, QualName,
    interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink},
    parse_document,
    tendril::{StrTendril, TendrilSink},
    tree_builder::TreeBuilderOpts,
};
use std::{
    borrow::Cow,
    cell::Cell,
    io::{self, Read},
    rc::Rc,
    time::{Duration, Instant},
};

pub(super) const MAX_NODES: usize = 200_000;
pub(super) const MAX_DEPTH: usize = 512;
pub(super) const PARSE_TIMEOUT: Duration = Duration::from_secs(3);
/// Small reads let the reader stop the parser soon after the deadline or budget.
const CHUNK_BYTES: usize = 256;

/// Parse `html` into a DOM no larger than `MAX_NODES` and no deeper than `MAX_DEPTH`.
pub(super) fn parse(html: &[u8], timeout: Duration) -> Result<RcDom> {
    let budget = Rc::new(Budget {
        nodes: Cell::new(0),
        deadline: Instant::now() + timeout,
    });
    let sink = BoundedSink {
        dom: RcDom::default(),
        budget: budget.clone(),
    };
    let opts = ParseOpts {
        tree_builder: TreeBuilderOpts {
            scripting_enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut reader = Paced {
        rest: html,
        budget: budget.clone(),
    };
    let parsed = parse_document(sink, opts)
        .from_utf8()
        .read_from(&mut reader);
    ensure!(
        budget.nodes.get() <= MAX_NODES,
        "HTML has more than {MAX_NODES} elements; Alt+E opens it in a browser"
    );
    let dom = match parsed {
        Ok(dom) => dom,
        Err(error) if error.kind() == io::ErrorKind::TimedOut => bail!(
            "HTML took longer than {} s to parse; Alt+E opens it in a browser",
            timeout.as_secs()
        ),
        Err(error) => bail!("parse HTML: {error}"),
    };
    let depth = depth(&dom.document);
    ensure!(
        depth <= MAX_DEPTH,
        "HTML nests deeper than {MAX_DEPTH} elements; Alt+E opens it in a browser"
    );
    Ok(dom)
}

/// The deepest node below `root`, walked without recursion.
pub(super) fn depth(root: &Handle) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(root.clone(), 0usize)];
    while let Some((node, level)) = stack.pop() {
        deepest = deepest.max(level);
        stack.extend(
            node.children
                .borrow()
                .iter()
                .map(|c| (c.clone(), level + 1)),
        );
    }
    deepest
}

struct Budget {
    nodes: Cell<usize>,
    deadline: Instant,
}

impl Budget {
    fn spent(&self) -> bool {
        self.nodes.get() > MAX_NODES
    }
}

struct Paced<'a> {
    rest: &'a [u8],
    budget: Rc<Budget>,
}

impl Read for Paced<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.budget.spent() {
            return Err(io::Error::other("HTML node budget spent"));
        }
        if Instant::now() >= self.budget.deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let n = self.rest.len().min(buf.len()).min(CHUNK_BYTES);
        let (chunk, rest) = self.rest.split_at(n);
        buf[..n].copy_from_slice(chunk);
        self.rest = rest;
        Ok(n)
    }
}

/// RcDom that counts what it creates and stops growing once the budget is spent.
/// Elements are still created after that, detached, so the parser's view stays valid.
struct BoundedSink {
    dom: RcDom,
    budget: Rc<Budget>,
}

impl BoundedSink {
    /// Count one node; true while the tree may still grow.
    fn grow(&self) -> bool {
        let nodes = self.budget.nodes.get().saturating_add(1);
        self.budget.nodes.set(nodes);
        nodes <= MAX_NODES
    }

    fn live(&self) -> bool {
        !self.budget.spent()
    }

    /// Text may become a new node; an element was counted when it was created.
    fn admits(&self, child: &NodeOrText<Handle>) -> bool {
        match child {
            NodeOrText::AppendText(_) => self.grow(),
            NodeOrText::AppendNode(_) => self.live(),
        }
    }
}

impl TreeSink for BoundedSink {
    type Output = RcDom;
    type Handle = Handle;
    type ElemName<'a> = ExpandedName<'a>;

    fn finish(self) -> RcDom {
        self.dom
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> Handle {
        self.dom.get_document()
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> ExpandedName<'a> {
        self.dom.elem_name(target)
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        let attrs = if self.grow() { attrs } else { Vec::new() };
        self.dom.create_element(name, attrs, flags)
    }

    fn create_comment(&self, text: StrTendril) -> Handle {
        self.grow();
        self.dom.create_comment(text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Handle {
        self.grow();
        self.dom.create_pi(target, data)
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        if self.admits(&child) {
            self.dom.append(parent, child);
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if self.admits(&child) {
            self.dom
                .append_based_on_parent_node(element, prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        if self.grow() {
            self.dom
                .append_doctype_to_document(name, public_id, system_id);
        }
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        self.dom.get_template_contents(target)
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        self.dom.same_node(x, y)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.dom.set_quirks_mode(mode);
    }

    fn append_before_sibling(&self, sibling: &Handle, new_node: NodeOrText<Handle>) {
        if self.admits(&new_node) {
            self.dom.append_before_sibling(sibling, new_node);
        }
    }

    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        if self.live() {
            self.dom.add_attrs_if_missing(target, attrs);
        }
    }

    fn remove_from_parent(&self, target: &Handle) {
        if self.live() {
            self.dom.remove_from_parent(target);
        }
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        if self.live() {
            self.dom.reparent_children(node, new_parent);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &Handle) -> bool {
        self.dom.is_mathml_annotation_xml_integration_point(handle)
    }
}
