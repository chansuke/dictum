//! The direction of dependencies between the modules of `dictum`: the top-level
//! modules and the submodules of `engine`. See docs/architecture.md.

use std::path::Path;

use dictum_testkit::layers::Layers;

/// Lowest layer first. A module may use the modules of the layers before its own.
/// `engine` is the engine's own `mod.rs`, the pipeline over its submodules.
const LAYERS: &[&[&str]] = &[
    &["value"],
    &["error", "facts", "func", "set"],
    &["ctx", "ops"],
    &["layout"],
    &["builtins"],
    &["dynamic"],
    &["explain", "frame", "output"],
    &["sprint", "trace"],
    &["engine"],
    &["json", "methods"],
    &["interp"],
    &["analysis"],
    &["compile", "optimize"],
    &["fold", "runtime"],
    &["schema"],
    &["rule"],
    &["rule_set"],
];

#[test]
fn modules_use_only_lower_layers() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let layers = Layers { src: &src, layers: LAYERS, nested: &["engine"], known: &[] };
    if let Some(failure) = layers.check().failure("crates/dictum/tests/architecture.rs") {
        panic!("{failure}");
    }
}
