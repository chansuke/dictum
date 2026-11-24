import Dictum.Soundness

/-!
# The type checker as a decision procedure

`HasType` (`Dictum.Soundness`) says which core-fragment rules are well typed, and
`soundness` says what that buys at run time. `infer` decides it: `infer_iff` says
it returns `T` exactly when the rule has type `T`. So a checker that agrees with
`infer` accepts every well-typed rule and only those, and every rule it accepts
is covered by `soundness`.

The `dictum-oracle` answers `check` queries with `infer`, and the differential
test `crates/dictum-check/tests/drt.rs` compares it with `dictum_check::check` on
generated rules, well typed and not.
-/

namespace Dictum

def binTy : BinOp → Ty → Ty → Option Ty
  | .add, .int, .int | .sub, .int, .int | .mul, .int, .int | .mod, .int, .int => some .int
  | .lt, .int, .int | .le, .int, .int | .gt, .int, .int | .ge, .int, .int => some .bool
  | .eq, a, b => if a = b then some .bool else none
  | _, _, _ => none

def infer (Γ : List Ty) : Expr → Option Ty
  | .lit (.int n) => if InI64 n then some .int else none
  | .lit (.bool _) => some .bool
  | .var i => Γ[i]?
  | .bin op a b =>
    match infer Γ a, infer Γ b with
    | some ta, some tb => binTy op ta tb
    | _, _ => none
  | .and a b | .or a b =>
    match infer Γ a, infer Γ b with
    | some .bool, some .bool => some .bool
    | _, _ => none
  | .not a =>
    match infer Γ a with
    | some .bool => some .bool
    | _ => none
  | .ite c t e =>
    match infer Γ c, infer Γ t, infer Γ e with
    | some .bool, some tt, some te => if tt = te then some tt else none
    | _, _, _ => none

theorem binTy_sound {Γ op a b ta tb T} (ha : HasType Γ a ta) (hb : HasType Γ b tb)
    (h : binTy op ta tb = some T) : HasType Γ (.bin op a b) T := by
  cases op <;> cases ta <;> cases tb <;> simp [binTy] at h <;> subst h
  all_goals first
    | exact .eq ha hb
    | (apply HasType.arith _ ha hb; decide)
    | (apply HasType.cmp _ ha hb; decide)

theorem infer_sound {Γ} : ∀ {e T}, infer Γ e = some T → HasType Γ e T := by
  intro e
  induction e with
  | lit v =>
    intro T h
    cases v with
    | int n =>
      simp only [infer] at h
      split at h
      · next hn => cases h; exact .int hn
      · cases h
    | bool b => simp only [infer, Option.some.injEq] at h; subst h; exact .bool b
  | var i => intro T h; exact .var h
  | bin op a b iha ihb =>
    intro T h
    simp only [infer] at h
    split at h
    · next ta tb hta htb => exact binTy_sound (iha hta) (ihb htb) h
    · cases h
  | and a b iha ihb =>
    intro T h
    simp only [infer] at h
    split at h
    · next hta htb => cases h; exact .and (iha hta) (ihb htb)
    · cases h
  | or a b iha ihb =>
    intro T h
    simp only [infer] at h
    split at h
    · next hta htb => cases h; exact .or (iha hta) (ihb htb)
    · cases h
  | not a iha =>
    intro T h
    simp only [infer] at h
    split at h
    · next hta => cases h; exact .not (iha hta)
    · cases h
  | ite c t e ihc iht ihe =>
    intro T h
    simp only [infer] at h
    split at h
    · next tt te hc ht he =>
      split at h
      · next heq => cases h; subst heq; exact .ite (ihc hc) (iht ht) (ihe he)
      · cases h
    · cases h

theorem infer_complete {Γ e T} (h : HasType Γ e T) : infer Γ e = some T := by
  induction h with
  | int hn => simp [infer, hn]
  | bool b => rfl
  | var h => exact h
  | arith hop _ _ iha ihb =>
    rcases hop with rfl | rfl | rfl | rfl <;> simp [infer, iha, ihb, binTy]
  | eq _ _ iha ihb => simp [infer, iha, ihb, binTy]
  | cmp hop _ _ iha ihb =>
    rcases hop with rfl | rfl | rfl | rfl <;> simp [infer, iha, ihb, binTy]
  | and _ _ iha ihb => simp [infer, iha, ihb]
  | or _ _ iha ihb => simp [infer, iha, ihb]
  | not _ iha => simp [infer, iha]
  | ite _ _ _ ihc iht ihe => simp [infer, ihc, iht, ihe]

/-- The checker accepts exactly the well-typed rules. -/
theorem infer_iff {Γ e T} : infer Γ e = some T ↔ HasType Γ e T :=
  ⟨infer_sound, infer_complete⟩

/-- So whatever `infer` accepts never reaches `internal` (with `never_internal`). -/
theorem infer_never_internal {env Γ e T} (henv : EnvTy env Γ) (h : infer Γ e = some T) :
    eval env e ≠ .error .internal :=
  never_internal henv (infer_sound h)

end Dictum
