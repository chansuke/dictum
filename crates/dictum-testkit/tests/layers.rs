//! The layer check reports what it should: a broken check would let every
//! `tests/architecture.rs` pass.

use std::path::Path;

use dictum_testkit::layers::Layers;

#[test]
fn the_check_reports_upward_uses_and_unused_known_edges_and_ignores_tests() {
    let process = if cfg!(target_os = "wasi") { 0 } else { std::process::id() };
    let src = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("dictum-layers-{process}"));
    let _ = std::fs::remove_dir_all(&src);
    std::fs::create_dir_all(src.join("engine")).unwrap();
    let write = |name: &str, text: &str| std::fs::write(src.join(name), text).unwrap();
    write("lib.rs", "mod engine;\nmod high;\nmod low;\npub use high::Thing;\n");
    write("high.rs", "pub struct Thing;\nfn f() -> crate::engine::inner::Inner { crate::low::Low }\n");
    write(
        "low.rs",
        "pub struct Low;\n\
         fn up() -> crate::Thing { super::high::Thing }\n\
         #[cfg(test)]\nmod tests { use crate::high::Thing; }\n",
    );
    write("engine/mod.rs", "pub mod inner;\npub mod outer;\n");
    write("engine/inner.rs", "pub struct Inner;\nfn f() -> super::outer::Outer { todo() }\n");
    write("engine/outer.rs", "pub struct Outer;\n");

    let layers: &[&[&str]] = &[&["low"], &["inner"], &["outer", "engine"], &["high"]];
    let known = &[("inner", "outer"), ("low", "engine")];
    let report = Layers { src: &src, layers, nested: &["engine"], known }.check();
    let _ = std::fs::remove_dir_all(&src);

    assert!(report.unknown.is_empty() && report.unreadable.is_empty(), "{report:?}");
    let found: Vec<_> = report.violations.iter().filter_map(|v| v.split(": ").nth(1)).collect();
    assert_eq!(
        found,
        [
            "`low` uses `crate::Thing`, which belongs to `high`",
            "`low` uses `crate::high::Thing`, which belongs to `high`",
        ],
        "{report:?}"
    );
    assert_eq!(report.fixed, [("low".to_owned(), "engine".to_owned())], "{report:?}");
    assert!(report.failure("t").is_some_and(|m| m.contains("Remove them from KNOWN")));
}
