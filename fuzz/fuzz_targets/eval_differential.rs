#![no_main]

use std::sync::OnceLock;

use dictum::{Ctx, Dynamic, EvalErrorKind, Limits, Rule, Schema};
use dictum_testkit::{Facts, sample_now};
use libfuzzer_sys::fuzz_target;

fn schema() -> &'static Schema<Facts> {
    static SCHEMA: OnceLock<Schema<Facts>> = OnceLock::new();
    SCHEMA.get_or_init(|| Schema::<Facts>::builder().build().expect("fixture schema"))
}

fuzz_target!(|input: (&str, Facts)| {
    let (src, facts) = input;
    let Ok(rule) = Rule::<Dynamic, Facts>::compile(src, schema()) else { return };
    let ctx = Ctx::new(&facts)
        .with_now(sample_now())
        .with_limits(Limits::default().with_max_steps(10_000).with_max_alloc(1 << 20));
    let fast = rule.eval(&ctx);
    let spec = rule.explain(&ctx).into_result();
    let budget = |k: &EvalErrorKind| matches!(k, EvalErrorKind::StepLimit | EvalErrorKind::AllocLimit);
    match (&fast, &spec) {
        (Ok(a), Ok(b)) => assert_eq!(a, b, "{src}"),
        (Err(a), Err(b)) if budget(a.kind()) || budget(b.kind()) => {}
        (Err(a), Err(b)) => assert_eq!((a.code(), a.span()), (b.code(), b.span()), "{src}"),
        (Err(e), Ok(_)) | (Ok(_), Err(e)) if budget(e.kind()) => {}
        _ => panic!("eval {fast:?} vs explain {spec:?}: {src}"),
    }
});
