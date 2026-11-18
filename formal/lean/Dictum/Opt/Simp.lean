import Dictum.Opt.Range

/-!
# Algebraic simplification, errors included

Rewrites the closure compiler may apply before building closures, each removing
work from every evaluation of the rule. Unlike constant folding they apply to
subtrees that read facts, so they need two facts the checker and the interval
analysis provide:

* typing: `x + 0` may become `x` only because `x` is an `int` in range (for a
  `bool` the original is an internal error, and out of range it overflows);
* `safe`: `x * 0` may become `0`, and `x && false` may become `false`, only if `x`
  cannot fail, because the original evaluates `x` and reports its error.

`simp_sound` says the bottom-up rewrite changes no result, errors included, for
facts that match the schema and lie in their declared ranges. A rule the
engine adds must be added here first; the property tests compare the engine's
optimized evaluation with `explain`, which runs the unoptimized IR.
-/

namespace Dictum

/-- Evaluation cannot fail: every operation is total on the operands' ranges. -/
def safe (ρ : REnv) : Expr → Bool
  | .lit _ => true
  | .var _ => true
  | .bin op a b =>
    safe ρ a && safe ρ b &&
      match op with
      | .add | .sub | .mul => skipCheck ρ op a b
      | .mod =>
        match rangeOf ρ b with
        | some rb => decide (0 < rb.lo ∨ rb.hi < 0)
        | none => false
      | .eq | .lt => true
  | .and a b => safe ρ a && safe ρ b
  | .or a b => safe ρ a && safe ρ b
  | .not a => safe ρ a
  | .ite c t e => safe ρ c && safe ρ t && safe ρ e

theorem safe_ok {env Γ ρ} (henv : EnvTy env Γ) (hρ : EnvRange env ρ) {e T} (ht : HasType Γ e T)
    (hs : safe ρ e = true) : ∃ v, eval env e = .ok v := by
  induction ht with
  | int _ => exact ⟨_, rfl⟩
  | bool _ => exact ⟨_, rfl⟩
  | var h =>
    obtain ⟨v, hv, _⟩ := henv _ _ h
    exact ⟨v, by simp [eval, hv]⟩
  | @arith op a b hop hta htb iha ihb =>
    simp only [safe, Bool.and_eq_true] at hs
    obtain ⟨⟨sa, sb⟩, hop'⟩ := hs
    obtain ⟨x, hx⟩ := iha sa
    obtain ⟨y, hy⟩ := ihb sb
    obtain ⟨x, rfl, _⟩ := ok_int henv hta hx
    obtain ⟨y, rfl, _⟩ := ok_int henv htb hy
    rcases hop with rfl | rfl | rfl | rfl
    all_goals simp only [eval, hx, hy]
    · obtain ⟨_, ra, rb, r, hra, hrb, hr, hf⟩ := skipCheck_spec hop'
      rw [← wrapping_eq (.inl rfl) (rangeOf_sound hρ a hra hx) (rangeOf_sound hρ b hrb hy) hr hf]
      exact ⟨_, rfl⟩
    · obtain ⟨_, ra, rb, r, hra, hrb, hr, hf⟩ := skipCheck_spec hop'
      rw [← wrapping_eq (.inr (.inl rfl)) (rangeOf_sound hρ a hra hx) (rangeOf_sound hρ b hrb hy) hr hf]
      exact ⟨_, rfl⟩
    · obtain ⟨_, ra, rb, r, hra, hrb, hr, hf⟩ := skipCheck_spec hop'
      rw [← wrapping_eq (.inr (.inr rfl)) (rangeOf_sound hρ a hra hx) (rangeOf_sound hρ b hrb hy) hr hf]
      exact ⟨_, rfl⟩
    · simp only at hop'
      split at hop'
      · next rb hrb =>
        have hm := rangeOf_sound hρ b hrb hy
        unfold Range.Mem at hm
        simp only [decide_eq_true_eq] at hop'
        have : y ≠ 0 := by omega
        simp [applyBin, this]
      · cases hop'
  | @eq a b T' hta htb iha ihb =>
    simp only [safe, Bool.and_eq_true] at hs
    obtain ⟨x, hx⟩ := iha hs.1.1
    obtain ⟨y, hy⟩ := ihb hs.1.2
    simp only [eval, hx, hy]
    cases T' with
    | int =>
      obtain ⟨_, rfl, _⟩ := ok_int henv hta hx
      obtain ⟨_, rfl, _⟩ := ok_int henv htb hy
      exact ⟨_, rfl⟩
    | bool =>
      obtain ⟨_, rfl⟩ := ok_bool henv hta hx
      obtain ⟨_, rfl⟩ := ok_bool henv htb hy
      exact ⟨_, rfl⟩
  | @lt a b hta htb iha ihb =>
    simp only [safe, Bool.and_eq_true] at hs
    obtain ⟨x, hx⟩ := iha hs.1.1
    obtain ⟨y, hy⟩ := ihb hs.1.2
    obtain ⟨_, rfl, _⟩ := ok_int henv hta hx
    obtain ⟨_, rfl, _⟩ := ok_int henv htb hy
    simp [eval, hx, hy, applyBin]
  | @and a b hta htb iha ihb =>
    simp only [safe, Bool.and_eq_true] at hs
    obtain ⟨x, hx⟩ := iha hs.1
    obtain ⟨y, hy⟩ := ihb hs.2
    obtain ⟨p, rfl⟩ := ok_bool henv hta hx
    obtain ⟨q, rfl⟩ := ok_bool henv htb hy
    cases p <;> simp [eval, hx, hy]
  | @or a b hta htb iha ihb =>
    simp only [safe, Bool.and_eq_true] at hs
    obtain ⟨x, hx⟩ := iha hs.1
    obtain ⟨y, hy⟩ := ihb hs.2
    obtain ⟨p, rfl⟩ := ok_bool henv hta hx
    obtain ⟨q, rfl⟩ := ok_bool henv htb hy
    cases p <;> simp [eval, hx, hy]
  | @not a hta iha =>
    simp only [safe] at hs
    obtain ⟨x, hx⟩ := iha hs
    obtain ⟨p, rfl⟩ := ok_bool henv hta hx
    simp [eval, hx]
  | @ite c t e T' htc htt hte ihc iht ihe =>
    simp only [safe, Bool.and_eq_true] at hs
    obtain ⟨x, hx⟩ := ihc hs.1.1
    obtain ⟨p, rfl⟩ := ok_bool henv htc hx
    cases p
    · obtain ⟨v, hv⟩ := ihe hs.2; exact ⟨v, by simp [eval, hx, hv]⟩
    · obtain ⟨v, hv⟩ := iht hs.1.2; exact ⟨v, by simp [eval, hx, hv]⟩

/-- Literal shorthands for the rules. -/
abbrev intLit (n : Int) : Expr := .lit (.int n)
abbrev boolLit (b : Bool) : Expr := .lit (.bool b)

/-- One rewrite at the root, if a rule applies. -/
def simp1 (ρ : REnv) (e : Expr) : Expr :=
  match e with
  | .bin .add a b => if b = intLit 0 then a else if a = intLit 0 then b else e
  | .bin .sub a b => if b = intLit 0 then a else e
  | .bin .mul a b =>
    if b = intLit 1 then a
    else if a = intLit 1 then b
    else if (b = intLit 0 ∧ safe ρ a) ∨ (a = intLit 0 ∧ safe ρ b) then intLit 0
    else e
  | .bin .lt a b =>
    if safe ρ a ∧ safe ρ b then
      match rangeOf ρ a, rangeOf ρ b with
      | some ra, some rb =>
        if ra.hi < rb.lo then boolLit true else if rb.hi ≤ ra.lo then boolLit false else e
      | _, _ => e
    else e
  | .bin .eq a b => if a = b ∧ safe ρ a then boolLit true else e
  | .not (.not a) => a
  | .and a b =>
    if a = boolLit true then b
    else if b = boolLit true then a
    else if a = boolLit false then boolLit false
    else if b = boolLit false ∧ safe ρ a then boolLit false
    else e
  | .or a b =>
    if a = boolLit false then b
    else if b = boolLit false then a
    else if a = boolLit true then boolLit true
    else if b = boolLit true ∧ safe ρ a then boolLit true
    else e
  | .ite c t f =>
    if c = boolLit true then t
    else if c = boolLit false then f
    else if t = f ∧ safe ρ c then t
    else e
  | _ => e

/-- Bottom-up: simplify the children, then the node. -/
def simp (ρ : REnv) : Expr → Expr
  | .lit v => .lit v
  | .var i => .var i
  | .bin op a b => simp1 ρ (.bin op (simp ρ a) (simp ρ b))
  | .and a b => simp1 ρ (.and (simp ρ a) (simp ρ b))
  | .or a b => simp1 ρ (.or (simp ρ a) (simp ρ b))
  | .not a => simp1 ρ (.not (simp ρ a))
  | .ite c t e => simp1 ρ (.ite (simp ρ c) (simp ρ t) (simp ρ e))

theorem simp1_type {Γ ρ e T} (ht : HasType Γ e T) : HasType Γ (simp1 ρ e) T := by
  cases ht with
  | int h => exact .int h
  | bool b => exact .bool b
  | var h => exact .var h
  | arith hop hta htb =>
    rcases hop with rfl | rfl | rfl | rfl <;> simp only [simp1]
    · split; · exact hta
      split; · exact htb
      exact .arith (.inl rfl) hta htb
    · split; · exact hta
      exact .arith (.inr (.inl rfl)) hta htb
    · split; · exact hta
      split; · exact htb
      split; · exact .int ⟨by decide, by decide⟩
      exact .arith (.inr (.inr (.inl rfl))) hta htb
    · exact .arith (.inr (.inr (.inr rfl))) hta htb
  | eq hta htb =>
    simp only [simp1]; split
    · exact .bool _
    · exact .eq hta htb
  | lt hta htb =>
    simp only [simp1]; split
    · split
      · split
        · exact .bool _
        · split
          · exact .bool _
          · exact .lt hta htb
      · exact .lt hta htb
    · exact .lt hta htb
  | and hta htb =>
    simp only [simp1]
    split; · exact htb
    split; · exact hta
    split; · exact .bool _
    split; · exact .bool _
    exact .and hta htb
  | or hta htb =>
    simp only [simp1]
    split; · exact htb
    split; · exact hta
    split; · exact .bool _
    split; · exact .bool _
    exact .or hta htb
  | not hta =>
    cases hta with
    | not h => exact h
    | var h => exact .not (.var h)
    | bool b => exact .not (.bool b)
    | eq a b => exact .not (.eq a b)
    | lt a b => exact .not (.lt a b)
    | and a b => exact .not (.and a b)
    | or a b => exact .not (.or a b)
    | ite c t e => exact .not (.ite c t e)
  | ite htc htt hte =>
    simp only [simp1]
    split; · exact htt
    split; · exact hte
    split; · exact htt
    exact .ite htc htt hte

theorem simp_type {Γ ρ e T} (ht : HasType Γ e T) : HasType Γ (simp ρ e) T := by
  induction ht with
  | int h => exact .int h
  | bool b => exact .bool b
  | var h => exact .var h
  | arith hop _ _ iha ihb => exact simp1_type (.arith hop iha ihb)
  | eq _ _ iha ihb => exact simp1_type (.eq iha ihb)
  | lt _ _ iha ihb => exact simp1_type (.lt iha ihb)
  | and _ _ iha ihb => exact simp1_type (.and iha ihb)
  | or _ _ iha ihb => exact simp1_type (.or iha ihb)
  | not _ iha => exact simp1_type (.not iha)
  | ite _ _ _ ihc iht ihe => exact simp1_type (.ite ihc iht ihe)

section
variable {env : List Val} {Γ : List Ty} {ρ : REnv} (henv : EnvTy env Γ) (hρ : EnvRange env ρ)
include henv hρ

theorem simp1_sound {e T} (ht : HasType Γ e T) : eval env (simp1 ρ e) = eval env e := by
  cases ht with
  | int _ | bool _ | var _ => rfl
  | @arith op a b hop hta htb =>
    -- `x + 0`, `x - 0`, `x * 1`: `x` is an `int` in range, so the checked operation returns it.
    have keep : ∀ {f : Int → Int}, (∀ n, InI64 n → f n = n) → ∀ {g : Expr},
        HasType Γ g .int → eval env g = (match eval env g with
          | .error e => .error e
          | .ok (.int n) => checked (f n)
          | .ok (.bool _) => .error .internal) := by
      intro f hf g htg
      cases hg : eval env g with
      | error => rfl
      | ok v =>
        obtain ⟨n, rfl, hn⟩ := ok_int henv htg hg
        simp [hf n hn, checked, hn]
    rcases hop with rfl | rfl | rfl | rfl <;> simp only [simp1]
    · split
      · next h => subst h; rw [keep (f := (· + 0)) (by simp) hta]; simp only [eval]
                  cases eval env a with
                  | error => rfl
                  | ok v => cases v <;> rfl
      split
      · next h => subst h; rw [keep (f := (0 + ·)) (by simp) htb]; simp only [eval]
                  cases eval env b with
                  | error => rfl
                  | ok v => cases v <;> rfl
      rfl
    · split
      · next h => subst h; rw [keep (f := (· - 0)) (by simp) hta]; simp only [eval]
                  cases eval env a with
                  | error => rfl
                  | ok v => cases v <;> rfl
      rfl
    · split
      · next h => subst h; rw [keep (f := (· * 1)) (by simp) hta]; simp only [eval]
                  cases eval env a with
                  | error => rfl
                  | ok v => cases v <;> rfl
      split
      · next h => subst h; rw [keep (f := (1 * ·)) (by simp) htb]; simp only [eval]
                  cases eval env b with
                  | error => rfl
                  | ok v => cases v <;> rfl
      split
      · next h =>
        -- `x * 0` with `x` safe: `x` succeeds with an int, and the product is 0.
        rcases h with ⟨rfl, hs⟩ | ⟨rfl, hs⟩
        · obtain ⟨v, hv⟩ := safe_ok henv hρ hta hs
          obtain ⟨n, rfl, _⟩ := ok_int henv hta hv
          simp [eval, hv, applyBin, checked, InI64, minInt, maxInt]
        · obtain ⟨v, hv⟩ := safe_ok henv hρ htb hs
          obtain ⟨n, rfl, _⟩ := ok_int henv htb hv
          simp [eval, hv, applyBin, checked, InI64, minInt, maxInt]
      rfl
  | @eq a b T' hta htb =>
    simp only [simp1]
    split
    · next h =>
      obtain ⟨rfl, hs⟩ := h
      obtain ⟨v, hv⟩ := safe_ok henv hρ hta hs
      cases T' with
      | int => obtain ⟨n, rfl, _⟩ := ok_int henv hta hv; simp [eval, hv, applyBin]
      | bool => obtain ⟨p, rfl⟩ := ok_bool henv hta hv; simp [eval, hv, applyBin]
    · rfl
  | @lt a b hta htb =>
    simp only [simp1]
    split
    · next hs =>
      obtain ⟨sa, sb⟩ := hs
      obtain ⟨x, hx⟩ := safe_ok henv hρ hta sa
      obtain ⟨y, hy⟩ := safe_ok henv hρ htb sb
      obtain ⟨x, rfl, _⟩ := ok_int henv hta hx
      obtain ⟨y, rfl, _⟩ := ok_int henv htb hy
      split
      · next ra rb hra hrb =>
        have mx := rangeOf_sound hρ a hra hx
        have my := rangeOf_sound hρ b hrb hy
        unfold Range.Mem at mx my
        split
        · next h => simp [eval, hx, hy, applyBin]; omega
        · split
          · next h => simp [eval, hx, hy, applyBin]; omega
          · rfl
      · rfl
    · rfl
  | @and a b hta htb =>
    simp only [simp1]
    split
    · next h => subst h; simp only [eval]
                cases hb : eval env b with
                | error => rfl
                | ok v => obtain ⟨p, rfl⟩ := ok_bool henv htb hb; rfl
    split
    · next h =>
      subst h; simp only [eval]
      cases ha : eval env a with
      | error => rfl
      | ok v => obtain ⟨p, rfl⟩ := ok_bool henv hta ha; cases p <;> rfl
    split
    · next h => subst h; rfl
    split
    · next h =>
      obtain ⟨rfl, hs⟩ := h
      obtain ⟨v, hv⟩ := safe_ok henv hρ hta hs
      obtain ⟨p, rfl⟩ := ok_bool henv hta hv
      cases p <;> simp [eval, hv]
    rfl
  | @or a b hta htb =>
    simp only [simp1]
    split
    · next h => subst h; simp only [eval]
                cases hb : eval env b with
                | error => rfl
                | ok v => obtain ⟨p, rfl⟩ := ok_bool henv htb hb; rfl
    split
    · next h =>
      subst h; simp only [eval]
      cases ha : eval env a with
      | error => rfl
      | ok v => obtain ⟨p, rfl⟩ := ok_bool henv hta ha; cases p <;> rfl
    split
    · next h => subst h; rfl
    split
    · next h =>
      obtain ⟨rfl, hs⟩ := h
      obtain ⟨v, hv⟩ := safe_ok henv hρ hta hs
      obtain ⟨p, rfl⟩ := ok_bool henv hta hv
      cases p <;> simp [eval, hv]
    rfl
  | @not a hta =>
    cases hta with
    | @not a' ha' =>
      simp only [simp1, eval]
      cases h : eval env a' with
      | error => rfl
      | ok v => obtain ⟨p, rfl⟩ := ok_bool henv ha' h; cases p <;> rfl
    | _ => rfl
  | @ite c t f T' htc htt htf =>
    simp only [simp1]
    split
    · next h => subst h; rfl
    split
    · next h => subst h; rfl
    split
    · next h =>
      obtain ⟨rfl, hs⟩ := h
      obtain ⟨v, hv⟩ := safe_ok henv hρ htc hs
      obtain ⟨p, rfl⟩ := ok_bool henv htc hv
      cases p <;> simp [eval, hv]
    rfl

/-- The bottom-up rewrite changes no result, errors included. -/
theorem simp_sound {e T} (ht : HasType Γ e T) : eval env (simp ρ e) = eval env e := by
  induction ht with
  | int _ | bool _ | var _ => rfl
  | arith hop hta htb iha ihb =>
    simp only [simp]
    rw [simp1_sound henv hρ (.arith hop (simp_type hta) (simp_type htb))]
    simp only [eval, iha, ihb]
  | eq hta htb iha ihb =>
    simp only [simp]
    rw [simp1_sound henv hρ (.eq (simp_type hta) (simp_type htb))]
    simp only [eval, iha, ihb]
  | lt hta htb iha ihb =>
    simp only [simp]
    rw [simp1_sound henv hρ (.lt (simp_type hta) (simp_type htb))]
    simp only [eval, iha, ihb]
  | and hta htb iha ihb =>
    simp only [simp]
    rw [simp1_sound henv hρ (.and (simp_type hta) (simp_type htb))]
    simp only [eval, iha, ihb]
  | or hta htb iha ihb =>
    simp only [simp]
    rw [simp1_sound henv hρ (.or (simp_type hta) (simp_type htb))]
    simp only [eval, iha, ihb]
  | not hta iha =>
    simp only [simp]
    rw [simp1_sound henv hρ (.not (simp_type hta))]
    simp only [eval, iha]
  | ite htc htt hte ihc iht ihe =>
    simp only [simp]
    rw [simp1_sound henv hρ (.ite (simp_type htc) (simp_type htt) (simp_type hte))]
    simp only [eval, ihc, iht, ihe]

end

/-- With `age: u8`: `age >= 0`-style tests decided by the range disappear, and
`age * 0 + x` becomes `x`. -/
example : simp [some ⟨0, 255⟩] (.bin .lt (.var 0) (intLit 256)) = boolLit true := by decide
example : simp [some ⟨0, 255⟩, none] (.bin .add (.bin .mul (.var 0) (intLit 0)) (.var 1)) = .var 1 := by decide
/-- `7 % 0 * 0` is not `0`: the left operand fails, and the error must stay. -/
example : simp [] (.bin .mul (.bin .mod (intLit 7) (intLit 0)) (intLit 0)) =
    .bin .mul (.bin .mod (intLit 7) (intLit 0)) (intLit 0) := by decide

end Dictum
