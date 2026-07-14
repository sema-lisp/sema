use std::rc::Rc;

use sema_core::{
    runtime::{
        CancelReason, CancellationParent, IdCounter, LifetimeOwner, RootId, RuntimeId,
        RuntimeScopedIdCounter, ScopeId, SettlementSeq, TaskId, TaskOutcome, TaskRelations,
        WaitGeneration, WaitId,
    },
    SemaError, Value,
};

use super::{
    ready::ReadyScheduler,
    root::{RootRecord, RootState, RootTransitionError},
    task::{CancellationRequest, StateName, TaskRecord, TaskTransitionError, WaitKey},
};

struct Ids {
    tasks: IdCounter<TaskId>,
    roots: RuntimeScopedIdCounter<RootId>,
    scopes: IdCounter<ScopeId>,
    waits: IdCounter<WaitId>,
    generations: IdCounter<WaitGeneration>,
    settlements: IdCounter<SettlementSeq>,
}

impl Ids {
    fn new() -> Self {
        let runtime = RuntimeId::allocate().expect("runtime ID available");
        Self {
            tasks: IdCounter::new(),
            roots: RuntimeScopedIdCounter::new(runtime),
            scopes: IdCounter::new(),
            waits: IdCounter::new(),
            generations: IdCounter::new(),
            settlements: IdCounter::new(),
        }
    }

    fn wait_key(&mut self) -> WaitKey {
        WaitKey {
            id: self.waits.allocate().expect("wait ID available"),
            generation: self
                .generations
                .allocate()
                .expect("wait generation available"),
        }
    }
}

fn task(ids: &mut Ids) -> TaskRecord {
    let root = ids.roots.allocate().expect("root ID available");
    let parent = ids.tasks.allocate().expect("parent task ID available");
    let scope = ids.scopes.allocate().expect("scope ID available");
    let id = ids.tasks.allocate().expect("task ID available");
    TaskRecord::new(
        id,
        TaskRelations {
            origin_root: root,
            cancellation_parent: CancellationParent::Task(parent),
            lifetime_owner: LifetimeOwner::Scope(scope),
        },
    )
}

fn ready_ids(ids: &mut Ids, task_count: usize) -> (RootId, Vec<TaskId>) {
    let root = ids.roots.allocate().expect("root ID available");
    let tasks = (0..task_count)
        .map(|_| ids.tasks.allocate().expect("task ID available"))
        .collect();
    (root, tasks)
}

#[test]
fn ready_round_robins_perpetually_requeued_roots() {
    let mut ids = Ids::new();
    let [(a, a_tasks), (b, b_tasks), (c, c_tasks)] = [
        ready_ids(&mut ids, 1),
        ready_ids(&mut ids, 1),
        ready_ids(&mut ids, 1),
    ];
    let [a1] = a_tasks.as_slice() else {
        unreachable!()
    };
    let [b1] = b_tasks.as_slice() else {
        unreachable!()
    };
    let [c1] = c_tasks.as_slice() else {
        unreachable!()
    };
    let mut ready = ReadyScheduler::new();
    for (root, task) in [(a, *a1), (b, *b1), (c, *c1)] {
        assert!(ready.enqueue(root, task));
    }

    let mut actual = Vec::new();
    for _ in 0..6 {
        let (root, task) = ready.dequeue().expect("a task remains ready");
        actual.push(root);
        assert!(ready.enqueue(root, task));
    }
    assert_eq!(actual, [a, b, c, a, b, c]);
}

#[test]
fn ready_is_fifo_within_each_root_and_fair_across_roots() {
    let mut ids = Ids::new();
    let (a, a_tasks) = ready_ids(&mut ids, 3);
    let (b, b_tasks) = ready_ids(&mut ids, 1);
    let [a1, a2, a3] = a_tasks.as_slice() else {
        unreachable!()
    };
    let [b1] = b_tasks.as_slice() else {
        unreachable!()
    };
    let mut ready = ReadyScheduler::new();
    for task in [*a1, *a2, *a3] {
        assert!(ready.enqueue(a, task));
    }
    assert!(ready.enqueue(b, *b1));

    let mut actual = Vec::new();
    for _ in 0..6 {
        let (root, task) = ready.dequeue().expect("a task remains ready");
        actual.push(task);
        if root == b {
            assert!(ready.enqueue(root, task));
        }
    }
    assert_eq!(actual, [*a1, *b1, *a2, *b1, *a3, *b1]);
}

#[test]
fn ready_removing_settled_root_preserves_remaining_rotation() {
    let mut ids = Ids::new();
    let [(a, a_tasks), (b, b_tasks), (c, c_tasks)] = [
        ready_ids(&mut ids, 1),
        ready_ids(&mut ids, 1),
        ready_ids(&mut ids, 1),
    ];
    let mut ready = ReadyScheduler::new();
    for (root, task) in [(a, a_tasks[0]), (b, b_tasks[0]), (c, c_tasks[0])] {
        assert!(ready.enqueue(root, task));
    }
    assert_eq!(ready.dequeue(), Some((a, a_tasks[0])));
    assert!(ready.enqueue(a, a_tasks[0]));

    assert_eq!(ready.remove_root(b), vec![b_tasks[0]]);
    assert_eq!(ready.dequeue(), Some((c, c_tasks[0])));
    assert_eq!(ready.dequeue(), Some((a, a_tasks[0])));
    assert_eq!(ready.dequeue(), None);
}

#[test]
fn ready_duplicate_task_wakes_and_root_membership_are_idempotent() {
    let mut ids = Ids::new();
    let (root, tasks) = ready_ids(&mut ids, 2);
    let mut ready = ReadyScheduler::new();

    assert!(ready.enqueue(root, tasks[0]));
    assert!(!ready.enqueue(root, tasks[0]));
    assert!(ready.enqueue(root, tasks[1]));
    assert!(!ready.enqueue(root, tasks[1]));
    assert_eq!(ready.dequeue(), Some((root, tasks[0])));
    assert_eq!(ready.dequeue(), Some((root, tasks[1])));
    assert_eq!(ready.dequeue(), None);
}

#[test]
fn state_task_legal_transition_table() {
    let mut ids = Ids::new();
    let mut record = task(&mut ids);
    assert_eq!(record.state_name(), StateName::Ready);

    record.start().expect("ready task starts");
    assert_eq!(record.state_name(), StateName::Running);
    record.yield_ready().expect("running task yields");
    assert_eq!(record.state_name(), StateName::Ready);

    record.start().expect("ready task restarts");
    let key = ids.wait_key();
    record.wait(key).expect("running task waits");
    assert_eq!(record.state_name(), StateName::Waiting);
    record.wake(key).expect("matching wait wakes");
    assert_eq!(record.state_name(), StateName::Ready);
}

#[test]
fn state_task_rejects_invalid_edges_with_named_states() {
    let mut ids = Ids::new();
    let mut record = task(&mut ids);
    assert_eq!(
        record.yield_ready(),
        Err(TaskTransitionError::Invalid {
            from: StateName::Ready,
            to: StateName::Ready,
        })
    );

    record.start().expect("ready task starts");
    assert_eq!(
        record.start(),
        Err(TaskTransitionError::Invalid {
            from: StateName::Running,
            to: StateName::Running,
        })
    );

    let expected = ids.wait_key();
    let actual = ids.wait_key();
    record.wait(expected).expect("running task waits");
    assert_eq!(
        record.wake(actual),
        Err(TaskTransitionError::WaitMismatch { expected, actual })
    );
    assert_eq!(record.state_name(), StateName::Waiting);
}

#[test]
fn state_task_settles_once_with_one_canonical_rc_for_every_outcome() {
    let outcomes = [
        TaskOutcome::Returned(Value::int(42)),
        TaskOutcome::Failed(SemaError::eval("failed")),
        TaskOutcome::Cancelled(CancelReason::Explicit),
    ];

    for outcome in outcomes {
        let mut ids = Ids::new();
        let mut record = task(&mut ids);
        let sequence = ids.settlements.allocate().expect("sequence available");
        let settlement = record
            .settle(sequence, outcome)
            .expect("unsettled task settles");
        let stored = Rc::clone(record.settlement().expect("settlement stored"));
        assert!(Rc::ptr_eq(&settlement, &stored));
        assert_eq!(
            record
                .settle(
                    ids.settlements.allocate().expect("sequence available"),
                    TaskOutcome::Returned(Value::NIL),
                )
                .unwrap_err(),
            TaskTransitionError::Invalid {
                from: StateName::Settled,
                to: StateName::Settled,
            }
        );
        assert!(Rc::ptr_eq(&stored, record.settlement().unwrap()));
    }
}

#[test]
fn state_task_settlement_is_legal_from_every_live_state() {
    for target in [StateName::Ready, StateName::Running, StateName::Waiting] {
        let mut ids = Ids::new();
        let mut record = task(&mut ids);
        if target != StateName::Ready {
            record.start().unwrap();
        }
        if target == StateName::Waiting {
            let key = ids.wait_key();
            record.wait(key).unwrap();
        }
        record
            .settle(
                ids.settlements.allocate().unwrap(),
                TaskOutcome::Returned(Value::NIL),
            )
            .unwrap();
        assert_eq!(record.state_name(), StateName::Settled);
        assert_eq!(
            record.start(),
            Err(TaskTransitionError::Invalid {
                from: StateName::Settled,
                to: StateName::Running,
            })
        );
    }
}

#[test]
fn state_task_relations_are_stable_and_cancellation_is_first_reason_wins() {
    let mut ids = Ids::new();
    let mut record = task(&mut ids);
    let id = record.id();
    let relations = record.relations();
    record.start().unwrap();
    record.wait(ids.wait_key()).unwrap();
    record.wake(record.wait_key().unwrap()).unwrap();
    assert_eq!(record.id(), id);
    assert_eq!(record.relations(), relations);

    assert!(record.request_cancellation(CancelReason::Timeout));
    assert!(!record.request_cancellation(CancelReason::HostStop));
    assert_eq!(
        record.cancellation(),
        Some(CancellationRequest {
            reason: CancelReason::Timeout
        })
    );
}

#[test]
fn state_root_settles_only_from_its_main_task_and_only_once() {
    let mut ids = Ids::new();
    let root_id = ids.roots.allocate().unwrap();
    let main_task = ids.tasks.allocate().unwrap();
    let other_task = ids.tasks.allocate().unwrap();
    let mut root = RootRecord::new(root_id, main_task);
    assert_eq!(root.id(), root_id);
    assert!(matches!(
        root.state(),
        RootState::Running { main_task: actual } if *actual == main_task
    ));

    let settlement = Rc::new(sema_core::runtime::TaskSettlement {
        sequence: ids.settlements.allocate().unwrap(),
        outcome: TaskOutcome::Returned(Value::NIL),
    });
    assert_eq!(
        root.settle(other_task, Rc::clone(&settlement)),
        Err(RootTransitionError::WrongMainTask {
            expected: main_task,
            actual: other_task,
        })
    );
    root.settle(main_task, Rc::clone(&settlement)).unwrap();
    let RootState::Settled(stored) = root.state() else {
        panic!("root should be settled")
    };
    assert!(Rc::ptr_eq(stored, &settlement));
    assert_eq!(
        root.settle(main_task, settlement),
        Err(RootTransitionError::AlreadySettled)
    );
}
