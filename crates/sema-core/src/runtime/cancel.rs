use super::{RootId, ScopeId, TaskId};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CancellationParent {
    None,
    Root(RootId),
    Scope(ScopeId),
    Task(TaskId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LifetimeOwner {
    Interpreter,
    Root(RootId),
    Scope(ScopeId),
    Task(TaskId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CancelReason {
    Root,
    Owner,
    Explicit,
    Timeout,
    HostStop,
    ResourceDisconnect,
    InterpreterShutdown,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaskRelations {
    pub origin_root: RootId,
    pub cancellation_parent: CancellationParent,
    pub lifetime_owner: LifetimeOwner,
}
