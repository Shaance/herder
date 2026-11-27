# 0002. Keep dispatch policy with the host

The coordinating agent already knows the user's goal, what it may change, and whether it may spend worker quota. It can request a review or a bounded implementation directly. The local companion translates that choice into a Herder brief. The host selects the worker model and effort, and Herder owns the run, its identity, its attempts and its verification record.

## Checkout and recovery

A worktree root identifies the directory a worker can touch. A branch name or commit alone can identify several directories. A caller can compare its intended root with the registered checkout before spending quota. The CLI checks ownership before observing an executor or opening a report.

A request ID identifies one initial launch in one checkout. The CLI claims it before starting the worker, so callers can reconnect through another MCP process sharing the state directory. Reusing the key with different work is a conflict. If the original process disappears between claiming the request and publishing the run, the caller retains the reserved ID. An operator can investigate without an automatic second launch.

## Write authority

The operator enables writing for a dedicated checkout and selects the allowed path prefixes and an external acceptance program. The caller narrows that scope for a work order. A clean Git baseline and exclusive use of the worktree let the operator attribute its changes to that work order. A worker has executor-level workspace permissions during the run. Scope verification checks Git-visible changes afterward and leaves the evidence in place.

The CLI records which protected run owns the checkout. A later protected work order must wait until the previous one has a verdict or a definitive unsuccessful worker outcome. The host's acceptance program should inspect the result as data or run project code in its own sandbox. It executes with the coordinator's permissions. Herder records the first verdict and checks the final Git scope before accepting a successful check. Retrieving an earlier verdict after the worktree is reused still returns that work order's evidence.

## When a Pi router would earn its place

Record representative tasks, the coordinator's authorized choices, the route taken, verified outcomes, elapsed time and total cost. Include simple reviews, bounded edits, ambiguous requests and tasks that required decomposition. On a held-out set, compare a cheap selector with the best fixed route under the same budget. Count the selector call, retries and failed work. Agree on a minimum useful improvement in cost or latency at equal verified success before running the comparison.

A selector may recommend a route within the host's permissions. A lead additionally needs a harness that can start and account for child runs, an explicit delegation budget, and evidence that a bounded worker could not complete those tasks alone. A persistent Pi session and ACP can be evaluated when a caller needs to steer one across turns.
