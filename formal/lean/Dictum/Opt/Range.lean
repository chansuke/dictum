import Dictum.Soundness

/-!
# Interval analysis, and skipping overflow checks it proves unneeded

`#[derive(Dictum)]` knows each integer field's Rust type, so the checker knows its
range: a `u8` field is in `0..=255`, an `i32` field in `i32::MIN..=i32::MAX`.
`rangeOf` propagates those ranges bottom-up. Where the range of `a + b` (or `-`,
`*`) fits in `i64`, the operation cannot overflow, and the closure compiler may
emit `wrapping_add` (no branch, no error path) instead of `checked_add`.

`evalFast` is the evaluator with exactly that choice made, and `evalFast_eq` says
it gives the same result as `eval` on every input whose fields lie in their
declared ranges. The engine's version of `skipCheck` must be this one, or a
weaker one (skipping fewer checks); the differential tests compare the two
evaluators on generated rules.
-/

namespace Dictum

/-- The closed interval `lo..=hi`. -/
structure Range where
  lo : Int
  hi : Int
  deriving DecidableEq, Repr

namespace Range

def Mem (r : Range) (n : Int) : Prop := r.lo ≤ n ∧ n ≤ r.hi

/-- `i64` itself: the range of an `i64` field. -/
def full : Range := ⟨minInt, maxInt⟩

/-- Every member fits in `i64`. -/
def fits (r : Range) : Bool := decide (minInt ≤ r.lo) && decide (r.hi ≤ maxInt)

def union (a b : Range) : Range := ⟨min a.lo b.lo, max a.hi b.hi⟩

/-- The largest absolute value in the range. -/
def absMax (r : Range) : Int := max r.lo.natAbs r.hi.natAbs

/-- Intersect with `i64` (`minInt..=maxInt`); `none` if the intersection is empty. -/
def clampI64 (r : Range) : Option Range :=
  let lo := max r.lo minInt
  let hi := min r.hi maxInt
  if lo ≤ hi then some ⟨lo, hi⟩ else none

end Range

/-- The exact range of `a op b` for `a ∈ ra`, `b ∈ rb`, before any overflow check;
`none` for the operators it does not track or a guaranteed division by zero. -/
def opRange : BinOp → Range → Range → Option Range
  | .add, a, b => some ⟨a.lo + b.lo, a.hi + b.hi⟩
  | .sub, a, b => some ⟨a.lo - b.hi, a.hi - b.lo⟩
  | .mul, a, b =>
    let p := a.lo * b.lo
    let q := a.lo * b.hi
    let r := a.hi * b.lo
    let s := a.hi * b.hi
    some ⟨min (min p q) (min r s), max (max p q) (max r s)⟩
  | .mod, a, b =>
    -- `|a % b| ≤ |a|` and `|a % b| < |b|`; `b.absMax = 0` means the divisor is 0.
    if b.absMax = 0 then none
    else
      let m := min a.absMax (b.absMax - 1)
      some ⟨-m, m⟩
  | _, _, _ => none

/-- Declared field ranges, by `FieldId`. `none` for fields that are not integers. -/
abbrev REnv := List (Option Range)

/-- The facts lie in their declared ranges. -/
def EnvRange (env : List Val) (ρ : REnv) : Prop :=
  ∀ (i : Nat) (r : Range), ρ[i]? = some (some r) → ∀ v, env[i]? = some v → ∃ n, v = .int n ∧ r.Mem n

/-- A range for every `int` the expression can succeed with, if one is known.
Every returned range lies within `i64` (`rangeOf_fits`). -/
def rangeOf (ρ : REnv) : Expr → Option Range
  | .lit (.int n) => Range.clampI64 ⟨n, n⟩
  | .lit (.bool _) => none
  | .var i => (ρ[i]?.join).filter Range.fits
  | .bin op a b =>
    match rangeOf ρ a, rangeOf ρ b with
    | some ra, some rb => opRange op ra rb >>= Range.clampI64
    | _, _ => none
  | .ite _ t e =>
    match rangeOf ρ t, rangeOf ρ e with
    | some rt, some re => some (rt.union re)
    | _, _ => none
  | _ => none

/-- `a op b` cannot overflow: its operands' ranges are known and the result range fits. -/
def skipCheck (ρ : REnv) (op : BinOp) (a b : Expr) : Bool :=
  (op == .add || op == .sub || op == .mul) &&
    match rangeOf ρ a, rangeOf ρ b with
    | some ra, some rb =>
      match opRange op ra rb with
      | some r => r.fits
      | none => false
    | _, _ => false

/-- Two's-complement wrap to `i64`: Rust's `wrapping_add` and friends. -/
def wrap (n : Int) : Int := (n + 2 ^ 63) % 2 ^ 64 - 2 ^ 63

/-- The operations without an overflow check. -/
def applyWrapping : BinOp → Val → Val → Except Err Val
  | .add, .int a, .int b => .ok (.int (wrap (a + b)))
  | .sub, .int a, .int b => .ok (.int (wrap (a - b)))
  | .mul, .int a, .int b => .ok (.int (wrap (a * b)))
  | op, x, y => applyBin op x y

/-- `eval`, with the overflow check left out wherever `skipCheck` allows. -/
def evalFast (ρ : REnv) (env : List Val) : Expr → Except Err Val
  | .lit v => .ok v
  | .var i =>
    match env[i]? with
    | some v => .ok v
    | none => .error .internal
  | .bin op a b =>
    match evalFast ρ env a with
    | .error e => .error e
    | .ok x =>
      match evalFast ρ env b with
      | .error e => .error e
      | .ok y => if skipCheck ρ op a b then applyWrapping op x y else applyBin op x y
  | .and a b =>
    match evalFast ρ env a with
    | .error e => .error e
    | .ok (.bool false) => .ok (.bool false)
    | .ok (.bool true) =>
      match evalFast ρ env b with
      | .ok (.bool y) => .ok (.bool y)
      | .ok (.int _) => .error .internal
      | .error e => .error e
    | .ok (.int _) => .error .internal
  | .or a b =>
    match evalFast ρ env a with
    | .error e => .error e
    | .ok (.bool true) => .ok (.bool true)
    | .ok (.bool false) =>
      match evalFast ρ env b with
      | .ok (.bool y) => .ok (.bool y)
      | .ok (.int _) => .error .internal
      | .error e => .error e
    | .ok (.int _) => .error .internal
  | .not a =>
    match evalFast ρ env a with
    | .ok (.bool b) => .ok (.bool !b)
    | .ok (.int _) => .error .internal
    | .error e => .error e
  | .ite c t e =>
    match evalFast ρ env c with
    | .ok (.bool true) => evalFast ρ env t
    | .ok (.bool false) => evalFast ρ env e
    | .ok (.int _) => .error .internal
    | .error err => .error err

theorem wrap_id {n : Int} (h : InI64 n) : wrap n = n := by
  unfold wrap; unfold InI64 minInt maxInt at h; omega

theorem mul_between {lo hi n : Int} (m : Int) (h : lo ≤ n ∧ n ≤ hi) :
    min (lo * m) (hi * m) ≤ n * m ∧ n * m ≤ max (lo * m) (hi * m) := by
  by_cases hm : 0 ≤ m
  · have h1 := Int.mul_le_mul_of_nonneg_right h.1 hm
    have h2 := Int.mul_le_mul_of_nonneg_right h.2 hm
    omega
  · have hm' : m ≤ 0 := by omega
    have h1 := Int.mul_le_mul_of_nonpos_right h.1 hm'
    have h2 := Int.mul_le_mul_of_nonpos_right h.2 hm'
    omega

theorem opRange_mem {op : BinOp} {ra rb r : Range} {x y : Int} (hx : ra.Mem x) (hy : rb.Mem y)
    (hr : opRange op ra rb = some r) :
    (op = .add → r.Mem (x + y)) ∧ (op = .sub → r.Mem (x - y)) ∧ (op = .mul → r.Mem (x * y)) ∧
      (op = .mod → y ≠ 0 → r.Mem (x.tmod y)) := by
  unfold Range.Mem at *
  cases op <;> simp only [opRange, Option.some.injEq, reduceCtorEq] at hr <;>
    (try subst hr) <;> simp only [reduceCtorEq, false_implies, true_implies, and_true, true_and]
  · omega
  · omega
  · -- x * y lies between the products of the corners.
    have h1 := mul_between y hx
    have h2 := mul_between ra.lo hy
    have h3 := mul_between ra.hi hy
    rw [Int.mul_comm rb.lo ra.lo, Int.mul_comm rb.hi ra.lo] at h2
    rw [Int.mul_comm rb.lo ra.hi, Int.mul_comm rb.hi ra.hi] at h3
    try rw [Int.mul_comm y ra.lo] at h2
    try rw [Int.mul_comm y ra.hi] at h3
    omega
  · intro hy0
    split at hr
    · cases hr
    · simp only [Option.some.injEq] at hr
      subst hr
      have h := Int.natAbs_tmod x y
      have : x.natAbs % y.natAbs < y.natAbs := Nat.mod_lt _ (by omega)
      have : x.natAbs % y.natAbs ≤ x.natAbs := Nat.mod_le _ _
      unfold Range.absMax at *
      simp only
      omega

theorem fits_inI64 {r : Range} {n : Int} (hf : r.fits = true) (h : r.Mem n) : InI64 n := by
  unfold Range.fits at hf; unfold Range.Mem at h; unfold InI64
  simp only [Bool.and_eq_true, decide_eq_true_eq] at hf
  omega

theorem clampI64_fits {r r' : Range} (h : r.clampI64 = some r') : r'.fits = true := by
  simp only [Range.clampI64] at h
  split at h
  · simp only [Option.some.injEq] at h
    subst h
    unfold Range.fits
    simp only [Bool.and_eq_true, decide_eq_true_eq]
    omega
  · cases h

theorem clampI64_mem {r r' : Range} {n : Int} (hm : r.Mem n) (hi : InI64 n)
    (hc : r.clampI64 = some r') : r'.Mem n := by
  simp only [Range.clampI64] at hc
  split at hc
  · simp only [Option.some.injEq] at hc
    subst hc
    unfold Range.Mem at *
    unfold InI64 at hi
    simp only
    omega
  · cases hc

/-- Every range `rangeOf` returns fits in `i64`, so `opRange` on two `rangeOf`
outputs never overflows `i128` in Rust. -/
theorem rangeOf_fits {ρ : REnv} : ∀ {e : Expr} {r : Range}, rangeOf ρ e = some r → r.fits = true := by
  intro e
  induction e with
  | lit v =>
    intro r hr
    cases v with
    | int _ => exact clampI64_fits hr
    | bool _ => simp [rangeOf] at hr
  | var i =>
    intro r hr
    simp only [rangeOf, Option.filter_eq_some_iff] at hr
    exact hr.2
  | bin _ _ _ _ _ =>
    intro r hr
    simp only [rangeOf] at hr
    split at hr
    · simp only [Bind.bind, Option.bind_eq_some_iff] at hr
      obtain ⟨_, _, hc⟩ := hr
      exact clampI64_fits hc
    · cases hr
  | and _ _ _ _ | or _ _ _ _ | not _ _ => intro r hr; simp [rangeOf] at hr
  | ite _ _ _ _ iht ihe =>
    intro r hr
    simp only [rangeOf] at hr
    split at hr
    · next rt re hrt hre =>
      simp only [Option.some.injEq] at hr
      subst hr
      have ht := iht hrt
      have he := ihe hre
      unfold Range.fits Range.union at *
      simp only [Bool.and_eq_true, decide_eq_true_eq] at *
      omega
    · cases hr

/-- The analysis is sound: a successful `int` result lies in the computed range. -/
theorem rangeOf_sound {env ρ} (hρ : EnvRange env ρ) :
    ∀ (e : Expr) {r n}, rangeOf ρ e = some r → eval env e = .ok (.int n) → r.Mem n := by
  intro e
  induction e with
  | lit v =>
    intro r n hr he
    cases v with
    | int m =>
      simp only [eval, Except.ok.injEq, Val.int.injEq] at he
      subst he
      simp only [rangeOf, Range.clampI64] at hr
      split at hr
      · simp only [Option.some.injEq] at hr
        subst hr
        unfold Range.Mem
        simp only
        omega
      · cases hr
    | bool b => simp [rangeOf] at hr
  | var i =>
    intro r n hr he
    simp only [rangeOf, Option.filter_eq_some_iff, Option.join_eq_some_iff] at hr
    simp only [eval] at he
    split at he
    · next v hv =>
      obtain ⟨m, rfl, hm⟩ := hρ i r hr.1 v hv
      simp only [Except.ok.injEq, Val.int.injEq] at he
      subst he; exact hm
    · cases he
  | bin op a b iha ihb =>
    intro r n hr he
    simp only [rangeOf] at hr
    split at hr
    · next ra rb hra hrb =>
      simp only [Bind.bind, Option.bind_eq_some_iff] at hr
      obtain ⟨r0, hr0, hc⟩ := hr
      simp only [eval] at he
      split at he
      · cases he
      · next x hx =>
        split at he
        · cases he
        · next y hy =>
          cases x with
          | bool _ => cases op <;> cases y <;> simp [applyBin] at he
          | int x =>
            cases y with
            | bool _ => cases op <;> simp [applyBin] at he
            | int y =>
              have mx := iha hra hx
              have my := ihb hrb hy
              have hm := opRange_mem mx my hr0
              cases op with
              | add | sub | mul =>
                simp only [applyBin, checked] at he
                split at he
                · next h64 =>
                  simp only [Except.ok.injEq, Val.int.injEq] at he
                  subst he
                  exact clampI64_mem
                    (by first | exact hm.1 rfl | exact hm.2.1 rfl | exact hm.2.2.1 rfl) h64 hc
                · cases he
              | mod =>
                simp only [applyBin] at he
                split at he
                · cases he
                · next hy0 =>
                  simp only [Except.ok.injEq, Val.int.injEq] at he
                  subst he
                  have h64 := tmod_inI64 (a := x) (fits_inI64 (rangeOf_fits hrb) my) hy0
                  exact clampI64_mem (hm.2.2.2 rfl hy0) h64 hc
              | eq | lt | le | gt | ge => simp [opRange] at hr0
    · cases hr
  | and a b _ _ => intro r n hr _; simp [rangeOf] at hr
  | or a b _ _ => intro r n hr _; simp [rangeOf] at hr
  | not a _ => intro r n hr _; simp [rangeOf] at hr
  | ite c t e _ iht ihe =>
    intro r n hr he
    simp only [rangeOf] at hr
    split at hr
    · next rt re hrt hre =>
      simp only [Option.some.injEq] at hr
      subst hr
      simp only [eval] at he
      split at he
      · have := iht hrt he; unfold Range.union Range.Mem at *; simp only; omega
      · have := ihe hre he; unfold Range.union Range.Mem at *; simp only; omega
      · cases he
      · cases he
    · cases hr

theorem skipCheck_spec {ρ op a b} (h : skipCheck ρ op a b = true) :
    (op = .add ∨ op = .sub ∨ op = .mul) ∧ ∃ ra rb r, rangeOf ρ a = some ra ∧ rangeOf ρ b = some rb ∧
      opRange op ra rb = some r ∧ r.fits = true := by
  unfold skipCheck at h
  simp only [Bool.and_eq_true, Bool.or_eq_true, beq_iff_eq] at h
  obtain ⟨hop, h⟩ := h
  refine ⟨by rcases hop with (h | h) | h <;> simp [h], ?_⟩
  split at h
  · next ra rb hra hrb =>
    split at h
    · next r hr => exact ⟨ra, rb, r, hra, hrb, hr, h⟩
    · cases h
  · cases h

theorem wrapping_eq {op ra rb r x y} (hop : op = .add ∨ op = .sub ∨ op = .mul) (hx : ra.Mem x) (hy : rb.Mem y)
    (hr : opRange op ra rb = some r) (hf : r.fits = true) :
    applyWrapping op (.int x) (.int y) = applyBin op (.int x) (.int y) := by
  have hm := opRange_mem hx hy hr
  rcases hop with rfl | rfl | rfl
  · have h := fits_inI64 hf (hm.1 rfl); simp [applyWrapping, applyBin, checked, wrap_id h, h]
  · have h := fits_inI64 hf (hm.2.1 rfl); simp [applyWrapping, applyBin, checked, wrap_id h, h]
  · have h := fits_inI64 hf (hm.2.2.1 rfl); simp [applyWrapping, applyBin, checked, wrap_id h, h]

/-- Skipping the checks `skipCheck` allows changes no result, errors included, on any
facts within their declared ranges. -/
theorem evalFast_eq {env ρ} (hρ : EnvRange env ρ) (e : Expr) : evalFast ρ env e = eval env e := by
  induction e with
  | lit v => rfl
  | var i => rfl
  | bin op a b iha ihb =>
    simp only [evalFast, eval, iha, ihb]
    cases ha : eval env a with
    | error e => rfl
    | ok x =>
      cases hb : eval env b with
      | error e => rfl
      | ok y =>
        simp only
        split
        · next hs =>
          obtain ⟨hop, ra, rb, r, hra, hrb, hr, hf⟩ := skipCheck_spec hs
          cases x with
          | bool _ => rcases hop with rfl | rfl | rfl <;> rfl
          | int x =>
            cases y with
            | bool _ => rcases hop with rfl | rfl | rfl <;> rfl
            | int y => exact wrapping_eq hop (rangeOf_sound hρ a hra ha) (rangeOf_sound hρ b hrb hb) hr hf
        · rfl
  | and a b iha ihb | or a b iha ihb =>
    simp only [evalFast, eval, iha, ihb]
    cases eval env a with
    | error => rfl
    | ok v =>
      cases v with
      | int => rfl
      | bool p => cases p <;> (try rfl) <;> (cases eval env b with | error => rfl | ok w => cases w <;> rfl)
  | not a iha =>
    simp only [evalFast, eval, iha]
    cases eval env a with
    | error => rfl
    | ok v => cases v <;> rfl
  | ite c t e ihc iht ihe =>
    simp only [evalFast, eval, ihc, iht, ihe]
    cases eval env c with
    | error => rfl
    | ok v => cases v with
      | int => rfl
      | bool p => cases p <;> rfl

/-- Example: with `age: u8`, `age + 1` and `age * 1000` need no overflow check, and
`age * age * age * age * age * age * age * age` does. -/
example : skipCheck [some ⟨0, 255⟩] .add (.var 0) (.lit (.int 1)) = true := by decide
example : skipCheck [some ⟨0, 255⟩] .mul (.var 0) (.lit (.int 1000)) = true := by decide
example :
    let a := Expr.var 0
    let a4 := Expr.bin .mul (.bin .mul a a) (.bin .mul a a)
    skipCheck [some ⟨0, 255⟩] .mul a4 a4 = false := by decide
example : rangeOf [] (.bin .mod (.lit (.int 7)) (.lit (.int 0))) = none := by decide
example : rangeOf [some Range.full] (.bin .add (.var 0) (.lit (.int 1))) =
    some ⟨minInt + 1, maxInt⟩ := by decide
example : rangeOf [some ⟨maxInt, maxInt⟩] (.bin .add (.var 0) (.lit (.int 1))) = none := by decide

end Dictum
