//! The usage sketch from docs/architecture.md: a schema with functions, a single
//! rule with explain, and a rule set. `just test` runs it.
#![expect(clippy::print_stdout, clippy::print_stderr, reason = "an example shows its results")]

use std::collections::BTreeMap;
use std::fmt;

use dictum::jiff::Timestamp;
use dictum::{CompileError, Ctx, Dictum, Dynamic, Limits, Rule, RuleSet, Schema, SchemaError};

#[derive(Dictum)]
#[dictum(rename_all = "PascalCase")]
pub struct User {
    pub age: i64,
    pub group: String,
    pub email: Option<String>,
    pub tags: Vec<String>,
    /// Attribute bag from JSON: type `any` in rules, borrowed, not copied.
    pub attrs: Dynamic,
    #[dictum(skip)]
    pub password_hash: [u8; 32],
}

#[derive(Dictum)]
#[dictum(rename_all = "PascalCase")]
pub struct Item {
    pub sku: String,
    pub price: f64,
}

#[derive(Dictum)]
#[dictum(rename_all = "PascalCase")]
pub struct Order {
    pub total: f64,
    pub currency: String,
    pub items: Vec<Item>,
    pub notes: BTreeMap<String, String>,
}

/// The schema root. Its fields are the rule's variables: `user`, `order`.
/// It borrows its parts, so building facts per request copies nothing.
#[derive(Dictum)]
pub struct Facts<'a> {
    pub user: &'a User,
    pub order: &'a Order,
}

/// Rules are compiled against the `'static` form of the root.
type F = Facts<'static>;

#[derive(Debug)]
pub struct RateError(String);
impl fmt::Display for RateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no rate for {}", self.0)
    }
}
impl std::error::Error for RateError {}

fn is_holiday(_t: Timestamp) -> bool {
    false
}

// Call site 1: build the schema once at startup.
fn build_schema() -> Result<Schema<F>, SchemaError> {
    Schema::<F>::builder()
        // Deterministic: folded when its argument is constant, shared across a RuleSet.
        .pure_func("isHoliday", |t: Timestamp| is_holiday(t))
        // Borrowed `&str` parameter, fallible: never folded, never shared.
        .func("rate", |code: &str| -> Result<f64, RateError> {
            if code == "USD" { Ok(1.0) } else { Err(RateError(code.to_owned())) }
        })
        .build()
}

// Call site 2: one rule, evaluated per request, explained on demand.
fn single_rule(schema: &Schema<F>, user: &User, order: &Order) -> Result<bool, Box<dyn std::error::Error>> {
    let rule: Rule<bool, F> = Rule::compile(r#"user.Group in ["admin"] && user.Age >= 18"#, schema)?;

    let facts = Facts { user, order };
    let ctx = Ctx::new(&facts).with_now(Timestamp::now()).with_limits(Limits::default().with_max_steps(10_000));
    let ok = rule.eval(&ctx)?;

    if !ok {
        let report = rule.explain(&ctx); // never fails; the outcome is inside
        println!("{report}");
    }

    // Other result types are a type parameter away.
    let discount: Rule<f64, F> = Rule::compile("order.Total > 100 ? 0.1 : 0", schema)?; // int 0 promoted
    let _d: f64 = discount.eval(&ctx)?;
    let anything: Rule<Dynamic, F> = Rule::compile("map(order.Items, .Sku)", schema)?;
    let _v: Dynamic = anything.eval(&ctx)?;
    Ok(ok)
}

// Call site 3: a rule set loaded from storage, rebuilt on change and swapped in.
fn load_rules(schema: &Schema<F>, rows: &[(String, String)]) -> (RuleSet<bool, F>, Vec<CompileError>) {
    let mut builder = RuleSet::builder(schema);
    let mut errors = Vec::new();
    for (name, src) in rows {
        if let Err(e) = builder.insert(name, src) {
            errors.push(e); // e.rule() == Some(name); Display points into the source
        }
    }
    (builder.build(), errors)
}

fn evaluate_set(set: &RuleSet<bool, F>, user: &User, order: &Order) -> Result<(), dictum::EvalError> {
    let facts = Facts { user, order };
    // No rule in this set calls `now()`, so no time is needed.
    let ctx = Ctx::new(&facts);
    // Stops at the first failing rule; `err.rule()` names it.
    let outcomes = set.eval(&ctx)?;
    for name in outcomes.matched() {
        println!("matched {name}");
    }
    if let Some(report) = set.explain("adult", &ctx) {
        println!("{report}");
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema = build_schema()?;
    let user = User {
        age: 16,
        group: "admin".into(),
        email: None,
        tags: vec!["beta".into()],
        attrs: Dynamic::Nil,
        password_hash: [0; 32],
    };
    let order = Order { total: 120.0, currency: "USD".into(), items: Vec::new(), notes: BTreeMap::new() };

    single_rule(&schema, &user, &order)?;

    let rows = vec![
        ("adult".to_owned(), "user.Age >= 18".to_owned()),
        ("staff".to_owned(), r#"user.Group in ["admin", "moderator"] && user.Age >= 18"#.to_owned()),
        ("big".to_owned(), r#"order.Total * rate(order.Currency) > 100"#.to_owned()),
    ];
    let (set, errors) = load_rules(&schema, &rows);
    for e in &errors {
        eprintln!("{e}");
    }
    // Hot reload: keep `Arc<RuleSet>` in an `ArcSwap`; build a new set and store it.
    let shared = std::sync::Arc::new(set);
    let worker = std::sync::Arc::clone(&shared);
    std::thread::spawn(move || {
        let _names: Vec<&str> = worker.names().collect();
    });
    evaluate_set(&shared, &user, &order)?;
    Ok(())
}
