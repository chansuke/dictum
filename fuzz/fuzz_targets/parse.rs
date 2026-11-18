#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|src: &str| {
    if let Err(diags) = dictum_syntax::parse(src) {
        let (kept, _) = diags.finish();
        assert!(!kept.is_empty(), "Err must carry at least one diagnostic");
        assert!(kept.len() <= dictum_syntax::MAX_DIAGNOSTICS);
        for d in &kept {
            assert!(d.span().text(src).is_some(), "span {:?} outside the source", d.span());
        }
    }
});
