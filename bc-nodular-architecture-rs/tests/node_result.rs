use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bc_nodular_architecture::{NodeResult, Run, RunStep};

#[derive(Debug, PartialEq, Eq)]
struct TestOutput {
    value: i32,
}

fn output(value: i32) -> NodeResult<TestOutput> {
    NodeResult::output(TestOutput { value })
}

fn outputs(values: &[i32]) -> NodeResult<TestOutput> {
    NodeResult::outputs(
        values
            .iter()
            .map(|value| TestOutput { value: *value })
            .collect(),
    )
}

fn run(values: &'static [i32]) -> NodeResult<TestOutput> {
    NodeResult::run(values.iter().map(|value| TestOutput { value: *value }))
}

fn collect(result: NodeResult<TestOutput>) -> Vec<i32> {
    match result {
        NodeResult::None => Vec::new(),
        NodeResult::Output(output) => vec![output.value],
        NodeResult::Outputs(outputs) => outputs.into_iter().map(|output| output.value).collect(),
        NodeResult::Run(run) => collect_run(run)
            .into_iter()
            .map(|output| output.value)
            .collect(),
    }
}

fn collect_i32(result: NodeResult<i32>) -> Vec<i32> {
    match result {
        NodeResult::None => Vec::new(),
        NodeResult::Output(output) => vec![output],
        NodeResult::Outputs(outputs) => outputs,
        NodeResult::Run(run) => collect_run(run),
    }
}

fn collect_run<O>(mut run: Run<O>) -> Vec<O>
where
    O: Send + 'static,
{
    let mut outputs = Vec::new();
    loop {
        match run.advance() {
            RunStep::Output(output) => outputs.push(output),
            RunStep::Continue => {}
            RunStep::Complete => return outputs,
        }
    }
}

#[test]
fn run_producer_delivers_output() {
    assert_eq!(collect(run(&[42])), [42]);
}

#[test]
fn run_producer_delivers_struct_output() {
    let result = NodeResult::run(std::iter::once(TestOutput { value: 99 }));

    assert_eq!(collect(result), [99]);
}

#[test]
fn none_constructs_the_none_variant() {
    assert!(matches!(NodeResult::<TestOutput>::none(), NodeResult::None));
}

#[test]
fn output_constructs_the_output_variant() {
    let result = output(42);

    assert!(matches!(
        result,
        NodeResult::Output(TestOutput { value: 42 })
    ));
}

#[test]
fn outputs_constructs_the_outputs_variant_in_source_order() {
    let result = outputs(&[1, 2, 3]);

    assert_eq!(collect(result), [1, 2, 3]);
}

#[test]
fn outputs_empty_vector_remains_distinct_from_none() {
    let result = NodeResult::<TestOutput>::outputs(Vec::new());

    assert!(matches!(result, NodeResult::Outputs(outputs) if outputs.is_empty()));
}

#[test]
fn run_constructs_the_run_variant() {
    assert!(matches!(run(&[]), NodeResult::Run(_)));
}

#[test]
fn run_executes_and_sends_zero_outputs() {
    let outputs = collect(run(&[]));
    assert!(outputs.is_empty(), "{outputs:?}");
}

#[test]
fn run_executes_and_sends_many_outputs() {
    assert_eq!(collect(run(&[1, 2, 3])), [1, 2, 3]);
}

#[test]
fn serial_none_plus_none_stays_none() {
    let result = NodeResult::<TestOutput>::none().serial(NodeResult::none());

    assert!(matches!(result, NodeResult::None));
}

#[test]
fn serial_none_plus_output_returns_the_output() {
    let result = NodeResult::none().serial(output(1));

    assert!(matches!(result, NodeResult::Output(_)));
    assert_eq!(collect(result), [1]);
}

#[test]
fn serial_none_plus_outputs_returns_the_outputs() {
    let result = NodeResult::none().serial(outputs(&[1, 2]));

    assert!(matches!(result, NodeResult::Outputs(_)));
    assert_eq!(collect(result), [1, 2]);
}

#[test]
fn serial_none_plus_run_returns_the_run() {
    let result = NodeResult::none().serial(run(&[1, 2]));

    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect(result), [1, 2]);
}

#[test]
fn serial_output_plus_none_returns_the_output() {
    let result = output(1).serial(NodeResult::none());

    assert!(matches!(result, NodeResult::Output(_)));
    assert_eq!(collect(result), [1]);
}

#[test]
fn serial_output_plus_output_flattens_in_order() {
    let result = output(1).serial(output(2));

    assert!(matches!(result, NodeResult::Outputs(_)));
    assert_eq!(collect(result), [1, 2]);
}

#[test]
fn serial_output_plus_outputs_flattens_in_order() {
    let result = output(1).serial(outputs(&[2, 3]));

    assert!(matches!(result, NodeResult::Outputs(_)));
    assert_eq!(collect(result), [1, 2, 3]);
}

#[test]
fn serial_output_plus_run_executes_in_order() {
    let result = output(1).serial(run(&[2, 3]));

    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect(result), [1, 2, 3]);
}

#[test]
fn serial_outputs_plus_none_returns_the_outputs() {
    let result = outputs(&[1, 2]).serial(NodeResult::none());

    assert!(matches!(result, NodeResult::Outputs(_)));
    assert_eq!(collect(result), [1, 2]);
}

#[test]
fn serial_outputs_plus_output_flattens_in_order() {
    let result = outputs(&[1, 2]).serial(output(3));

    assert!(matches!(result, NodeResult::Outputs(_)));
    assert_eq!(collect(result), [1, 2, 3]);
}

#[test]
fn serial_outputs_plus_outputs_flattens_in_order() {
    let result = outputs(&[1, 2]).serial(outputs(&[3, 4]));

    assert!(matches!(result, NodeResult::Outputs(_)));
    assert_eq!(collect(result), [1, 2, 3, 4]);
}

#[test]
fn serial_outputs_plus_run_executes_in_order() {
    let result = outputs(&[1, 2]).serial(run(&[3, 4]));

    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect(result), [1, 2, 3, 4]);
}

#[test]
fn serial_run_plus_none_returns_the_run() {
    let result = run(&[1, 2]).serial(NodeResult::none());

    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect(result), [1, 2]);
}

#[test]
fn serial_run_plus_output_executes_in_order() {
    let result = run(&[1, 2]).serial(output(3));

    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect(result), [1, 2, 3]);
}

#[test]
fn serial_run_plus_outputs_executes_in_order() {
    let result = run(&[1, 2]).serial(outputs(&[3, 4]));

    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect(result), [1, 2, 3, 4]);
}

#[test]
fn serial_run_plus_run_executes_in_order() {
    let result = run(&[1, 2]).serial(run(&[3, 4]));

    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect(result), [1, 2, 3, 4]);
}

#[test]
fn concurrent_none_plus_none_stays_none() {
    let result = NodeResult::<TestOutput>::none().concurrent(NodeResult::none());

    assert!(matches!(result, NodeResult::None));
}

#[test]
fn concurrent_none_plus_output_returns_the_output() {
    assert!(matches!(
        NodeResult::none().concurrent(output(1)),
        NodeResult::Output(TestOutput { value: 1 })
    ));
}

#[test]
fn concurrent_output_plus_none_returns_the_output() {
    assert!(matches!(
        output(1).concurrent(NodeResult::none()),
        NodeResult::Output(TestOutput { value: 1 })
    ));
}

#[test]
fn concurrent_output_plus_output_delivers_both_without_an_ordering_assertion() {
    let result = output(1).concurrent(output(2));
    assert!(matches!(result, NodeResult::Run(_)));
    let mut collected = collect(result);
    collected.sort_unstable();

    assert_eq!(collected, [1, 2]);
}

#[test]
fn concurrent_outputs_plus_run_proves_both_effect_branches_without_ordering() {
    let result = outputs(&[1, 2]).concurrent(run(&[3, 4]));
    let mut collected = collect(result);
    collected.sort_unstable();

    assert_eq!(collected, [1, 2, 3, 4]);
}

#[test]
fn transform_none_stays_none() {
    let result = NodeResult::<TestOutput>::none().map(|output| output.value * 2);

    assert!(matches!(result, NodeResult::None));
}

#[test]
fn transform_output_maps_synchronously() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let transform_call_count = Arc::clone(&call_count);
    let result = output(21).map(move |output| {
        transform_call_count.fetch_add(1, Ordering::SeqCst);
        output.value * 2
    });

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert!(matches!(result, NodeResult::Output(42)));
}

#[test]
fn transform_outputs_maps_synchronously_in_order() {
    let result = outputs(&[1, 2, 3]).map(|output| output.value * 10);

    assert!(matches!(result, NodeResult::Outputs(_)));
    assert_eq!(collect_i32(result), [10, 20, 30]);
}

#[test]
fn transform_empty_outputs_remains_empty_outputs() {
    let result = NodeResult::<TestOutput>::outputs(Vec::new()).map(|output| output.value);

    assert!(matches!(result, NodeResult::Outputs(outputs) if outputs.is_empty()));
}

#[test]
fn transform_run_maps_each_output_when_the_run_executes() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let transform_call_count = Arc::clone(&call_count);
    let result = run(&[1, 2]).map(move |output| {
        transform_call_count.fetch_add(1, Ordering::SeqCst);
        output.value * 3
    });

    assert_eq!(call_count.load(Ordering::SeqCst), 0);
    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect_i32(result), [3, 6]);
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[test]
fn transform_async_none_stays_none() {
    let result = NodeResult::<TestOutput>::none().map_effect(|output| output.value);

    assert!(matches!(result, NodeResult::None));
}

#[test]
fn transform_async_output_becomes_run_and_defers_mapping() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let transform_call_count = Arc::clone(&call_count);
    let result = output(5).map_effect(move |output| {
        transform_call_count.fetch_add(1, Ordering::SeqCst);
        output.value * 2
    });

    assert_eq!(call_count.load(Ordering::SeqCst), 0);
    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect_i32(result), [10]);
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[test]
fn transform_async_outputs_becomes_run_and_defers_mapping_in_order() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let transform_call_count = Arc::clone(&call_count);
    let result = outputs(&[1, 2, 3]).map_effect(move |output| {
        transform_call_count.fetch_add(1, Ordering::SeqCst);
        output.value * 10
    });

    assert_eq!(call_count.load(Ordering::SeqCst), 0);
    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect_i32(result), [10, 20, 30]);
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
}

#[test]
fn transform_async_run_maps_each_output_when_the_run_executes() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let transform_call_count = Arc::clone(&call_count);
    let result = run(&[1, 2]).map_effect(move |output| {
        transform_call_count.fetch_add(1, Ordering::SeqCst);
        output.value * 4
    });

    assert_eq!(call_count.load(Ordering::SeqCst), 0);
    assert!(matches!(result, NodeResult::Run(_)));
    assert_eq!(collect_i32(result), [4, 8]);
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[test]
fn deep_serial_run_composition_is_iterative() {
    const DEPTH: i32 = 100_000;

    let result = (0..DEPTH).fold(NodeResult::none(), |result, value| {
        result.serial(NodeResult::run(std::iter::once(value)))
    });
    let outputs = collect_i32(result);

    assert_eq!(outputs.len(), usize::try_from(DEPTH).unwrap());
    assert_eq!(outputs.first(), Some(&0));
    assert_eq!(outputs.last(), Some(&(DEPTH - 1)));
}

#[test]
fn deep_concurrent_run_composition_is_iterative() {
    const DEPTH: i32 = 100_000;

    let result = (0..DEPTH).fold(NodeResult::none(), |result, value| {
        result.concurrent(NodeResult::run(std::iter::once(value)))
    });
    let outputs = collect_i32(result);

    assert_eq!(outputs.len(), usize::try_from(DEPTH).unwrap());
}

#[test]
fn deep_run_mapping_is_iterative() {
    const DEPTH: i32 = 100_000;

    let mut result = NodeResult::run(std::iter::once(1));
    for _ in 0..DEPTH {
        result = result.map(|value| value + 1);
    }

    assert_eq!(collect_i32(result), [DEPTH + 1]);
}

#[test]
fn deep_effect_mapping_is_iterative() {
    const DEPTH: i32 = 100_000;

    let mut result = NodeResult::output(1);
    for _ in 0..DEPTH {
        result = result.map_effect(|value| value + 1);
    }

    assert_eq!(collect_i32(result), [DEPTH + 1]);
}

#[test]
fn one_hundred_thousand_flat_maps_advance_iteratively() {
    const DEPTH: i32 = 100_000;

    let mut run = Run::new(std::iter::once(0));
    for _ in 0..DEPTH {
        run = run.flat_map(|value| NodeResult::output(value + 1));
    }

    assert_eq!(collect_run(run), [DEPTH]);
}

#[test]
fn one_hundred_thousand_flat_maps_drop_iteratively_without_advancing() {
    const DEPTH: i32 = 100_000;

    let mut run = Run::new(std::iter::once(0));
    for _ in 0..DEPTH {
        run = run.flat_map(|value| NodeResult::output(value + 1));
    }

    drop(run);
}

fn assert_failed_run_panic<O>(panic: Result<RunStep<O>, Box<dyn std::any::Any + Send>>) {
    let Err(payload) = panic else {
        panic!("expected the failed Run to panic");
    };
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    assert_eq!(
        message,
        Some("the Run is failed after a previous advance panic")
    );
}

#[test]
fn producer_panic_clears_run_and_reuse_fails_stop() {
    let mut run = Run::new(std::iter::from_fn(|| -> Option<i32> {
        panic!("producer panic");
    }));

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.advance()));
    let reuse = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.advance()));

    assert!(panic.is_err());
    assert_failed_run_panic(reuse);
}

#[test]
fn transform_panic_clears_run_and_reuse_fails_stop() {
    let result = NodeResult::run([1, 2].into_iter()).map(|_value| -> i32 {
        panic!("transform panic");
    });
    let NodeResult::Run(mut run) = result else {
        panic!("expected a deferred mapped result");
    };

    // The panic arrives on the first advance, not the second: a `map` folds into
    // the typed source, so there is no separate producer step ahead of it. What
    // this pins is the fail-stop, which is unchanged either way.
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.advance()));
    let reuse = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.advance()));

    assert!(panic.is_err());
    assert_failed_run_panic(reuse);
}

#[test]
fn flat_map_expansion_advances_one_child_at_a_time_in_source_order() {
    let children = NodeResult::run(std::iter::once(1)).serial(NodeResult::run(std::iter::once(2)));
    let mut children = Some(children);
    let mut run = Run::new(std::iter::once(0)).flat_map(move |_output| children.take().unwrap());

    assert_eq!(run.advance(), RunStep::Continue);
    assert_eq!(run.advance(), RunStep::Continue);
    assert_eq!(run.advance(), RunStep::Continue);
    assert_eq!(run.advance(), RunStep::Output(1));
    assert_eq!(run.advance(), RunStep::Continue);
    assert_eq!(run.advance(), RunStep::Continue);
    assert_eq!(run.advance(), RunStep::Output(2));
    assert_eq!(run.advance(), RunStep::Continue);
    assert_eq!(run.advance(), RunStep::Complete);
}

#[test]
fn wide_flat_map_requires_an_expansion_step_before_its_first_child() {
    const WIDTH: i32 = 100_000;

    let children = (0..WIDTH).fold(NodeResult::none(), |result, value| {
        result.serial(NodeResult::run(std::iter::once(value)))
    });
    let mut children = Some(children);
    let mut run = Run::new(std::iter::once(())).flat_map(move |()| children.take().unwrap());

    assert_eq!(run.advance(), RunStep::Continue);
    assert_eq!(run.advance(), RunStep::Continue);
    assert_eq!(run.advance(), RunStep::Continue);
    assert_eq!(run.advance(), RunStep::Output(0));
}

// --- Appending transforms to a run that has already started ---
//
// A run shares one transform chain between a producer and every output still
// travelling through it, so appending a stage after the chain is shared has to
// copy rather than relink. These pin that the copy keeps each operation's own
// chain intact.

fn advance_to_first_output<O>(run: &mut Run<O>) -> O
where
    O: Send + 'static,
{
    loop {
        match run.advance() {
            RunStep::Output(output) => return output,
            RunStep::Continue => {}
            RunStep::Complete => panic!("the run completed before producing an output"),
        }
    }
}

#[test]
fn a_transform_appended_after_the_first_output_applies_to_later_outputs() {
    let mut run = Run::new([1, 2, 3].into_iter());
    assert_eq!(advance_to_first_output(&mut run), 1);

    let run = run.flat_map(|value| NodeResult::output(value * 10));

    assert_eq!(collect_run(run), [20, 30]);
}

#[test]
fn transforms_appended_after_an_output_is_in_flight_do_not_cross_chains() {
    let mut run = Run::new([1, 2].into_iter()).flat_map(|value| NodeResult::output(value + 100));
    // Stops mid-chain: the first output has been produced and is queued behind
    // the transform it has not run yet, so the producer and that output share a
    // chain whose tail is about to be appended to.
    assert!(matches!(run.advance(), RunStep::Continue));

    let run = run.flat_map(|value| NodeResult::output(value * 2));

    // Every output passes each stage exactly once, in order.
    assert_eq!(collect_run(run), [202, 204]);
}

#[test]
fn a_transform_appended_after_an_expansion_applies_once_to_every_child() {
    let mut run =
        Run::new([1, 2].into_iter()).flat_map(|value| NodeResult::outputs(vec![value, value + 10]));
    // Advances into the expansion so the children inherit the parent's chain.
    assert_eq!(advance_to_first_output(&mut run), 1);

    let run = run.flat_map(|value| NodeResult::output(value * 100));

    assert_eq!(collect_run(run), [1100, 200, 1200]);
}

#[test]
fn a_transform_appended_to_a_completed_run_produces_nothing() {
    let mut run = Run::new(std::iter::once(1));
    assert_eq!(advance_to_first_output(&mut run), 1);

    let run = run.flat_map(|value| NodeResult::output(value + 1));

    assert_eq!(collect_run(run), Vec::<i32>::new());
}

// --- values crossing a recycled carrier ---
//
// A run reuses the allocation each stage's output travelled in. The reuse itself
// is not observable from outside the crate — a carrier holds no value between
// stages — so it is measured by `scripts/bench.sh` and pinned structurally by
// `tests/run_arena.rs`. What these three assert is the half that *is* observable
// and that a reuse bug would break: values must not blur together, duplicate, go
// missing, or come back with a neighbouring stage's shape.

#[test]
fn a_chain_of_differing_types_keeps_each_stage_value_distinct() {
    const OUTPUTS: i32 = 64;

    // Each stage's output type differs from its neighbours', so every carrier a
    // stage is offered back is the wrong shape for the stage beside it. A
    // recycling scheme that ignored shape would surface here as a wrong value or
    // a panic rather than as a silently declined carrier.
    let result = NodeResult::run(0..OUTPUTS)
        .map(|value| format!("{value:04}"))
        .map(|text| {
            let width = i32::try_from(text.len()).expect("four digits fit");
            width + text.parse::<i32>().expect("four digits")
        })
        .map(|value| vec![value; 2])
        .map(|values| values.into_iter().sum::<i32>());

    let expected: Vec<i32> = (0..OUTPUTS).map(|value| (value + 4) * 2).collect();
    assert_eq!(collect_i32(result), expected);
}

#[test]
fn a_long_chain_neither_duplicates_nor_loses_an_output() {
    const OUTPUTS: i32 = 10_000;

    // Long enough that every stage is recycling rather than allocating for most
    // of the run: if a reused carrier ever kept its previous value, or a value
    // were taken twice, the totals below would not both hold.
    let result = NodeResult::run(0..OUTPUTS)
        .map(|value| value * 2)
        .map(|value| value + 1);

    let collected = collect_i32(result);
    assert_eq!(collected.len(), OUTPUTS as usize);
    let expected: Vec<i32> = (0..OUTPUTS).map(|value| value * 2 + 1).collect();
    assert_eq!(collected, expected);
}

#[test]
fn a_declining_stage_drops_only_the_outputs_it_declined() {
    const OUTPUTS: i32 = 1_000;

    // `filter_map` declining an output ends that carrier's journey early, on a
    // path that recycles from a different place than a delivered output does.
    // What is checked here is that the surviving outputs are exactly the right
    // ones — a carrier returned to the wrong stage would surface as a wrong value.
    let NodeResult::Run(run) = NodeResult::run(0..OUTPUTS) else {
        unreachable!("an iterator result is a run");
    };
    let run = run
        .filter_map(|value| (value % 2 == 0).then_some(value))
        .map(|value| value / 2);

    let expected: Vec<i32> = (0..OUTPUTS / 2).collect();
    assert_eq!(collect_run(run), expected);
}

// --- Folding `map` into the typed source ---
//
// A `map` folds into the run's source rather than erasing it, so a mapped run
// stays one iterator. These pin the two conditions that gate the fold and the
// bound that keeps it from becoming an unbounded nested chain.

#[test]
fn a_folded_map_chain_delivers_an_output_on_every_advance() {
    // The observable difference the fold makes: no work-deque round trip, so no
    // intermediate `Continue` between the source and the output.
    let result = NodeResult::run([1, 2].into_iter())
        .map(|value| value * 10)
        .map(|value| value + 1);
    let NodeResult::Run(mut run) = result else {
        panic!("expected a deferred mapped result");
    };

    assert_eq!(run.advance(), RunStep::Output(11));
    assert_eq!(run.advance(), RunStep::Output(21));
    assert_eq!(run.advance(), RunStep::Complete);
}

#[test]
fn a_map_chain_past_the_fusion_limit_falls_back_and_still_delivers() {
    // Past the limit the remaining stages take the erased path, which is what
    // keeps a long chain from nesting one box per stage. Both halves have to
    // apply, and the erased half has to show itself as an intermediate step.
    const STAGES: i32 = 100;

    let mut result = NodeResult::run(std::iter::once(0));
    for _ in 0..STAGES {
        result = result.map(|value| value + 1);
    }
    let NodeResult::Run(mut run) = result else {
        panic!("expected a deferred mapped result");
    };

    let mut outputs = Vec::new();
    let mut intermediate_steps = 0;
    loop {
        match run.advance() {
            RunStep::Output(output) => outputs.push(output),
            RunStep::Continue => intermediate_steps += 1,
            RunStep::Complete => break,
        }
    }

    assert_eq!(outputs, [STAGES]);
    assert!(
        intermediate_steps > 0,
        "every stage folded, so the limit did not bound anything"
    );
}

#[test]
fn a_map_after_serial_applies_to_the_spliced_work_too() {
    // `serial` leaves operations queued behind a source that is still typed.
    // Folding there would map the source's outputs and silently spare theirs,
    // which is why the fold declines whenever anything is queued.
    let result = NodeResult::run(std::iter::once(1))
        .serial(NodeResult::run(std::iter::once(2)))
        .map(|value| value * 10);

    assert_eq!(collect_i32(result), [10, 20]);
}
