//! Go's `fmt.Sprint` of a value, for `string(x)` and `t.String()`. Containers
//! charge one step per element, and every byte is charged as it is written.

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::ControlFlow;

use dictum_syntax::Span;

use super::builtins;
use super::frame::{Fault, Frame, Text, deeper, internal, steps};
use super::value::{Depth, MapRef, Repr, Value};

/// `string(x)`: Go's `fmt.Sprint(x)`. One step per container element.
pub(super) fn go_string(
    v: &Value<'_>,
    out: &mut Text,
    depth: Depth,
    f: &mut Frame<'_>,
    span: Span,
) -> Result<(), Fault> {
    match &v.0 {
        Repr::Array(s) => {
            steps(f, s.len(), span)?;
            out.push(f, '[', span)?;
            for (i, x) in s.iter().enumerate() {
                if i > 0 {
                    out.push(f, ' ', span)?;
                }
                go_string(&x, out, deeper(depth, span)?, f, span)?;
            }
            out.push(f, ']', span)?;
        }
        Repr::Map(m) => {
            steps(f, m.len(), span)?;
            out.push_str(f, "map[", span)?;
            for (i, (k, x)) in sorted_entries(m).iter().enumerate() {
                if i > 0 {
                    out.push(f, ' ', span)?;
                }
                out.push_str(f, k, span)?;
                out.push(f, ':', span)?;
                go_string(x, out, deeper(depth, span)?, f, span)?;
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
                go_string(&o.field(dictum_check::FieldId::new(id)), out, deeper(depth, span)?, f, span)?;
            }
            out.push(f, '}', span)?;
        }
        _ => scalar(v, out, f, span)?,
    }
    Ok(())
}

/// A value that holds no other. Kept out of [`go_string`] so that each level of a
/// deep value costs the recursion a small frame: a 256-level value must fit WASM's
/// 1 MiB stack (`a_host_dynamic_nested_past_the_walk_limit_is_a_depth_error`).
fn scalar(v: &Value<'_>, out: &mut Text, f: &mut Frame<'_>, span: Span) -> Result<(), Fault> {
    let mut text = String::new();
    let shown = match &v.0 {
        Repr::Nil => "<nil>",
        Repr::Bool(b) => {
            if *b {
                "true"
            } else {
                "false"
            }
        }
        Repr::Int(n) => {
            text = alloc::format!("{n}");
            &text
        }
        Repr::Float(x) => {
            builtins::go_float(*x, &mut text);
            &text
        }
        #[cfg(feature = "decimal")]
        Repr::Decimal(d) => {
            text = alloc::format!("{d}");
            &text
        }
        Repr::Str(s) => s.as_str(),
        Repr::Bytes(b) => return out.push_str(f, &String::from_utf8_lossy(b.as_slice()), span),
        Repr::Time(t, z) => {
            builtins::go_time(*t, z, &mut text);
            &text
        }
        Repr::Location(z) => builtins::location_name(z),
        Repr::Duration(d) => {
            builtins::go_duration(*d, &mut text);
            &text
        }
        Repr::Array(_) | Repr::Map(_) | Repr::Object(_) => return Err(internal("a container as a scalar", span)),
    };
    out.push_str(f, shown, span)
}

/// A map's entries by key, as Go prints them.
fn sorted_entries<'a>(m: &MapRef<'a>) -> Vec<(String, Value<'a>)> {
    let mut entries: Vec<(String, Value<'a>)> = Vec::with_capacity(m.len());
    m.for_each(&mut |k, x| {
        entries.push((k.into(), x));
        ControlFlow::Continue(())
    });
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}
