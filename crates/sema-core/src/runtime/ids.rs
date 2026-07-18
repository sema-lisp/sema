use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("runtime identity space is exhausted")]
pub struct IdExhausted;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("runtime identity must be nonzero")]
pub struct InvalidRuntimeId;

macro_rules! scalar_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(NonZeroU64);

        impl $name {
            pub fn get(self) -> u64 {
                self.0.get()
            }
        }

        impl private::Sealed for $name {
            fn from_nonzero(value: NonZeroU64) -> Self {
                Self(value)
            }
        }
    };
}

scalar_id!(TaskId);
scalar_id!(ScopeId);
scalar_id!(WaitId);
scalar_id!(WaitGeneration);
scalar_id!(OperationId);
scalar_id!(SettlementSeq);
scalar_id!(CompletionKind);

impl TaskId {
    pub fn try_from_raw(raw: u64) -> Result<Self, InvalidRuntimeId> {
        NonZeroU64::new(raw).map(Self).ok_or(InvalidRuntimeId)
    }
}

impl CompletionKind {
    pub fn try_from_raw(raw: u64) -> Result<Self, InvalidRuntimeId> {
        NonZeroU64::new(raw).map(Self).ok_or(InvalidRuntimeId)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeId(NonZeroU64);

impl RuntimeId {
    pub(crate) fn allocate() -> Result<Self, IdExhausted> {
        allocate_atomic(&NEXT_RUNTIME_ID).map(Self)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// A task's complete identity across every runtime in the process.
///
/// [`TaskId`] values are local to one runtime and may collide. Native registries
/// that outlive a task quantum use this composite identity for ownership and
/// cancellation cleanup.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeTaskId {
    runtime: RuntimeId,
    task: TaskId,
}

impl RuntimeTaskId {
    pub fn new(runtime: RuntimeId, task: TaskId) -> Self {
        Self { runtime, task }
    }

    pub fn runtime(self) -> RuntimeId {
        self.runtime
    }

    pub fn task(self) -> TaskId {
        self.task
    }
}

fn allocate_atomic(counter: &AtomicU64) -> Result<NonZeroU64, IdExhausted> {
    let raw = counter
        .fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| match current {
                0 => None,
                u64::MAX => Some(0),
                value => Some(value + 1),
            },
        )
        .map_err(|_| IdExhausted)?;
    NonZeroU64::new(raw).ok_or(IdExhausted)
}

mod private {
    use super::RuntimeId;
    use std::num::NonZeroU64;

    pub trait Sealed: Sized {
        fn from_nonzero(value: NonZeroU64) -> Self;
    }

    pub trait ScopedSealed: Sized {
        fn from_parts(runtime: RuntimeId, local: NonZeroU64) -> Self;
    }
}

#[doc(hidden)]
pub trait RuntimeIdType: private::Sealed {}

impl<T: private::Sealed> RuntimeIdType for T {}

#[derive(Clone, Debug)]
pub struct IdCounter<I> {
    next: Option<NonZeroU64>,
    marker: PhantomData<fn() -> I>,
}

impl<I: RuntimeIdType> Default for IdCounter<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: RuntimeIdType> IdCounter<I> {
    pub fn new() -> Self {
        Self {
            next: NonZeroU64::new(1),
            marker: PhantomData,
        }
    }

    pub fn allocate(&mut self) -> Result<I, IdExhausted> {
        let current = self.next.ok_or(IdExhausted)?;
        self.next = current.get().checked_add(1).and_then(NonZeroU64::new);
        Ok(I::from_nonzero(current))
    }

    pub fn is_exhausted(&self) -> bool {
        self.next.is_none()
    }

    #[cfg(test)]
    fn starting_at(next: u64) -> Self {
        Self {
            next: NonZeroU64::new(next),
            marker: PhantomData,
        }
    }
}

macro_rules! scoped_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name {
            runtime: RuntimeId,
            local: NonZeroU64,
        }

        impl $name {
            pub fn runtime(self) -> RuntimeId {
                self.runtime
            }

            pub fn local(self) -> u64 {
                self.local.get()
            }

            pub fn get(self) -> u64 {
                self.local.get()
            }
        }
    };
}

scoped_id!(RootId);
scoped_id!(PromiseId);
scoped_id!(ChannelId);
scoped_id!(ResourceGateId);

#[doc(hidden)]
pub trait RuntimeScopedIdType: private::ScopedSealed {}

impl<T: private::ScopedSealed> RuntimeScopedIdType for T {}

macro_rules! scoped_id_type {
    ($name:ident) => {
        impl private::ScopedSealed for $name {
            fn from_parts(runtime: RuntimeId, local: NonZeroU64) -> Self {
                Self { runtime, local }
            }
        }
    };
}

scoped_id_type!(RootId);
scoped_id_type!(PromiseId);
scoped_id_type!(ChannelId);
scoped_id_type!(ResourceGateId);

#[derive(Debug)]
pub struct RuntimeScopedIdCounter<I> {
    runtime: RuntimeId,
    local: IdCounter<NonZeroU64>,
    marker: PhantomData<fn() -> I>,
}

impl private::Sealed for NonZeroU64 {
    fn from_nonzero(value: NonZeroU64) -> Self {
        value
    }
}

impl<I: RuntimeScopedIdType> RuntimeScopedIdCounter<I> {
    /// Mint a scoped-id counter for `runtime`. Runtime-internal registries
    /// (channels, promises, resource gates) that live inside one runtime's
    /// state cell construct their own counter from the runtime identity the
    /// completion registrar issued, so a scoped id always carries its owning
    /// runtime for cross-runtime misuse detection.
    pub fn new(runtime: RuntimeId) -> Self {
        Self {
            runtime,
            local: IdCounter::new(),
            marker: PhantomData,
        }
    }

    pub fn allocate(&mut self) -> Result<I, IdExhausted> {
        self.local
            .allocate()
            .map(|local| <I as private::ScopedSealed>::from_parts(self.runtime, local))
    }

    pub fn is_exhausted(&self) -> bool {
        self.local.is_exhausted()
    }
}

/// The complete, single-owner set of scoped ID allocators for one runtime.
///
/// This value is issued together with the completion registrar and is neither
/// cloneable nor constructible by runtime consumers.
#[doc(hidden)]
pub struct RuntimeScopedIdIssuers {
    root: RuntimeScopedIdCounter<RootId>,
    promise: RuntimeScopedIdCounter<PromiseId>,
    channel: RuntimeScopedIdCounter<ChannelId>,
}

impl RuntimeScopedIdIssuers {
    pub(crate) fn new(runtime: RuntimeId) -> Self {
        Self {
            root: RuntimeScopedIdCounter::new(runtime),
            promise: RuntimeScopedIdCounter::new(runtime),
            channel: RuntimeScopedIdCounter::new(runtime),
        }
    }

    #[doc(hidden)]
    pub fn into_parts(
        self,
    ) -> (
        RuntimeScopedIdCounter<RootId>,
        RuntimeScopedIdCounter<PromiseId>,
        RuntimeScopedIdCounter<ChannelId>,
    ) {
        (self.root, self.promise, self.channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SemaError, Value};
    use std::collections::BTreeMap;

    #[test]
    fn counter_starts_at_one() {
        let mut counter = IdCounter::<TaskId>::new();
        assert_eq!(counter.allocate().expect("ID available").get(), 1);
    }

    #[test]
    fn counter_issues_max_once_then_stays_exhausted() {
        let mut counter = IdCounter::<TaskId>::starting_at(u64::MAX);
        assert_eq!(
            counter.allocate().expect("last ID available").get(),
            u64::MAX
        );
        assert_eq!(counter.allocate(), Err(IdExhausted));
        assert_eq!(counter.allocate(), Err(IdExhausted));
    }

    #[test]
    fn condition_ids_are_emitted_as_integers() {
        // Numeric condition fields are integers per the plan's language-facing
        // contract (callers do arithmetic on :duration-ms / :operation-id).
        // Ids are monotonic counters and durations are bounded, so every real
        // value fits an i64; a large-but-representable id round-trips losslessly.
        let big: u64 = 1_000_000_000_000;
        let operation_id = IdCounter::<OperationId>::starting_at(big)
            .allocate()
            .expect("large operation ID");
        let condition =
            SemaError::timeout_condition("timed out", "runtime/wait", big, Some(operation_id));
        let expected = BTreeMap::from([
            (Value::keyword("type"), Value::keyword("timeout")),
            (Value::keyword("message"), Value::string("timed out")),
            (Value::keyword("operation"), Value::string("runtime/wait")),
            (Value::keyword("duration-ms"), Value::int(big as i64)),
            (Value::keyword("operation-id"), Value::int(big as i64)),
        ]);

        assert!(matches!(condition, SemaError::Condition(value) if value == Value::map(expected)));
    }

    #[test]
    fn atomic_allocator_issues_max_once_then_stays_exhausted() {
        let counter = AtomicU64::new(u64::MAX);
        assert_eq!(
            allocate_atomic(&counter).expect("last ID available").get(),
            u64::MAX
        );
        assert_eq!(allocate_atomic(&counter), Err(IdExhausted));
        assert_eq!(allocate_atomic(&counter), Err(IdExhausted));
    }

    #[test]
    fn runtime_ids_are_process_global_and_unique() {
        let first = RuntimeId::allocate().expect("runtime ID available");
        let second = RuntimeId::allocate().expect("runtime ID available");
        assert!(first < second);
    }

    #[test]
    fn scoped_ids_include_runtime_and_local_identity() {
        let runtime = RuntimeId::allocate().expect("runtime ID available");
        let mut counter = RuntimeScopedIdCounter::<PromiseId>::new(runtime);
        let id = counter.allocate().expect("promise ID available");
        assert_eq!(id.runtime(), runtime);
        assert_eq!(id.local(), 1);
    }

    #[test]
    fn scoped_ids_with_equal_locals_are_distinct_across_runtimes() {
        let first_runtime = RuntimeId::allocate().expect("runtime ID available");
        let second_runtime = RuntimeId::allocate().expect("runtime ID available");

        macro_rules! assert_scoped_identity {
            ($id_type:ty) => {{
                let mut first = RuntimeScopedIdCounter::<$id_type>::new(first_runtime);
                let mut second = RuntimeScopedIdCounter::<$id_type>::new(second_runtime);
                let first_id = first.allocate().expect("scoped ID available");
                let second_id = second.allocate().expect("scoped ID available");

                assert_eq!(first_id.local(), 1);
                assert_eq!(second_id.local(), 1);
                assert_ne!(first_id, second_id);
            }};
        }

        assert_scoped_identity!(RootId);
        assert_scoped_identity!(PromiseId);
        assert_scoped_identity!(ChannelId);
        assert_scoped_identity!(ResourceGateId);
    }

    #[test]
    fn runtime_task_ids_with_equal_local_tasks_are_distinct() {
        let first_runtime = RuntimeId::allocate().expect("runtime ID available");
        let second_runtime = RuntimeId::allocate().expect("runtime ID available");
        let local_task = TaskId::try_from_raw(7).expect("task ID is nonzero");

        let first = RuntimeTaskId::new(first_runtime, local_task);
        let second = RuntimeTaskId::new(second_runtime, local_task);

        assert_eq!(first.runtime(), first_runtime);
        assert_eq!(first.task(), local_task);
        assert_ne!(first, second);
    }

    #[test]
    fn every_identity_has_the_required_value_traits() {
        fn assert_traits<T: Copy + Clone + std::fmt::Debug + Eq + Ord + std::hash::Hash>() {}

        assert_traits::<RuntimeId>();
        assert_traits::<RuntimeTaskId>();
        assert_traits::<RootId>();
        assert_traits::<TaskId>();
        assert_traits::<ScopeId>();
        assert_traits::<PromiseId>();
        assert_traits::<ChannelId>();
        assert_traits::<ResourceGateId>();
        assert_traits::<WaitId>();
        assert_traits::<WaitGeneration>();
        assert_traits::<OperationId>();
        assert_traits::<SettlementSeq>();
        assert_traits::<CompletionKind>();
    }
}
