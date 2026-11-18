--------------------------- MODULE Trace_accept_error -----------------------
\* The second rule fails at n3; the set stops with that error.
EXTENDS RuleSetTrace

RunNodes == {"n1", "n2", "n3"}
RunRules == << <<"n1", "n2">>, <<"n1", "n3">> >>
RunCost == [n \in RunNodes |-> 1]
RunShared == {"n1"}
RunBudget == 3
Ok == [status |-> "ok", rule |-> 0, kind |-> "none", node |-> "none"]
RunOutcome == [n \in RunNodes |-> IF n = "n3" THEN "err" ELSE "ok"]
RunEvents == << [op |-> "compute", node |-> "n1"], [op |-> "compute", node |-> "n2"], [op |-> "next"], [op |-> "hit", node |-> "n1"], [op |-> "fail", node |-> "n3", kind |-> "eval"] >>
RunResult == [status |-> "err", rule |-> 2, kind |-> "eval", node |-> "n3"]
=============================================================================
