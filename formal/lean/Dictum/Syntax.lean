/-!
# Reference parser for the operator language

An executable statement of the precedence table in `docs/dictum-design.md`,
written independently of the Rust Pratt parser (`dictum-syntax/src/parser.rs`)
and in the shape of Go expr-lang's `parseExpression`: a unary operator parses its
operand with its own precedence, and the binary loop takes every operator whose
precedence is at least the caller's.

It covers what the precedence rules are about: identifiers, integers, `true`,
`false`, `nil`, `$env`, parentheses, array and map literals, every unary and
binary operator, comparison chains, `??` and its mixing rule, `?:`, `? :`,
`if`/`else`, pipes into named calls, calls, member, index and slice access, and
`let`. It leaves out strings, floats, predicates (`#`, `.Field`, builtins that
take one) and the depth limit, which the Rust tests cover.

`parse` returns the tree printed as the same s-expression the Rust tests print
(`dictum_testkit::syntax::sexpr`), or an error code. The `dictum-oracle`
executable answers `parse` queries with it, and the Rust differential test feeds
both parsers the same generated sources (`crates/dictum-syntax/tests/drt.rs`).

This is a specification checked by examples (`#guard` below) and by agreement
with the Rust parser, not a verified parser: nothing here is proved.
-/

namespace Dictum.Syntax

inductive Tok where
  | ident (s : String)
  | int (n : Nat)
  /-- Reserved words, `$env`, punctuation, and `not in`, by their text. -/
  | sym (s : String)
  | eof
  deriving BEq, Repr, Inhabited

def reserved : List String :=
  ["true", "false", "nil", "let", "if", "else", "not", "and", "or", "in", "matches", "contains",
    "startsWith", "endsWith"]

/-- Longest match first. -/
def puncts : List String :=
  ["?.[", "?.", "??", "?:", "**", "..", "==", "!=", "<=", ">=", "&&", "||",
    "+", "-", "*", "/", "%", "^", "<", ">", "!", "?", ":", ".", ",", "(", ")", "[", "]", "{", "}",
    "|", ";", "=", "#"]

def maxI64 : Nat := 2 ^ 63 - 1

def isIdStart (c : Char) : Bool := c.isAlpha || c == '_'
def isIdChar (c : Char) : Bool := c.isAlphanum || c == '_'

def startsWith : List Char → List Char → Bool
  | _, [] => true
  | c :: cs, p :: ps => c == p && startsWith cs ps
  | [], _ :: _ => false

/-- `not` followed by whitespace and the word `in`: one token, `not in`. -/
def notIn? (rest : List Char) : Option (List Char) :=
  let r := rest.dropWhile Char.isWhitespace
  if r.length < rest.length && startsWith r ['i', 'n'] && !((r.drop 2).head?.map isIdChar).getD false
  then some (r.drop 2) else none

partial def lex (cs : List Char) (acc : Array Tok) : Except String (Array Tok) :=
  match cs with
  | [] => .ok (acc.push .eof)
  | c :: rest =>
    if c.isWhitespace then lex rest acc
    else if isIdStart c then
      let word := cs.takeWhile isIdChar
      let rest := cs.dropWhile isIdChar
      let w := String.ofList word
      if w == "not" then
        match notIn? rest with
        | some r => lex r (acc.push (.sym "not in"))
        | none => lex rest (acc.push (.sym w))
      else if reserved.contains w then lex rest (acc.push (.sym w))
      else lex rest (acc.push (.ident w))
    else if c == '$' && startsWith rest ['e', 'n', 'v'] && !((rest.drop 3).head?.map isIdChar).getD false then
      lex (rest.drop 3) (acc.push (.sym "$env"))
    else if c.isDigit then
      let ds := cs.takeWhile Char.isDigit
      let n := ds.foldl (fun n d => n * 10 + (d.toNat - '0'.toNat)) 0
      if n > maxI64 then .error "E0104" else lex (cs.dropWhile Char.isDigit) (acc.push (.int n))
    else
      match puncts.find? (fun p => startsWith cs p.toList) with
      | some p => lex (cs.drop p.length) (acc.push (.sym p))
      | none => .error "E0101"

/-- The tree, already desugared as the Rust AST is. -/
inductive Ast where
  | atom (s : String)
  | unary (op : String) (a : Ast)
  | bin (op : String) (a b : Ast)
  | logic (op : String) (xs : List Ast)
  | cmp (first : Ast) (rest : List (String × Ast))
  | member (opt : Bool) (a : Ast) (field : String)
  /-- `a.name(args)`, `a?.name(args)`. -/
  | method (opt : Bool) (a : Ast) (name : String) (args : List Ast)
  | index (opt : Bool) (a i : Ast)
  | slice (opt : Bool) (a : Ast) (lo hi : Option Ast)
  | call (f : String) (args : List Ast)
  | cond (c t e : Ast)
  | array (xs : List Ast)
  | map (pairs : List (Option Ast × String × Ast))
  deriving Inhabited

partial def Ast.sexpr : Ast → String
  | .atom s => s
  | .unary op a => s!"({op} {a.sexpr})"
  | .bin op a b => s!"({op} {a.sexpr} {b.sexpr})"
  | .logic op xs => s!"({op} {" ".intercalate (xs.map Ast.sexpr)})"
  | .cmp f rest => s!"(cmp {f.sexpr}{String.join (rest.map fun (op, e) => s!" {op} {e.sexpr}")})"
  | .member opt a f => s!"({if opt then "?" else ""}. {a.sexpr} {f})"
  | .method opt a name args =>
    let xs := String.join (args.map fun x => s!" {x.sexpr}")
    s!"({if opt then "?" else ""}.call {a.sexpr} {name}{xs})"
  | .index opt a i => s!"({if opt then "?" else ""}[] {a.sexpr} {i.sexpr})"
  | .slice opt a lo hi =>
    let b := fun (x : Option Ast) => (x.map Ast.sexpr).getD "_"
    s!"({if opt then "?" else ""}[:] {a.sexpr} {b lo} {b hi})"
  | .call f [] => s!"(call {f})"
  | .call f args => s!"(call {f} {" ".intercalate (args.map Ast.sexpr)})"
  | .cond c t e => s!"(? {c.sexpr} {t.sexpr} {e.sexpr})"
  | .array xs => s!"[{" ".intercalate (xs.map Ast.sexpr)}]"
  | .map pairs =>
    let one := fun ((k, name, v) : Option Ast × String × Ast) =>
      match k with
      | some k => s!"({k.sexpr}): {v.sexpr}"
      | none => s!"{name}: {v.sexpr}"
    "{" ++ ", ".intercalate (pairs.map one) ++ "}"

/-- What a node is, for the `??` mixing rule: a `??` or another binary operator
written without parentheses. -/
inductive Bare where
  | none
  | coalesce
  | binary
  deriving BEq

abbrev P := StateT (Array Tok × Nat) (Except String)

def peek : P Tok := do
  let (ts, i) ← get
  return ts.getD i .eof

def peek2 : P Tok := do
  let (ts, i) ← get
  return ts.getD (i + 1) .eof

def advance : P Unit := modify fun (ts, i) => (ts, i + 1)

def fail {α} (code : String) : P α := throw code

def expect (s : String) : P Unit := do
  if (← peek) == .sym s then advance else fail "E0101"

def expectIdent : P String := do
  match ← peek with
  | .ident s => advance; return s
  | _ => fail "E0101"

def isRelational (s : String) : Bool := ["<", ">", "<=", ">="].contains s
def isEquality (s : String) : Bool :=
  ["==", "!=", "in", "not in", "matches", "contains", "startsWith", "endsWith"].contains s

/-- Name of a binary operator in the s-expressions. -/
def binName : String → String
  | "^" => "**"
  | "not in" => "notin"
  | s => s

/-- Binding powers of the left-associative and right-associative binary operators. -/
def arithBp : String → Option Nat
  | "+" | "-" => some 30
  | "*" | "/" | "%" => some 60
  | "**" | "^" => some 100
  | _ => none

mutual

/-- `expr = conditional`. -/
partial def parseExpr : P Ast := do
  if (← peek) == .sym "if" then
    advance
    let c ← parseExpr
    expect "{"
    let t ← parseExpr
    expect "}"
    expect "else"
    if (← peek) == .sym "{" then
      advance
      let e ← parseExpr
      expect "}"
      return .cond c t e
    else
      return .cond c t (← parseExpr)
  let (lhs, _) ← parseBinary 0
  match ← peek with
  | .sym "?" =>
    advance
    if (← peek) == .sym ":" then
      advance
      return .logic "||" [lhs, ← parseExpr]
    let t ← parseExpr
    expect ":"
    return .cond lhs t (← parseExpr)
  | .sym "?:" =>
    advance
    return .logic "||" [lhs, ← parseExpr]
  | _ => return lhs

partial def parseBinary (minBp : Nat) : P (Ast × Bare) := do
  let first ← parseUnary
  loop first
where
  noCoalesce (k : Bare) : P Unit := if k == .coalesce then fail "E0107" else pure ()
  loop : Ast × Bare → P (Ast × Bare)
  | (lhs, kind) => do
    match ← peek with
    | .sym "|" =>
      if minBp > 0 then return (lhs, kind)
      advance
      let f ← expectIdent
      expect "("
      let args ← parseArgs ")"
      loop (.call f (lhs :: args), .none)
    | .sym "??" =>
      if minBp > 500 then return (lhs, kind)
      advance
      let (rhs, rk) ← parseBinary 501
      if kind == .binary || rk == .binary then fail "E0107"
      loop (.bin "??" lhs rhs, .coalesce)
    | .sym s =>
      let logicOp := if s == "&&" || s == "and" then some ("&&", 15)
        else if s == "||" || s == "or" then some ("||", 10) else none
      match logicOp with
      | some (op, bp) =>
        if minBp > bp then return (lhs, kind)
        noCoalesce kind
        let operands ← collect op bp #[lhs]
        loop (.logic op operands.toList, .binary)
      | none =>
        if isRelational s then
          if minBp > 20 then return (lhs, kind)
          noCoalesce kind
          let rest ← chain #[]
          loop (.cmp lhs rest.toList, .binary)
        else if isEquality s then
          if minBp > 20 then return (lhs, kind)
          noCoalesce kind
          advance
          let (rhs, rk) ← parseBinary 21
          noCoalesce rk
          match ← peek with
          | .sym t => if isRelational t || isEquality t then fail "E0106" else pure ()
          | _ => pure ()
          loop (.bin (binName s) lhs rhs, .binary)
        else if s == ".." then
          if minBp > 25 then return (lhs, kind)
          noCoalesce kind
          advance
          let (rhs, rk) ← parseBinary 26
          noCoalesce rk
          if (← peek) == .sym ".." then fail "E0101"
          loop (.bin ".." lhs rhs, .binary)
        else
          match arithBp s with
          | some bp =>
            if minBp > bp then return (lhs, kind)
            noCoalesce kind
            advance
            -- `**` is right-associative: its right operand takes another `**`.
            let (rhs, rk) ← parseBinary (if bp == 100 then 100 else bp + 1)
            noCoalesce rk
            loop (.bin (binName s) lhs rhs, .binary)
          | none => return (lhs, kind)
    | _ => return (lhs, kind)
  collect (op : String) (bp : Nat) (acc : Array Ast) : P (Array Ast) := do
    match ← peek with
    | .sym s =>
      if (op == "&&" && (s == "&&" || s == "and")) || (op == "||" && (s == "||" || s == "or")) then
        advance
        let (rhs, rk) ← parseBinary (bp + 1)
        noCoalesce rk
        collect op bp (acc.push rhs)
      else return acc
    | _ => return acc
  chain (acc : Array (String × Ast)) : P (Array (String × Ast)) := do
    match ← peek with
    | .sym s =>
      if isRelational s then
        advance
        let (rhs, rk) ← parseBinary 21
        noCoalesce rk
        chain (acc.push (s, rhs))
      else if isEquality s then fail "E0106"
      else return acc
    | _ => return acc

partial def parseUnary : P (Ast × Bare) := do
  match ← peek with
  | .sym "not" | .sym "!" =>
    advance
    let (a, _) ← parseBinary 50
    return (.unary "not" a, .none)
  | .sym "-" =>
    advance
    let (a, _) ← parseBinary 90
    return (.unary "neg" a, .none)
  | .sym "+" =>
    advance
    let (a, _) ← parseBinary 90
    return (.unary "pos" a, .none)
  | _ => return (← parsePostfix, .none)

partial def parsePostfix : P Ast := do
  let (p, callable) ← parsePrimary
  -- `(` may follow only a plain identifier, which `parsePrimary` already turned into a call.
  let _ := callable
  postfixLoop p
where
  postfixLoop (e : Ast) : P Ast := do
    let tok ← peek
    match tok with
    -- A `(` after the name makes it a method call; a `(` anywhere else is E0108.
    | .sym "." | .sym "?." =>
      advance
      let name ← expectIdent
      let opt := tok == .sym "?."
      if (← peek) == .sym "(" then
        advance
        postfixLoop (.method opt e name (← parseArgs ")"))
      else postfixLoop (.member opt e name)
    | .sym "[" => advance; postfixLoop (← parseIndex false e)
    | .sym "?.[" => advance; postfixLoop (← parseIndex true e)
    | .sym "(" => fail "E0108"
    | _ => return e

partial def parseIndex (opt : Bool) (e : Ast) : P Ast := do
  if (← peek) == .sym ":" then
    advance
    if (← peek) == .sym "]" then advance; return .slice opt e none none
    let hi ← parseExpr
    expect "]"
    return .slice opt e none (some hi)
  let i ← parseExpr
  match ← peek with
  | .sym "]" => advance; return .index opt e i
  | .sym ":" =>
    advance
    if (← peek) == .sym "]" then advance; return .slice opt e (some i) none
    let hi ← parseExpr
    expect "]"
    return .slice opt e (some i) (some hi)
  | _ => fail "E0101"

/-- The primary, and whether it was a plain identifier. -/
partial def parsePrimary : P (Ast × Bool) := do
  match ← peek with
  | .ident s =>
    advance
    if (← peek) == .sym "(" then
      advance
      return (.call s (← parseArgs ")"), false)
    return (.atom s, true)
  | .int n => advance; return (.atom (toString n), false)
  | .sym "true" | .sym "false" | .sym "nil" | .sym "$env" =>
    let t ← peek
    advance
    match t with
    | .sym s => return (.atom s, false)
    | _ => fail "E0101"
  | .sym "(" =>
    advance
    let e ← parseExpr
    expect ")"
    return (e, false)
  | .sym "[" =>
    advance
    return (.array (← parseArgs "]"), false)
  | .sym "{" =>
    advance
    return (.map (← parsePairs #[]), false)
  | .sym "#" => fail "E0109"
  | .sym "." => fail "E0109"
  | _ => fail "E0101"

/-- `args = arg { "," arg } [ "," ]`, up to `close`. -/
partial def parseArgs (close : String) : P (List Ast) := do
  if (← peek) == .sym close then advance; return []
  let rec more (acc : Array Ast) : P (List Ast) := do
    let acc := acc.push (← parseExpr)
    match ← peek with
    | .sym "," =>
      advance
      if (← peek) == .sym close then advance; return acc.toList
      more acc
    | .sym s => if s == close then advance; return acc.toList else fail "E0101"
    | _ => fail "E0101"
  more #[]

partial def parsePairs (acc : Array (Option Ast × String × Ast)) : P (List (Option Ast × String × Ast)) := do
  if (← peek) == .sym "}" then advance; return acc.toList
  let key ← match ← peek with
    | .ident s => do advance; pure (none, s)
    | .sym "(" => do
      advance
      let k ← parseExpr
      expect ")"
      pure (some k, "")
    | _ => fail "E0101"
  expect ":"
  let v ← parseExpr
  let acc := acc.push (key.1, key.2, v)
  match ← peek with
  | .sym "," => advance; parsePairs acc
  | .sym "}" => advance; return acc.toList
  | _ => fail "E0101"

end

/-- `program = { "let" ident "=" expr ";" } expr EOF`. -/
partial def parseProgram : P String := do
  if (← peek) == .sym "let" then
    advance
    let name ← expectIdent
    expect "="
    let v ← parseExpr
    expect ";"
    return s!"(let {name} {v.sexpr}) " ++ (← parseProgram)
  let body ← parseExpr
  if (← peek) == .eof then return body.sexpr else fail "E0101"

/-- The s-expression of `src`, or the error code. -/
def parse (src : String) : Except String String := do
  let toks ← lex src.toList #[]
  let (out, _) ← parseProgram.run (toks, 0)
  return out

/-- `parse` as one string: the s-expression, or `!` and the error code. -/
def run (src : String) : String :=
  match parse src with
  | .ok s => s
  | .error code => "!" ++ code

-- The rows of the precedence table, as `crates/dictum-syntax/tests/parser.rs` states them.
#guard run "a ? b : c ? d : e" == "(? a b (? c d e))"
#guard run "if a { 1 } else if b { 2 } else { 3 }" == "(? a 1 (? b 2 3))"
#guard run "a || b | f()" == "(call f (|| a b))"
#guard run "a | f() ? x : y" == "(? (call f a) x y)"
#guard run "a || b && c" == "(|| a (&& b c))"
#guard run "a and b && c" == "(&& a b c)"
#guard run "a or b || c" == "(|| a b c)"
#guard run "1 < a <= 3" == "(cmp 1 < a <= 3)"
#guard run "a + b < c * d" == "(cmp (+ a b) < (* c d))"
#guard run "a not in b" == "(notin a b)"
#guard run "x in 1..3" == "(in x (.. 1 3))"
#guard run "1 + 2 * 3 - 4" == "(- (+ 1 (* 2 3)) 4)"
#guard run "7 % 3 * 2" == "(* (% 7 3) 2)"
#guard run "not a == b" == "(== (not a) b)"
#guard run "not a * b" == "(not (* a b))"
#guard run "-2 ** 2" == "(neg (** 2 2))"
#guard run "2 ** 3 ** 2" == "(** 2 (** 3 2))"
#guard run "2 ^ 3" == "(** 2 3)"
#guard run "a ?? b ?? c" == "(?? (?? a b) c)"
#guard run "-a.b" == "(neg (. a b))"
#guard run "a.b?.c[0]" == "([] (?. (. a b) c) 0)"
#guard run "x[1:]" == "([:] x 1 _)"
#guard run "x?.[:2]" == "(?[:] x _ 2)"
-- Method calls: a member name followed by `(`.
#guard run "a.b(c)" == "(.call a b c)"
#guard run "a.b()" == "(.call a b)"
#guard run "a?.b(c, d)" == "(?.call a b c d)"
#guard run "a.b().c()" == "(.call (.call a b) c)"
#guard run "a[0].b(1)" == "(.call ([] a 0) b 1)"
#guard run "a.b(c).d" == "(. (.call a b c) d)"
#guard run "-a.b(c)" == "(neg (.call a b c))"
#guard run "a.b(c) + 1" == "(+ (.call a b c) 1)"
-- Desugarings.
#guard run "a | f(b)" == "(call f a b)"
#guard run "a ?: b" == "(|| a b)"
#guard run "a ? : b" == "(|| a b)"
#guard run "let x = 1; let y = x; x + y" == "(let x 1) (let y x) (+ x y)"
#guard run "{a: 1, (k): 3}" == "{a: 1, (k): 3}"
#guard run "[1, 2,]" == "[1 2]"
#guard run "$env.user" == "(. $env user)"
-- Errors.
#guard run "1 ==" == "!E0101"
#guard run "1 == 1 == true" == "!E0106"
#guard run "a < b == c" == "!E0106"
#guard run "a ?? 0 + 1" == "!E0107"
#guard run "a + b ?? c" == "!E0107"
#guard run "(a ?? 0) + 1" == "(+ (?? a 0) 1)"
#guard run "f(x)(y)" == "!E0108"
#guard run "(a)(b)" == "!E0108"
#guard run "a[0](b)" == "!E0108"
#guard run "a[b](c)" == "!E0108"
-- The pipe takes a bare name and a `(`, as in Go's parser.
#guard run "a | b.f()" == "!E0101"
#guard run "a | f().g()" == "!E0101"
#guard run "# > 1" == "!E0109"
#guard run "9223372036854775808" == "!E0104"
#guard run "1..2..3" == "!E0101"
#guard run "1 + if a { 1 } else { 2 }" == "!E0101"

end Dictum.Syntax
