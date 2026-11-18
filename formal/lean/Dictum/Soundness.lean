import Dictum.Core

/-!
# Type soundness: well-typed rules never hit `Internal`

The checker's promise to the evaluator (`dictum_check::check` returns an IR that
satisfies every `ir::Program` invariant) is what lets the engine report
`EvalErrorKind::Internal` instead of panicking and still claim that it never
happens. Stated here for the core fragment: if a rule type-checks against the
schema and the facts match the schema, evaluation either produces a value of
the rule's type or fails with a real runtime error, never `internal`.
-/

namespace Dictum

inductive HasType (Γ : List Ty) : Expr → Ty → Prop where
  | int (n : Int) : HasType Γ (.lit (.int n)) .int
  | bool (b : Bool) : HasType Γ (.lit (.bool b)) .bool
  | var {i : Nat} {T : Ty} : Γ[i]? = some T → HasType Γ (.var i) T
  | arith {op a b} : (op = .add ∨ op = .sub ∨ op = .mul ∨ op = .mod) →
      HasType Γ a .int → HasType Γ b .int → HasType Γ (.bin op a b) .int
  | eq {a b T} : HasType Γ a T → HasType Γ b T → HasType Γ (.bin .eq a b) .bool
  | lt {a b} : HasType Γ a .int → HasType Γ b .int → HasType Γ (.bin .lt a b) .bool
  | and {a b} : HasType Γ a .bool → HasType Γ b .bool → HasType Γ (.and a b) .bool
  | or {a b} : HasType Γ a .bool → HasType Γ b .bool → HasType Γ (.or a b) .bool
  | not {a} : HasType Γ a .bool → HasType Γ (.not a) .bool
  | ite {c t e T} : HasType Γ c .bool → HasType Γ t T → HasType Γ e T → HasType Γ (.ite c t e) T

inductive ValTy : Val → Ty → Prop where
  | int (n : Int) : ValTy (.int n) .int
  | bool (b : Bool) : ValTy (.bool b) .bool

/-- The facts match the schema (what `#[derive(Dictum)]` guarantees by generating the
descriptor and the field reader from one field list). -/
def EnvTy (env : List Val) (Γ : List Ty) : Prop :=
  ∀ (i : Nat) (T : Ty), Γ[i]? = some T → ∃ v, env[i]? = some v ∧ ValTy v T

/-- A result is acceptable at type `T`: a value of type `T`, or a runtime error that
is not a dictum bug. -/
def Safe (T : Ty) : Except Err Val → Prop
  | .ok v => ValTy v T
  | .error e => e ≠ .internal

theorem checked_safe (n : Int) : Safe .int (checked n) := by
  unfold checked
  split
  · exact ValTy.int n
  · simp [Safe]

theorem ValTy.int_inv {v : Val} (h : ValTy v .int) : ∃ n, v = .int n := by
  cases h; exact ⟨_, rfl⟩

theorem ValTy.bool_inv {v : Val} (h : ValTy v .bool) : ∃ b, v = .bool b := by
  cases h; exact ⟨_, rfl⟩

theorem soundness {env Γ e T} (henv : EnvTy env Γ) (ht : HasType Γ e T) : Safe T (eval env e) := by
  induction ht with
  | int n => exact ValTy.int n
  | bool b => exact ValTy.bool b
  | var h =>
    obtain ⟨v, hv, hty⟩ := henv _ _ h
    simp [eval, hv, Safe, hty]
  | arith hop _ _ iha ihb =>
    simp only [eval]
    revert iha ihb
    cases eval env _ with
    | error e => intro iha _; exact iha
    | ok x =>
      cases eval env _ with
      | error e => intro _ ihb; exact ihb
      | ok y =>
        intro iha ihb
        obtain ⟨a, rfl⟩ := ValTy.int_inv iha
        obtain ⟨b, rfl⟩ := ValTy.int_inv ihb
        rcases hop with rfl | rfl | rfl | rfl
        · exact checked_safe _
        · exact checked_safe _
        · exact checked_safe _
        · simp only [applyBin]
          split
          · simp [Safe]
          · exact ValTy.int _
  | @eq a b T' _ _ iha ihb =>
    simp only [eval]
    revert iha ihb
    cases eval env a with
    | error e => intro iha _; exact iha
    | ok x =>
      cases eval env b with
      | error e => intro _ ihb; exact ihb
      | ok y =>
        intro iha ihb
        cases T' with
        | int =>
          obtain ⟨_, rfl⟩ := ValTy.int_inv iha
          obtain ⟨_, rfl⟩ := ValTy.int_inv ihb
          exact ValTy.bool _
        | bool =>
          obtain ⟨_, rfl⟩ := ValTy.bool_inv iha
          obtain ⟨_, rfl⟩ := ValTy.bool_inv ihb
          exact ValTy.bool _
  | lt _ _ iha ihb =>
    simp only [eval]
    revert iha ihb
    cases eval env _ with
    | error e => intro iha _; exact iha
    | ok x =>
      cases eval env _ with
      | error e => intro _ ihb; exact ihb
      | ok y =>
        intro iha ihb
        obtain ⟨_, rfl⟩ := ValTy.int_inv iha
        obtain ⟨_, rfl⟩ := ValTy.int_inv ihb
        exact ValTy.bool _
  | @and a b _ _ iha ihb =>
    simp only [eval]
    revert iha ihb
    cases eval env a with
    | error e => intro iha _; exact iha
    | ok x =>
      intro iha ihb
      obtain ⟨p, rfl⟩ := ValTy.bool_inv iha
      cases p with
      | false => exact ValTy.bool _
      | true =>
        revert ihb
        cases eval env b with
        | error e => intro ihb; exact ihb
        | ok y =>
          intro ihb
          obtain ⟨_, rfl⟩ := ValTy.bool_inv ihb
          exact ValTy.bool _
  | @or a b _ _ iha ihb =>
    simp only [eval]
    revert iha ihb
    cases eval env a with
    | error e => intro iha _; exact iha
    | ok x =>
      intro iha ihb
      obtain ⟨p, rfl⟩ := ValTy.bool_inv iha
      cases p with
      | true => exact ValTy.bool _
      | false =>
        revert ihb
        cases eval env b with
        | error e => intro ihb; exact ihb
        | ok y =>
          intro ihb
          obtain ⟨_, rfl⟩ := ValTy.bool_inv ihb
          exact ValTy.bool _
  | @not a _ iha =>
    simp only [eval]
    revert iha
    cases eval env a with
    | error e => intro iha; exact iha
    | ok x =>
      intro iha
      obtain ⟨_, rfl⟩ := ValTy.bool_inv iha
      exact ValTy.bool _
  | @ite c t e' T' _ _ _ ihc iht ihe =>
    simp only [eval]
    revert ihc
    cases eval env c with
    | error err => intro ihc; exact ihc
    | ok x =>
      intro ihc
      obtain ⟨p, rfl⟩ := ValTy.bool_inv ihc
      cases p with
      | true => exact iht
      | false => exact ihe

/-- The corollary the engine relies on. -/
theorem never_internal {env Γ e T} (henv : EnvTy env Γ) (ht : HasType Γ e T) :
    eval env e ≠ .error .internal := by
  intro h
  have := soundness henv ht
  rw [h] at this
  exact this rfl

end Dictum
