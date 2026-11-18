#![no_main]

use std::sync::OnceLock;

use dictum::{Dynamic, Rule, Schema};
use dictum_testkit::Facts;
use libfuzzer_sys::fuzz_target;

fn schema() -> &'static Schema<Facts> {
    static SCHEMA: OnceLock<Schema<Facts>> = OnceLock::new();
    SCHEMA.get_or_init(|| Schema::<Facts>::builder().build().expect("fixture schema"))
}

fuzz_target!(|src: &str| {
    if let Err(e) = Rule::<Dynamic, Facts>::compile(src, schema()) {
        // Rendering must not panic either, and must point into the source.
        let _ = e.to_string();
        assert!(!e.diagnostics().is_empty());
    }
});
