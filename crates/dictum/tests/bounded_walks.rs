//! A function that recurses into a value takes a `Depth` (`engine/value.rs`).
//!
//! A host's `Dynamic` can nest without bound, and each walk that recursed on the
//! value alone overflowed the stack on one, which aborts the process. The bound
//! had been added one walk at a time (`json::encode`, `reduce`, the runtime
//! schema reader), and `string(x)`, `==`, `flatten`, the output copy and
//! `explain` still had none. This test reads the crate's source and fails on a
//! function that calls itself, takes a value, and takes no `Depth`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::visit::Visit;

/// Parameter types that hold a value a walk can descend into.
const VALUE_TYPES: &[&str] = &["Value", "Seq", "MapRef", "Dynamic", "Object"];

/// Functions that call themselves and take a value but do not walk into it. The
/// list must match exactly, so a new one fails until it is listed with a reason.
const NOT_WALKS: &[(&str, &str, &str)] = &[
    ("engine/interp.rs", "binary", "calls itself once, from the Dyn operator to a typed one"),
    ("engine/interp.rs", "unary", "calls itself once, from the Dyn operator to a typed one"),
    ("engine/interp.rs", "equal", "calls itself once, from EqKind::Dyn to a typed kind"),
    ("engine/interp.rs", "order", "calls itself once, from CompareKind::Dyn to a typed kind"),
    ("engine/interp.rs", "index_value", "calls itself once, from IndexOp::Dyn to a typed op"),
    ("engine/interp.rs", "slice_value", "calls itself once, from SliceOp::Dyn to a typed op"),
    ("engine/interp.rs", "intrinsic", "calls itself once, for an overload"),
    ("engine/interp.rs", "check", "recurses on the checked type, which the rule's syntax bounds"),
    ("runtime.rs", "read", "bounded by its own depth against dictum_syntax::MAX_DEPTH, the runtime schema's limit"),
];

#[test]
fn recursive_walks_over_values_take_a_depth() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&src, &mut files);
    assert!(
        files.iter().any(|f| f.ends_with("engine/value.rs")),
        "found no src/engine/value.rs under {}; update this test to the new layout",
        src.display()
    );
    let mut unbounded = Vec::new();
    let mut seen = BTreeSet::new();
    for file in &files {
        let shown = file.strip_prefix(&src).unwrap_or(file).display().to_string();
        let walks = std::fs::read_to_string(file).map_err(|e| e.to_string()).and_then(|t| recursive_value_fns(&t));
        match walks {
            Ok(walks) => {
                for (name, line, has_depth) in walks {
                    if NOT_WALKS.iter().any(|(f, n, _)| *f == shown && *n == name) {
                        seen.insert((shown.clone(), name));
                    } else if !has_depth {
                        unbounded.push(format!("crates/dictum/src/{shown}:{line} `{name}`"));
                    }
                }
            }
            Err(e) => unbounded.push(format!("crates/dictum/src/{shown}: cannot read or parse: {e}")),
        }
    }
    assert!(
        unbounded.is_empty(),
        "These functions recurse into a value without a `Depth`:\n  {}\n\n\
         Take a `depth: Depth` parameter (crates/dictum/src/engine/value.rs) and pass \
         `deeper(depth, span)?` (engine/frame.rs), or `depth.deeper()`, to each recursive call; \
         start the walk at `Depth::TOP`. A host's Dynamic can nest a million levels, and a walk \
         without a bound overflows the stack. If the function does not descend into the value \
         (it dispatches once to a typed variant), add it to NOT_WALKS in \
         crates/dictum/tests/bounded_walks.rs with the reason.",
        unbounded.join("\n  ")
    );
    let stale: Vec<_> = NOT_WALKS
        .iter()
        .filter(|(f, n, _)| !seen.contains(&((*f).to_owned(), (*n).to_owned())))
        .map(|(f, n, _)| format!("{f} `{n}`"))
        .collect();
    assert!(
        stale.is_empty(),
        "NOT_WALKS in crates/dictum/tests/bounded_walks.rs lists functions that no longer recurse \
         on a value; remove them: {}",
        stale.join(", ")
    );
}

#[test]
fn the_check_finds_a_walk_without_a_depth() {
    let text = r#"
        fn walk(v: &Value<'_>) { for x in v.iter() { walk(&x); } }
        fn bounded(v: &Value<'_>, depth: Depth) { bounded(v, depth); }
        fn plain(n: u32) -> u32 { if n == 0 { 0 } else { plain(n - 1) } }
        impl Thing { fn show(&self, d: &Dynamic, out: &mut String) { self.show(d, out) } }
        impl Thing { fn copy(v: &Value<'_>) -> Thing { Thing::copy(v) } }
        impl Frame { fn new(v: &Value<'_>) -> Frame { Budget::new(); Frame } }
        #[cfg(test)]
        mod tests { fn walk_in_test(v: &Value<'_>) { walk_in_test(v) } }
    "#;
    assert_eq!(
        recursive_value_fns(text),
        Ok(vec![
            ("walk".to_owned(), 2, false),
            ("bounded".to_owned(), 3, true),
            ("show".to_owned(), 5, false),
            ("copy".to_owned(), 6, false)
        ])
    );
}

/// Each function outside `#[cfg(test)]` that calls itself and takes a value:
/// its name, line, and whether it takes a `Depth`.
fn recursive_value_fns(text: &str) -> Result<Vec<(String, usize, bool)>, String> {
    let file = syn::parse_file(text).map_err(|e| e.to_string())?;
    let mut visitor = Fns { found: Vec::new(), self_type: None };
    visitor.visit_file(&file);
    Ok(visitor.found)
}

struct Fns {
    found: Vec<(String, usize, bool)>,
    /// The type of the `impl` block being visited.
    self_type: Option<String>,
}

impl Fns {
    fn check(&mut self, sig: &syn::Signature, body: &syn::Block) {
        use syn::spanned::Spanned;
        let types: Vec<String> = sig
            .inputs
            .iter()
            .filter_map(|arg| match arg {
                syn::FnArg::Typed(t) => Some(t.ty.to_token_stream().to_string()),
                syn::FnArg::Receiver(_) => None,
            })
            .collect();
        let mentions = |ty: &str, name: &str| ty.split(|c: char| !c.is_alphanumeric() && c != '_').any(|w| w == name);
        if !types.iter().any(|ty| VALUE_TYPES.iter().any(|v| mentions(ty, v))) {
            return;
        }
        let name = sig.ident.to_string();
        let mut calls = SelfCalls { name: &name, self_type: self.self_type.as_deref(), found: false };
        calls.visit_block(body);
        if calls.found {
            let has_depth = types.iter().any(|ty| mentions(ty, "Depth"));
            self.found.push((name, sig.span().start().line, has_depth));
        }
    }
}

impl<'ast> Visit<'ast> for Fns {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        let test = m.attrs.iter().any(|a| a.path().is_ident("cfg") && a.to_token_stream().to_string().contains("test"));
        if !test {
            syn::visit::visit_item_mod(self, m);
        }
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let outer = self.self_type.take();
        if let syn::Type::Path(p) = &*i.self_ty {
            self.self_type = p.path.segments.last().map(|s| s.ident.to_string());
        }
        syn::visit::visit_item_impl(self, i);
        self.self_type = outer;
    }

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.check(&f.sig, &f.block);
        syn::visit::visit_item_fn(self, f);
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.check(&f.sig, &f.block);
        syn::visit::visit_impl_item_fn(self, f);
    }
}

/// Whether a body calls `name`: as `name(..)`, `Self::name(..)` or `Type::name(..)`
/// for the impl's own type, `self.name(..)`, or passes `name` as a function value.
struct SelfCalls<'n> {
    name: &'n str,
    self_type: Option<&'n str>,
    found: bool,
}

impl<'ast> Visit<'ast> for SelfCalls<'_> {
    fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
        let names: Vec<String> = p.path.segments.iter().map(|s| s.ident.to_string()).collect();
        let found = match names.as_slice() {
            [f] => f == self.name,
            [ty, f] => f == self.name && (ty == "Self" || Some(ty.as_str()) == self.self_type),
            _ => false,
        };
        self.found |= found;
        syn::visit::visit_expr_path(self, p);
    }

    fn visit_expr_method_call(&mut self, c: &'ast syn::ExprMethodCall) {
        if c.method == self.name && matches!(&*c.receiver, syn::Expr::Path(p) if p.path.is_ident("self")) {
            self.found = true;
        }
        syn::visit::visit_expr_method_call(self, c);
    }
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    files.sort();
}
