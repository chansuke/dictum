---------------------------- MODULE RuleSetEval ----------------------------
(***************************************************************************)
(* One RuleSet::eval with the shared-subexpression cache (engine/analysis.rs, *)
(* engine/frame.rs, rule_set.rs).                                          *)
(*                                                                         *)
(* A rule is the sequence of node evaluations its closure tree performs.  *)
(* Each node has a fixed outcome (value or runtime error) and a step cost. *)
(* Shared nodes have a cache slot: the first successful computation fills  *)
(* it, later reads are free. Rules run in insertion order, each with its   *)
(* own budget, and the set stops at the first error.                       *)
(*                                                                         *)
(* Init chooses every rule set, outcome, cost and sharing choice of the    *)
(* given sizes, so TLC checks all of them. The properties compare the set  *)
(* with each rule evaluated alone (no cache), which is what the proptest   *)
(* rule_set_agrees_with_standalone_rules checks on generated rules.        *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Nodes,        \* structural keys of subexpressions
    NumRules,     \* rules in the set
    MaxLen,       \* node evaluations per rule
    MaxCost,      \* step cost of one node
    Budget,       \* Limits::max_steps, per rule
    Bug           \* "none" in dictum; anything else is a seeded bug (see Bugs)

ASSUME NumRules \in Nat \ {0} /\ MaxLen \in Nat \ {0} /\ MaxCost \in Nat \ {0}
(* The seeded bugs. Each *_mutant.cfg turns one on and names the invariant
   that must catch it; check.sh fails if TLC does not report that invariant.
     budget_per_set    one budget for the whole set instead of one per rule
     cache_errors      a failed node fills its slot as if it had a value
     ignore_cache      shared nodes are recomputed and their slot rewritten *)
Bugs == {"none", "budget_per_set", "cache_errors", "ignore_cache"}

ASSUME Budget \in Nat /\ Bug \in Bugs

RuleIds == 1..NumRules
Traces == UNION {[1..n -> Nodes] : n \in 1..MaxLen}

VARIABLES
    rules, outcome, cost, shared,   \* the configuration, fixed by Init
    r, i,                           \* the next node is rules[r][i]
    cache,                          \* [Nodes -> {"empty", "value"}]
    writes,                         \* slot writes per node
    charged,                        \* steps charged per rule
    result                          \* [status, rule, kind, node]; see Running, Ok, Fail

vars == <<rules, outcome, cost, shared, r, i, cache, writes, charged, result>>

(* A rule evaluated alone: no cache, every node charged. Returns its result
   and the steps it charged before stopping. *)
NoNode == "none"
Running == [status |-> "running", rule |-> 0, kind |-> "none", node |-> NoNode]
Ok == [status |-> "ok", rule |-> 0, kind |-> "none", node |-> NoNode]
Fail(q, kind, n) == [status |-> "err", rule |-> q, kind |-> kind, node |-> n]

(* A rule evaluated alone: no cache, every node charged. `kind` is "ok",
   "eval" (a runtime error at `node`) or "limit". *)
RECURSIVE Alone(_, _, _)
Alone(t, k, spent) ==
    IF k > Len(t) THEN [kind |-> "ok", node |-> NoNode, spent |-> spent]
    ELSE LET n == t[k] IN
         IF spent + cost[n] > Budget THEN [kind |-> "limit", node |-> n, spent |-> spent]
         ELSE IF outcome[n] = "err" THEN [kind |-> "eval", node |-> n, spent |-> spent + cost[n]]
         ELSE Alone(t, k + 1, spent + cost[n])

Standalone(q) == Alone(rules[q], 1, 0)

(* The same rule alone with no budget: what it means, budgets aside. *)
RECURSIVE Unbounded(_, _)
Unbounded(t, k) ==
    IF k > Len(t) THEN [kind |-> "ok", node |-> NoNode]
    ELSE IF outcome[t[k]] = "err" THEN [kind |-> "eval", node |-> t[k]]
    ELSE Unbounded(t, k + 1)

Meaning(q) == Unbounded(rules[q], 1)

Init ==
    /\ rules \in [RuleIds -> Traces]
    /\ outcome \in [Nodes -> {"ok", "err"}]
    /\ cost \in [Nodes -> 1..MaxCost]
    /\ shared \in SUBSET Nodes
    /\ r = 1 /\ i = 1
    /\ cache = [n \in Nodes |-> "empty"]
    /\ writes = [n \in Nodes |-> 0]
    /\ charged = [q \in RuleIds |-> 0]
    /\ result = Running

Fixed == UNCHANGED <<rules, outcome, cost, shared>>

Spent == IF Bug # "budget_per_set" THEN charged[r]
         ELSE LET S[q \in 0..NumRules] == IF q = 0 THEN 0 ELSE S[q - 1] + charged[q] IN S[NumRules]

\* The current rule has finished: move on, or finish the set.
NextRule ==
    /\ result = Running /\ i > Len(rules[r])
    /\ IF r = NumRules THEN result' = Ok /\ UNCHANGED <<r, i>>
       ELSE r' = r + 1 /\ i' = 1 /\ UNCHANGED result
    /\ Fixed /\ UNCHANGED <<cache, writes, charged>>

\* Read a filled slot: no work, no charge.
Hit ==
    LET n == rules[r][i] IN
    /\ result = Running /\ i <= Len(rules[r])
    /\ n \in shared /\ cache[n] = "value"
    /\ Bug # "ignore_cache"
    /\ i' = i + 1
    /\ Fixed /\ UNCHANGED <<r, cache, writes, charged, result>>

\* Compute a node: charge first (Budget::step), then fail or produce a value,
\* and fill the slot only on success.
Compute ==
    LET n == rules[r][i] IN
    /\ result = Running /\ i <= Len(rules[r])
    /\ (~(n \in shared /\ cache[n] = "value") \/ Bug = "ignore_cache")
    /\ IF Spent + cost[n] > Budget THEN
          /\ result' = Fail(r, "limit", n)
          /\ UNCHANGED <<i, cache, writes, charged>>
       ELSE IF outcome[n] = "err" THEN
          /\ result' = Fail(r, "eval", n)
          /\ charged' = [charged EXCEPT ![r] = @ + cost[n]]
          /\ IF Bug = "cache_errors" /\ n \in shared
                THEN cache' = [cache EXCEPT ![n] = "value"]
                ELSE UNCHANGED cache
          /\ UNCHANGED <<i, writes>>
       ELSE
          /\ charged' = [charged EXCEPT ![r] = @ + cost[n]]
          /\ IF n \in shared
                THEN /\ cache' = [cache EXCEPT ![n] = "value"]
                     /\ writes' = [writes EXCEPT ![n] = @ + 1]
                ELSE UNCHANGED <<cache, writes>>
          /\ i' = i + 1
          /\ UNCHANGED result
    /\ Fixed /\ UNCHANGED r

Done == result # Running /\ UNCHANGED vars

Next == NextRule \/ Hit \/ Compute \/ Done

Spec == Init /\ [][Next]_vars /\ WF_vars(NextRule \/ Hit \/ Compute)

-----------------------------------------------------------------------------
(* Properties *)

TypeOK ==
    /\ r \in RuleIds /\ i \in 1..(MaxLen + 1)
    /\ cache \in [Nodes -> {"empty", "value"}]
    /\ charged \in [RuleIds -> Nat]

\* Errors are never cached: a filled slot holds a value the node really produces.
CacheHoldsOnlyValues == \A n \in Nodes : cache[n] = "value" => outcome[n] = "ok" /\ n \in shared

\* A slot is written at most once per evaluation.
SlotWrittenOnce == \A n \in Nodes : writes[n] <= 1

\* Sharing never makes a rule pay more than it would alone.
NeverChargedMore == \A q \in 1..r : charged[q] <= Standalone(q).spent

\* Budgets aside, sharing changes nothing: the set succeeds only if every rule
\* means a value, and a runtime error is the error of the first rule whose
\* meaning is an error, at the same node.
\*
\* "Budgets aside" matters. A rule may run out of budget alone yet finish in the
\* set because another rule paid for shared work (the tradeoff accepted in
\* docs/architecture.md), and so reach, and report, a runtime error that alone
\* it never got to. TLC found this: comparing with the budgeted standalone
\* result instead of Meaning is violated.
OkMeansEveryRuleMeansAValue ==
    result = Ok => \A q \in RuleIds : Meaning(q).kind = "ok"

ErrorIsFirstMeaningfulError ==
    (result.status = "err" /\ result.kind = "eval") =>
        /\ Meaning(result.rule) = [kind |-> "eval", node |-> result.node]
        /\ \A q \in 1..(result.rule - 1) : Meaning(q).kind = "ok"

\* If every rule succeeds alone within its budget, the set succeeds.
StandaloneOkMeansSetOk ==
    ((\A q \in RuleIds : Standalone(q).kind = "ok") /\ result # Running) => result = Ok

\* A rule that runs out of budget in the set also does alone.
LimitMeansStandaloneLimit ==
    (result.status = "err" /\ result.kind = "limit") => Standalone(result.rule).kind = "limit"

\* Every evaluation finishes.
Terminates == <>(result # Running)
=============================================================================
