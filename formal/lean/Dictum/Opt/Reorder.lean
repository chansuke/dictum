import Dictum.Opt.Simp

/-!
# Reordering `&&` and `||` operands by cost

A flattened `Logic` node may evaluate its cheap operands first (a field
comparison before an `all(...)` over a list), so that a `false` found early
skips the expensive ones. `and_not_comm` (`Dictum.Commute`) shows this is wrong
in general: `false && 1 % 0 == 0` is `false`, the swapped order fails.

It is right among operands that cannot fail. `&&` and `||` are associative
(`and_assoc`, `or_assoc`, for any operands, errors included), and two adjacent
operands that are both `safe` commute (`and_comm_safe`, `or_comm_safe`). So the
compiler may sort each run of consecutive safe operands, and must keep every
operand that can fail where it is relative to the others.
-/

namespace Dictum

theorem and_assoc (env : List Val) (a b c : Expr) :
    eval env (.and (.and a b) c) = eval env (.and a (.and b c)) := by
  simp only [eval]
  cases eval env a with
  | error => rfl
  | ok v =>
    cases v with
    | int => rfl
    | bool p =>
      cases p
      · rfl
      · cases eval env b with
        | error => rfl
        | ok w =>
          cases w with
          | int => rfl
          | bool q =>
            cases q
            · rfl
            · cases eval env c with
              | error => rfl
              | ok u => cases u <;> rfl

theorem or_assoc (env : List Val) (a b c : Expr) :
    eval env (.or (.or a b) c) = eval env (.or a (.or b c)) := by
  simp only [eval]
  cases eval env a with
  | error => rfl
  | ok v =>
    cases v with
    | int => rfl
    | bool p =>
      cases p
      · cases eval env b with
        | error => rfl
        | ok w =>
          cases w with
          | int => rfl
          | bool q =>
            cases q
            · cases eval env c with
              | error => rfl
              | ok u => cases u <;> rfl
            · rfl
      · rfl

theorem and_congr_right {env : List Val} {x b c : Expr} (h : eval env b = eval env c) :
    eval env (.and x b) = eval env (.and x c) := by
  simp only [eval, h]

theorem or_congr_right {env : List Val} {x b c : Expr} (h : eval env b = eval env c) :
    eval env (.or x b) = eval env (.or x c) := by
  simp only [eval, h]

section
variable {env : List Val} {Γ : List Ty} {ρ : REnv} (henv : EnvTy env Γ) (hρ : EnvRange env ρ)
include henv hρ

theorem and_comm_safe {a b} (ha : HasType Γ a .bool) (hb : HasType Γ b .bool)
    (sa : safe ρ a = true) (sb : safe ρ b = true) : eval env (.and a b) = eval env (.and b a) := by
  obtain ⟨x, hx⟩ := safe_ok henv hρ ha sa
  obtain ⟨y, hy⟩ := safe_ok henv hρ hb sb
  obtain ⟨p, rfl⟩ := ok_bool henv ha hx
  obtain ⟨q, rfl⟩ := ok_bool henv hb hy
  cases p <;> cases q <;> simp [eval, hx, hy]

theorem or_comm_safe {a b} (ha : HasType Γ a .bool) (hb : HasType Γ b .bool)
    (sa : safe ρ a = true) (sb : safe ρ b = true) : eval env (.or a b) = eval env (.or b a) := by
  obtain ⟨x, hx⟩ := safe_ok henv hρ ha sa
  obtain ⟨y, hy⟩ := safe_ok henv hρ hb sb
  obtain ⟨p, rfl⟩ := ok_bool henv ha hx
  obtain ⟨q, rfl⟩ := ok_bool henv hb hy
  cases p <;> cases q <;> simp [eval, hx, hy]

/-- The swap inside a chain: `(x && a) && b` and `(x && b) && a` agree when `a` and
`b` are safe, whatever `x` does. With associativity this lets the compiler sort any
run of safe operands. -/
theorem and_swap_in_chain {x a b} (ha : HasType Γ a .bool) (hb : HasType Γ b .bool)
    (sa : safe ρ a = true) (sb : safe ρ b = true) :
    eval env (.and (.and x a) b) = eval env (.and (.and x b) a) := by
  rw [and_assoc, and_assoc]
  exact and_congr_right (and_comm_safe henv hρ ha hb sa sb)

theorem or_swap_in_chain {x a b} (ha : HasType Γ a .bool) (hb : HasType Γ b .bool)
    (sa : safe ρ a = true) (sb : safe ρ b = true) :
    eval env (.or (.or x a) b) = eval env (.or (.or x b) a) := by
  rw [or_assoc, or_assoc]
  exact or_congr_right (or_comm_safe henv hρ ha hb sa sb)

end

end Dictum
