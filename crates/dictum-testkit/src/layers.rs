//! The module layer check behind each crate's `tests/architecture.rs`.
//!
//! A crate lists its modules from the lowest layer up. A module may use only the
//! modules of lower layers, so the graph has no cycles and a change low in the
//! stack cannot need one high up. Uses that already point up or sideways are listed
//! as known edges: the check fails on a new one, and on a known one that is gone,
//! so the list only shrinks. Test code (`#[cfg(test)]`) is not checked.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::visit::Visit;

/// One crate's layers.
pub struct Layers<'a> {
    /// The crate's `src` directory.
    pub src: &'a Path,
    /// Lowest layer first. A name is a top-level module, or a submodule of one of
    /// `nested`.
    pub layers: &'a [&'a [&'a str]],
    /// Top-level modules whose submodules are layered one by one. The parent's own
    /// items (its `mod.rs`) count as the parent's name.
    pub nested: &'a [&'a str],
    /// Edges `(from, to)` that already break the layers, to be removed one by one.
    pub known: &'a [(&'a str, &'a str)],
}

/// What [`Layers::check`] found.
#[derive(Debug, Default)]
pub struct Report {
    /// Modules that are in no layer.
    pub unknown: BTreeSet<String>,
    /// `file:line: description` of each use on an edge that is not known.
    pub violations: Vec<String>,
    /// Known edges that no use needs any more.
    pub fixed: Vec<(String, String)>,
    /// Files that could not be read or parsed.
    pub unreadable: Vec<String>,
}

impl Report {
    /// The failure message for a crate whose test is `test`, or `None` if it holds.
    pub fn failure(&self, test: &str) -> Option<String> {
        let mut parts = Vec::new();
        if !self.unreadable.is_empty() {
            parts.push(format!("Cannot read these files: {}", self.unreadable.join(", ")));
        }
        if !self.unknown.is_empty() {
            parts.push(format!(
                "These modules are in no layer in {test}: {:?}.\n\
                 Put each in the lowest layer that holds everything it uses, and describe it in docs/architecture.md.",
                self.unknown
            ));
        }
        if !self.violations.is_empty() {
            parts.push(format!(
                "{} uses point up or sideways in the layers:\n  {}\n\n\
                 A module may use only modules in lower layers (LAYERS in {test}).\n\
                 Fix the design, not the check: move what both modules need down into a lower module, \
                 or let the caller pass it in (a value, a function, or a trait the lower module defines).\n\
                 Do not re-export the item from another module to hide the use, and do not add the edge to KNOWN.",
                self.violations.len(),
                self.violations.join("\n  ")
            ));
        }
        if !self.fixed.is_empty() {
            parts.push(format!(
                "These KNOWN edges in {test} are no longer used: {:?}.\nRemove them from KNOWN so they cannot come back.",
                self.fixed
            ));
        }
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }
}

/// A use of another module.
struct Use {
    /// The module path the use is in.
    from: Vec<String>,
    /// The path used, from the crate root.
    to: Vec<String>,
    at: String,
}

const ROOT: &str = "crate";

impl Layers<'_> {
    /// Check the crate.
    pub fn check(&self) -> Report {
        let mut report = Report::default();
        let mut rank = BTreeMap::new();
        for (index, layer) in self.layers.iter().enumerate() {
            for module in *layer {
                if rank.insert(*module, index).is_some() {
                    report.unknown.insert(format!("{module} (listed twice)"));
                }
            }
        }
        let Some(root) = parse(&self.src.join("lib.rs"), &mut report) else {
            return report;
        };
        let reexports = root_reexports(&root);
        let mut modules = BTreeSet::new();
        for name in child_modules(&root.items) {
            if self.nested.contains(&name.as_str()) {
                let file = module_file(self.src, std::slice::from_ref(&name));
                if let Some(parsed) = parse(&file, &mut report) {
                    modules.extend(child_modules(&parsed.items));
                }
            }
            modules.insert(name);
        }
        let mut uses = Vec::new();
        collect_module(self.src, &self.src.join("lib.rs"), &[], &root.items, &mut uses, &mut report);

        let unit = |path: &[String]| -> String {
            match path {
                [] => ROOT.into(),
                [first, second, ..] if self.nested.contains(&first.as_str()) && modules.contains(second) => {
                    second.clone()
                }
                [first, ..] if modules.contains(first) && !first.starts_with("__") => first.clone(),
                [first, ..] => reexports.get(first).cloned().unwrap_or_else(|| ROOT.into()),
            }
        };
        report
            .unknown
            .extend(modules.iter().filter(|m| !m.starts_with("__") && !rank.contains_key(m.as_str())).cloned());
        let mut seen = BTreeSet::new();
        for found in uses {
            let (from, to) = (unit(&found.from), unit(&found.to));
            if from == to || from == ROOT || to == ROOT {
                continue;
            }
            let allowed = match (rank.get(from.as_str()), rank.get(to.as_str())) {
                (Some(f), Some(t)) => t < f,
                _ => true,
            };
            if allowed {
                continue;
            }
            let known = self.known.contains(&(from.as_str(), to.as_str()));
            if !known {
                let path = format!("crate::{}", found.to.join("::"));
                report.violations.push(format!("{}: `{from}` uses `{path}`, which belongs to `{to}`", found.at));
            }
            seen.insert((from, to));
        }
        report.fixed = self
            .known
            .iter()
            .filter(|(f, t)| !seen.contains(&((*f).to_owned(), (*t).to_owned())))
            .map(|(f, t)| ((*f).to_owned(), (*t).to_owned()))
            .collect();
        report
    }
}

fn parse(file: &Path, report: &mut Report) -> Option<syn::File> {
    let parsed = std::fs::read_to_string(file)
        .map_err(|e| e.to_string())
        .and_then(|text| syn::parse_file(&text).map_err(|e| e.to_string()));
    match parsed {
        Ok(parsed) => Some(parsed),
        Err(e) => {
            report.unreadable.push(format!("{}: {e}", file.display()));
            None
        }
    }
}

/// The non-test `mod` items.
fn child_modules(items: &[syn::Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Mod(m) if !is_test_only(&m.attrs) => Some(m.ident.to_string()),
            _ => None,
        })
        .collect()
}

/// Names that `lib.rs` re-exports from a module, so a use of `crate::Rule` counts
/// against `rule`.
fn root_reexports(root: &syn::File) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    for item in &root.items {
        if let syn::Item::Use(item) = item {
            for (path, alias) in flatten(&item.tree, Vec::new()) {
                let path: Vec<_> = path.into_iter().skip_while(|s| s == "crate").collect();
                if let (Some(first), Some(alias)) = (path.first(), alias) {
                    names.insert(alias, first.clone());
                }
            }
        }
    }
    names
}

/// Every `use` path in a tree, with the name it is bound to.
fn flatten(tree: &syn::UseTree, prefix: Vec<String>) -> Vec<(Vec<String>, Option<String>)> {
    let with = |segment: String| {
        let mut path = prefix.clone();
        path.push(segment);
        path
    };
    match tree {
        syn::UseTree::Path(path) => flatten(&path.tree, with(path.ident.to_string())),
        syn::UseTree::Name(name) => {
            let ident = name.ident.to_string();
            let bound = (ident != "self").then(|| ident.clone());
            let path = if ident == "self" { prefix } else { with(ident) };
            vec![(path, bound)]
        }
        syn::UseTree::Rename(rename) => vec![(with(rename.ident.to_string()), Some(rename.rename.to_string()))],
        syn::UseTree::Glob(_) => vec![(prefix, None)],
        syn::UseTree::Group(group) => group.items.iter().flat_map(|tree| flatten(tree, prefix.clone())).collect(),
    }
}

/// `#[cfg(test)]`, or a `cfg` whose condition requires `test`.
fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr.meta.require_list().is_ok_and(|list| {
                let condition = list.tokens.to_string().replace(' ', "");
                condition == "test"
                    || condition.starts_with("all(") && condition.split([',', '(', ')']).any(|part| part == "test")
            })
    })
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

fn collect_module(
    src: &Path,
    file: &Path,
    module: &[String],
    items: &[syn::Item],
    uses: &mut Vec<Use>,
    report: &mut Report,
) {
    for item in items {
        if is_test_only(item_attrs(item)) {
            continue;
        }
        match item {
            syn::Item::Mod(child) => {
                let mut path = module.to_vec();
                path.push(child.ident.to_string());
                match &child.content {
                    Some((_, inner)) => collect_module(src, file, &path, inner, uses, report),
                    None => {
                        let child_file = module_file(src, &path);
                        if let Some(parsed) = parse(&child_file, report) {
                            collect_module(src, &child_file, &path, &parsed.items, uses, report);
                        }
                    }
                }
            }
            other => Collector { file, module, uses }.visit_item(other),
        }
    }
}

/// `a/b.rs` or `a/b/mod.rs` for the module `a::b`.
fn module_file(src: &Path, module: &[String]) -> PathBuf {
    let base = module.iter().fold(src.to_path_buf(), |dir, part| dir.join(part));
    let flat = base.with_extension("rs");
    if flat.exists() { flat } else { base.join("mod.rs") }
}

struct Collector<'a> {
    file: &'a Path,
    module: &'a [String],
    uses: &'a mut Vec<Use>,
}

impl Collector<'_> {
    fn record(&mut self, segments: &[String], line: usize) {
        let Some(to) = self.absolute(segments) else {
            return;
        };
        let file = self.file.to_string_lossy();
        let file = file.find("crates/").and_then(|i| file.get(i..)).unwrap_or(&file);
        self.uses.push(Use { from: self.module.to_vec(), to, at: format!("{file}:{line}") });
    }

    /// The path from the crate root, for `crate::`, `super::` and `self::` paths;
    /// `None` for other crates and names in scope.
    fn absolute(&self, segments: &[String]) -> Option<Vec<String>> {
        // A lone `self` is the receiver in an expression, not a module.
        if segments.len() == 1 {
            return None;
        }
        let (first, mut rest) = segments.split_first()?;
        let mut base = match first.as_str() {
            "crate" => Vec::new(),
            "self" => self.module.to_vec(),
            "super" => {
                let mut base = self.module.to_vec();
                base.pop();
                base
            }
            _ => return None,
        };
        while rest.first().is_some_and(|s| s == "super") {
            base.pop();
            rest = rest.get(1..).unwrap_or_default();
        }
        base.extend(rest.iter().cloned());
        Some(base)
    }
}

fn line(span: proc_macro2::Span) -> usize {
    span.start().line
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        if is_test_only(&item.attrs) {
            return;
        }
        let at = line(item.use_token.span);
        for (path, _) in flatten(&item.tree, Vec::new()) {
            self.record(&path, at);
        }
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if let Some(first) = path.segments.first() {
            self.record(&segments, line(first.ident.span()));
        }
        syn::visit::visit_path(self, path);
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_fn(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_item_mod(&mut self, _: &'ast syn::ItemMod) {
        // `collect_module` walks submodules with their own module path.
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        // Paths inside macro calls such as `matches!` and `format!` are tokens;
        // parse what is a list of expressions.
        if let Ok(exprs) =
            mac.parse_body_with(syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated)
        {
            for expr in &exprs {
                self.visit_expr(expr);
            }
        }
        syn::visit::visit_macro(self, mac);
    }
}
