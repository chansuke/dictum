#![no_main]

use std::sync::OnceLock;

use dictum_check::{Env, Type};
use dictum_testkit::Facts;
use libfuzzer_sys::fuzz_target;

fn env() -> &'static Env<()> {
    static ENV: OnceLock<Env<()>> = OnceLock::new();
    ENV.get_or_init(|| Env::new(<Facts as dictum::Dictum>::__desc(), Vec::new()).expect("fixture schema"))
}

fuzz_target!(|src: &str| {
    let Ok(ast) = dictum_syntax::parse(src) else { return };
    match dictum_check::check(&ast, env(), &Type::Any) {
        Ok(program) => {
            if let Err(broken) = program.validate(env()) {
                panic!("the checker accepted {src:?} with a broken IR invariant: {broken}");
            }
        }
        Err(diags) => {
            let (kept, _) = diags.finish();
            assert!(!kept.is_empty(), "Err must carry at least one diagnostic");
            for d in &kept {
                assert!(d.span().text(src).is_some(), "span {:?} outside the source", d.span());
            }
        }
    }
});
