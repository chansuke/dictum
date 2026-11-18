import Dictum.Core

/-!
# Which operand orders common-subexpression sharing may identify

A `RuleSet` caches the *value* of a subexpression computed by one rule and
reuses it in another rule that contains the same subexpression
(`engine/analysis.rs`). Its structural key treats `==`, `+` and `*` as
commutative, so `a + b` in one rule and `b + a` in another share a slot.

That is sound for values: whenever one order succeeds, the other succeeds with
the same value (`comm_ok`). It is not sound for errors, since strict operands
are evaluated left to right and the first failure wins (`comm_error_differs`),
which is why errors are never cached and each rule recomputes with its own
closure tree. And it is unsound for `&&`/`||` even for values (`and_not_comm`):
a cached `false` for `false && x` would hide the error of `x && false`. This is
the defect the synthesis fixed; the property test
`short_circuit_operands_are_not_shared_across_orders` checks the engine.
-/

namespace Dictum

/-- The operators the CSE key treats as commutative. -/
def BinOp.shareCommutes : BinOp → Bool
  | .add | .mul | .eq => true
  | _ => false

theorem applyBin_comm {op : BinOp} (hop : op.shareCommutes = true) (x y : Val) :
    applyBin op x y = applyBin op y x := by
  cases op <;> simp [BinOp.shareCommutes] at hop <;> cases x <;> cases y <;>
    simp [applyBin, Int.add_comm, Int.mul_comm, eq_comm]

theorem comm_ok {op : BinOp} (hop : op.shareCommutes = true) {env : List Val} {a b : Expr} {v : Val}
    (h : eval env (.bin op a b) = .ok v) : eval env (.bin op b a) = .ok v := by
  simp only [eval] at h ⊢
  cases ha : eval env a <;> cases hb : eval env b <;> simp_all [applyBin_comm hop]

/-- With two failing operands, each order reports its left operand's error. -/
theorem comm_error_differs :
    let overflow := Expr.bin .add (.lit (.int maxInt)) (.lit (.int 1))
    let divZero := Expr.bin .mod (.lit (.int 1)) (.lit (.int 0))
    eval [] (.bin .add overflow divZero) = .error .overflow ∧
      eval [] (.bin .add divZero overflow) = .error .divByZero := by
  intro _ _
  exact ⟨rfl, rfl⟩

/-- `&&` must not be keyed as commutative: one order succeeds, the other fails. -/
theorem and_not_comm :
    let a := Expr.lit (.bool false)
    let b := Expr.bin .eq (.bin .mod (.lit (.int 7)) (.lit (.int 0))) (.lit (.int 0))
    eval [] (.and a b) = .ok (.bool false) ∧ eval [] (.and b a) = .error .divByZero := by
  intro _ _
  exact ⟨rfl, rfl⟩

/-- The same for `||`. -/
theorem or_not_comm :
    let a := Expr.lit (.bool true)
    let b := Expr.bin .eq (.bin .mod (.lit (.int 7)) (.lit (.int 0))) (.lit (.int 0))
    eval [] (.or a b) = .ok (.bool true) ∧ eval [] (.or b a) = .error .divByZero := by
  intro _ _
  exact ⟨rfl, rfl⟩

/-- Sharing a value between two rules is sound because evaluation is a function of
the facts alone: the same subexpression on the same facts has one outcome. -/
theorem eval_deterministic (env : List Val) (e : Expr) (r₁ r₂ : Except Err Val)
    (h₁ : eval env e = r₁) (h₂ : eval env e = r₂) : r₁ = r₂ := h₁ ▸ h₂

end Dictum
