//! Go's `fmt.Sprint` of a value, for `string(x)` and `t.String()`. Containers
//! charge one step per element.

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::ControlFlow;

use dictum_syntax::Span;

use super::builtins;
use super::frame::{Fault, Frame, internal, steps};
use super::value::{Repr, Value};

/// `string(x)`: Go's `fmt.Sprint(x)`. One step per container element.
pub(super) fn go_string(v: &Value<'_>, out: &mut String, f: &mut Frame<'_>, span: Span) -> Result<(), Fault> {
    use core::fmt::Write;
    match &v.0 {
        Repr::Nil => out.push_str("<nil>"),
        Repr::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Repr::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Repr::Float(x) => builtins::go_float(*x, out),
        #[cfg(feature = "decimal")]
        Repr::Decimal(d) => {
            let _ = write!(out, "{d}");
        }
        Repr::Str(s) => out.push_str(s.as_str()),
        Repr::Bytes(b) => out.push_str(&String::from_utf8_lossy(b.as_slice())),
        Repr::Time(t, z) => builtins::go_time(*t, z, out),
        Repr::Location(z) => out.push_str(builtins::location_name(z)),
        Repr::Duration(d) => builtins::go_duration(*d, out),
        Repr::Array(s) => {
            steps(f, s.len(), span)?;
            out.push('[');
            for (i, x) in s.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                go_string(&x, out, f, span)?;
            }
            out.push(']');
        }
        Repr::Map(m) => {
            steps(f, m.len(), span)?;
            // Go prints map keys sorted.
            let mut entries: Vec<(String, Value<'_>)> = Vec::with_capacity(m.len());
            m.for_each(&mut |k, x| {
                entries.push((k.into(), x));
                ControlFlow::Continue(())
            });
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            out.push_str("map[");
            for (i, (k, x)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(k);
                out.push(':');
                go_string(x, out, f, span)?;
            }
            out.push(']');
        }
        Repr::Object(o) => {
            let fields = o.desc().fields();
            steps(f, fields.len(), span)?;
            out.push('{');
            for i in 0..fields.len() {
                if i > 0 {
                    out.push(' ');
                }
                let id = u16::try_from(i).map_err(|_| internal("too many fields", span))?;
                go_string(&o.field(dictum_check::FieldId::new(id)), out, f, span)?;
            }
            out.push('}');
        }
    }
    Ok(())
}
