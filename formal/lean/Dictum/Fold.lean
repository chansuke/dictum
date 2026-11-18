import Dictum.Core

/-!
# Constant folding preserves meaning, errors included

`engine/fold.rs` replaces a constant subtree with its value, and leaves a
subtree whose evaluation fails as it is, so `false && 1/0 > 0` still compiles
and `7 % 0` still fails at run time with its own span. `fold` below is that
policy: fold the children, then replace the node with a literal if it reads no
facts and evaluates successfully.

`fold_sound` says folding never changes any outcome, for any facts.
-/

namespace Dictum

/-- Reads no facts: the value is the same for every `env`. Purity of calls is
modeled by their absence (only pure calls are folded in the engine). -/
def Expr.closed : Expr → Bool
  | .lit _ => true
  | .var _ => false
  | .bin _ a b => a.closed && b.closed
  | .and a b => a.closed && b.closed
  | .or a b => a.closed && b.closed
  | .not a => a.closed
  | .ite c t e => c.closed && t.closed && e.closed

theorem eval_closed {e : Expr} (h : e.closed = true) (env env' : List Val) : eval env e = eval env' e := by
  induction e with
  | lit v => rfl
  | var i => simp [Expr.closed] at h
  | bin op a b iha ihb =>
    simp only [Expr.closed, Bool.and_eq_true] at h
    simp only [eval, iha h.1, ihb h.2]
  | and a b iha ihb =>
    simp only [Expr.closed, Bool.and_eq_true] at h
    simp only [eval, iha h.1, ihb h.2]
  | or a b iha ihb =>
    simp only [Expr.closed, Bool.and_eq_true] at h
    simp only [eval, iha h.1, ihb h.2]
  | not a iha =>
    simp only [Expr.closed] at h
    simp only [eval, iha h]
  | ite c t e ihc iht ihe =>
    simp only [Expr.closed, Bool.and_eq_true] at h
    simp only [eval, ihc h.1.1, iht h.1.2, ihe h.2]

/-- Replace `e` by its value if it is closed and evaluates successfully. -/
def tryConst (e : Expr) : Expr :=
  if e.closed then
    match eval [] e with
    | .ok v => .lit v
    | .error _ => e
  else e

def fold : Expr → Expr
  | .lit v => .lit v
  | .var i => .var i
  | .bin op a b => tryConst (.bin op (fold a) (fold b))
  | .and a b => tryConst (.and (fold a) (fold b))
  | .or a b => tryConst (.or (fold a) (fold b))
  | .not a => tryConst (.not (fold a))
  | .ite c t e => tryConst (.ite (fold c) (fold t) (fold e))

theorem tryConst_sound (e : Expr) (env : List Val) : eval env (tryConst e) = eval env e := by
  unfold tryConst
  split
  · next hc =>
    have := eval_closed hc [] env
    split
    · next v hv => rw [← this, hv]; rfl
    · rfl
  · rfl

theorem fold_sound (e : Expr) (env : List Val) : eval env (fold e) = eval env e := by
  induction e with
  | lit v => rfl
  | var i => rfl
  | bin op a b iha ihb => simp only [fold, tryConst_sound, eval, iha, ihb]
  | and a b iha ihb => simp only [fold, tryConst_sound, eval, iha, ihb]
  | or a b iha ihb => simp only [fold, tryConst_sound, eval, iha, ihb]
  | not a iha => simp only [fold, tryConst_sound, eval, iha]
  | ite c t e ihc iht ihe => simp only [fold, tryConst_sound, eval, ihc, iht, ihe]

/-- The case the folder must not "fix": the error stays, so it still happens at
run time (and only if reached). -/
example : fold (.bin .mod (.lit (.int 7)) (.lit (.int 0))) = .bin .mod (.lit (.int 7)) (.lit (.int 0)) := by
  decide

/-- `false && 7 % 0 == 0` folds to `false`: the failing operand is never reached. -/
example :
    fold (.and (.lit (.bool false)) (.bin .eq (.bin .mod (.lit (.int 7)) (.lit (.int 0))) (.lit (.int 0)))) =
      .lit (.bool false) := by
  decide

end Dictum
