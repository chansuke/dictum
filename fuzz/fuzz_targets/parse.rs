#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|src: &str| {
    match dictum_syntax::parse(src) {
        Ok(program) => {
            for span in dictum_testkit::syntax::spans(&program) {
                assert!(span.text(src).is_some(), "node span {span:?} outside the source");
            }
        }
        Err(diags) => {
            let (kept, _) = diags.finish();
            assert!(!kept.is_empty(), "Err must carry at least one diagnostic");
            assert!(kept.len() <= dictum_syntax::MAX_DIAGNOSTICS);
            for d in &kept {
                assert!(d.span().text(src).is_some(), "span {:?} outside the source", d.span());
            }
        }
    }
});
