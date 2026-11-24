/-!
# A core fragment of dictum

The fragment the engine's invariants are stated over: 64-bit checked integer
arithmetic, booleans, field reads from the fact root, strict binary operators,
short-circuit `&&`/`||`, `not` and conditionals.

It mirrors the Rust code as follows:

* `Val` / `Ty`        ~ `engine::value::Value` / `dictum_check::Type` (int and bool only)
* `Err.overflow` etc. ~ `EvalErrorKind::{Overflow, DivisionByZero}`
* `Err.internal`      ~ `EvalErrorKind::Internal`: an IR invariant did not hold
* `Expr.var i`        ~ `ir` field read by `FieldId(i)` from `Frame::root`
* `eval`              ~ the spec interpreter (`engine/interp.rs`): strict operators
                        evaluate left to right, `&&`/`||`/`?:` short-circuit
* `applyBin`          ~ the primitives in `engine/ops.rs`; `lt`, `le`, `gt`, `ge` are
                        a `Compare` with one operator (chains are left out)

What is left out (strings, floats, collections, predicates, `let`, budgets) is
covered by the property and differential tests instead. The `Oracle` executable
runs `eval` on inputs the Rust tests generate, so the Rust evaluator is compared
with this definition, not only with itself.
-/

namespace Dictum

/-- `i64::MIN` and `i64::MAX`. -/
def minInt : Int := -(2 ^ 63)
def maxInt : Int := 2 ^ 63 - 1

/-- Fits in `i64`. Every value the engine holds does: literals are checked by the
lexer, fields are `i64` or narrower, and every operation is checked. -/
abbrev InI64 (n : Int) : Prop := minInt ≤ n ∧ n ≤ maxInt

inductive Ty where
  | int
  | bool
  deriving DecidableEq, Repr

inductive Val where
  | int (n : Int)
  | bool (b : Bool)
  deriving DecidableEq, Repr

inductive Err where
  | overflow
  | divByZero
  /-- A dictum bug, reported instead of panicking. Well-typed rules never reach it. -/
  | internal
  deriving DecidableEq, Repr

inductive BinOp where
  | add
  | sub
  | mul
  /-- Go's `%`: truncated, sign of the dividend. -/
  | mod
  | eq
  | lt
  | le
  | gt
  | ge
  deriving DecidableEq, Repr

inductive Expr where
  | lit (v : Val)
  /-- A field of the fact root, by position. -/
  | var (i : Nat)
  /-- Strict: both operands are evaluated, left first. -/
  | bin (op : BinOp) (a b : Expr)
  | and (a b : Expr)
  | or (a b : Expr)
  | not (a : Expr)
  | ite (c t e : Expr)
  deriving DecidableEq, Repr

/-- `checked_add` and friends: in range or `overflow`. -/
def checked (n : Int) : Except Err Val :=
  if InI64 n then .ok (.int n) else .error .overflow

def applyBin : BinOp → Val → Val → Except Err Val
  | .add, .int a, .int b => checked (a + b)
  | .sub, .int a, .int b => checked (a - b)
  | .mul, .int a, .int b => checked (a * b)
  | .mod, .int a, .int b => if b = 0 then .error .divByZero else .ok (.int (a.tmod b))
  | .eq, .int a, .int b => .ok (.bool (decide (a = b)))
  | .eq, .bool a, .bool b => .ok (.bool (decide (a = b)))
  | .lt, .int a, .int b => .ok (.bool (decide (a < b)))
  | .le, .int a, .int b => .ok (.bool (decide (a ≤ b)))
  | .gt, .int a, .int b => .ok (.bool (decide (a > b)))
  | .ge, .int a, .int b => .ok (.bool (decide (a ≥ b)))
  | _, _, _ => .error .internal

/-- The reference semantics. `env` is the fact root's fields in `FieldId` order. -/
def eval (env : List Val) : Expr → Except Err Val
  | .lit v => .ok v
  | .var i =>
    match env[i]? with
    | some v => .ok v
    | none => .error .internal
  | .bin op a b =>
    match eval env a with
    | .error e => .error e
    | .ok x =>
      match eval env b with
      | .error e => .error e
      | .ok y => applyBin op x y
  | .and a b =>
    match eval env a with
    | .error e => .error e
    | .ok (.bool false) => .ok (.bool false)
    | .ok (.bool true) =>
      match eval env b with
      | .ok (.bool y) => .ok (.bool y)
      | .ok (.int _) => .error .internal
      | .error e => .error e
    | .ok (.int _) => .error .internal
  | .or a b =>
    match eval env a with
    | .error e => .error e
    | .ok (.bool true) => .ok (.bool true)
    | .ok (.bool false) =>
      match eval env b with
      | .ok (.bool y) => .ok (.bool y)
      | .ok (.int _) => .error .internal
      | .error e => .error e
    | .ok (.int _) => .error .internal
  | .not a =>
    match eval env a with
    | .ok (.bool b) => .ok (.bool !b)
    | .ok (.int _) => .error .internal
    | .error e => .error e
  | .ite c t e =>
    match eval env c with
    | .ok (.bool true) => eval env t
    | .ok (.bool false) => eval env e
    | .ok (.int _) => .error .internal
    | .error err => .error err

end Dictum
