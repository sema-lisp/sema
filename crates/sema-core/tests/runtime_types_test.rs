use std::collections::BTreeSet;

use sema_core::runtime::{
    CancellationParent, ChannelId, CompletionKind, LifetimeOwner, OperationId, PromiseId, RootId,
    RuntimeId, RuntimeScopedIdCounter, ScopeId, SettlementSeq, TaskId, TaskRelations,
    WaitGeneration, WaitId,
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

    assert_traits::<RuntimeId>();
    assert_traits::<RootId>();
    assert_traits::<TaskId>();
    assert_traits::<ScopeId>();
    assert_traits::<PromiseId>();
    assert_traits::<ChannelId>();
    assert_traits::<WaitId>();
    assert_traits::<WaitGeneration>();
    assert_traits::<OperationId>();
    assert_traits::<SettlementSeq>();
    assert_traits::<CompletionKind>();

    let first = TaskId::try_from_raw(1).expect("valid ID");
    let second = TaskId::try_from_raw(2).expect("valid ID");
    assert_eq!(
        BTreeSet::from([second, first]),
        BTreeSet::from([first, second])
    );
}

#[test]
fn scoped_ids_expose_runtime_and_local_identity_publicly() {
    let runtime = RuntimeId::allocate().expect("runtime ID should be available");
    assert_ne!(runtime.get(), 0);

    macro_rules! assert_scoped_accessors {
        ($id_type:ty) => {{
            let mut counter = RuntimeScopedIdCounter::<$id_type>::new(runtime);
            let id = counter.allocate().expect("scoped ID should be available");
            assert_eq!(id.runtime(), runtime);
            assert_eq!(id.local(), 1);
            assert_eq!(id.get(), 1);
        }};
    }

    assert_scoped_accessors!(RootId);
    assert_scoped_accessors!(PromiseId);
    assert_scoped_accessors!(ChannelId);
}

#[test]
fn relationships_keep_origin_cancellation_and_lifetime_separate() {
    let root_task = TaskId::try_from_raw(1).expect("valid ID");
    let child_task = TaskId::try_from_raw(2).expect("valid ID");
    let runtime = RuntimeId::allocate().expect("runtime ID should be available");
    let mut roots = RuntimeScopedIdCounter::<RootId>::new(runtime);
    let origin_root = roots.allocate().expect("root ID should be available");
    let relations = TaskRelations {
        origin_root,
        cancellation_parent: CancellationParent::Task(root_task),
        lifetime_owner: LifetimeOwner::Task(child_task),
    };

    assert_eq!(relations.origin_root, origin_root);
    assert_eq!(
        relations.cancellation_parent,
        CancellationParent::Task(root_task)
    );
    assert_eq!(relations.lifetime_owner, LifetimeOwner::Task(child_task));
    assert_ne!(relations.cancellation_parent, CancellationParent::None);
}
