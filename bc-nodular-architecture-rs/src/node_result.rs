//! The effect value returned by a nodule transition.

use std::any::Any;
use std::collections::VecDeque;
use std::marker::PhantomData;

use bc_core::{Arena as ProducerArena, ArenaRef};

/// One erased output in flight.
///
/// The payload is an `Option<T>`, not a bare `T`, so a stage can take the value
/// and hand the emptied allocation back to whoever filled it. A steady-state
/// chain of maps therefore allocates nothing per output. A `flat_map` is not
/// steady state: each expansion brings a producer of its own.
type ErasedOutput = Box<dyn Any + Send>;

/// Fills a carrier, reusing a recycled one when the filler has been given one.
fn fill_output<T>(spare: &mut Option<Box<Option<T>>>, value: T) -> ErasedOutput
where
    T: Send + 'static,
{
    let mut carrier = spare.take().unwrap_or_else(|| Box::new(None));
    *carrier = Some(value);
    carrier
}

/// Takes a carrier's value, returning the emptied carrier for recycling.
fn take_output<T>(output: ErasedOutput) -> (T, ErasedOutput)
where
    T: Send + 'static,
{
    let Ok(mut carrier) = output.downcast::<Option<T>>() else {
        unreachable!("a run's stages are typed by construction")
    };
    let value = carrier
        .take()
        .expect("a carrier holds its value until a stage takes it");
    (value, carrier)
}

/// Accepts an emptied carrier back, if it is the right shape and none is held.
fn accept_spare<T>(spare: &mut Option<Box<Option<T>>>, empty: ErasedOutput)
where
    T: Send + 'static,
{
    if spare.is_none() {
        if let Ok(carrier) = empty.downcast::<Option<T>>() {
            *spare = Some(carrier);
        }
    }
}

enum ErasedStep {
    Output(ErasedOutput),
    Complete,
}

trait ErasedProducer: Send {
    fn advance(&mut self) -> ErasedStep;

    /// Takes back a carrier this producer filled, to fill again.
    fn recycle(&mut self, empty: ErasedOutput);
}

struct IteratorProducer<I, O> {
    outputs: I,
    spare: Option<Box<Option<O>>>,
    output: PhantomData<fn() -> O>,
}

impl<I, O> ErasedProducer for IteratorProducer<I, O>
where
    I: Iterator<Item = O> + Send,
    O: Send + 'static,
{
    fn advance(&mut self) -> ErasedStep {
        match self.outputs.next() {
            Some(output) => ErasedStep::Output(fill_output(&mut self.spare, output)),
            None => ErasedStep::Complete,
        }
    }

    fn recycle(&mut self, empty: ErasedOutput) {
        accept_spare(&mut self.spare, empty);
    }
}

enum TransformResult {
    Output(ErasedOutput),
    Expansion(RunParts),
    None,
}

trait ErasedTransform: Send {
    /// Consumes one carrier, leaving the emptied one in `emptied` for the run to
    /// return to whoever filled it.
    fn transform(
        &mut self,
        output: ErasedOutput,
        emptied: &mut Option<ErasedOutput>,
    ) -> TransformResult;

    /// Takes back a carrier this transform filled, to fill again.
    fn recycle(&mut self, empty: ErasedOutput);
}

struct MapTransform<F, O, M> {
    transform: F,
    spare: Option<Box<Option<M>>>,
    types: PhantomData<fn(O) -> M>,
}

impl<F, O, M> ErasedTransform for MapTransform<F, O, M>
where
    F: FnMut(O) -> Option<M> + Send,
    O: Send + 'static,
    M: Send + 'static,
{
    fn transform(
        &mut self,
        output: ErasedOutput,
        emptied: &mut Option<ErasedOutput>,
    ) -> TransformResult {
        let (output, empty) = take_output::<O>(output);
        *emptied = Some(empty);
        match (self.transform)(output) {
            Some(output) => TransformResult::Output(fill_output(&mut self.spare, output)),
            None => TransformResult::None,
        }
    }

    fn recycle(&mut self, empty: ErasedOutput) {
        accept_spare(&mut self.spare, empty);
    }
}

struct FlatMapTransform<F, O, M> {
    transform: F,
    types: PhantomData<fn(O) -> M>,
}

impl<F, O, M> ErasedTransform for FlatMapTransform<F, O, M>
where
    F: FnMut(O) -> NodeResult<M> + Send,
    O: Send + 'static,
    M: Send + 'static,
{
    fn transform(
        &mut self,
        output: ErasedOutput,
        emptied: &mut Option<ErasedOutput>,
    ) -> TransformResult {
        let (output, empty) = take_output::<O>(output);
        *emptied = Some(empty);
        TransformResult::Expansion((self.transform)(output).into_run().into_parts())
    }

    /// A flat map fills no carrier of its own, so it keeps none.
    fn recycle(&mut self, _empty: ErasedOutput) {}
}

/// Narrows an arena length to the 32-bit width the ids use.
///
/// A run with four billion stages is not a case this type is for; failing at the
/// boundary beats a silent wrap into another stage's slot.
fn arena_position(length: usize) -> u32 {
    u32::try_from(length).expect("a run holds fewer than u32::MAX arena entries")
}

/// Position of one producer in a run's producer arena.
///
/// A slot is released the moment its producer runs dry and reused by the next
/// producer the run needs, so the arena is sized by the producers alive at once
/// rather than by every producer the run has ever had. The generation in the
/// handle is what makes that reuse safe to reason about: a handle to a released
/// producer stops resolving rather than naming whichever producer inherited its
/// slot.
type ProducerId = ArenaRef;

/// Position of one transform in a run's transform arena.
#[derive(Clone, Copy, PartialEq, Eq)]
struct TransformId(u32);

/// Which arena slot filled the carrier an operation is holding.
///
/// Producers and transforms live in arenas rather than inside the operations
/// themselves precisely so this stays valid: an operation moves in and out of
/// the work deque, and `serial` and expansion rebase whole runs into this one,
/// but an arena index survives all of it.
#[derive(Clone, Copy)]
enum Filler {
    Producer(ProducerId),
    Transform(TransformId),
}

/// Position of one chain link in a run's pipeline arena.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PipelineId(u32);

/// One link of a transform chain.
///
/// Chains are singly linked through the arena rather than copied, so handing a
/// produced output the chain its producer still holds costs two indices.
struct PipelineNode {
    transform: TransformId,
    next: Option<PipelineId>,
    /// Set once a second operation holds this node as its tail. Appending then
    /// has to copy the chain, because relinking would redirect that operation
    /// too. Only the tail is ever relinked, so only the tail needs the flag.
    shared: bool,
}

/// The transforms an operation's outputs still have to pass through.
#[derive(Clone, Copy)]
struct Pipeline {
    head: Option<PipelineId>,
    tail: Option<PipelineId>,
}

impl Pipeline {
    const EMPTY: Self = Self {
        head: None,
        tail: None,
    };

    const fn is_empty(self) -> bool {
        self.head.is_none()
    }
}

struct ProducerOperation {
    producer: ProducerId,
    pipeline: Pipeline,
}

struct TransformOperation {
    output: ErasedOutput,
    filler: Filler,
    pipeline: Pipeline,
}

struct ExpansionOperation {
    children: VecDeque<Operation>,
    pipeline: Pipeline,
}

enum Operation {
    Producer(ProducerOperation),
    Transform(TransformOperation),
    Expansion(ExpansionOperation),
}

impl Operation {
    const fn pipeline(&self) -> Pipeline {
        match self {
            Self::Producer(operation) => operation.pipeline,
            Self::Transform(operation) => operation.pipeline,
            Self::Expansion(operation) => operation.pipeline,
        }
    }

    const fn set_pipeline(&mut self, pipeline: Pipeline) {
        match self {
            Self::Producer(operation) => operation.pipeline = pipeline,
            Self::Transform(operation) => operation.pipeline = pipeline,
            Self::Expansion(operation) => operation.pipeline = pipeline,
        }
    }
}

/// A run's movable interior, used to splice one run's work into another.
struct RunParts {
    operations: VecDeque<Operation>,
    arena: Option<Box<RunTables>>,
}

/// Everything a run addresses by index.
///
/// Held behind a box, and not made at all until something stands in front of a
/// run's source, because a `Run` travels by value inside every `NodeResult` a
/// nodule returns: three tables inline would put their size on the crate's
/// hottest signature whether or not the run ever grows a stage.
///
/// Only the producers need an arena. They are released and their slots reused,
/// which is what generational handles are for. Transforms and pipeline nodes are
/// appended and never removed, so a `Vec` says exactly as much and costs a
/// narrower index.
#[derive(Default)]
struct RunTables {
    producers: ProducerArena<Box<dyn ErasedProducer>>,
    transforms: Vec<Box<dyn ErasedTransform>>,
    pipelines: Vec<PipelineNode>,
}

/// Result of advancing a [`Run`] by one bounded step.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a RunStep must be handled before advancing the Run again"]
pub enum RunStep<O> {
    /// The step produced one output.
    Output(O),
    /// The run has more work but this step produced no output.
    Continue,
    /// The run has completed.
    Complete,
}

/// Scheduler-pulled resumable output producer.
///
/// Each source iterator step and transform must be short, nonblocking, and free
/// of I/O. [`advance`](Self::advance) performs at most one such operation so a
/// scheduler can bound work and finish output feedback before continuing.
///
/// A run owns its transforms outright. Nothing inside is shared across threads,
/// so the type carries no lock.
#[must_use = "a Run does nothing until a node loop advances it"]
pub struct Run<O> {
    /// The still-typed output source, held until something has to go in front of
    /// it. A run in this shape hands outputs straight to the scheduler: no
    /// boxing, no downcast. `erase_source` moves it into `operations`, and every
    /// path that enumerates or takes those operations calls that first.
    ///
    /// While this is `Some`, any operations behind it were spliced in by
    /// `serial`, and draining the source before them is exactly the order
    /// `serial` promises.
    source: Option<Box<dyn Iterator<Item = O> + Send>>,
    operations: VecDeque<Operation>,
    /// Absent until the run has an operation to address; see [`RunTables`].
    arena: Option<Box<RunTables>>,
    failed: bool,
    /// How many `map` stages have been folded into `source`. Bounds both the
    /// work one `advance` can do and how deep dropping the source recurses;
    /// see [`FUSION_LIMIT`].
    fused_stages: u8,
    output: PhantomData<fn() -> O>,
}

/// How many `map` stages may fold into a run's typed source.
///
/// A fused stage costs one indirect call, where an erased one costs a work-deque
/// round trip, two table hops, two downcasts and two virtual calls — so folding
/// is worth a great deal. Two things stop it from being unbounded, and one
/// constant bounds both:
///
/// - **Work per `advance`.** A fused chain delivers an output through every one
///   of its stages in a single step, so the bound on stages is the bound on the
///   work a scheduler cannot preempt.
/// - **Drop recursion.** Each fold nests one `Box<dyn Iterator>` inside the
///   next, and dropping the outer one recurses through them.
///
/// Past the limit, stages take the erased path as they always did. That is what
/// separates this from an unbounded nested adapter chain, where a long enough
/// chain overflows the stack on drop — the failure this crate exists to remove.
const FUSION_LIMIT: u8 = 32;

/// Fails a run permanently if the work it is guarding unwinds.
///
/// The panic itself is never caught: catching only to run cleanup and resume
/// costs an optimization barrier around the hot path for no observable
/// difference, and cleanup on unwind is what `Drop` is for.
struct FailRunOnUnwind<'run, O> {
    run: &'run mut Run<O>,
    armed: bool,
}

impl<O> Drop for FailRunOnUnwind<'_, O> {
    fn drop(&mut self) {
        if self.armed {
            self.run.failed = true;
            self.run.source = None;
            self.run.operations.clear();
            self.run.arena = None;
        }
    }
}

impl RunTables {
    /// Advances the producer an operation names.
    fn advance_producer(&mut self, producer: ProducerId) -> ErasedStep {
        self.producers
            .get_mut(producer)
            .expect("a queued producer operation holds its arena slot")
            .advance()
    }

    /// Releases a producer that can no longer be advanced.
    ///
    /// Sound because a producer emits at most one carrier before its own
    /// operation is queued again behind that carrier's work: the carrier is
    /// handed back a step later, so by the time the producer reports itself
    /// complete nothing is still holding one of its. The arena's generations
    /// make that an enforced claim rather than a remembered one — a handle to a
    /// released producer stops resolving instead of naming its successor.
    fn release_producer(&mut self, producer: ProducerId) {
        self.producers.remove(producer);
    }

    /// Runs one carrier through the transform an operation named.
    fn apply(
        &mut self,
        transform: TransformId,
        output: ErasedOutput,
        emptied: &mut Option<ErasedOutput>,
    ) -> TransformResult {
        self.transforms[transform.0 as usize].transform(output, emptied)
    }

    /// Returns an emptied carrier to the slot that filled it.
    fn recycle(&mut self, filler: Filler, empty: ErasedOutput) {
        match filler {
            Filler::Producer(producer) => self
                .producers
                .get_mut(producer)
                .expect("a carrier is handed back before its producer is released")
                .recycle(empty),
            Filler::Transform(transform) => self.transforms[transform.0 as usize].recycle(empty),
        }
    }

    /// Marks a chain as reachable from a second operation.
    fn share(&mut self, pipeline: Pipeline) -> Pipeline {
        if let Some(tail) = pipeline.tail {
            self.pipelines[tail.0 as usize].shared = true;
        }
        pipeline
    }

    /// Takes the next transform, leaving the rest of the chain in place.
    fn pop_stage(&mut self, pipeline: &mut Pipeline) -> Option<TransformId> {
        let head = pipeline.head?;
        let node = &self.pipelines[head.0 as usize];
        pipeline.head = node.next;
        if pipeline.head.is_none() {
            pipeline.tail = None;
        }
        Some(node.transform)
    }

    /// Appends one transform to the end of a chain.
    fn append_stage(&mut self, pipeline: Pipeline, transform: TransformId) -> Pipeline {
        let mut pipeline = match pipeline.tail {
            Some(tail) if self.pipelines[tail.0 as usize].shared => self.copy_chain(pipeline),
            _ => pipeline,
        };

        let node = PipelineId(arena_position(self.pipelines.len()));
        self.pipelines.push(PipelineNode {
            transform,
            next: None,
            shared: false,
        });
        match pipeline.tail {
            Some(tail) => self.pipelines[tail.0 as usize].next = Some(node),
            None => pipeline.head = Some(node),
        }
        pipeline.tail = Some(node);
        pipeline
    }

    /// Appends a whole chain to the end of another.
    fn concatenate(&mut self, prefix: Pipeline, suffix: Pipeline) -> Pipeline {
        if suffix.is_empty() {
            return prefix;
        }
        if prefix.is_empty() {
            return self.share(suffix);
        }

        let prefix = match prefix.tail {
            Some(tail) if self.pipelines[tail.0 as usize].shared => self.copy_chain(prefix),
            _ => prefix,
        };
        let tail = prefix.tail.expect("a non-empty chain has a tail");
        self.pipelines[tail.0 as usize].next = suffix.head;
        self.share(suffix);
        Pipeline {
            head: prefix.head,
            tail: suffix.tail,
        }
    }

    /// Rebuilds a chain in fresh, unshared nodes so its tail can be relinked.
    fn copy_chain(&mut self, pipeline: Pipeline) -> Pipeline {
        let mut copy = Pipeline::EMPTY;
        let mut cursor = pipeline.head;
        while let Some(id) = cursor {
            let transform = self.pipelines[id.0 as usize].transform;
            cursor = self.pipelines[id.0 as usize].next;

            let fresh = PipelineId(arena_position(self.pipelines.len()));
            self.pipelines.push(PipelineNode {
                transform,
                next: None,
                shared: false,
            });
            match copy.tail {
                Some(tail) => self.pipelines[tail.0 as usize].next = Some(fresh),
                None => copy.head = Some(fresh),
            }
            copy.tail = Some(fresh);
        }
        copy
    }

    /// Moves another run's tables into this one, rebasing every index.
    fn splice(&mut self, parts: RunParts) -> VecDeque<Operation> {
        let mut operations = parts.operations;
        let Some(other) = parts.arena else {
            return operations;
        };

        let transform_base = arena_position(self.transforms.len());
        let pipeline_base = arena_position(self.pipelines.len());

        // Transform and pipeline tables only grow, so their indices shift by a
        // base. Producer slots are reused and their handles carry a generation,
        // so each producer is moved across individually and the spliced run's
        // handles are read through the resulting map. The map is indexed by the
        // *old* slot, which is what an operation over there still names.
        let mut other = other;
        let moving: Vec<ProducerId> = other.producers.iter().map(|(handle, _)| handle).collect();
        let mut producer_map: Vec<Option<ProducerId>> = vec![None; other.producers.capacity()];
        for handle in moving {
            let producer = other
                .producers
                .remove(handle)
                .expect("the handle came from this arena a moment ago");
            producer_map[handle.index() as usize] = Some(self.producers.insert(producer));
        }
        let remap = |handle: ProducerId| {
            producer_map[handle.index() as usize]
                .expect("a spliced operation names a producer its run still held")
        };

        self.transforms.extend(other.transforms);
        self.pipelines
            .extend(other.pipelines.into_iter().map(|node| PipelineNode {
                transform: TransformId(node.transform.0 + transform_base),
                next: node.next.map(|id| PipelineId(id.0 + pipeline_base)),
                shared: node.shared,
            }));

        // Every index in the spliced run points into its own arenas, so all of
        // them shift. Expansion operations carry nested operations with indices
        // of their own, walked here with an explicit worklist rather than
        // recursion — a run may nest arbitrarily deep, which is the whole reason
        // this type exists.
        let mut pending: Vec<&mut Operation> = operations.iter_mut().collect();
        while let Some(operation) = pending.pop() {
            let pipeline = operation.pipeline();
            operation.set_pipeline(Pipeline {
                head: pipeline.head.map(|id| PipelineId(id.0 + pipeline_base)),
                tail: pipeline.tail.map(|id| PipelineId(id.0 + pipeline_base)),
            });
            match operation {
                Operation::Producer(operation) => {
                    operation.producer = remap(operation.producer);
                }
                Operation::Transform(operation) => {
                    operation.filler = match operation.filler {
                        Filler::Producer(id) => Filler::Producer(remap(id)),
                        Filler::Transform(id) => {
                            Filler::Transform(TransformId(id.0 + transform_base))
                        }
                    };
                }
                Operation::Expansion(operation) => {
                    pending.extend(operation.children.iter_mut());
                }
            }
        }
        operations
    }
}

impl<O> Run<O>
where
    O: Send + 'static,
{
    /// Wraps a short, nonblocking, I/O-free output iterator.
    pub fn new(outputs: impl Iterator<Item = O> + Send + 'static) -> Self {
        Self {
            source: Some(Box::new(outputs)),
            operations: VecDeque::new(),
            arena: None,
            failed: false,
            fused_stages: 0,
            output: PhantomData,
        }
    }

    fn empty() -> Self {
        Self {
            source: None,
            operations: VecDeque::new(),
            arena: None,
            failed: false,
            fused_stages: 0,
            output: PhantomData,
        }
    }

    /// Moves a still-typed source into the operation arena, ahead of anything
    /// already queued behind it.
    ///
    /// Every path that appends a stage to each operation, or that hands this
    /// run's operations to another run, calls this first — otherwise the source
    /// would escape the work it is supposed to pass through.
    fn erase_source(&mut self) {
        if let Some(outputs) = self.source.take() {
            let producer = self
                .arena_mut()
                .producers
                .insert(Box::new(IteratorProducer {
                    outputs,
                    spare: None,
                    output: PhantomData,
                }));
            self.operations
                .push_front(Operation::Producer(ProducerOperation {
                    producer,
                    pipeline: Pipeline::EMPTY,
                }));
        }
    }

    /// The run's tables, made on first use.
    fn arena_mut(&mut self) -> &mut RunTables {
        self.arena.get_or_insert_with(Box::default)
    }

    fn from_outputs(outputs: Vec<O>) -> Self {
        Self::new(outputs.into_iter())
    }

    /// Advances exactly one producer, transform, or expansion operation.
    ///
    /// # Panics
    ///
    /// Resumes any producer or transform panic after marking the run failed.
    /// Panics immediately if called again after such a failure.
    pub fn advance(&mut self) -> RunStep<O> {
        assert!(
            !self.failed,
            "the Run is failed after a previous advance panic"
        );

        let mut guard = FailRunOnUnwind {
            run: self,
            armed: true,
        };
        let step = guard.run.advance_one();
        guard.armed = false;
        step
    }

    fn advance_one(&mut self) -> RunStep<O> {
        // The fast path: nothing has been put in front of the source, so its
        // output is already the run's output type.
        if let Some(source) = &mut self.source {
            match source.next() {
                Some(output) => return RunStep::Output(output),
                None => self.source = None,
            }
        }

        let Some(operation) = self.operations.pop_front() else {
            return RunStep::Complete;
        };

        match operation {
            Operation::Producer(mut operation) => {
                let producer = operation.producer;
                match self.arena_mut().advance_producer(producer) {
                    ErasedStep::Output(output) => {
                        let pipeline = self.arena_mut().share(operation.pipeline);
                        operation.pipeline = pipeline;
                        self.operations.push_front(Operation::Producer(operation));
                        if pipeline.is_empty() {
                            self.deliver(output, Filler::Producer(producer))
                        } else {
                            self.operations
                                .push_front(Operation::Transform(TransformOperation {
                                    output,
                                    filler: Filler::Producer(producer),
                                    pipeline,
                                }));
                            RunStep::Continue
                        }
                    }
                    ErasedStep::Complete => {
                        self.arena_mut().release_producer(producer);
                        self.incomplete_or_complete()
                    }
                }
            }
            Operation::Transform(mut operation) => {
                let Some(transform) = self.arena_mut().pop_stage(&mut operation.pipeline) else {
                    return self.deliver(operation.output, operation.filler);
                };

                let mut emptied = None;
                let result = self
                    .arena_mut()
                    .apply(transform, operation.output, &mut emptied);
                if let Some(empty) = emptied {
                    self.arena_mut().recycle(operation.filler, empty);
                }

                match result {
                    TransformResult::Output(output) => {
                        if operation.pipeline.is_empty() {
                            self.deliver(output, Filler::Transform(transform))
                        } else {
                            operation.output = output;
                            operation.filler = Filler::Transform(transform);
                            self.operations.push_front(Operation::Transform(operation));
                            RunStep::Continue
                        }
                    }
                    TransformResult::Expansion(parts) => {
                        let children = self.arena_mut().splice(parts);
                        self.operations
                            .push_front(Operation::Expansion(ExpansionOperation {
                                children,
                                pipeline: operation.pipeline,
                            }));
                        RunStep::Continue
                    }
                    TransformResult::None => self.incomplete_or_complete(),
                }
            }
            Operation::Expansion(mut operation) => {
                let Some(mut child) = operation.children.pop_front() else {
                    return self.incomplete_or_complete();
                };
                let combined = self
                    .arena_mut()
                    .concatenate(child.pipeline(), operation.pipeline);
                child.set_pipeline(combined);
                if !operation.children.is_empty() {
                    self.operations.push_front(Operation::Expansion(operation));
                }
                self.operations.push_front(child);
                RunStep::Continue
            }
        }
    }

    /// Hands a finished carrier's value to the scheduler and returns the
    /// emptied carrier to the slot that filled it.
    fn deliver(&mut self, output: ErasedOutput, filler: Filler) -> RunStep<O> {
        let (value, empty) = take_output::<O>(output);
        self.arena_mut().recycle(filler, empty);
        RunStep::Output(value)
    }

    fn incomplete_or_complete(&self) -> RunStep<O> {
        if self.operations.is_empty() {
            RunStep::Complete
        } else {
            RunStep::Continue
        }
    }

    // --- the transform and pipeline arenas ---

    fn into_parts(mut self) -> RunParts {
        self.erase_source();
        RunParts {
            operations: self.operations,
            arena: self.arena,
        }
    }

    // --- composition ---

    fn serial(mut self, other: Self) -> Self {
        let failed = other.failed;
        let mut operations = self.arena_mut().splice(other.into_parts());
        self.operations.append(&mut operations);
        self.failed |= failed;
        self
    }

    fn push_transform<M>(mut self, transform: Box<dyn ErasedTransform>) -> Run<M>
    where
        M: Send + 'static,
    {
        self.erase_source();
        let arena = self.arena_mut();
        let id = TransformId(arena_position(arena.transforms.len()));
        arena.transforms.push(transform);
        for index in 0..self.operations.len() {
            let pipeline = self.operations[index].pipeline();
            let appended = self.arena_mut().append_stage(pipeline, id);
            self.operations[index].set_pipeline(appended);
        }

        Run {
            source: None,
            operations: self.operations,
            arena: self.arena,
            failed: self.failed,
            // The source is erased by now, so nothing is folded into it.
            fused_stages: 0,
            output: PhantomData,
        }
    }

    /// Maps each output, dropping the ones the transform declines.
    ///
    /// Never folded into the source the way [`map`](Self::map) is: a filter that
    /// declined a long stretch of its input would search for the next output
    /// inside a single `advance`, and how long that took would be the input's
    /// business rather than the stage count's. Bounded work is the contract this
    /// keeps and that one could not.
    pub fn filter_map<M, F>(self, transform: F) -> Run<M>
    where
        M: Send + 'static,
        F: FnMut(O) -> Option<M> + Send + 'static,
    {
        self.push_transform(Box::new(MapTransform {
            transform,
            spare: None,
            types: PhantomData,
        }))
    }

    /// Maps each output as it is produced.
    ///
    /// Folds into the still-typed source where it can, rather than erasing it:
    /// the run stays one iterator and the stage costs one indirect call. See
    /// `FUSION_LIMIT` in this module for what bounds that and why.
    ///
    /// Two conditions gate the fold, and both are about correctness rather than
    /// cost. The source must still be typed, since an erased run has no iterator
    /// to fold into. And nothing may be queued behind it — operations there were
    /// spliced in by `serial`, and folding would map the source's outputs while
    /// silently sparing theirs.
    pub fn map<M, F>(self, mut transform: F) -> Run<M>
    where
        M: Send + 'static,
        F: FnMut(O) -> M + Send + 'static,
    {
        if self.operations.is_empty() && self.fused_stages < FUSION_LIMIT {
            if let Some(outputs) = self.source {
                return Run {
                    source: Some(Box::new(outputs.map(transform))),
                    operations: self.operations,
                    arena: self.arena,
                    failed: self.failed,
                    fused_stages: self.fused_stages + 1,
                    output: PhantomData,
                };
            }
        }
        self.filter_map(move |output| Some(transform(output)))
    }

    /// Maps each output to another result without nesting run execution.
    pub fn flat_map<M, F>(self, transform: F) -> Run<M>
    where
        M: Send + 'static,
        F: FnMut(O) -> NodeResult<M> + Send + 'static,
    {
        self.push_transform(Box::new(FlatMapTransform {
            transform,
            types: PhantomData,
        }))
    }
}

impl<O> std::fmt::Debug for Run<O> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Run(..)")
    }
}

/// Outputs or resumable work produced by a nodule transition.
#[derive(Debug)]
#[must_use = "a NodeResult must be dispatched by a node loop"]
pub enum NodeResult<O> {
    /// No output.
    None,
    /// One immediate output.
    Output(O),
    /// Multiple immediate outputs in source order.
    Outputs(Vec<O>),
    /// Resumable work that can emit zero or many outputs.
    Run(Run<O>),
}

impl<O> NodeResult<O> {
    /// Constructs a result with no output.
    pub const fn none() -> Self {
        Self::None
    }

    /// Constructs one immediate output.
    pub const fn output(output: O) -> Self {
        Self::Output(output)
    }

    /// Constructs multiple immediate outputs in source order.
    pub const fn outputs(outputs: Vec<O>) -> Self {
        Self::Outputs(outputs)
    }

    /// Constructs resumable work from a short, nonblocking, I/O-free iterator.
    pub fn run(outputs: impl Iterator<Item = O> + Send + 'static) -> Self
    where
        O: Send + 'static,
    {
        Self::Run(Run::new(outputs))
    }

    /// Combines two results while preserving left-to-right output order.
    pub fn serial(self, other: Self) -> Self
    where
        O: Send + 'static,
    {
        match (self, other) {
            (Self::None, result) | (result, Self::None) => result,
            (Self::Output(first), Self::Output(second)) => Self::Outputs(vec![first, second]),
            (Self::Output(first), Self::Outputs(rest)) => {
                let mut outputs = Vec::with_capacity(rest.len() + 1);
                outputs.push(first);
                outputs.extend(rest);
                Self::Outputs(outputs)
            }
            (Self::Outputs(mut first), Self::Output(second)) => {
                first.push(second);
                Self::Outputs(first)
            }
            (Self::Outputs(mut first), Self::Outputs(second)) => {
                first.extend(second);
                Self::Outputs(first)
            }
            (first, second) => Self::Run(first.into_run().serial(second.into_run())),
        }
    }

    /// Combines two results without promising an order between their outputs.
    ///
    /// `None` is the identity. A pair of non-`None` results produces a [`Run`].
    pub fn concurrent(self, other: Self) -> Self
    where
        O: Send + 'static,
    {
        match (self, other) {
            (Self::None, result) | (result, Self::None) => result,
            (first, second) => Self::Run(first.into_run().serial(second.into_run())),
        }
    }

    /// Maps every output to a new type while preserving the result variant.
    pub fn map<M>(self, mut transform: impl FnMut(O) -> M + Send + 'static) -> NodeResult<M>
    where
        O: Send + 'static,
        M: Send + 'static,
    {
        match self {
            Self::None => NodeResult::None,
            Self::Output(output) => NodeResult::Output(transform(output)),
            Self::Outputs(outputs) => {
                NodeResult::Outputs(outputs.into_iter().map(transform).collect())
            }
            Self::Run(run) => NodeResult::Run(run.map(transform)),
        }
    }

    /// Defers mapping every output until the returned run is advanced.
    pub fn map_effect<M>(self, transform: impl FnMut(O) -> M + Send + 'static) -> NodeResult<M>
    where
        O: Send + 'static,
        M: Send + 'static,
    {
        match self {
            Self::None => NodeResult::None,
            result => NodeResult::Run(result.into_run().map(transform)),
        }
    }

    pub(crate) fn into_run(self) -> Run<O>
    where
        O: Send + 'static,
    {
        match self {
            Self::None => Run::empty(),
            Self::Output(output) => Run::new(std::iter::once(output)),
            Self::Outputs(outputs) => Run::from_outputs(outputs),
            Self::Run(run) => run,
        }
    }
}
