use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sema_core::runtime::{
    CancelReason, CancellationParent, ChannelId, CompletionDelivery, CompletionKind,
    CompletionRegistrar, CompletionSender, ExternalCompletion, IdCounter, LifetimeOwner,
    OperationId, PromiseId, RootId, RuntimeId, RuntimeScopedIdIssuers, ScopeId, SettlementSeq,
    TaskId, TaskOutcome, TaskRelations, TaskSettlement, WaitGeneration, WaitId,
};
use sema_core::{SemaError, Value};

struct ClosedInbox;

impl CompletionSender for ClosedInbox {
    fn send(&self, _: ExternalCompletion) -> CompletionDelivery {
        CompletionDelivery::InboxClosed
    }
}

fn runtime_issuers() -> (RuntimeId, RuntimeScopedIdIssuers) {
    let (runtime, _registrar, issuers) = CompletionRegistrar::register(Arc::new(ClosedInbox))
        .expect("runtime authority should be available");
    (runtime, issuers)
}

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
    let (runtime, issuers) = runtime_issuers();
    let (roots, promises, channels) = issuers.into_parts();
    assert_ne!(runtime.get(), 0);

    macro_rules! assert_scoped_accessors {
        ($id_type:ty, $counter:expr) => {{
            let mut counter = $counter;
            let id = counter.allocate().expect("scoped ID should be available");
            assert_eq!(id.runtime(), runtime);
            assert_eq!(id.local(), 1);
            assert_eq!(id.get(), 1);
        }};
    }

    assert_scoped_accessors!(RootId, roots);
    assert_scoped_accessors!(PromiseId, promises);
    assert_scoped_accessors!(ChannelId, channels);
}

#[test]
fn relationships_keep_origin_cancellation_and_lifetime_separate() {
    let root_task = TaskId::try_from_raw(1).expect("valid ID");
    let child_task = TaskId::try_from_raw(2).expect("valid ID");
    let (_runtime, issuers) = runtime_issuers();
    let (mut roots, _, _) = issuers.into_parts();
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

#[test]
fn settlement_outcomes_preserve_return_failure_and_cancellation() {
    let mut sequences = IdCounter::<SettlementSeq>::new();
    let returned = TaskSettlement {
        sequence: sequences.allocate().expect("settlement sequence"),
        outcome: TaskOutcome::Returned(Value::int(42)),
    };
    let failed = TaskSettlement {
        sequence: sequences.allocate().expect("settlement sequence"),
        outcome: TaskOutcome::Failed(SemaError::eval("broken")),
    };
    let cancelled = TaskSettlement {
        sequence: sequences.allocate().expect("settlement sequence"),
        outcome: TaskOutcome::Cancelled(CancelReason::Explicit),
    };

    assert!(matches!(returned.outcome, TaskOutcome::Returned(value) if value == Value::int(42)));
    assert!(
        matches!(failed.outcome, TaskOutcome::Failed(SemaError::Eval(message)) if message == "broken")
    );
    assert!(matches!(
        cancelled.outcome,
        TaskOutcome::Cancelled(CancelReason::Explicit)
    ));
    assert_ne!(returned.sequence, failed.sequence);
    assert_ne!(failed.sequence, cancelled.sequence);
}

#[test]
fn condition_constructors_produce_exact_stable_maps() {
    let (_runtime, issuers) = runtime_issuers();
    let (mut roots, _, _) = issuers.into_parts();
    let root_id = roots.allocate().expect("root ID");
    let scope_id = IdCounter::<ScopeId>::new().allocate().expect("scope ID");
    let operation_id = IdCounter::<OperationId>::new()
        .allocate()
        .expect("operation ID");

    let cancelled = SemaError::cancelled_condition(
        "request cancelled",
        CancelReason::ResourceDisconnect,
        Some(root_id),
        Some(scope_id),
        Some(operation_id),
        Some("http/get"),
        Some(u64::MAX),
        Some("socket"),
    );
    let expected_cancelled = BTreeMap::from([
        (Value::keyword("type"), Value::keyword("cancelled")),
        (
            Value::keyword("message"),
            Value::string("request cancelled"),
        ),
        (
            Value::keyword("reason"),
            Value::keyword("resource-disconnect"),
        ),
        (Value::keyword("root-id"), Value::string("1")),
        (Value::keyword("scope-id"), Value::string("1")),
        (Value::keyword("operation-id"), Value::string("1")),
        (Value::keyword("operation"), Value::string("http/get")),
        (
            Value::keyword("duration-ms"),
            Value::string(&u64::MAX.to_string()),
        ),
        (Value::keyword("resource-kind"), Value::string("socket")),
    ]);
    assert!(
        matches!(cancelled, SemaError::Condition(value) if value == Value::map(expected_cancelled))
    );

    let timeout = SemaError::timeout_condition(
        "operation timed out",
        "http/get",
        u64::MAX,
        Some(operation_id),
    );
    let expected_timeout = BTreeMap::from([
        (Value::keyword("type"), Value::keyword("timeout")),
        (
            Value::keyword("message"),
            Value::string("operation timed out"),
        ),
        (Value::keyword("operation"), Value::string("http/get")),
        (
            Value::keyword("duration-ms"),
            Value::string(&u64::MAX.to_string()),
        ),
        (Value::keyword("operation-id"), Value::string("1")),
    ]);
    assert!(
        matches!(timeout, SemaError::Condition(value) if value == Value::map(expected_timeout))
    );
}

#[test]
fn condition_cancellation_reasons_are_stable_keywords_and_optional_fields_are_omitted() {
    let cases = [
        (CancelReason::Root, "root"),
        (CancelReason::Owner, "owner"),
        (CancelReason::Explicit, "explicit"),
        (CancelReason::Timeout, "timeout"),
        (CancelReason::HostStop, "host-stop"),
        (CancelReason::ResourceDisconnect, "resource-disconnect"),
        (CancelReason::InterpreterShutdown, "interpreter-shutdown"),
    ];

    for (reason, keyword) in cases {
        let condition =
            SemaError::cancelled_condition("cancelled", reason, None, None, None, None, None, None);
        let expected = BTreeMap::from([
            (Value::keyword("type"), Value::keyword("cancelled")),
            (Value::keyword("message"), Value::string("cancelled")),
            (Value::keyword("reason"), Value::keyword(keyword)),
        ]);
        assert!(matches!(condition, SemaError::Condition(value) if value == Value::map(expected)));
    }
}

#[test]
fn condition_maps_for_cancellation_and_timeout_reraise_verbatim() {
    let conditions = [
        SemaError::cancelled_condition(
            "stopped",
            CancelReason::Explicit,
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        SemaError::timeout_condition("timed out", "runtime/wait", u64::MAX, None),
    ];

    for condition in conditions {
        let SemaError::Condition(expected) = condition else {
            panic!("condition constructor returned the wrong error variant");
        };
        let actual = SemaError::from_thrown(expected.clone());

        assert!(matches!(actual, SemaError::Condition(value) if value == expected));
    }
}
