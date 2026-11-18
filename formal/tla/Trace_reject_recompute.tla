------------------------------- MODULE Trace_reject_recompute -----------------------
\* Must be rejected: the second rule recomputes n1 although its slot is filled.
EXTENDS RuleSetTrace

RunNodes == {"n1", "n2", "n3"}
RunRules == << <<"n1", "n2">>, <<"n1", "n3">> >>
RunCost == [n \in RunNodes |-> 1]
RunShared == {"n1"}
RunBudget == 3
Ok == [status |-> "ok", rule |-> 0, kind |-> "none", node |-> "none"]
RunOutcome == [n \in RunNodes |-> "ok"]
RunEvents == << [op |-> "compute", node |-> "n1"], [op |-> "compute", node |-> "n2"], [op |-> "next"], [op |-> "compute", node |-> "n1"], [op |-> "compute", node |-> "n3"], [op |-> "next"] >>
RunResult == Ok
=============================================================================
