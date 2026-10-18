# bc-nodular-architecture-rs

A runtime-neutral, unidirectional data-flow core: nodules that turn input and state into output, nodes that wire them,
and a stepping loop any executor can drive — a deterministic one, a `std::thread` one, or tokio through
[`bc-nodular-tokio`](crates/tokio/README.md).

The core depends on `bc-core-rs` and `bc-date-time-rs` alone, mandates no async runtime, and builds for
`wasm32-unknown-unknown`.

Depend on it by git and release tag, as the [workspace README](../README.md#depending-on-bc-rs) shows; the package is
`bc-nodular-architecture-rs` and the library is imported as `bc_nodular_architecture`.

It is a semantics-preserving port of the C++ `libs/nodular_framework/nodular_architecture` library of Bourgeois
Concern's codebase: the same API and behaviour, with every case of the original's test suite carried into this crate's
tests or set aside for a recorded reason.

Licensed under Apache-2.0 ([LICENSE.txt](LICENSE.txt)); copyright 2026 Jared Bourgeois.

## Architecture

The crate provides the nodule contracts, `NodeResult` effect algebra, `Node`, consuming
`NodeBuilder`, binary and flat N-way composition, application-lifecycle nodule, injected shared
dependencies, and one iterative `NodeLoop` queue core.

The crate mandates no runtime and depends on no runtime. `NodeLoop` is the deterministic driver:
`receive` and `send` return only after every consequence has been processed. The same core is also
driveable a piece at a time — `submit` queues one `Submission`, `step` performs at most one piece of
work, and `step_quantum` performs a bounded batch — so a caller's own executor can interleave its
work without breaking causal order. `ThreadNodeLoopRunner` is the second driver, over a bounded
standard channel and a thread the caller owns; the crate itself never spawns anything. A successful
send means queue acceptance, not processing. A handle can submit an ordered graceful-close marker,
after which the runner rejects new work and drains everything already accepted, returning each
rejected payload to its sender.

One owner drains inputs, outputs, feedback, and suspended `Run` continuations. `Run` is a
scheduler-pulled producer: each advance performs one short, nonblocking, I/O-free producer,
transform, or expansion operation. After an output, its observation and complete causal feedback
precede the next continuation step; unrelated accepted work remains behind that causal chain. Every
driver therefore uses bounded scheduler stack.

A `Run` holds its producers in a `bc-core-rs` generational arena and its transforms in a table
beside it, and links each operation's remaining chain by index, so handing a produced output the
chain its producer still holds costs two indices rather than a clone. Only the producers need an
arena: they are released and their slots reused, and the generation in a handle is what makes a
handle to a released producer stop resolving rather than name its successor. Transforms and pipeline
links are appended and never removed, so a plain table says as much and costs a narrower index. Until something is put in front of it, a run keeps its source typed and hands outputs
straight to the scheduler, with no type erasure at all; once a stage exists, each value travels in a
carrier that is handed back to the stage that filled it, so a steady-state chain of maps allocates
nothing per output. A `flat_map` is not steady state — each expansion brings a producer of its own —
but a producer's arena slot is released the moment it runs dry and reused by the next, so the arena
is sized by the producers alive at once rather than by every output the run has ever made.
Nothing inside a `Run` is shared across threads and it holds no lock. The
crate's only lock is `Node`'s state mutex, which is that type's whole contract; a checked-in guard
keeps it the only one, and confines every other thread-coordination primitive — the thread runner's
bounded channel and the event-sequence counter — to the one file whose contract needs it.

The normal dependency graph contains only the first-party `bc-core-rs` and `bc-date-time-rs`
siblings, with every feature enabled, because the crate has no features. `bc-core-rs` supplies the
generational arena a `Run` holds its producers in; both are dependency-free themselves, so the
transitive graph is those three crates and nothing more. Driving the core from an async runtime is a
consumer's concern, not this crate's: the workspace's `bc-nodular-tokio` member is one such
consumer, written against the stepping API above, and nothing here depends on it.

## A nodule, end to end

A nodule is a pure `receive(input, &mut state, dependency) -> NodeResult<Output>`: no clock, no
I/O, no ambient state. That is what makes one testable by calling it, and it is the whole of the
contract a consumer implements.

```rust
use bc_date_time::{Epoch, fixed_epoch_source};
use bc_nodular_architecture::{Input, NodeResult, Nodule, NoduleEventSequence, Output, SharedDependency};

#[derive(PartialEq)]
struct Deposit(i32);

impl Input for Deposit {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(PartialEq)]
struct Balance(i32);

impl Output for Balance {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

struct Account;

impl Nodule for Account {
    type Input = Deposit;
    type Output = Balance;
    type State = i32;
    type Dependency = SharedDependency;

    fn receive(
        deposit: &Self::Input,
        balance: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        *balance += deposit.0;
        NodeResult::none()
    }
}

let dependency = SharedDependency::new(fixed_epoch_source(Epoch::ZERO));
let mut balance = 0;
Account::receive(&Deposit(30), &mut balance, &dependency);
Account::receive(&Deposit(12), &mut balance, &dependency);
assert_eq!(balance, 42);
```

## Performance

Nanoseconds per operation, lower better, on one Apple Silicon machine (macOS 24.6.0, Rust 1.85.0
release, `clang++ -std=c++20 -O2`). The whole set drifts several percent between sessions, so a
"before" from an earlier day compares two machines rather than two revisions; every claim of movement
here therefore comes from alternating two trees run-for-run within one sitting, never from two
columns measured on different days. "Rust before" is carried forward from the sitting that first
measured it, and the tree it names still reproduces within noise. The
C++ column is the source library measured on the same workloads. Numbers are indicative of shape, not a benchmark
suite: run them yourself before relying on any one of them.

| Workload | Rust before | Rust | C++ |
|---|---|---|---|
| W1 node receive (1 thread) | 9.55 | 9.20 | 9.75 |
| W2 node receive (4 threads) | 26.95 | 27.40 | 25.72 |
| W3 loop feedback cascade | 11.90 | 9.58 | 48.90 |
| W4 run pipeline (0 stages) | 18.48 | 1.92 | 2.00 |
| W4 run pipeline (1 stages) | 52.96 | 2.46 | 2.00 |
| W4 run pipeline (8 stages) | 297.81 | 7.59 | 11.04 |
| W5 combined receive | 9.71 | 10.68 | 7.38 |
| W5 linked receive | 9.51 | 9.33 | 15.75 |

What the workloads isolate: W1 and W2 are `Node::receive` uncontended and under four threads, so
they measure dispatch and the state lock. W3 is one `NodeLoop` feedback cascade. W4 is a `Run`
producing 100,000 outputs through a chain of map stages. W5 is composed nodules on the
immediate-output path.

"Rust before" is this crate with the transform arena but before five changes to how a run carries
values: a typed source that skips erasure entirely while nothing stands in front of it, `map` stages
folded into that source instead of erasing it, carriers that are handed back to the stage that filled
them instead of being freed, narrow arena indices that shrink an in-flight operation, and the arenas
themselves moved behind one box that is not made at all until a run has an operation to address.
Per-stage cost — reading `(8 stages − 0 stages) / 8` on both sides — fell from 34.9 ns to 0.71 ns,
against C++'s 1.13 ns.

Four results are worth reading carefully.

**W3 is not a like-for-like row.** The Rust cascade runs at depth 100,000; the C++ one is measured at
depth 10,000 because it recurses per feedback step and cannot reach 100,000 — it segfaults between
30,000 and 40,000 on an 8 MB stack. Bounded stack is the reason the port replaced recursive feedback
with a work queue, and this is what that costs and buys.

**W4 at zero stages is at parity** (1.92 against 2.00, inside the run-to-run spread of either). A
run that nothing has been put in front of hands its iterator's outputs straight to the scheduler: no
boxing, no downcast, no work deque, and no arena allocated at all.

**W4 with map stages was the honest weak spot, and is no longer.** It used to cost 21.2 ns per stage
against C++'s 1.13 — a work-deque round trip, two table hops, two type-erased downcasts and two
virtual calls, every one of them per stage per output. The whole of that was the cost of *leaving*
the zero-stage path, which `map` did on its first stage and never came back from. It no longer
leaves: a `map` folds into the still-typed source, so a mapped run is one iterator and a stage costs
one indirect call. Measured by alternating the two trees run-for-run, medians of eight: a one-stage
W4 went from 31.7 to 2.46 ns, an eight-stage chain from 172.7 to 7.59 ns, and **0.71 ns per stage
against C++'s 1.13**. The other six workloads moved by less than the run-to-run spread, which is
what a change confined to the `Run` map path should do.

The source reaches its 1.13 ns by pushing outputs through nested `std::function` calls, which cannot
be suspended, interleaved, or bounded. The fold gives up none of that. It is bounded by a constant —
`FUSION_LIMIT` in `src/node_result.rs` — that caps both the stages one `advance` may run and how deep
dropping the source recurses; past it, stages take the erased path exactly as before. `filter_map` is
never folded, because a filter that declined a long stretch of its input would search for the next
output inside a single `advance`, and how long that took would be the input's business rather than
the stage count's.

**Where the arenas live was decided by measurement, and it is a trade.** `size_of::<NodeResult<_>>()`
is 64 bytes with them boxed, 152 with them inline, against 88 before this work — and `NodeResult` is
what every nodule transition returns, so its size is paid on the crate's most common signature
whether or not a run ever grows a stage. Inline arenas are faster where a pipeline is deep
(W4 at 8 stages: 127 ns rather than 158, as both were measured then) and slower everywhere else,
regressing W3 and W5 combined by about 13% each against the baseline. Boxed wins six of the eight
workloads, so boxed is what ships. The deep pipeline sits at 1.7× the baseline once the arena
adoption below is counted.

**Holding producers in `bc-core-rs`'s generational arena costs about 9%, deliberately.** A
`bc_core::ArenaRef` is eight bytes against the four-byte index it replaced, which takes `Filler` from
8 to 12 and `Operation` — the element the work deque moves once per stage — from 48 to 56. Measured
by alternating the two trees run-for-run: an 8-stage W4 went from 157.06 to 171.65 and a 1-stage
one from 26.5 to 28.9, while 0 stages *improved* (2.18 to 1.98) because that path returns from the
typed source and never reaches the arena. `size_of::<NodeResult<_>>()` is unchanged at 64, since
the tables stay boxed. What it buys: a handle to a released producer stops resolving instead of
being hedged by a poison sentinel, and the family keeps one arena rather than a second hand-rolled
free list. In about a
quarter of arena-tree samples the 1-stage figure jumps to roughly 35 ns and then returns; the staged
tree never did this in the same sittings, and the cause is not identified — recorded rather than
explained.

**W5 combined receive sits about 6% above the baseline** (9.71 to 10.33) and is the one number here
without an explanation. It is not size: `NodeResult` is now smaller than the baseline's. It is not a
general dispatch cost: W1, W3, and W5 linked all improved across the same changes. It is 0.6 ns on
the immediate-output path, recorded rather than explained away.

### Approaches rejected, and why

Recorded so they are not re-derived. Each was considered against W4 specifically.

- **An _unbounded_ nested adapter chain**, where `map` wraps the source and no erasure ever happens.
  It is the fastest possible shape and unbounded is wrong here: `advance` would recurse one frame per
  stage and drop would recurse one frame per link, so a hundred thousand chained stages would
  overflow the stack — precisely the failure the C++ original has (see W3) and that this port exists
  to remove. **A bounded form of it was later taken**, and the bound is the whole difference: `map`
  folds into the source only while fewer than `FUSION_LIMIT` stages have, so recursion depth and the
  work one `advance` may do are both capped by a constant this crate chooses rather than by the
  caller's chain length. Everything past the cap takes the erased path. `deep_run_mapping_is_iterative`
  builds a 100,000-stage chain and is the test that says so.
- **Inline storage for small erased values**, which would remove the remaining allocation entirely.
  It requires `unsafe`, and `unsafe_code = "forbid"` is not negotiable for a performance win.
- **Running a whole output's pipeline inside one `advance`**, removing the per-stage deque round
  trip. It breaks the bounded-work contract a scheduler relies on: a deep pipeline would become
  unpreemptible, which is the property W3 is about.
- **Boxing the expansion variant** of the internal operation enum, to shrink what the work deque
  moves. Measured at 8 bytes off a 48-byte enum in exchange for an allocation per expansion — a bad
  trade, since expansion is the `flat_map` path.
- **Reaching the boxed arena once per step** instead of once per lookup, by destructuring the run so
  a step holds the arena and the deque as disjoint borrows. It reads as the obvious win and measured
  as the opposite: W4 at 8 stages went 158 ns to 174. Kept per-lookup on the strength of that.
- **Storing a bare slot index in `Filler`** rather than the whole `ArenaRef`, to take `Operation` back
  to 48 bytes. Recycling is the one path where naming the wrong live producer is harmless, since
  `accept_spare` drops a carrier whose type does not match — so it would have been sound. It needs an
  index-only accessor on `bc_core::Arena`, weakening that crate's whole guarantee for one consumer's
  hot path, and the 9% was accepted instead.
- **An arena, or a free list, for the transform and pipeline tables too.** They are appended and
  never removed, so generational handles would buy them nothing and cost the same id width on the
  same hot path — and `Pipeline` is two `Option<u32>` sitting *inline* in every `Operation`, the
  element the work deque moves once per stage, so widening it costs strictly more than the producer
  arena's 9% did. A free list would be worse than a free list sounds: a transform is named both by
  its pipeline nodes and by an in-flight `Filler`, so releasing one needs a reference count nothing
  tracks today; and pipeline chains are a shared, unowned singly linked structure whose only
  sharing information is one `shared` bit on the tail, with `copy_chain`'s abandoned originals a
  second unreclaimed lifetime on top. They stay `Vec`s.

  What that trade costs is smaller than it first reads and is now measured rather than assumed.
  Since `map` folds into the typed source, reaching either table at all takes an expansion whose
  *inner* run carries an erased stage — a `filter_map`, a nested `flat_map`, a `serial`/`concurrent`
  pair, or more than `FUSION_LIMIT` `map`s. An expansion returning `None`, `Output`, `Outputs`,
  `run(iter)`, or a fused `map` chain adds nothing to either. When the shape does occur the retained
  entry is a boxed closure, so what lives until the run does is the closure's *captured state*, not
  just index space. `tests/run_arena.rs`'s third test pins exactly that, next to the producer
  releases it contrasts with, and fails if a release path is ever added without revisiting this
  entry.
- **Replacing `catch_unwind` with an unwind-only drop guard** was kept, but it bought no measurable
  speed. It stays because it is less machinery, not because it is faster: cleanup that only ran to
  re-raise the panic is what `Drop` is for, and it removes an `AssertUnwindSafe` assertion that was
  papering over the question rather than answering it.

Reproduce the Rust figures with `scripts/bench.sh` here. The C++ figures come from a harness that is
not part of this repository, compiled against the C++ source with `clang++ -std=c++20 -O2`.

## Verification

Run `scripts/verify.sh`. It checks formatting, default/all-feature/debug/release tests, the
test-collection guard, doctests, Clippy with warnings denied, rustdoc, the feature matrix, wasm when
the target is installed (and names the skip when it is not), the port's and the adapter's dependency
budgets, runtime and lock independence, and that the benchmarks compile. Tests use nextest's CI
profile for per-test timeouts when nextest is installed.

## The stepping contract a resident consumer drives

How a long-running service built on this crate drives it: the `NodeLoop`, the work queue, the
tokio adapter, quantum bounds, and where the synchronous engine runs.

The event loop is a `NodeLoop` driven by the service's own tokio mpsc work queue for accepted
top-level work, fed through the port's stepping API (`submit`/`step_quantum`), over one
runtime-neutral causal queue for inputs, outputs, feedback, and resumable `Run`
continuations. The port mandates no runtime: the adapter pairing its stepping API with a
tokio channel is **`bc-nodular-tokio`**, this member's second package
(not a port), which the service wires at its composition root rather than redefining. The
adapter steps bounded quanta and yields without interleaving unrelated work into an
active causal chain; the port's own deterministic driver drains the same core
synchronously, and its standard-thread driver drives it from an owned thread with
bounded backpressure. `Run` is a short, nonblocking, I/O-free output producer — the
synchronous engine runs in a caller-owned `spawn_blocking` task or joined OS thread at
the service's boundary and returns through a typed input. Cron schedules follow the clock-free
polling-state-machine pattern (tick in, due-set out — no clock inside a nodule).
