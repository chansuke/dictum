//! The direction of dependencies between the modules of `dictum-check`. See
//! docs/architecture.md.

use std::path::Path;

use dictum_testkit::layers::Layers;

/// Lowest layer first. A module may use the modules of the layers before its own.
const LAYERS: &[&[&str]] = &[&["types"], &["env"], &["ir"], &["resolve"], &["checker"]];

/// Uses that break the layers today. Each refactoring that removes one deletes it
/// here; the test fails while a listed edge is still used and once it is not.
const KNOWN: &[(&str, &str)] = &[];

#[test]
fn modules_use_only_lower_layers() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let layers = Layers { src: &src, layers: LAYERS, nested: &[], known: KNOWN };
    if let Some(failure) = layers.check().failure("crates/dictum-check/tests/architecture.rs") {
        panic!("{failure}");
    }
}
