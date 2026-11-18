----------------------------- MODULE HotReload -----------------------------
(***************************************************************************)
(* Hot reload as docs/dictum-design.md describes it: a writer builds a new  *)
(* RuleSet off to the side and swaps an Arc in (arc-swap); each evaluation  *)
(* takes one Arc at its start and evaluates every rule with it.           *)
(*                                                                         *)
(* Builds are not atomic (rules are inserted one at a time), loads and     *)
(* drops are separate steps, and a version is freed when its last holder   *)
(* drops it. Readers and the writer interleave arbitrarily.                *)
(***************************************************************************)
EXTENDS Integers, FiniteSets

CONSTANTS
    Readers,      \* concurrent evaluations
    NumRules,     \* rules per set
    MaxVersion,   \* versions the writer publishes
    Bug           \* "none" in dictum; anything else is a seeded bug (see Bugs)

(* The seeded bugs. Each *_mutant.cfg turns one on and names the invariant
   that must catch it; check.sh fails if TLC does not report that invariant.
     publish_early     swap in a set before every rule is inserted
     forget_readers    free the old set on swap, as if readers held no Arc
     reload_per_rule   load the current set for every rule, not once per evaluation *)
Bugs == {"none", "publish_early", "forget_readers", "reload_per_rule"}

ASSUME Bug \in Bugs

Versions == 0..MaxVersion

VARIABLES
    built,        \* [Versions -> 0..NumRules]: rules inserted so far
    current,      \* the version the ArcSwap points at
    building,     \* the version the writer is building, or -1
    refs,         \* [Versions -> Nat]: Arc strong counts (the ArcSwap holds one)
    freed,        \* versions whose memory is released
    reader        \* [Readers -> record]: idle, or evaluating `ver` at rule `k`

vars == <<built, current, building, refs, freed, reader>>

Idle == [busy |-> FALSE, ver |-> 0, k |-> 0, seen |-> {}]

Init ==
    /\ built = [v \in Versions |-> IF v = 0 THEN NumRules ELSE 0]
    /\ current = 0
    /\ building = -1
    /\ refs = [v \in Versions |-> IF v = 0 THEN 1 ELSE 0]
    /\ freed = {}
    /\ reader = [r \in Readers |-> Idle]

\* The writer starts the next version (RuleSet::builder).
StartBuild ==
    /\ building = -1 /\ current < MaxVersion
    /\ building' = current + 1
    /\ refs' = [refs EXCEPT ![current + 1] = 1]
    /\ UNCHANGED <<built, current, freed, reader>>

\* RuleSetBuilder::insert
Insert ==
    /\ building # -1 /\ built[building] < NumRules
    /\ built' = [built EXCEPT ![building] = @ + 1]
    /\ UNCHANGED <<current, building, refs, freed, reader>>

\* build() then ArcSwap::store: the old Arc held by the swap is dropped.
Publish ==
    /\ building # -1
    /\ Bug = "publish_early" \/ built[building] = NumRules
    /\ current' = building
    /\ building' = -1
    /\ refs' = [refs EXCEPT ![current] = IF Bug = "forget_readers" THEN 0 ELSE @ - 1]
    /\ UNCHANGED <<built, freed, reader>>

\* An evaluation starts: ArcSwap::load_full clones the current Arc.
Load(r) ==
    /\ ~reader[r].busy
    /\ reader' = [reader EXCEPT ![r] = [busy |-> TRUE, ver |-> current, k |-> 0, seen |-> {}]]
    /\ refs' = [refs EXCEPT ![current] = @ + 1]
    /\ UNCHANGED <<built, current, building, freed>>

\* Evaluate the next rule of the held set.
Step(r) ==
    /\ reader[r].busy /\ reader[r].k < NumRules
    /\ reader' = [reader EXCEPT ![r].k = @ + 1,
                                ![r].seen = @ \cup {IF Bug = "reload_per_rule" THEN current ELSE reader[r].ver}]
    /\ UNCHANGED <<built, current, building, refs, freed>>

\* The evaluation ends and drops its Arc.
Drop(r) ==
    /\ reader[r].busy /\ reader[r].k = NumRules
    /\ refs' = [refs EXCEPT ![reader[r].ver] = @ - 1]
    /\ reader' = [reader EXCEPT ![r] = Idle]
    /\ UNCHANGED <<built, current, building, freed>>

\* The last Arc is gone: the memory is released.
Free(v) ==
    /\ v \notin freed /\ refs[v] = 0 /\ built[v] > 0
    /\ freed' = freed \cup {v}
    /\ UNCHANGED <<built, current, building, refs, reader>>

Next ==
    \/ StartBuild \/ Insert \/ Publish
    \/ \E r \in Readers : Load(r) \/ Step(r) \/ Drop(r)
    \/ \E v \in Versions : Free(v)

Spec == Init /\ [][Next]_vars /\ WF_vars(Insert \/ Publish) /\ \A r \in Readers : WF_vars(Step(r) \/ Drop(r))

-----------------------------------------------------------------------------
(* Properties *)

TypeOK ==
    /\ current \in Versions /\ refs \in [Versions -> Nat] /\ freed \subseteq Versions

\* Readers only ever see complete rule sets.
OnlyCompleteSetsArePublished == built[current] = NumRules

\* A set in use is never freed (no use after free), and the published one never.
NoUseAfterFree ==
    /\ current \notin freed
    /\ \A r \in Readers : reader[r].busy => reader[r].ver \notin freed

\* One evaluation uses one set for all its rules, however many swaps happen.
OneSetPerEvaluation == \A r \in Readers : Cardinality(reader[r].seen) <= 1

\* Every evaluation that starts finishes.
EvaluationsFinish == \A r \in Readers : reader[r].busy ~> ~reader[r].busy
=============================================================================
