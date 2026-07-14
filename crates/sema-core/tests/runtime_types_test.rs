use std::collections::BTreeSet;

use sema_core::runtime::{
    CancelReason, CancellationParent, CompletionKind, LifetimeOwner, RootId, RuntimeId,
    RuntimeScopedIdCounter, TaskId, TaskRelations,
};

#[test]
fn ids_reject_zero_and_expose_nonzero_raw_values() {
    assert!(TaskId::try_from_raw(0).is_err());
    assert!(CompletionKind::try_from_raw(0).is_err());

    let task = TaskId::try_from_raw(42).expect("nonzero task ID should be valid");
    let kind = CompletionKind::try_from_raw(7).expect("nonzero completion kind should be valid");
    assert_eq!(task.get(), 42);
    assert_eq!(kind.get(), 7);
}

#[test]
fn ids_support_value_traits() {
    fn assert_traits<T: Copy + Clone + std::fmt::Debug + Eq + Ord + std::hash::Hash>() {}
    assert_traits::<TaskId>();
    assert_traits::<CompletionKind>();

    let first = TaskId::try_from_raw(1).expect("valid ID");
    let second = TaskId::try_from_raw(2).expect("valid ID");
    assert_eq!(
        BTreeSet::from([second, first]),
        BTreeSet::from([first, second])
    );
}

#[test]
fn relationships_keep_origin_cancellation_and_lifetime_separate() {
    let root_task = TaskId::try_from_raw(1).expect("valid ID");
    let child_task = TaskId::try_from_raw(2).expect("valid ID");
    let runtime = RuntimeId::allocate().expect("runtime ID should be available");
    let mut roots = RuntimeScopedIdCounter::<RootId>::new(runtime);
    let relations = TaskRelations {
        origin_root: roots.allocate().expect("root ID should be available"),
        cancellation_parent: CancellationParent::Task(root_task),
        lifetime_owner: LifetimeOwner::Task(child_task),
    };

    assert_eq!(
        relations.cancellation_parent,
        CancellationParent::Task(root_task)
    );
    assert_eq!(relations.lifetime_owner, LifetimeOwner::Task(child_task));
    assert_ne!(relations.cancellation_parent, CancellationParent::None);
    assert_eq!(CancelReason::Explicit, CancelReason::Explicit);
}
