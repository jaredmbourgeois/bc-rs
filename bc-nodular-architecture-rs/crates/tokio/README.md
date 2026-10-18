# bc-nodular-tokio

A tokio driver for [`bc-nodular-architecture-rs`](../../README.md)'s stepping API.

The core crate deliberately mandates no runtime: it ships a deterministic driver and a
`std::thread` driver, and exposes `submit` / `step` / `step_quantum` so any executor can drive it.
This crate is the tokio *consumer* of that seam — not a third driver, and not a dependency the core
takes on. It exists so every application that already runs on tokio shares one definition of the
adapter instead of each rewriting it.

**This crate is not a port.** It has no counterpart in `bourgeois_concern`, so it records no source
revision and nothing in it is measured against a C++ original.

## Using it

```rust
use std::num::NonZeroUsize;

use bc_date_time::{Epoch, fixed_epoch_source};
use bc_nodular_architecture::{
    Input, NodeLoop, NodeResult, Nodule, NoduleEventSequence, NoduleLoop, Output, SharedDependency,
    Submission,
};

#[derive(PartialEq)]
struct Tick;

impl Input for Tick {
    fn sequence(&self) -> NoduleEventSequence { 0 }
}

#[derive(PartialEq)]
struct Tocked(usize);

impl Output for Tocked {
    fn sequence(&self) -> NoduleEventSequence { 0 }
}

struct Clock;

impl Nodule for Clock {
    type Input = Tick;
    type Output = Tocked;
    type State = usize;
    type Dependency = SharedDependency;

    fn receive(_: &Tick, state: &mut usize, _: &SharedDependency) -> NodeResult<Tocked> {
        *state += 1;
        NodeResult::Output(Tocked(*state))
    }
}

impl NoduleLoop for Clock {
    fn input_from_output(_: &Tocked, _: &usize, _: &SharedDependency) -> Option<Tick> {
        None
    }
}

# fn main() {
let runtime = tokio::runtime::Builder::new_current_thread().build().expect("runtime");
let ticked = runtime.block_on(async {
    let capacity = NonZeroUsize::new(8).expect("8 is not zero");
    let (submitter, submissions) = bc_nodular_tokio::channel::<Clock>(capacity);

    for _ in 0..3 {
        submitter.send(Submission::Input(Tick)).await.expect("capacity is free");
    }
    drop(submitter);

    bc_nodular_tokio::run(
        NodeLoop::<Clock>::new(SharedDependency::new(fixed_epoch_source(Epoch::ZERO)), 0),
        submissions,
        bc_nodular_tokio::DEFAULT_QUANTUM,
    )
    .await
});

assert_eq!(ticked, 3);
# }
```

## Why the future is not `Send`

`NodeLoop`'s output observer is intentionally not required to be `Send`, so a wasm consumer can
hold one. That makes the future `run` returns non-`Send` too, which rules out bare `tokio::spawn`.
Drive it on a current-thread runtime, inside a [`LocalSet`], or through `Handle::block_on` in a
blocking task. A loop that genuinely has to move between threads wants the core's own
`ThreadNodeLoopRunner`, which requires a `Send` observer and needs no runtime at all.

[`LocalSet`]: https://docs.rs/tokio/latest/tokio/task/struct.LocalSet.html

## Building and testing

The adapter depends on the core crate and tokio alone. The member's `scripts/verify.sh` covers both
its packages and refuses any other direct dependency here, and bc-rs's root `scripts/verify.sh` runs
it with every other member's.
