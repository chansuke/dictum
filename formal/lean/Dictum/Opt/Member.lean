/-!
# `in` without a linear scan

Two rewrites from the design document's optimization list:

* `x in [c₁, c₂, ...]` with constant elements: the compiler sorts the constants
  once, and evaluation runs a binary search. `bsearch_iff` says the search finds
  `x` exactly when `x` is in the list, for any sorted list.
* `x in lo..hi`: no list is built; evaluation compares `lo ≤ x ≤ hi`.
  `mem_rangeList` says that is membership in the inclusive range `lo..hi`
  (empty when `hi < lo`), which is what `..` means.

These are statements about integers; the engine applies the same search to every
ordered constant type.
-/

namespace Dictum

/-- `l` is sorted in nondecreasing order. -/
def Sorted (l : List Int) : Prop :=
  ∀ i j (hi : i < j) (hj : j < l.length), l[i]'(Nat.lt_trans hi hj) ≤ l[j]

/-- Binary search in `l[lo:hi]`. -/
def bsearchAux (l : List Int) (x : Int) (lo hi : Nat) : Bool :=
  if h : lo < hi ∧ hi ≤ l.length then
    let mid := (lo + hi) / 2
    let v := l[mid]'(by omega)
    if x < v then bsearchAux l x lo mid
    else if v < x then bsearchAux l x (mid + 1) hi
    else true
  else false
termination_by hi - lo

def bsearch (l : List Int) (x : Int) : Bool := bsearchAux l x 0 l.length

theorem bsearchAux_iff {l : List Int} (hs : Sorted l) (x : Int) :
    ∀ lo hi, hi ≤ l.length → (bsearchAux l x lo hi = true ↔ ∃ i, lo ≤ i ∧ i < hi ∧ l[i]? = some x) := by
  intro lo hi
  induction lo, hi using bsearchAux.induct l x with
  | case1 lo hi h mid v hlt ih =>
    intro _
    rw [bsearchAux, dite_eq_left_of_eq_true (eq_true h)]
    have hlt' : x < l[(lo + hi) / 2]'(by omega) := hlt
    rw [ite_eq_left hlt', ih (by omega)]
    constructor
    · rintro ⟨i, h1, h2, h3⟩; exact ⟨i, h1, by omega, h3⟩
    · rintro ⟨i, h1, h2, h3⟩
      refine ⟨i, h1, ?_, h3⟩
      -- `l[i] = x < l[mid]`, so `i < mid` by sortedness.
      obtain ⟨hi', hx⟩ := List.getElem?_eq_some_iff.mp h3
      apply Classical.byContradiction
      intro hn
      by_cases he : i = (lo + hi) / 2
      · subst he; omega
      · have := hs ((lo + hi) / 2) i (by omega) hi'
        omega
  | case2 lo hi h mid v hnlt hgt ih =>
    intro hle
    rw [bsearchAux, dite_eq_left_of_eq_true (eq_true h)]
    have hnlt' : ¬ x < l[(lo + hi) / 2]'(by omega) := hnlt
    have hgt' : l[(lo + hi) / 2]'(by omega) < x := hgt
    rw [ite_eq_right hnlt', ite_eq_left hgt', ih hle]
    constructor
    · rintro ⟨i, h1, h2, h3⟩; exact ⟨i, by omega, h2, h3⟩
    · rintro ⟨i, h1, h2, h3⟩
      refine ⟨i, ?_, h2, h3⟩
      obtain ⟨hi', hx⟩ := List.getElem?_eq_some_iff.mp h3
      apply Classical.byContradiction
      intro hn
      by_cases he : i = (lo + hi) / 2
      · subst he; omega
      · have := hs i ((lo + hi) / 2) (by omega) (by omega)
        omega
  | case3 lo hi h mid v hnlt hngt =>
    intro _
    rw [bsearchAux, dite_eq_left_of_eq_true (eq_true h)]
    have hnlt' : ¬ x < l[(lo + hi) / 2]'(by omega) := hnlt
    have hngt' : ¬ l[(lo + hi) / 2]'(by omega) < x := hngt
    rw [ite_eq_right hnlt', ite_eq_right hngt']
    simp only [true_iff]
    exact ⟨(lo + hi) / 2, by omega, by omega, List.getElem?_eq_some_iff.mpr ⟨by omega, by omega⟩⟩
  | case4 lo hi h =>
    intro hle
    rw [bsearchAux, dite_eq_right_of_eq_false (eq_false h)]
    simp only [Bool.false_eq_true, false_iff, not_exists, not_and]
    intro i h1 h2
    exact absurd ⟨by omega, hle⟩ h

/-- On a sorted list, binary search is membership. -/
theorem bsearch_iff {l : List Int} (hs : Sorted l) (x : Int) : bsearch l x = true ↔ x ∈ l := by
  rw [bsearch, bsearchAux_iff hs x 0 l.length (Nat.le_refl _), List.mem_iff_getElem?]
  constructor
  · rintro ⟨i, _, _, h⟩; exact ⟨i, h⟩
  · rintro ⟨i, h⟩
    exact ⟨i, Nat.zero_le _, (List.getElem?_eq_some_iff.mp h).1, h⟩

/-- `lo..hi`: the integers from `lo` to `hi`, both included. -/
def rangeList (lo hi : Int) : List Int :=
  (List.range (hi - lo + 1).toNat).map (fun (k : Nat) => lo + (k : Int))

theorem mem_rangeList (lo hi x : Int) : x ∈ rangeList lo hi ↔ lo ≤ x ∧ x ≤ hi := by
  simp only [rangeList, List.mem_map, List.mem_range]
  constructor
  · rintro ⟨k, hk, rfl⟩; omega
  · intro h; exact ⟨(x - lo).toNat, by omega, by omega⟩

#guard bsearch [1, 3, 5, 7, 9, 11, 13, 15] 9
#guard !bsearch [1, 3, 5, 7, 9, 11, 13, 15] 8

end Dictum
