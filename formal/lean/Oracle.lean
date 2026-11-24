import Dictum.Check
import Dictum.Syntax

/-!
# `dictum-oracle`: the Lean definitions as a test oracle

Reads one query per line on stdin and writes one answer per line, so a Rust test
can compare dictum with the definitions the proofs are about. Fields are
separated by a tab.

```text
parse <src>               ok <s-expression>  |  err <code>     (Dictum.Syntax.parse)
eval <env> <expr>         ok int <n>  |  ok bool <b>  |  err <kind>   (Dictum.eval)
check <types> <expr>      ok int  |  ok bool  |  err                    (Dictum.infer)
```

`<types>` is the fact root's field types in order, space-separated: `int bool`.
`<env>` is the fact root's fields in order, space-separated: `i:36 b:true`.
`<expr>` is a core expression: `$0` (field 0), integers, `true`, `false`,
`(+ a b)`, `(- a b)`, `(* a b)`, `(% a b)`, `(== a b)`, `(< a b)`, `(<= a b)`,
`(> a b)`, `(>= a b)`, `(&& a b)`,
`(|| a b)`, `(not a)`, `(? c t e)`. An unreadable query is answered `bad`.

The Rust side lives in `dictum_testkit::oracle`.
-/

open Dictum

inductive SExp where
  | atom (s : String)
  | list (xs : List SExp)
  deriving Inhabited

def tokenize (cs : List Char) : List String :=
  let rec go (cs : List Char) (cur : List Char) (acc : Array String) : Array String :=
    let flush := if cur.isEmpty then acc else acc.push (String.ofList cur.reverse)
    match cs with
    | [] => flush
    | c :: rest =>
      if c == '(' || c == ')' then go rest [] (flush.push (String.singleton c))
      else if c.isWhitespace then go rest [] flush
      else go rest (c :: cur) acc
  (go cs [] #[]).toList

partial def readSExp : List String → Option (SExp × List String)
  | "(" :: rest =>
    let rec items (ts : List String) (acc : Array SExp) : Option (SExp × List String) :=
      match ts with
      | ")" :: rest => some (.list acc.toList, rest)
      | [] => none
      | _ => do
        let (x, rest) ← readSExp ts
        items rest (acc.push x)
    items rest #[]
  | ")" :: _ => none
  | t :: rest => some (.atom t, rest)
  | [] => none

def binOp : String → Option BinOp
  | "+" => some .add
  | "-" => some .sub
  | "*" => some .mul
  | "%" => some .mod
  | "==" => some .eq
  | "<" => some .lt
  | "<=" => some .le
  | ">" => some .gt
  | ">=" => some .ge
  | _ => none

partial def toExpr : SExp → Option Expr
  | .atom "true" => some (.lit (.bool true))
  | .atom "false" => some (.lit (.bool false))
  | .atom s =>
    if s.startsWith "$" then (s.drop 1).toString.toNat?.map Expr.var
    else s.toInt?.map (fun n => .lit (.int n))
  | .list [.atom "not", a] => .not <$> toExpr a
  | .list [.atom "&&", a, b] => .and <$> toExpr a <*> toExpr b
  | .list [.atom "||", a, b] => .or <$> toExpr a <*> toExpr b
  | .list [.atom "?", c, t, e] => .ite <$> toExpr c <*> toExpr t <*> toExpr e
  | .list [.atom op, a, b] => do return Expr.bin (← binOp op) (← toExpr a) (← toExpr b)
  | _ => none

def toVal (s : String) : Option Val :=
  match s.toList with
  | 'i' :: ':' :: n => (String.ofList n).toInt?.map Val.int
  | 'b' :: ':' :: b =>
    match String.ofList b with
    | "true" => some (.bool true)
    | "false" => some (.bool false)
    | _ => none
  | _ => none

def toTy : String → Option Ty
  | "int" => some .int
  | "bool" => some .bool
  | _ => none

def readExpr (e : String) : Option Expr :=
  (readSExp (tokenize e.toList)).bind fun (x, rest) => if rest.isEmpty then toExpr x else none

def showResult : Except Err Val → String
  | .ok (.int n) => s!"ok\tint\t{n}"
  | .ok (.bool b) => s!"ok\tbool\t{b}"
  | .error .overflow => "err\toverflow"
  | .error .divByZero => "err\tdivByZero"
  | .error .internal => "err\tinternal"

def splitTabs (cs : List Char) : List String :=
  let rec go (cs : List Char) (cur : List Char) (acc : Array String) : Array String :=
    match cs with
    | [] => acc.push (String.ofList cur.reverse)
    | '\t' :: rest => go rest [] (acc.push (String.ofList cur.reverse))
    | c :: rest => go rest (c :: cur) acc
  (go cs [] #[]).toList

def answer (line : String) : String :=
  match splitTabs line.toList with
  | ["parse", src] =>
    match Syntax.parse src with
    | .ok s => s!"ok\t{s}"
    | .error code => s!"err\t{code}"
  | ["eval", env, e] =>
    let env := (tokenize env.toList).mapM toVal
    match env, readExpr e with
    | some env, some e => showResult (eval env e)
    | _, _ => "bad"
  | ["check", types, e] =>
    match (tokenize types.toList).mapM toTy, readExpr e with
    | some Γ, some e =>
      match infer Γ e with
      | some .int => "ok\tint"
      | some .bool => "ok\tbool"
      | none => "err"
    | _, _ => "bad"
  | _ => "bad"

partial def serve (stdin : IO.FS.Stream) (stdout : IO.FS.Stream) : IO Unit := do
  let line ← stdin.getLine
  if line.isEmpty then return
  stdout.putStrLn (answer (String.ofList (line.toList.filter (· != '\n'))))
  stdout.flush
  serve stdin stdout

def main : IO Unit := do
  serve (← IO.getStdin) (← IO.getStdout)
