---------------------------- MODULE RuleSetTrace ----------------------------
(***************************************************************************)
(* Trace validation: is a run that RuleSet::eval recorded a behavior of    *)
(* RuleSetEval?                                                            *)
(*                                                                         *)
(* RuleSetEval checks the design over every small configuration. This      *)
(* module checks the implementation against the design, one recorded run   *)
(* at a time: the configuration the run used (rules as node sequences,     *)
(* outcomes, costs, shared nodes, budget), the events it recorded, and the *)
(* result it returned. Each event must be a step RuleSetEval allows from   *)
(* the current state; an event it does not allow leaves no successor, and  *)
(* TLC reports a deadlock. At the end, the returned result must be the     *)
(* model's.                                                                *)
(*                                                                         *)
(* A trace is a module that EXTENDS this one and defines Run* operators,  *)
(* which its cfg substitutes for the constants: see Trace_accept_*.tla     *)
(* and Trace_reject_*.tla. From M5 on, a Rust test records traces of       *)
(* generated rule sets and checks them the same way.                       *)
(*                                                                         *)
(* Events, in the order the engine performs them:                          *)
(*   [op |-> "hit",     node |-> n]  read a filled cache slot              *)
(*   [op |-> "compute", node |-> n]  compute a node, which succeeds        *)
(*   [op |-> "fail",    node |-> n, kind |-> "eval" | "limit"]             *)
(*   [op |-> "next"]                 a rule finished                       *)
(***************************************************************************)
EXTENDS Naturals, Sequences

CONSTANTS
    TNodes,      \* node keys
    TRules,      \* <<rule, ...>>, each a sequence of node keys
    TOutcome,    \* [TNodes -> {"ok", "err"}]
    TCost,       \* [TNodes -> Nat]
    TShared,     \* nodes with a cache slot
    TBudget,     \* Limits::max_steps
    Trace,       \* the recorded events
    TResult      \* the result RuleSet::eval returned, as RuleSetEval writes it

VARIABLES rules, outcome, cost, shared, r, i, cache, writes, charged, result, pos

vars == <<rules, outcome, cost, shared, r, i, cache, writes, charged, result>>

M == INSTANCE RuleSetEval WITH
    Nodes <- TNodes, NumRules <- Len(TRules), MaxLen <- 1000, MaxCost <- 1000,
    Budget <- TBudget, Bug <- "none"

Init ==
    /\ rules = [q \in 1..Len(TRules) |-> TRules[q]]
    /\ outcome = TOutcome /\ cost = TCost /\ shared = TShared
    /\ r = 1 /\ i = 1
    /\ cache = [n \in TNodes |-> "empty"]
    /\ writes = [n \in TNodes |-> 0]
    /\ charged = [q \in 1..Len(TRules) |-> 0]
    /\ result = M!Running
    /\ pos = 1

Event == Trace[pos]

Replay ==
    /\ pos <= Len(Trace)
    /\ \/ Event.op = "hit" /\ M!Hit /\ rules[r][i] = Event.node
       \/ Event.op = "compute" /\ M!Compute /\ rules[r][i] = Event.node /\ result' = M!Running
       \/ Event.op = "fail" /\ M!Compute /\ rules[r][i] = Event.node
          /\ result' = M!Fail(r, Event.kind, Event.node)
       \/ Event.op = "next" /\ M!NextRule
    /\ pos' = pos + 1

Finished == pos > Len(Trace) /\ UNCHANGED <<vars, pos>>

Spec == Init /\ [][Replay \/ Finished]_<<vars, pos>>

\* Once every event is replayed, the model's result is the one the engine returned.
SameResult == pos > Len(Trace) => result = TResult

\* The design's invariants hold along the recorded run too.
CacheHoldsOnlyValues == M!CacheHoldsOnlyValues
SlotWrittenOnce == M!SlotWrittenOnce
NeverChargedMore == M!NeverChargedMore
=============================================================================
