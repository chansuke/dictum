//! Go's `fmt.Sprint` of a value, for `string(x)` and `t.String()`. Containers
//! charge one step per element, and every byte is charged as it is written.

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::ControlFlow;

use dictum_syntax::Span;

use super::builtins;
use super::frame::{Fault, Frame, Text, internal, steps};
use super::value::{Repr, Value};

/// `string(x)`: Go's `fmt.Sprint(x)`. One step per container element.
pub(super) fn go_string(v: &Value<'_>, out: &mut Text, f: &mut Frame<'_>, span: Span) -> Result<(), Fault> {
    // A scalar's text is short; it is formatted aside and charged as it is copied in.
    let scalar = |write: &dyn Fn(&mut String), out: &mut Text, f: &mut Frame<'_>| {
        let mut text = String::new();
        write(&mut text);
        out.push_str(f, &text, span)
    };
    match &v.0 {
        Repr::Nil => out.push_str(f, "<nil>", span)?,
        Repr::Bool(b) => out.push_str(f, if *b { "true" } else { "false" }, span)?,
        Repr::Int(n) => scalar(&|t| t.push_str(&alloc::format!("{n}")), out, f)?,
        Repr::Float(x) => scalar(&|t| builtins::go_float(*x, t), out, f)?,
        #[cfg(feature = "decimal")]
        Repr::Decimal(d) => scalar(&|t| t.push_str(&alloc::format!("{d}")), out, f)?,
        Repr::Str(s) => out.push_str(f, s.as_str(), span)?,
        Repr::Bytes(b) => out.push_str(f, &String::from_utf8_lossy(b.as_slice()), span)?,
        Repr::Time(t, z) => scalar(&|text| builtins::go_time(*t, z, text), out, f)?,
        Repr::Location(z) => out.push_str(f, builtins::location_name(z), span)?,
        Repr::Duration(d) => scalar(&|t| builtins::go_duration(*d, t), out, f)?,
        Repr::Array(s) => {
            steps(f, s.len(), span)?;
            out.push(f, '[', span)?;
            for (i, x) in s.iter().enumerate() {
                if i > 0 {
                    out.push(f, ' ', span)?;
                }
                go_string(&x, out, f, span)?;
            }
            out.push(f, ']', span)?;
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
            out.push_str(f, "map[", span)?;
            for (i, (k, x)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(f, ' ', span)?;
                }
                out.push_str(f, k, span)?;
                out.push(f, ':', span)?;
                go_string(x, out, f, span)?;
            }
            out.push(f, ']', span)?;
        }
        Repr::Object(o) => {
            let fields = o.desc().fields();
            steps(f, fields.len(), span)?;
            out.push(f, '{', span)?;
            for i in 0..fields.len() {
                if i > 0 {
                    out.push(f, ' ', span)?;
                }
                let id = u16::try_from(i).map_err(|_| internal("too many fields", span))?;
                go_string(&o.field(dictum_check::FieldId::new(id)), out, f, span)?;
            }
            out.push(f, '}', span)?;
        }
    }
    Ok(())
}
