use std::any::Any;
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sema_core::{
    runtime::{
        CancelReason, CancellationParent, CompletionDecoder, CompletionDelivery, CompletionKind,
        CompletionRegistrar, CompletionSender, ExecutorAttachError, ExecutorDispatch,
        ExecutorLease, ExecutorShutdown, ExecutorSnapshot, ExternalCompletion, IdCounter,
        InterruptibleResource, IoExecutor, LifetimeOwner, NativeCall, NativeCallContext,
        NativeContinuation, NativeOutcome, NativeResult, NativeSuspend, PreparedExternalOperation,
        ResumeInput, RootId, RunningSubmission, RuntimeId, RuntimeScopedIdCounter,
        RuntimeScopedIdIssuers, ScopeId, SettlementSeq, SubmissionRejected, SubmitErrorKind,
        TaskContextHandle, TaskId, TaskOutcome, TaskRelations, Trace, WaitGeneration, WaitId,
        WaitKind,
    },
    SemaError, Value,
};

struct ClosedCompletionInbox;

impl CompletionSender for ClosedCompletionInbox {
    fn send(&self, _: ExternalCompletion) -> CompletionDelivery {
        CompletionDelivery::InboxClosed
    }
}

fn runtime_issuers() -> (RuntimeId, RuntimeScopedIdIssuers) {
    let (runtime, _registrar, issuers) =
        CompletionRegistrar::register(Arc::new(ClosedCompletionInbox)).unwrap();
    (runtime, issuers)
}

use super::{
    drive::{BoundedDriver, DriveBudget, RuntimeClock},
    ready::ReadyScheduler,
    root::{RootRecord, RootState, RootTransitionError},
    task::{CancellationRequest, StateName, TaskRecord, TaskTransitionError, WaitKey},
    timer::TimerQueue,
    wait::{
        CompletionRoute, ForgedCompletionMutation, RegisterExternalError, RuntimeCreateError,
        WaitRuntime,
    },
    RootPoll, Runtime, TestPreparedTask,
};

#[test]
fn protocol_registries_reject_foreign_ids_and_preserve_canonical_settlements() {
    use super::{PromiseRegistry, PromiseState};
    let (first_runtime, first_issuers) = runtime_issuers();
    let (_, first_promises, _) = first_issuers.into_parts();
    let (second_runtime, second_issuers) = runtime_issuers();
    let (_, second_promises, _) = second_issuers.into_parts();
    let mut promises = PromiseRegistry::new(first_runtime, first_promises);
    let id = promises.allocate_pending(None).unwrap();
    let foreign = super::PromiseRegistry::new(second_runtime, second_promises)
        .allocate_pending(None)
        .unwrap();
    assert!(matches!(
        promises.state(foreign),
        Err(super::RegistryError::WrongRuntime)
    ));
    let settlement = Rc::new(sema_core::runtime::TaskSettlement {
        sequence: IdCounter::<SettlementSeq>::new().allocate().unwrap(),
        outcome: TaskOutcome::Returned(Value::int(7)),
    });
    promises.settle(id, Rc::clone(&settlement)).unwrap();
    let PromiseState::Returned(observed) = promises.state(id).unwrap() else {
        panic!()
    };
    assert!(Rc::ptr_eq(&settlement, &observed));
}

#[test]
fn channel_registry_is_fifo_and_cancellation_is_exact() {
    use super::{ChannelRegistry, ChannelResult};
    let (runtime, issuers) = runtime_issuers();
    let (_, _, channel_ids) = issuers.into_parts();
    let mut channels = ChannelRegistry::new(runtime, channel_ids);
    let channel = channels.allocate(0).unwrap();
    let waits = WaitRuntime::new(Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    }))
    .unwrap();
    let mut send_key = waits.issue_internal_wait().unwrap();
    send_key.runtime = runtime;
    let mut receive_key = waits.issue_internal_wait().unwrap();
    receive_key.runtime = runtime;
    assert_eq!(
        channels
            .send(
                channel,
                send_key,
                TaskId::try_from_raw(1).unwrap(),
                Value::int(4)
            )
            .unwrap(),
        ChannelResult::Waiting
    );
    assert_eq!(
        channels
            .receive(channel, receive_key, TaskId::try_from_raw(2).unwrap())
            .unwrap(),
        ChannelResult::Received(Value::int(4))
    );
    let wake = channels.take_wake(send_key).unwrap();
    assert_eq!(wake.key, send_key);
    assert_eq!(wake.task, TaskId::try_from_raw(1).unwrap());
    assert!(channels
        .cancel_wait(channel, receive_key)
        .unwrap()
        .is_none());
}

#[test]
fn channel_cancel_wait_surfaces_blocked_sender_value_and_receiver_kind() {
    use super::{CancelledChannelWait, ChannelRegistry, ChannelResult};
    let (runtime, issuers) = runtime_issuers();
    let (_, _, channel_ids) = issuers.into_parts();
    let mut channels = ChannelRegistry::new(runtime, channel_ids);
    let channel = channels.allocate(0).unwrap();
    let waits = WaitRuntime::new(Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    }))
    .unwrap();

    // A blocked sender on an unbuffered channel with no receiver.
    let mut send_key = waits.issue_internal_wait().unwrap();
    send_key.runtime = runtime;
    assert_eq!(
        channels
            .send(
                channel,
                send_key,
                TaskId::try_from_raw(1).unwrap(),
                Value::int(7)
            )
            .unwrap(),
        ChannelResult::Waiting
    );
    // Cancelling the sender surfaces its unsent value rather than swallowing it.
    match channels.cancel_wait(channel, send_key).unwrap() {
        Some(CancelledChannelWait::Sender(value)) => assert_eq!(value, Value::int(7)),
        Some(CancelledChannelWait::Receiver) => panic!("cancelled a sender, got a receiver"),
        None => panic!("expected a registered sender wait to cancel"),
    }

    // A blocked receiver on the now-empty channel is distinguishable from a sender.
    let mut receive_key = waits.issue_internal_wait().unwrap();
    receive_key.runtime = runtime;
    assert_eq!(
        channels
            .receive(channel, receive_key, TaskId::try_from_raw(2).unwrap())
            .unwrap(),
        ChannelResult::Waiting
    );
    assert!(matches!(
        channels.cancel_wait(channel, receive_key).unwrap(),
        Some(CancelledChannelWait::Receiver)
    ));
}

#[test]
fn channel_try_receive_rendezvous_and_promotes_blocked_senders() {
    use super::{ChannelRegistry, ChannelResult};
    let (runtime, issuers) = runtime_issuers();
    let (_, _, channel_ids) = issuers.into_parts();
    let mut channels = ChannelRegistry::new(runtime, channel_ids);
    let waits = WaitRuntime::new(Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    }))
    .unwrap();
    let keys: Vec<_> = (0..5)
        .map(|_| {
            let mut key = waits.issue_internal_wait().unwrap();
            key.runtime = runtime;
            key
        })
        .collect();
    let unbuffered = channels.allocate(0).unwrap();
    assert_eq!(
        channels
            .send(
                unbuffered,
                keys[0],
                TaskId::try_from_raw(1).unwrap(),
                Value::FALSE
            )
            .unwrap(),
        ChannelResult::Waiting
    );
    assert_eq!(
        channels.try_receive(unbuffered).unwrap(),
        ChannelResult::Received(Value::FALSE)
    );
    assert_eq!(
        channels.take_wake(keys[0]).unwrap().result,
        ChannelResult::Sent
    );

    let buffered = channels.allocate(1).unwrap();
    assert_eq!(
        channels
            .send(
                buffered,
                keys[1],
                TaskId::try_from_raw(2).unwrap(),
                Value::int(1)
            )
            .unwrap(),
        ChannelResult::Sent
    );
    assert_eq!(
        channels
            .send(
                buffered,
                keys[2],
                TaskId::try_from_raw(3).unwrap(),
                Value::int(2)
            )
            .unwrap(),
        ChannelResult::Waiting
    );
    assert_eq!(
        channels.try_receive(buffered).unwrap(),
        ChannelResult::Received(Value::int(1))
    );
    assert_eq!(
        channels.take_wake(keys[2]).unwrap().result,
        ChannelResult::Sent
    );
    assert_eq!(
        channels.try_receive(buffered).unwrap(),
        ChannelResult::Received(Value::int(2))
    );
}

#[test]
fn promise_observers_preserve_registration_order_and_reject_duplicates() {
    let (runtime, issuers) = runtime_issuers();
    let (_, promise_ids, _) = issuers.into_parts();
    let mut promises = super::PromiseRegistry::new(runtime, promise_ids);
    let promise = promises.allocate_pending(None).unwrap();
    let waits = WaitRuntime::new(Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    }))
    .unwrap();
    let keys: Vec<_> = (0..3)
        .map(|_| {
            let mut key = waits.issue_internal_wait().unwrap();
            key.runtime = runtime;
            key
        })
        .collect();
    let tasks: Vec<_> = (1..=3)
        .map(|raw| TaskId::try_from_raw(raw).unwrap())
        .collect();
    for (&key, &task) in keys.iter().zip(&tasks) {
        assert!(promises.observe(promise, key, task).unwrap());
    }
    assert!(matches!(
        promises.observe(promise, keys[1], tasks[1]),
        Err(super::RegistryError::DuplicateWait)
    ));
    assert!(promises.cancel_observation(promise, keys[1]).unwrap());
    let settlement = Rc::new(sema_core::runtime::TaskSettlement {
        sequence: IdCounter::<SettlementSeq>::new().allocate().unwrap(),
        outcome: TaskOutcome::Returned(Value::int(1)),
    });
    assert_eq!(
        promises
            .settle(promise, Rc::clone(&settlement))
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![(keys[0], tasks[0]), (keys[2], tasks[2])]
    );
    assert!(matches!(
        promises.settle(promise, settlement),
        Err(super::RegistryError::AlreadySettled)
    ));
}

#[test]
fn dormant_protocol_wait_fails_through_continuation() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let (foreign_runtime, issuers) = runtime_issuers();
    let (_, promise_ids, _) = issuers.into_parts();
    let promise = super::PromiseRegistry::new(foreign_runtime, promise_ids)
        .allocate_pending(None)
        .unwrap();
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            sema_core::runtime::NativeSuspend {
                wait: sema_core::runtime::WaitKind::Promise(promise),
                continuation: Box::new(RecordingContinuation(Arc::clone(&events))),
            },
        ))))
        .unwrap();
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(*events.lock().unwrap(), vec!["resume-failed"]);
}

struct RuntimeResponseContinuation(Arc<Mutex<Vec<&'static str>>>);
impl Trace for RuntimeResponseContinuation {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

struct FateContinuation {
    events: Arc<Mutex<Vec<&'static str>>>,
    resumed: bool,
}

impl Drop for FateContinuation {
    fn drop(&mut self) {
        if !self.resumed {
            self.events.lock().unwrap().push("dropped");
        }
    }
}

impl Trace for FateContinuation {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for FateContinuation {
    fn resume(mut self: Box<Self>, _: &mut NativeCallContext<'_>, _: ResumeInput) -> NativeResult {
        self.resumed = true;
        self.events.lock().unwrap().push("resumed");
        Ok(NativeOutcome::Return(Value::NIL))
    }
}
impl NativeContinuation for RuntimeResponseContinuation {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, input: ResumeInput) -> NativeResult {
        self.0.lock().unwrap().push(match input {
            ResumeInput::Runtime(_) => "apply-response",
            _ => "wrong-response",
        });
        Ok(NativeOutcome::Return(Value::int(3)))
    }
}

#[test]
fn runtime_request_dispatch_and_response_application_are_separately_charged() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::CreateChannel {
                capacity: 1,
                continuation: Box::new(RuntimeResponseContinuation(Arc::clone(&events))),
            },
        ))))
        .unwrap();
    let one = drive_budget(1);
    let mut turns = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        assert!(matches!(
            runtime.drive(&one).unwrap(),
            super::DriveState::Progress { work_items: 1, .. }
        ));
        turns += 1;
        assert!(turns < 10);
    }
    assert_eq!(*events.lock().unwrap(), vec!["apply-response"]);
    assert_eq!(
        turns, 6,
        "visit -> action -> dispatch -> response/resume -> apply -> settle"
    );
}

#[test]
fn synthetic_settlement_exhaustion_is_terminal_and_keeps_one_continuation_fate() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let fates = Arc::new(Mutex::new(Vec::new()));
    let _handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::CreateSettledPromise {
                outcome: TaskOutcome::Returned(Value::int(1)),
                continuation: Box::new(FateContinuation {
                    events: Arc::clone(&fates),
                    resumed: false,
                }),
            },
        ))))
        .unwrap();
    runtime.force_settlement_exhaustion_for_test();

    assert_eq!(
        runtime.drive(&drive_budget(8)),
        Err(super::RuntimeFault::IdExhausted { kind: "settlement" })
    );
    assert!(
        fates.lock().unwrap().is_empty(),
        "terminal cleanup retains fate"
    );
    assert_eq!(
        runtime.drive(&drive_budget(8)),
        Err(super::RuntimeFault::IdExhausted { kind: "settlement" })
    );
    assert!(
        fates.lock().unwrap().is_empty(),
        "terminal drive must not resume or drop queued continuation"
    );
    assert_eq!(runtime.registry_counts_for_test().0, 0);
    assert!(matches!(
        runtime.submit_test_root(TestPreparedTask::returned(Value::NIL)),
        Err(super::SubmitRootError::ShuttingDown)
    ));
    assert_eq!(
        runtime.shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(8),
        }),
        Err(super::RuntimeFault::IdExhausted { kind: "settlement" })
    );
    assert_eq!(
        *fates.lock().unwrap(),
        vec!["dropped"],
        "terminal abort drops the continuation exactly once without resuming it"
    );
    assert_eq!(
        runtime.drive(&drive_budget(8)),
        Err(super::RuntimeFault::IdExhausted { kind: "settlement" })
    );
    assert_eq!(*fates.lock().unwrap(), vec!["dropped"]);
}

#[test]
fn promise_and_channel_allocator_exhaustion_leave_no_registry_record() {
    for kind in ["promise", "channel"] {
        let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
        runtime.force_registry_exhaustion_for_test(kind);
        let request = if kind == "promise" {
            sema_core::runtime::RuntimeRequest::CreateSettledPromise {
                outcome: TaskOutcome::Returned(Value::NIL),
                continuation: Box::new(RuntimeResponseContinuation(Arc::new(Mutex::new(
                    Vec::new(),
                )))),
            }
        } else {
            sema_core::runtime::RuntimeRequest::CreateChannel {
                capacity: 1,
                continuation: Box::new(RuntimeResponseContinuation(Arc::new(Mutex::new(
                    Vec::new(),
                )))),
            }
        };
        let handle = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
                request,
            ))))
            .unwrap();
        while matches!(handle.poll_result(), RootPoll::Pending) {
            runtime.drive(&drive_budget(1)).unwrap();
        }
        assert_eq!(runtime.registry_counts_for_test(), (0, 0), "{kind}");
    }
}

#[test]
fn settled_promise_identity_exhaustion_drops_outcome_outside_state_borrow() {
    for kind in ["promise", "settlement"] {
        let clock = Rc::new(FakeClock::new());
        let runtime = runtime_with_inline_executor(clock.clone());
        let observed = runtime
            .submit_test_root(TestPreparedTask::yield_forever())
            .unwrap();
        let owner = PollHandleOnDrop(observed.clone());
        let value = Value::native_fn(sema_core::NativeFn::simple("outcome-drop", move |_| {
            let _owner = &owner;
            Ok(Value::NIL)
        }));
        let request = sema_core::runtime::RuntimeRequest::CreateSettledPromise {
            outcome: TaskOutcome::Returned(value),
            continuation: Box::new(RuntimeResponseContinuation(Arc::new(
                Mutex::new(Vec::new()),
            ))),
        };
        let request_handle = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
                request,
            ))))
            .unwrap();
        if kind == "settlement" {
            runtime.force_settlement_exhaustion_for_test();
        } else {
            runtime.force_registry_exhaustion_for_test(kind);
        }

        let drive = runtime.drive(&drive_budget(8));
        if kind == "settlement" {
            assert_eq!(
                drive,
                Err(super::RuntimeFault::IdExhausted { kind: "settlement" })
            );
            assert!(matches!(request_handle.poll_result(), RootPoll::Pending));
        } else {
            drive.unwrap();
            assert!(!matches!(request_handle.poll_result(), RootPoll::Pending));
        }
        assert!(matches!(observed.poll_result(), RootPoll::Pending));
        assert!(observed.cancel(CancelReason::Explicit));

        let shutdown = runtime.shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(8),
        });
        if kind == "settlement" {
            assert_eq!(
                shutdown,
                Err(super::RuntimeFault::IdExhausted { kind: "settlement" })
            );
        } else {
            shutdown.unwrap();
        }
        assert!(!matches!(observed.poll_result(), RootPoll::Pending));
    }
}

#[test]
fn promise_observer_cancellation_does_not_cancel_supplied_promise() {
    let (runtime, issuers) = runtime_issuers();
    let (_, promise_ids, _) = issuers.into_parts();
    let mut promises = super::PromiseRegistry::new(runtime, promise_ids);
    let promise = promises
        .allocate_pending(Some(TaskId::try_from_raw(9).unwrap()))
        .unwrap();
    let waits = WaitRuntime::new(Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    }))
    .unwrap();
    let mut key = waits.issue_internal_wait().unwrap();
    key.runtime = runtime;
    assert!(promises
        .observe(promise, key, TaskId::try_from_raw(10).unwrap())
        .unwrap());
    assert!(promises.cancel_observation(promise, key).unwrap());
    assert_eq!(
        promises.task(promise).unwrap(),
        Some(TaskId::try_from_raw(9).unwrap())
    );
    assert!(matches!(
        promises.state(promise).unwrap(),
        super::PromiseState::Pending
    ));
}

struct CaptureRuntimeContinuation(Rc<RefCell<Vec<sema_core::runtime::RuntimeResponse>>>);

impl Trace for CaptureRuntimeContinuation {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for CaptureRuntimeContinuation {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, input: ResumeInput) -> NativeResult {
        let ResumeInput::Runtime(response) = input else {
            panic!("protocol wait must resume with a runtime response");
        };
        self.0.borrow_mut().push(response);
        Ok(NativeOutcome::Return(Value::NIL))
    }
}

struct CaptureFailureContinuation(Rc<Cell<usize>>);

impl Trace for CaptureFailureContinuation {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for CaptureFailureContinuation {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, input: ResumeInput) -> NativeResult {
        assert!(matches!(input, ResumeInput::Failed(_)));
        self.0.set(self.0.get() + 1);
        Ok(NativeOutcome::Return(Value::NIL))
    }
}

#[test]
fn runtime_delayed_promise_wait_resumes_with_canonical_settlement() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let promise = runtime.create_pending_promise_for_test();
    let responses = Rc::new(RefCell::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::PromiseSetWait {
                wait: sema_core::runtime::PromiseSetWait {
                    promises: vec![promise],
                    mode: sema_core::runtime::PromiseSetMode::Race,
                },
                continuation: Box::new(CaptureRuntimeContinuation(Rc::clone(&responses))),
            },
        ))))
        .unwrap();

    runtime.drive(&drive_budget(8)).unwrap();
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    let settlement = runtime.settle_promise_for_test(promise, TaskOutcome::Returned(Value::FALSE));
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    let captured = responses.borrow();
    let sema_core::runtime::RuntimeResponse::Settlement(Some(observed)) = &captured[0] else {
        panic!("race returns one settlement");
    };
    assert!(Rc::ptr_eq(&settlement, observed));
    assert!(matches!(&settlement.outcome, TaskOutcome::Returned(value) if *value == Value::FALSE));
}

#[test]
fn runtime_promise_all_preserves_input_order_after_reverse_settlement() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let first = runtime.create_pending_promise_for_test();
    let second = runtime.create_pending_promise_for_test();
    let responses = Rc::new(RefCell::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::PromiseSetWait {
                wait: sema_core::runtime::PromiseSetWait {
                    promises: vec![first, second],
                    mode: sema_core::runtime::PromiseSetMode::All,
                },
                continuation: Box::new(CaptureRuntimeContinuation(Rc::clone(&responses))),
            },
        ))))
        .unwrap();
    runtime.drive(&drive_budget(8)).unwrap();
    let second_settlement =
        runtime.settle_promise_for_test(second, TaskOutcome::Returned(Value::int(2)));
    runtime.drive(&drive_budget(1)).unwrap();
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    let first_settlement =
        runtime.settle_promise_for_test(first, TaskOutcome::Returned(Value::int(1)));
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    let captured = responses.borrow();
    let sema_core::runtime::RuntimeResponse::Settlements(settlements) = &captured[0] else {
        panic!("all returns canonical settlements");
    };
    assert!(Rc::ptr_eq(&settlements[0], &first_settlement));
    assert!(Rc::ptr_eq(&settlements[1], &second_settlement));
}

#[test]
fn runtime_pending_duplicate_promise_is_valid_for_all_and_race() {
    for mode in [
        sema_core::runtime::PromiseSetMode::All,
        sema_core::runtime::PromiseSetMode::Race,
    ] {
        let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
        let promise = runtime.create_pending_promise_for_test();
        let responses = Rc::new(RefCell::new(Vec::new()));
        let handle = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
                sema_core::runtime::RuntimeRequest::PromiseSetWait {
                    wait: sema_core::runtime::PromiseSetWait {
                        promises: vec![promise, promise],
                        mode,
                    },
                    continuation: Box::new(CaptureRuntimeContinuation(Rc::clone(&responses))),
                },
            ))))
            .unwrap();
        runtime.drive(&drive_budget(8)).unwrap();
        assert!(matches!(handle.poll_result(), RootPoll::Pending));
        let settlement =
            runtime.settle_promise_for_test(promise, TaskOutcome::Returned(Value::int(3)));
        while matches!(handle.poll_result(), RootPoll::Pending) {
            runtime.drive(&drive_budget(1)).unwrap();
        }
        match &responses.borrow()[0] {
            sema_core::runtime::RuntimeResponse::Settlements(settlements) => {
                assert_eq!(settlements.len(), 2);
                assert!(settlements.iter().all(|item| Rc::ptr_eq(item, &settlement)));
            }
            sema_core::runtime::RuntimeResponse::Settlement(Some(item)) => {
                assert!(Rc::ptr_eq(item, &settlement));
            }
            response => panic!("unexpected duplicate wait response: {response:?}"),
        };
    }
}

#[test]
fn runtime_promise_all_fail_fast_leaves_pending_sibling_observable() {
    for outcome in [
        TaskOutcome::Failed(SemaError::eval("failed")),
        TaskOutcome::Cancelled(CancelReason::Explicit),
    ] {
        let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
        let failed = runtime.create_pending_promise_for_test();
        let pending = runtime.create_pending_promise_for_test();
        let responses = Rc::new(RefCell::new(Vec::new()));
        let handle = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
                sema_core::runtime::RuntimeRequest::PromiseSetWait {
                    wait: sema_core::runtime::PromiseSetWait {
                        promises: vec![pending, failed],
                        mode: sema_core::runtime::PromiseSetMode::All,
                    },
                    continuation: Box::new(CaptureRuntimeContinuation(Rc::clone(&responses))),
                },
            ))))
            .unwrap();
        runtime.drive(&drive_budget(8)).unwrap();
        let terminal = runtime.settle_promise_for_test(failed, outcome);
        while matches!(handle.poll_result(), RootPoll::Pending) {
            runtime.drive(&drive_budget(1)).unwrap();
        }
        let sema_core::runtime::RuntimeResponse::Settlement(Some(observed)) =
            &responses.borrow()[0]
        else {
            panic!("all fail-fast returns terminal settlement");
        };
        assert!(Rc::ptr_eq(observed, &terminal));
        runtime.settle_promise_for_test(pending, TaskOutcome::Returned(Value::int(9)));
    }
}

#[test]
fn protocol_internal_wait_exhaustion_fails_through_continuation() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let promise = runtime.create_pending_promise_for_test();
    let resumes = Rc::new(Cell::new(0));
    runtime.force_completion_identity_exhaustion_for_test("wait");
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::PromiseSetWait {
                wait: sema_core::runtime::PromiseSetWait {
                    promises: vec![promise],
                    mode: sema_core::runtime::PromiseSetMode::Race,
                },
                continuation: Box::new(CaptureFailureContinuation(Rc::clone(&resumes))),
            },
        ))))
        .unwrap();
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(resumes.get(), 1);
    assert_eq!(runtime.protocol_wait_count_for_test(), 0);
}

#[test]
fn runtime_task_promise_publishes_the_tasks_canonical_settlement_once() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let (source, promise) = runtime
        .submit_test_root_with_promise(TestPreparedTask::returned(Value::int(7)))
        .unwrap();
    let responses = Rc::new(RefCell::new(Vec::new()));
    let observer = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::PromiseSetWait {
                wait: sema_core::runtime::PromiseSetWait {
                    promises: vec![promise],
                    mode: sema_core::runtime::PromiseSetMode::Race,
                },
                continuation: Box::new(CaptureRuntimeContinuation(Rc::clone(&responses))),
            },
        ))))
        .unwrap();
    while matches!(observer.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    let RootPoll::Ready(source_settlement) = source.poll_result() else {
        panic!("source settled");
    };
    let sema_core::runtime::RuntimeResponse::Settlement(Some(observed)) = &responses.borrow()[0]
    else {
        panic!("observer received settlement");
    };
    assert!(Rc::ptr_eq(&source_settlement, observed));
}

#[test]
fn runtime_blocked_channel_receive_preserves_false_rendezvous_value() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let channel = runtime.create_channel_for_test(0);
    let received = Rc::new(RefCell::new(Vec::new()));
    let receiver = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Receive { channel }),
                continuation: Box::new(CaptureRuntimeContinuation(Rc::clone(&received))),
            },
        ))))
        .unwrap();
    runtime.drive(&drive_budget(8)).unwrap();
    let sender = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Send {
                    channel,
                    value: Value::FALSE,
                }),
                continuation: Box::new(CaptureRuntimeContinuation(Rc::new(RefCell::new(
                    Vec::new(),
                )))),
            },
        ))))
        .unwrap();
    while matches!(receiver.poll_result(), RootPoll::Pending)
        || matches!(sender.poll_result(), RootPoll::Pending)
    {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert!(matches!(
        &received.borrow()[0],
        sema_core::runtime::RuntimeResponse::Receive(
            sema_core::runtime::ChannelReceive::Received(value)
        ) if *value == Value::FALSE
    ));
}

#[test]
fn channel_buffer_fifo_close_and_exact_waiter_cleanup() {
    let (runtime, issuers) = runtime_issuers();
    let (_, _, channel_ids) = issuers.into_parts();
    let mut channels = super::ChannelRegistry::new(runtime, channel_ids);
    let channel = channels.allocate(2).unwrap();
    let waits = WaitRuntime::new(Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    }))
    .unwrap();
    let keys: Vec<_> = (0..4)
        .map(|_| {
            let mut key = waits.issue_internal_wait().unwrap();
            key.runtime = runtime;
            key
        })
        .collect();
    assert_eq!(
        channels
            .send(
                channel,
                keys[0],
                TaskId::try_from_raw(1).unwrap(),
                Value::int(1)
            )
            .unwrap(),
        super::ChannelResult::Sent
    );
    assert_eq!(
        channels
            .send(
                channel,
                keys[1],
                TaskId::try_from_raw(2).unwrap(),
                Value::int(2)
            )
            .unwrap(),
        super::ChannelResult::Sent
    );
    assert_eq!(
        channels
            .receive(channel, keys[2], TaskId::try_from_raw(3).unwrap())
            .unwrap(),
        super::ChannelResult::Received(Value::int(1))
    );
    assert_eq!(
        channels
            .receive(channel, keys[3], TaskId::try_from_raw(4).unwrap())
            .unwrap(),
        super::ChannelResult::Received(Value::int(2))
    );
    assert!(channels.close(channel).unwrap().is_some());
    assert_eq!(
        channels
            .receive(channel, keys[3], TaskId::try_from_raw(4).unwrap())
            .unwrap(),
        super::ChannelResult::Closed
    );
}

#[test]
fn channel_close_detaches_long_fanout_and_emits_one_wake_at_a_time() {
    let (runtime, issuers) = runtime_issuers();
    let (_, _, channel_ids) = issuers.into_parts();
    let mut channels = super::ChannelRegistry::new(runtime, channel_ids);
    let channel = channels.allocate(0).unwrap();
    let waits = WaitRuntime::new(Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    }))
    .unwrap();
    for raw in 1..=257 {
        let mut key = waits.issue_internal_wait().unwrap();
        key.runtime = runtime;
        assert_eq!(
            channels
                .send(
                    channel,
                    key,
                    TaskId::try_from_raw(raw).unwrap(),
                    Value::int(raw as i64),
                )
                .unwrap(),
            super::ChannelResult::Waiting
        );
    }

    let mut close = channels.close(channel).unwrap().unwrap();
    for expected in 1..=257 {
        let wake = close.next_wake().expect("one pending wake");
        assert_eq!(wake.task.get(), expected);
        assert_eq!(wake.result, super::ChannelResult::Closed);
    }
    assert!(close.next_wake().is_none());
}

#[test]
fn unsupported_spawn_drops_callable_outside_runtime_state_borrow() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let polled = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .unwrap();
    let owner = PollHandleOnDrop(polled.clone());
    let callable = Value::native_fn(sema_core::NativeFn::simple("spawn-drop", move |_| {
        let _owner = &owner;
        Ok(Value::NIL)
    }));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::Spawn {
                callable,
                continuation: Box::new(RuntimeResponseContinuation(Arc::new(Mutex::new(
                    Vec::new(),
                )))),
            },
        ))))
        .unwrap();
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(1)).unwrap();
    }
}

fn runtime_with_inline_executor(clock: Rc<dyn RuntimeClock>) -> Runtime {
    Runtime::new(
        Rc::new(sema_core::EvalContext::new()),
        clock,
        Arc::new(FakeExecutor {
            mode: FakeSubmit::Inline,
            failure: None,
        }),
    )
    .expect("runtime")
}

thread_local! {
    static REENTRANT_HANDLE: RefCell<Option<super::RootHandle>> = const { RefCell::new(None) };
}

struct PollHandleOnDrop(super::RootHandle);

impl Drop for PollHandleOnDrop {
    fn drop(&mut self) {
        let _ = self.0.poll_result();
    }
}

fn poll_reentrant_handle() {
    REENTRANT_HANDLE.with(|slot| {
        if let Some(handle) = slot.borrow().as_ref() {
            assert!(matches!(handle.poll_result(), RootPoll::Pending));
        }
    });
}

fn poll_and_cancel_reentrant_handle() {
    let handle = REENTRANT_HANDLE.with(|slot| slot.borrow().clone().unwrap());
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    assert!(handle.cancel(CancelReason::Explicit));
}

#[test]
fn runtime_root_handles_poll_canonical_settlement_and_reap_after_final_drop() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock);
    let handle = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(42)))
        .expect("root admitted");
    let clone = handle.clone();

    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    let report = runtime.drive(&drive_budget(8)).expect("drive");
    assert!(matches!(report, super::DriveState::Progress { .. }));
    let RootPoll::Ready(first) = handle.poll_result() else {
        panic!("root should settle");
    };
    let RootPoll::Ready(second) = clone.poll_result() else {
        panic!("clone should observe settlement");
    };
    assert!(Rc::ptr_eq(&first, &second));
    assert_eq!(runtime.task_count(), 0, "settled main task may be reaped");
    assert_eq!(runtime.root_count(), 1, "live handles retain settled root");
    drop(handle);
    runtime.drive(&drive_budget(8)).expect("cleanup turn");
    assert_eq!(runtime.root_count(), 1);
    drop(clone);
    runtime.drive(&drive_budget(8)).expect("final cleanup turn");
    assert_eq!(runtime.root_count(), 0);
}

#[test]
fn runtime_cancel_is_sticky_and_settles_root_once() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    assert!(handle.cancel(CancelReason::Explicit));
    assert!(!handle.cancel(CancelReason::Timeout));
    runtime.drive(&drive_budget(8)).expect("drive cancellation");
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("cancelled root should settle");
    };
    assert!(matches!(
        settlement.outcome,
        TaskOutcome::Cancelled(CancelReason::Explicit)
    ));
}

#[test]
fn runtime_drop_turns_weak_root_handle_into_runtime_dropped() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    drop(runtime);
    assert!(matches!(handle.poll_result(), RootPoll::RuntimeDropped));
    assert!(!handle.cancel(CancelReason::Explicit));
}

#[test]
fn runtime_drive_charges_external_extract_decode_resume_and_apply_stages() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(Arc::clone(&events)),
        ))))
        .expect("root admitted");

    let one = drive_budget(1);
    for expected_work in 1..=9 {
        let state = runtime.drive(&one).expect("bounded stage");
        assert!(matches!(
            state,
            super::DriveState::Progress { work_items: 1, .. }
        ));
        if expected_work < 9 {
            assert!(matches!(handle.poll_result(), RootPoll::Pending));
        }
    }
    assert_eq!(*events.lock().unwrap(), vec!["decode", "returned"]);
    assert!(matches!(handle.poll_result(), RootPoll::Ready(_)));
}

struct ChainContinuation {
    remaining: usize,
}

impl Trace for ChainContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ChainContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let ResumeInput::Returned(value) = input else {
            return Err(SemaError::eval("chain call failed"));
        };
        if self.remaining == 0 {
            return Ok(NativeOutcome::Return(value));
        }
        Ok(chain_call(self.remaining - 1))
    }
}

fn chain_call(remaining: usize) -> NativeOutcome {
    NativeOutcome::Call(NativeCall {
        callable: Value::native_fn(sema_core::NativeFn::simple_result("chain-step", |_args| {
            Ok(NativeOutcome::Return(Value::int(42)))
        })),
        args: Vec::new(),
        continuation: Box::new(ChainContinuation { remaining }),
    })
}

#[test]
fn continuation_chain_is_traceable_and_bounded_by_drive_work_items() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(chain_call(12))))
        .expect("root admitted");
    let budget = drive_budget(1);
    let mut turns = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        let state = runtime.drive(&budget).expect("bounded continuation turn");
        assert!(matches!(
            state,
            super::DriveState::Progress { work_items: 1, .. }
        ));
        assert!(runtime.trace(&mut |_| {}));
        turns += 1;
        assert!(turns < 100, "continuation chain must converge");
    }
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("chain should settle");
    };
    assert!(
        matches!(settlement.outcome, TaskOutcome::Returned(ref value) if value.as_int() == Some(42))
    );
    assert!(
        turns > 12,
        "each call and continuation transition is charged"
    );
}

struct RecordingContinuation(Arc<Mutex<Vec<&'static str>>>);

impl Trace for RecordingContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for RecordingContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        self.0.lock().unwrap().push(match input {
            ResumeInput::Returned(_) => "resume-returned",
            ResumeInput::Failed(_) => "resume-failed",
            ResumeInput::Cancelled(_) => "resume-cancelled",
            ResumeInput::Runtime(_) => "resume-runtime",
        });
        Ok(NativeOutcome::Return(Value::int(9)))
    }
}

#[test]
fn continuation_call_apply_invoke_resume_and_final_apply_are_distinct_turns() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let native_events = Arc::clone(&events);
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
            NativeCall {
                callable: Value::native_fn(sema_core::NativeFn::simple_result(
                    "recording-native",
                    move |_| {
                        native_events.lock().unwrap().push("invoke");
                        Ok(NativeOutcome::Return(Value::int(8)))
                    },
                )),
                args: Vec::new(),
                continuation: Box::new(RecordingContinuation(Arc::clone(&events))),
            },
        ))))
        .unwrap();

    let one = drive_budget(1);
    let expected = [
        vec![],
        vec![],
        vec![],
        vec!["invoke"],
        vec!["invoke"],
        vec!["invoke", "resume-returned"],
        vec!["invoke", "resume-returned"],
    ];
    for events_after_turn in expected {
        let report = runtime.drive(&one).unwrap();
        assert!(matches!(
            report,
            super::DriveState::Progress { work_items: 1, .. }
        ));
        assert_eq!(*events.lock().unwrap(), events_after_turn);
    }
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&one).unwrap();
    }
    assert!(matches!(
        handle.poll_result(),
        RootPoll::Ready(settlement)
            if matches!(settlement.outcome, TaskOutcome::Returned(ref value) if value.as_int() == Some(9))
    ));
}

#[test]
fn invalid_callable_failure_is_delivered_to_its_continuation() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
            NativeCall {
                callable: Value::int(3),
                args: Vec::new(),
                continuation: Box::new(RecordingContinuation(Arc::clone(&events))),
            },
        ))))
        .unwrap();
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(*events.lock().unwrap(), vec!["resume-failed"]);
}

#[test]
fn cancellation_before_callable_invocation_is_delivered_to_its_continuation() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let native_events = Arc::clone(&events);
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
            NativeCall {
                callable: Value::native_fn(sema_core::NativeFn::simple_result(
                    "must-not-run",
                    move |_| {
                        native_events.lock().unwrap().push("invoked");
                        Ok(NativeOutcome::Return(Value::NIL))
                    },
                )),
                args: Vec::new(),
                continuation: Box::new(RecordingContinuation(Arc::clone(&events))),
            },
        ))))
        .unwrap();
    runtime.drive(&drive_budget(1)).unwrap();
    runtime.drive(&drive_budget(1)).unwrap();
    runtime.drive(&drive_budget(1)).unwrap();
    assert!(handle.cancel(CancelReason::Explicit));
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(*events.lock().unwrap(), vec!["resume-cancelled"]);
}

#[test]
fn native_call_into_sema_closure_and_back_to_native_uses_task_vm() {
    let forms = sema_reader::read_many("(lambda (f value) (f value))").unwrap();
    let program = crate::compile_program(&forms, None).unwrap();
    let context = sema_core::EvalContext::new();
    let mut vm = crate::VM::new(
        Rc::new(sema_core::Env::new()),
        program.functions,
        &program.native_table,
        program.main_cache_slots,
    )
    .unwrap();
    let callable = vm.execute(program.closure, &context).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let inner_events = Arc::clone(&events);
    let inner_native = Value::native_fn(sema_core::NativeFn::simple("inner", move |args| {
        inner_events.lock().unwrap().push("inner-native");
        Ok(args[0].clone())
    }));
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
            NativeCall {
                callable,
                args: vec![inner_native, Value::int(17)],
                continuation: Box::new(RecordingContinuation(Arc::clone(&events))),
            },
        ))))
        .unwrap();
    while matches!(handle.poll_result(), RootPoll::Pending) {
        let report = runtime.drive(&drive_budget(1)).unwrap();
        assert!(matches!(
            report,
            super::DriveState::Progress { work_items: 1, .. }
        ));
        assert!(
            runtime.trace(&mut |_| {}),
            "every VM-call stage is traceable"
        );
    }
    assert_eq!(
        *events.lock().unwrap(),
        vec!["inner-native", "resume-returned"]
    );
}

#[test]
fn extracted_runtime_closure_preserves_call_native_table() {
    let forms = sema_reader::read_many("(lambda () (identity 42))").unwrap();
    let globals = Rc::new(sema_core::Env::new());
    globals.set(
        sema_core::intern("identity"),
        Value::native_fn(sema_core::NativeFn::simple("identity", |args| {
            Ok(args[0].clone())
        })),
    );
    let known = [sema_core::intern("identity")].into_iter().collect();
    let program = crate::compile_program(&forms, Some(known)).unwrap();
    let context = sema_core::EvalContext::new();
    let mut vm = crate::VM::new(
        globals,
        program.functions,
        &program.native_table,
        program.main_cache_slots,
    )
    .unwrap();
    let callable = vm.execute(program.closure, &context).unwrap();
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
            NativeCall {
                callable,
                args: Vec::new(),
                continuation: Box::new(ChainContinuation { remaining: 0 }),
            },
        ))))
        .unwrap();

    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
    }
    assert!(matches!(
        handle.poll_result(),
        RootPoll::Ready(settlement)
            if matches!(settlement.outcome, TaskOutcome::Returned(ref value) if value.as_int() == Some(42))
    ));
}

#[test]
fn cancellation_after_vm_quantum_expiry_resumes_installed_continuation() {
    let forms =
        sema_reader::read_many("(lambda () (let loop ((n 1000)) (if (= n 0) n (loop (- n 1)))))")
            .unwrap();
    let program = crate::compile_program(&forms, None).unwrap();
    let context = sema_core::EvalContext::new();
    let mut vm = crate::VM::new(
        Rc::new(sema_core::Env::new()),
        program.functions,
        &program.native_table,
        program.main_cache_slots,
    )
    .unwrap();
    let callable = vm.execute(program.closure, &context).unwrap();
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
            NativeCall {
                callable,
                args: Vec::new(),
                continuation: Box::new(RecordingContinuation(Arc::clone(&events))),
            },
        ))))
        .unwrap();

    for _ in 0..8 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert!(handle.cancel(CancelReason::Explicit));
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
    }
    assert_eq!(*events.lock().unwrap(), vec!["resume-cancelled"]);
}

#[test]
fn runtime_forced_trace_keeps_values_in_root_settlement_and_every_pending_stage() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            edge_suspend(),
        ))))
        .unwrap();
    let edge_count = |runtime: &Runtime| {
        let mut edges = 0;
        assert!(runtime.trace(&mut |_| edges += 1));
        edges
    };

    assert_eq!(edge_count(&runtime), 2, "prepared suspend owners");
    runtime.drive(&drive_budget(1)).unwrap();
    assert_eq!(edge_count(&runtime), 2, "registered wait owners");
    runtime.drive(&drive_budget(1)).unwrap();
    assert_eq!(edge_count(&runtime), 2, "task pending resume owners");
    runtime.drive(&drive_budget(1)).unwrap();
    assert_eq!(edge_count(&runtime), 2, "decode stage owners");
    runtime.drive(&drive_budget(1)).unwrap();
    assert_eq!(edge_count(&runtime), 2, "continuation stage owners");
    runtime.drive(&drive_budget(1)).unwrap();
    assert_eq!(edge_count(&runtime), 2, "apply stage outcome");
    runtime.drive(&drive_budget(1)).unwrap();
    assert_eq!(edge_count(&runtime), 2, "settlement action outcome");
    runtime.drive(&drive_budget(1)).unwrap();
    assert_eq!(edge_count(&runtime), 2, "settlement action outcome");
    runtime.drive(&drive_budget(1)).unwrap();
    assert_eq!(edge_count(&runtime), 1, "root settlement outcome");
    runtime.drive(&drive_budget(1)).unwrap();
    assert!(matches!(handle.poll_result(), RootPoll::Ready(_)));
}

fn cyclic_cell() -> (Value, std::rc::Weak<sema_core::MutableCell>) {
    let value = Value::mutable_cell(Value::NIL);
    let cell = value.as_mutable_cell_rc().unwrap();
    *cell.value.borrow_mut() = value.clone();
    let weak = Rc::downgrade(&cell);
    drop(cell);
    (value, weak)
}

#[test]
fn runtime_settlement_keeps_cycle_through_forced_collection_until_reaped() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let (value, weak) = cyclic_cell();
    let handle = runtime
        .submit_test_root(TestPreparedTask::returned(value))
        .unwrap();
    runtime.drive(&drive_budget(8)).unwrap();

    sema_core::cycle::collect(&[], sema_core::cycle::GcTrigger::Explicit);
    assert!(weak.upgrade().is_some(), "settlement is a runtime GC root");

    drop(handle);
    runtime.drive(&drive_budget(8)).unwrap();
    sema_core::cycle::collect(&[], sema_core::cycle::GcTrigger::Explicit);
    assert!(
        weak.upgrade().is_none(),
        "reaping releases the runtime root"
    );
}

#[test]
fn runtime_pending_apply_keeps_cycle_through_forced_collection_until_settlement_release() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let (value, weak) = cyclic_cell();
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Return(value))))
        .unwrap();
    runtime.drive(&drive_budget(2)).unwrap();

    sema_core::cycle::collect(&[], sema_core::cycle::GcTrigger::Explicit);
    assert!(
        weak.upgrade().is_some(),
        "pending apply is a runtime GC root"
    );

    runtime.drive(&drive_budget(8)).unwrap();
    drop(handle);
    runtime.drive(&drive_budget(8)).unwrap();
    sema_core::cycle::collect(&[], sema_core::cycle::GcTrigger::Explicit);
    assert!(
        weak.upgrade().is_none(),
        "settlement release permits collection"
    );
}

#[test]
fn runtime_pending_action_keeps_cycle_through_forced_collection() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let (value, weak) = cyclic_cell();
    let _handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Return(value))))
        .unwrap();

    runtime.drive(&drive_budget(1)).unwrap();
    sema_core::cycle::collect(&[], sema_core::cycle::GcTrigger::Explicit);

    assert!(
        weak.upgrade().is_some(),
        "pending action is a runtime GC root"
    );
}

#[test]
fn runtime_native_quantum_may_poll_and_cancel_root_reentrantly() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native_call(|| {
            poll_and_cancel_reentrant_handle();
            Ok(NativeOutcome::Return(Value::int(7)))
        }))
        .unwrap();
    REENTRANT_HANDLE.with(|slot| *slot.borrow_mut() = Some(handle.clone()));

    runtime.drive(&drive_budget(2)).unwrap();
    assert!(!handle.cancel(CancelReason::Timeout));

    REENTRANT_HANDLE.with(|slot| slot.borrow_mut().take());
}

#[test]
fn nested_runtime_drive_fails_without_resetting_outer_instruction_accounting() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let nested = runtime.clone_for_test();
    let handle = runtime
        .submit_test_root(TestPreparedTask::native_call(move || {
            assert!(matches!(
                nested.drive(&drive_budget(1)),
                Err(super::RuntimeFault::Invariant { ref message })
                    if message.contains("already active")
            ));
            Ok(NativeOutcome::Return(Value::int(7)))
        }))
        .unwrap();

    let report = runtime.drive(&drive_budget(8)).unwrap();
    assert!(matches!(report, super::DriveState::Progress { .. }));
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
    }
}

#[test]
fn runtime_native_call_invocation_and_application_use_separate_credits() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let invoked = Rc::new(Cell::new(false));
    let observed = Rc::clone(&invoked);
    let handle = runtime
        .submit_test_root(TestPreparedTask::native_call(move || {
            observed.set(true);
            Ok(NativeOutcome::Return(Value::int(7)))
        }))
        .unwrap();

    runtime.drive(&drive_budget(1)).unwrap();
    assert!(!invoked.get());
    runtime.drive(&drive_budget(1)).unwrap();
    assert!(invoked.get());
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    runtime.drive(&drive_budget(1)).unwrap();
    assert!(matches!(handle.poll_result(), RootPoll::Ready(_)));
}

#[test]
fn runtime_drive_rotates_past_completion_backlog_to_visit_root() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let mut backlog_handles = Vec::new();
    for _ in 0..8 {
        backlog_handles.push(
            runtime
                .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
                    external_suspend(Arc::new(Mutex::new(Vec::new()))),
                ))))
                .unwrap(),
        );
    }
    let mut setup = drive_budget(32);
    setup.completion_limit = std::num::NonZeroUsize::new(1).unwrap();
    runtime.drive(&setup).unwrap();

    let root = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(11)))
        .unwrap();
    let mut one = drive_budget(1);
    one.completion_limit = std::num::NonZeroUsize::new(1).unwrap();
    for _ in 0..50 {
        runtime.drive(&one).unwrap();
        if matches!(root.poll_result(), RootPoll::Ready(_)) {
            return;
        }
    }
    panic!("completion backlog starved the ready root across drive turns");
}

#[test]
fn runtime_continuation_may_suspend_again_without_nested_state_borrow() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let first = NativeSuspend {
        wait: external_suspend(Arc::clone(&events)).wait,
        continuation: Box::new(SecondSuspendContinuation(Arc::clone(&events))),
    };
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(first))))
        .expect("root admitted");

    for _ in 0..16 {
        runtime.drive(&drive_budget(1)).expect("staged drive");
        if matches!(handle.poll_result(), RootPoll::Ready(_)) {
            break;
        }
    }
    assert!(matches!(handle.poll_result(), RootPoll::Ready(_)));
    assert_eq!(
        *events.lock().unwrap(),
        vec!["decode", "decode", "returned"]
    );
}

#[test]
fn runtime_submit_executor_may_poll_root_handle_reentrantly() {
    let runtime = Runtime::new(
        Rc::new(sema_core::EvalContext::new()),
        Rc::new(FakeClock::new()),
        Arc::new(FakeExecutor {
            mode: FakeSubmit::Reenter,
            failure: None,
        }),
    )
    .unwrap();
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(Arc::new(Mutex::new(Vec::new()))),
        ))))
        .unwrap();
    REENTRANT_HANDLE.with(|slot| *slot.borrow_mut() = Some(handle.clone()));
    runtime.drive(&drive_budget(1)).unwrap();
    REENTRANT_HANDLE.with(|slot| slot.borrow_mut().take());
}

#[test]
fn runtime_shutdown_rejects_admission_cancels_roots_and_reports_clean() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let report = runtime
        .shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(8),
        })
        .expect("bounded shutdown");
    assert!(report.clean, "{report:?}");
    assert_eq!(report.live_tasks, 0);
    assert!(matches!(handle.poll_result(), RootPoll::Ready(_)));
    assert!(matches!(
        runtime.submit_test_root(TestPreparedTask::returned(Value::NIL)),
        Err(super::SubmitRootError::ShuttingDown)
    ));
}

#[test]
fn runtime_shutdown_cancels_waiting_task_and_reaps_quarantine_completion() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(Arc::clone(&events)),
        ))))
        .expect("root admitted");
    runtime.drive(&drive_budget(2)).expect("register wait");

    let report = runtime
        .shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(8),
        })
        .expect("bounded shutdown");
    assert!(report.clean, "{report:?}");
    assert_eq!(*events.lock().unwrap(), vec!["cancelled"]);
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("waiting root settles during shutdown");
    };
    assert!(matches!(
        &settlement.outcome,
        TaskOutcome::Returned(value) if *value == Value::NIL
    ));
}

#[test]
fn runtime_cancel_and_reap_hooks_may_poll_root_handle_reentrantly() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let suspend = NativeSuspend {
        wait: WaitKind::External(Box::new(PreparedExternalOperation::interruptible_blocking(
            CompletionKind::try_from_raw(7).unwrap(),
            Box::new(CountingDecoder(Arc::clone(&events))),
            InterruptibleResource::new("reentrant", Box::new(ReentrantHook)),
            || Ok(Box::new(7_i32)),
        ))),
        continuation: Box::new(CountingContinuation(events)),
    };
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            suspend,
        ))))
        .unwrap();
    runtime.drive(&drive_budget(1)).unwrap();
    REENTRANT_HANDLE.with(|slot| *slot.borrow_mut() = Some(handle.clone()));
    handle.cancel(CancelReason::Explicit);
    let report = runtime
        .shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(8),
        })
        .unwrap();
    REENTRANT_HANDLE.with(|slot| slot.borrow_mut().take());
    assert!(report.clean);
}

#[test]
fn runtime_settlement_exhaustion_preserves_owner_until_terminal_shutdown() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let handle = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(9)))
        .expect("root admitted");
    runtime.force_settlement_exhaustion_for_test();

    assert_eq!(
        runtime.drive(&drive_budget(8)),
        Err(super::RuntimeFault::IdExhausted { kind: "settlement" })
    );
    assert_eq!(
        runtime.task_count(),
        1,
        "failed allocation keeps task owner"
    );
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    let result = runtime.shutdown(&super::ShutdownOptions {
        deadline: clock.now() + Duration::from_secs(1),
        drive_budget: drive_budget(8),
    });
    assert_eq!(
        result,
        Err(super::RuntimeFault::IdExhausted { kind: "settlement" })
    );
    assert_eq!(
        runtime.task_count(),
        0,
        "terminal cleanup cancels every task"
    );
    assert!(matches!(handle.poll_result(), RootPoll::Aborted(_)));
}

#[test]
fn runtime_root_and_task_exhaustion_reject_transactionally() {
    for kind in ["root", "task"] {
        let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
        runtime.force_admission_exhaustion_for_test(kind);
        assert!(matches!(
            runtime.submit_test_root(TestPreparedTask::returned(Value::NIL)),
            Err(super::SubmitRootError::IdExhausted)
        ));
        assert_eq!(runtime.root_count(), 0, "{kind} exhaustion leaked root");
        assert_eq!(runtime.task_count(), 0, "{kind} exhaustion leaked task");
    }
}

#[test]
fn runtime_wait_and_operation_exhaustion_are_separate_transactional_seams() {
    for kind in ["wait", "operation"] {
        let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
        runtime.force_completion_identity_exhaustion_for_test(kind);
        let events = Arc::new(Mutex::new(Vec::new()));
        let handle = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
                external_suspend(Arc::clone(&events)),
            ))))
            .unwrap();
        assert!(matches!(
            runtime.drive(&drive_budget(8)),
            Ok(super::DriveState::Progress { .. })
        ));
        assert_eq!(runtime.active_wait_count_for_test(), 0, "{kind}");
        let RootPoll::Ready(settlement) = handle.poll_result() else {
            panic!("identity exhaustion settles the root")
        };
        assert!(matches!(settlement.outcome, TaskOutcome::Returned(_)));
        assert_eq!(*events.lock().unwrap(), vec!["failed"]);
    }
}

#[test]
fn runtime_rejects_forged_completion_identity_before_decode() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(events.clone()),
        ))))
        .unwrap();
    let mut one = drive_budget(1);
    one.root_visit_limit = std::num::NonZeroUsize::new(1).unwrap();
    one.completion_limit = std::num::NonZeroUsize::new(1).unwrap();
    for _ in 0..3 {
        runtime.drive(&one).unwrap();
        if runtime.active_wait_count_for_test() == 1 {
            break;
        }
    }
    assert_eq!(runtime.active_wait_count_for_test(), 1);

    let (wrong_runtime, _) = runtime_issuers();
    let wrong_operation = IdCounter::<sema_core::runtime::OperationId>::new()
        .allocate()
        .unwrap();
    let wrong_kind = CompletionKind::try_from_raw(99).unwrap();
    let wrong_generation = IdCounter::<WaitGeneration>::new().allocate().unwrap();
    let mutations = [
        ForgedCompletionMutation::Runtime(wrong_runtime),
        ForgedCompletionMutation::Operation(wrong_operation),
        ForgedCompletionMutation::Kind(wrong_kind),
        ForgedCompletionMutation::Generation(wrong_generation),
    ];
    for mutation in mutations.into_iter().rev() {
        runtime.forge_completion_for_test(mutation, Ok(Box::new(())));
    }
    for _ in 0..4 {
        runtime.drive(&one).unwrap();
        assert!(events.lock().unwrap().is_empty());
        assert!(matches!(handle.poll_result(), RootPoll::Pending));
    }
    for _ in 0..32 {
        runtime.drive(&drive_budget(4)).unwrap();
        if matches!(handle.poll_result(), RootPoll::Ready(_)) {
            break;
        }
    }
    assert_eq!(&*events.lock().unwrap(), &["decode", "returned"]);
}

#[test]
fn runtime_decode_failure_settles_once_and_duplicate_completion_is_late() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            decode_failure_suspend(events.clone()),
        ))))
        .unwrap();
    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    runtime.forge_completion_for_test(
        ForgedCompletionMutation::None,
        Ok(Box::new("wrong payload type")),
    );
    runtime.forge_completion_for_test(ForgedCompletionMutation::None, Ok(Box::new(())));
    for _ in 0..32 {
        runtime.drive(&drive_budget(4)).unwrap();
        if matches!(handle.poll_result(), RootPoll::Ready(_)) {
            break;
        }
    }

    let RootPoll::Ready(first) = handle.poll_result() else {
        panic!("decode failure settles root")
    };
    let RootPoll::Ready(second) = handle.poll_result() else {
        panic!("settlement remains pollable")
    };
    assert!(Rc::ptr_eq(&first, &second));
    assert_eq!(runtime.late_completion_count_for_test(), 2);
}

#[test]
fn runtime_generation_reuse_rejects_stale_generation_without_consuming_live_wait() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(events.clone()),
        ))))
        .unwrap();
    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    let live_generation = runtime.active_wait_key_for_test().generation;
    let mut generations = IdCounter::<WaitGeneration>::new();
    let stale_generation = loop {
        let candidate = generations.allocate().unwrap();
        if candidate != live_generation {
            break candidate;
        }
    };
    runtime.forge_completion_for_test(
        ForgedCompletionMutation::Generation(stale_generation),
        Ok(Box::new(())),
    );

    runtime.drive(&drive_budget(1)).unwrap();
    assert_eq!(runtime.active_wait_count_for_test(), 1);
    assert_eq!(runtime.late_completion_count_for_test(), 1);
    assert!(events.lock().unwrap().is_empty());
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
}

#[test]
fn runtime_quarantine_completion_first_and_cancel_first_have_one_winner() {
    let completion_first = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let completed = completion_first
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(Arc::new(Mutex::new(Vec::new()))),
        ))))
        .unwrap();
    while completion_first.active_wait_count_for_test() == 0 {
        completion_first.drive(&drive_budget(1)).unwrap();
    }
    while completion_first.active_wait_count_for_test() != 0 {
        completion_first.drive(&drive_budget(1)).unwrap();
    }
    while matches!(completed.poll_result(), RootPoll::Pending) {
        completion_first.drive(&drive_budget(1)).unwrap();
    }
    assert!(!completed.cancel(CancelReason::Explicit));
    assert_eq!(completion_first.cleanup_count_for_test(), 0);

    let cancel_first = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let cancelled = cancel_first
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(Arc::new(Mutex::new(Vec::new()))),
        ))))
        .unwrap();
    while cancel_first.active_wait_count_for_test() == 0 {
        cancel_first.drive(&drive_budget(1)).unwrap();
    }
    assert!(cancelled.cancel(CancelReason::Explicit));
    cancel_first.set_drive_cursor_for_test(2);
    while cancel_first.cleanup_count_for_test() == 0 {
        cancel_first.drive(&drive_budget(1)).unwrap();
    }
    while cancel_first.cleanup_count_for_test() != 0 {
        cancel_first.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(cancel_first.quarantine_reaped_count_for_test(), 1);
    assert_eq!(cancel_first.late_completion_count_for_test(), 0);
}

#[test]
fn runtime_wrong_kind_cleanup_completion_leaves_quarantine_live() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(Arc::new(Mutex::new(Vec::new()))),
        ))))
        .unwrap();
    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    runtime.forge_completion_for_test(
        ForgedCompletionMutation::Kind(CompletionKind::try_from_raw(99).unwrap()),
        Ok(Box::new(())),
    );
    assert!(handle.cancel(CancelReason::Explicit));
    runtime.set_drive_cursor_for_test(2);
    while runtime.cleanup_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    while runtime.late_completion_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(runtime.cleanup_count_for_test(), 1);
    assert_eq!(runtime.quarantine_reaped_count_for_test(), 0);
}

#[test]
fn runtime_duplicate_after_quarantine_reap_is_late() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(Arc::new(Mutex::new(Vec::new()))),
        ))))
        .unwrap();
    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    runtime.forge_completion_for_test(ForgedCompletionMutation::None, Ok(Box::new(())));
    runtime.forge_completion_for_test(ForgedCompletionMutation::None, Ok(Box::new(())));
    assert!(handle.cancel(CancelReason::Explicit));
    runtime.set_drive_cursor_for_test(2);
    for _ in 0..24 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(runtime.cleanup_count_for_test(), 0);
    assert_eq!(runtime.quarantine_reaped_count_for_test(), 1);
    assert_eq!(runtime.late_completion_count_for_test(), 2);
}

#[test]
fn runtime_expired_quarantine_bound_reports_invariant_and_retains_non_clean_owner() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            edge_suspend(),
        ))))
        .unwrap();
    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert!(handle.cancel(CancelReason::Explicit));
    runtime.set_drive_cursor_for_test(2);
    while runtime.cleanup_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    clock.advance(Duration::from_secs(2));

    let error = runtime.drive(&drive_budget(8)).unwrap_err();
    assert!(matches!(
        &error,
        super::RuntimeFault::Invariant { message } if message.contains("quarantine bound expired")
    ));
    assert_eq!(runtime.cleanup_count_for_test(), 1);
    assert!(runtime.cleanup_diagnostics_for_test()[0].bound_expired);
    assert!(runtime
        .shutdown(&super::ShutdownOptions {
            deadline: clock.now(),
            drive_budget: drive_budget(8),
        })
        .is_err());
    assert_eq!(runtime.cleanup_count_for_test(), 1);
}

#[test]
fn runtime_two_handles_repeat_every_terminal_outcome_and_reap_in_both_drop_orders() {
    for index in 0..3 {
        for drop_original_first in [false, true] {
            let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
            let first = runtime.submit_test_root(prepared_for_case(index)).unwrap();
            let second = first.clone();
            if index == 2 {
                assert!(first.cancel(CancelReason::Explicit));
            }
            runtime.drive(&drive_budget(16)).unwrap();
            for _ in 0..3 {
                let RootPoll::Ready(a) = first.poll_result() else {
                    panic!("first terminal poll")
                };
                let RootPoll::Ready(b) = second.poll_result() else {
                    panic!("second terminal poll")
                };
                assert!(Rc::ptr_eq(&a, &b));
            }
            if drop_original_first {
                drop(first);
                runtime.drive(&drive_budget(2)).unwrap();
                assert_eq!(runtime.root_count(), 1);
                drop(second);
            } else {
                drop(second);
                runtime.drive(&drive_budget(2)).unwrap();
                assert_eq!(runtime.root_count(), 1);
                drop(first);
            }
            runtime.drive(&drive_budget(2)).unwrap();
            assert_eq!(runtime.root_count(), 0);
        }
    }
}

fn prepared_for_case(index: usize) -> TestPreparedTask {
    match index {
        0 => TestPreparedTask::returned(Value::int(1)),
        1 => TestPreparedTask::native(Err(SemaError::eval("failed"))),
        _ => TestPreparedTask::yield_forever(),
    }
}

#[test]
fn runtime_settled_root_waits_for_retained_descendant_then_reaps() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::returned(Value::NIL))
        .unwrap();
    let root = handle.id();
    runtime.retain_descendant_for_test(root);
    runtime.drive(&drive_budget(8)).unwrap();
    drop(handle);
    runtime.drive(&drive_budget(8)).unwrap();
    assert_eq!(runtime.root_count(), 1);
    runtime.release_descendant_for_test(root);
    runtime.drive(&drive_budget(8)).unwrap();
    assert_eq!(runtime.root_count(), 0);
}

#[test]
fn runtime_persistent_reap_diagnostic_survives_and_shutdown_is_not_clean() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            interruptible_suspend_with_hook(
                Arc::new(Mutex::new(Vec::new())),
                RecordingHook {
                    result: CancelResult::Error,
                    calls: calls.clone(),
                    edge: None,
                    trace_ok: true,
                },
            ),
        ))))
        .unwrap();
    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    runtime.forge_completion_for_test(
        ForgedCompletionMutation::Kind(CompletionKind::try_from_raw(99).unwrap()),
        Ok(Box::new(())),
    );
    assert!(handle.cancel(CancelReason::Explicit));
    runtime.drive(&drive_budget(8)).unwrap();
    clock.advance(Duration::from_secs(2));
    let report = runtime
        .shutdown(&super::ShutdownOptions {
            deadline: clock.now(),
            drive_budget: drive_budget(8),
        })
        .unwrap();
    assert!(!report.clean);
    assert_eq!(report.retained_cleanup, 1);
    assert_eq!(report.cleanup_diagnostics.len(), 1);
    let diagnostic = &report.cleanup_diagnostics[0];
    assert!(diagnostic.reap_attempts > 0);
    assert_eq!(diagnostic.operation.get(), 1);
    assert_eq!(diagnostic.resource, "recording");
    assert_eq!(
        diagnostic.suppressed_hook_error.as_deref(),
        Some("resource cancellation hook failed: cancel failed")
    );
    assert_eq!(report.invariant_failures.len(), 1);
    assert_eq!(report.invariant_failures[0].name, "retained-cleanup");
    assert_eq!(report.invariant_failures[0].diagnostic, *diagnostic);
    assert!(calls.lock().unwrap().starts_with(&["cancel", "reap"]));
}

#[test]
fn runtime_timer_sequence_exhaustion_leaves_task_running_and_no_timer() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    runtime
        .submit_test_root(TestPreparedTask::timer_returned(
            clock.now() + Duration::from_secs(1),
            Value::NIL,
        ))
        .unwrap();
    runtime.force_timer_failure_for_test("sequence");

    assert_eq!(
        runtime.drive(&drive_budget(8)),
        Err(super::RuntimeFault::IdExhausted { kind: "timer" })
    );
    assert_eq!(runtime.only_task_state_for_test(), StateName::Running);
    assert_eq!(runtime.timer_count_for_test(), 0);
}

#[test]
fn runtime_duplicate_timer_key_leaves_task_running_and_no_timer() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    runtime
        .submit_test_root(TestPreparedTask::timer_returned(
            clock.now() + Duration::from_secs(1),
            Value::NIL,
        ))
        .unwrap();
    runtime.force_timer_failure_for_test("duplicate");

    assert_eq!(
        runtime.drive(&drive_budget(8)),
        Err(super::RuntimeFault::IdExhausted { kind: "timer" })
    );
    assert_eq!(runtime.only_task_state_for_test(), StateName::Running);
    assert_eq!(runtime.timer_count_for_test(), 0);
}

#[test]
fn runtime_shutdown_finalizes_fault_first_raised_during_shutdown() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let handle = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(9)))
        .unwrap();
    runtime.force_settlement_exhaustion_for_test();

    assert_eq!(
        runtime.shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(8),
        }),
        Err(super::RuntimeFault::IdExhausted { kind: "settlement" })
    );
    assert_eq!(runtime.task_count(), 0);
    assert!(matches!(handle.poll_result(), RootPoll::Aborted(_)));
    drop(handle);
    runtime.drive(&drive_budget(8)).unwrap_err();
    assert_eq!(runtime.root_count(), 0);
}

#[test]
fn runtime_terminal_abort_drops_pending_owners_outside_state_borrow() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let owner_handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .unwrap();
    let owner = PollHandleOnDrop(owner_handle.clone());
    let _pending_handle = runtime
        .submit_test_root(TestPreparedTask::native_call(move || {
            let _owner = owner;
            Ok(NativeOutcome::Return(Value::NIL))
        }))
        .unwrap();
    runtime.drive(&drive_budget(2)).unwrap();

    runtime.abort_terminal_for_test();
}

#[test]
fn runtime_terminal_abort_preserves_settled_roots_and_reaps_unhandled_aborts() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let settled = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(3)))
        .unwrap();
    runtime.drive(&drive_budget(8)).unwrap();
    let abandoned = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(4)))
        .unwrap();
    drop(abandoned);
    runtime.force_settlement_exhaustion_for_test();

    assert!(runtime
        .shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(8),
        })
        .is_err());
    assert!(matches!(settled.poll_result(), RootPoll::Ready(_)));
    runtime.drive(&drive_budget(8)).unwrap_err();
    assert_eq!(
        runtime.root_count(),
        1,
        "aborted root without handles is reaped"
    );
}

#[test]
fn runtime_drive_reserves_every_eligible_root_visit_credit() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let mut backlog_handles = Vec::new();
    for _ in 0..8 {
        backlog_handles.push(
            runtime
                .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
                    external_suspend(Arc::new(Mutex::new(Vec::new()))),
                ))))
                .unwrap(),
        );
    }
    let cleanup = runtime
        .submit_test_root(TestPreparedTask::returned(Value::NIL))
        .unwrap();
    let mut setup = drive_budget(32);
    setup.completion_limit = std::num::NonZeroUsize::new(1).unwrap();
    setup.root_visit_limit = std::num::NonZeroUsize::new(9).unwrap();
    runtime.drive(&setup).unwrap();
    assert!(matches!(cleanup.poll_result(), RootPoll::Ready(_)));
    drop(cleanup);

    let first = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(1)))
        .unwrap();
    let second = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(2)))
        .unwrap();
    let mut budget = drive_budget(5);
    budget.root_visit_limit = std::num::NonZeroUsize::new(2).unwrap();
    runtime.set_drive_cursor_for_test(0);
    let visits_before = runtime.ready_visit_count_for_test();

    assert!(matches!(
        runtime.drive(&budget).unwrap(),
        super::DriveState::Progress { work_items: 5, .. }
    ));
    assert_eq!(
        runtime.ready_visit_count_for_test() - visits_before,
        2,
        "the old single-reservation algorithm visits only one ready root"
    );
    assert!(matches!(first.poll_result(), RootPoll::Pending));
    assert!(matches!(second.poll_result(), RootPoll::Pending));
    assert!(
        backlog_handles
            .iter()
            .any(|handle| matches!(handle.poll_result(), RootPoll::Pending)),
        "completion backlog must remain"
    );
}

#[test]
fn runtime_shutdown_reports_executor_result() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let report = runtime
        .shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(8),
        })
        .unwrap();
    assert!(matches!(
        report.executor,
        Some(ExecutorShutdown::Drained(_))
    ));
}

#[derive(Clone, Copy)]
enum FakeSubmit {
    Inline,
    Reject,
    Reenter,
}

struct FakeLease {
    mode: FakeSubmit,
}
impl ExecutorLease for FakeLease {
    fn submit(
        &self,
        submission: sema_core::runtime::ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        let operation = submission.operation_id();
        if matches!(self.mode, FakeSubmit::Reject) {
            return Err(submission.reject(SubmitErrorKind::Capacity));
        }
        if matches!(self.mode, FakeSubmit::Reenter) {
            poll_reentrant_handle();
        }
        match submission.into_dispatch() {
            ExecutorDispatch::Blocking(dispatch) => {
                dispatch.run();
            }
            ExecutorDispatch::Async(_) => panic!("test operation is blocking"),
        }
        Ok(RunningSubmission::new(operation))
    }
    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }
    fn shutdown(&self, _deadline: Instant) -> ExecutorShutdown {
        ExecutorShutdown::Drained(self.snapshot())
    }
}

struct FakeExecutor {
    mode: FakeSubmit,
    failure: Option<ExecutorAttachError>,
}
impl IoExecutor for FakeExecutor {
    fn attach_runtime(
        &self,
        _runtime_id: RuntimeId,
    ) -> Result<Arc<dyn ExecutorLease>, ExecutorAttachError> {
        self.failure.map_or_else(
            || Ok(Arc::new(FakeLease { mode: self.mode }) as Arc<dyn ExecutorLease>),
            Err,
        )
    }
    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }
}

struct CountingDecoder(Arc<Mutex<Vec<&'static str>>>);
impl Trace for CountingDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

struct DecodeFailureDecoder(Arc<Mutex<Vec<&'static str>>>);
impl Trace for DecodeFailureDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}
impl CompletionDecoder for DecodeFailureDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        _result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> Result<Value, SemaError> {
        self.0.lock().unwrap().push("decode");
        Err(SemaError::eval("decode failed"))
    }
}
impl CompletionDecoder for CountingDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> Result<Value, SemaError> {
        self.0.lock().unwrap().push("decode");
        result
            .map(|_| Value::int(7))
            .map_err(|failure| SemaError::eval(failure.message()))
    }
}
struct CountingContinuation(Arc<Mutex<Vec<&'static str>>>);
impl Trace for CountingContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

struct SecondSuspendContinuation(Arc<Mutex<Vec<&'static str>>>);
impl Trace for SecondSuspendContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}
impl NativeContinuation for SecondSuspendContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        assert!(matches!(input, ResumeInput::Returned(_)));
        Ok(NativeOutcome::Suspend(external_suspend(self.0)))
    }
}

struct EdgeLocal(Value);
impl Trace for EdgeLocal {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Value(&self.0));
        true
    }
}
impl sema_core::runtime::TaskLocalValue for EdgeLocal {
    fn inherit(&self) -> Rc<dyn sema_core::runtime::TaskLocalValue> {
        Rc::new(Self(self.0.clone()))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct EdgeDecoder(Value);
impl Trace for EdgeDecoder {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Value(&self.0));
        true
    }
}
impl CompletionDecoder for EdgeDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        _result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> Result<Value, SemaError> {
        Ok(self.0)
    }
}

struct EdgeContinuation(Value);
impl Trace for EdgeContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Value(&self.0));
        true
    }
}
impl NativeContinuation for EdgeContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        _input: ResumeInput,
    ) -> NativeResult {
        Ok(NativeOutcome::Return(self.0))
    }
}

fn edge_suspend() -> NativeSuspend {
    NativeSuspend {
        wait: WaitKind::External(Box::new(PreparedExternalOperation::quarantined_blocking(
            CompletionKind::try_from_raw(3).unwrap(),
            Box::new(EdgeDecoder(Value::string("decoder"))),
            sema_core::runtime::QuarantineBound::hard_deadline(Duration::from_secs(1)).unwrap(),
            || Ok(Box::new(())),
        ))),
        continuation: Box::new(EdgeContinuation(Value::string("continuation"))),
    }
}
impl NativeContinuation for CountingContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        self.0.lock().unwrap().push(match input {
            ResumeInput::Returned(_) => "returned",
            ResumeInput::Failed(_) => "failed",
            ResumeInput::Cancelled(_) => "cancelled",
            ResumeInput::Runtime(_) => "runtime",
        });
        Ok(NativeOutcome::Return(Value::NIL))
    }
}

fn external_suspend(events: Arc<Mutex<Vec<&'static str>>>) -> NativeSuspend {
    let kind = CompletionKind::try_from_raw(1).unwrap();
    NativeSuspend {
        wait: WaitKind::External(Box::new(PreparedExternalOperation::quarantined_blocking(
            kind,
            Box::new(CountingDecoder(Arc::clone(&events))),
            sema_core::runtime::QuarantineBound::hard_deadline(Duration::from_secs(1)).unwrap(),
            || Ok(Box::new(7_i32)),
        ))),
        continuation: Box::new(CountingContinuation(events)),
    }
}

fn decode_failure_suspend(events: Arc<Mutex<Vec<&'static str>>>) -> NativeSuspend {
    NativeSuspend {
        wait: WaitKind::External(Box::new(PreparedExternalOperation::quarantined_blocking(
            CompletionKind::try_from_raw(1).unwrap(),
            Box::new(DecodeFailureDecoder(Arc::clone(&events))),
            sema_core::runtime::QuarantineBound::hard_deadline(Duration::from_secs(1)).unwrap(),
            || Ok(Box::new(())),
        ))),
        continuation: Box::new(CountingContinuation(events)),
    }
}

struct PendingHook;
impl Trace for PendingHook {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

struct ReentrantHook;
impl Trace for ReentrantHook {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}
impl sema_core::runtime::CancelHook for ReentrantHook {
    fn cancel(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        poll_reentrant_handle();
        Ok(sema_core::runtime::CancelDisposition::PendingReap)
    }

    fn reap(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        poll_reentrant_handle();
        Ok(sema_core::runtime::CancelDisposition::Reaped)
    }
}

#[derive(Clone, Copy)]
enum CancelResult {
    Reaped,
    PendingReap,
    Error,
}

struct RecordingHook {
    result: CancelResult,
    calls: Arc<Mutex<Vec<&'static str>>>,
    edge: Option<Value>,
    trace_ok: bool,
}

impl Trace for RecordingHook {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        if let Some(value) = &self.edge {
            sink(sema_core::cycle::GcEdge::Value(value));
        }
        self.trace_ok
    }
}

impl sema_core::runtime::CancelHook for RecordingHook {
    fn cancel(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        self.calls.lock().unwrap().push("cancel");
        match self.result {
            CancelResult::Reaped => Ok(sema_core::runtime::CancelDisposition::Reaped),
            CancelResult::PendingReap => Ok(sema_core::runtime::CancelDisposition::PendingReap),
            CancelResult::Error => Err(sema_core::runtime::CancelHookError::new("cancel failed")),
        }
    }

    fn reap(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        self.calls.lock().unwrap().push("reap");
        Ok(sema_core::runtime::CancelDisposition::PendingReap)
    }
}

fn interruptible_suspend_with_hook(
    events: Arc<Mutex<Vec<&'static str>>>,
    hook: RecordingHook,
) -> NativeSuspend {
    NativeSuspend {
        wait: WaitKind::External(Box::new(PreparedExternalOperation::interruptible_blocking(
            CompletionKind::try_from_raw(2).unwrap(),
            Box::new(CountingDecoder(Arc::clone(&events))),
            InterruptibleResource::new("recording", Box::new(hook)),
            || Ok(Box::new(7_i32)),
        ))),
        continuation: Box::new(CountingContinuation(events)),
    }
}
impl sema_core::runtime::CancelHook for PendingHook {
    fn cancel(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        Ok(sema_core::runtime::CancelDisposition::PendingReap)
    }
    fn reap(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        Ok(sema_core::runtime::CancelDisposition::PendingReap)
    }
}

fn interruptible_suspend(events: Arc<Mutex<Vec<&'static str>>>) -> NativeSuspend {
    let kind = CompletionKind::try_from_raw(2).unwrap();
    NativeSuspend {
        wait: WaitKind::External(Box::new(PreparedExternalOperation::interruptible_blocking(
            kind,
            Box::new(CountingDecoder(Arc::clone(&events))),
            InterruptibleResource::new("pending", Box::new(PendingHook)),
            || Ok(Box::new(7_i32)),
        ))),
        continuation: Box::new(CountingContinuation(events)),
    }
}

#[test]
fn wait_inline_completion_observes_registered_state_then_consumes_owners_once() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    });
    let mut runtime = WaitRuntime::new(executor).unwrap();
    let mut ids = Ids::new();
    let mut task = task(&mut ids);
    task.start().unwrap();
    assert!(runtime
        .register_external(
            &mut task,
            external_suspend(Arc::clone(&events)),
            TaskContextHandle::default(),
        )
        .is_ok());
    assert_eq!(task.state_name(), StateName::Waiting);
    assert_eq!(runtime.active_len(), 1);

    let Some((CompletionRoute::Active, Some(pending))) = runtime.drain_one(&mut task) else {
        panic!("completion must extract a pending resume");
    };
    assert_eq!(pending.task_id(), task.id());
    assert_eq!(
        pending.wait_key(),
        task.wait_key().unwrap_or_else(|| pending.wait_key())
    );
    assert_eq!(task.state_name(), StateName::Ready);
    let pending = pending.invoke_decoder();
    assert_eq!(&*events.lock().unwrap(), &["decode"]);
    assert!(matches!(
        pending.invoke_continuation(),
        Ok(NativeOutcome::Return(_))
    ));
    assert_eq!(&*events.lock().unwrap(), &["decode", "returned"]);
    assert_eq!(runtime.active_len(), 0);
    assert!(runtime.drain_one(&mut task).is_none());
}

#[test]
fn wait_submit_rejection_traverses_decoder_then_continuation() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Reject,
        failure: None,
    });
    let mut runtime = WaitRuntime::new(executor).unwrap();
    let mut ids = Ids::new();
    let mut task = task(&mut ids);
    task.start().unwrap();
    let result = runtime.register_external(
        &mut task,
        external_suspend(Arc::clone(&events)),
        TaskContextHandle::default(),
    );
    let Err(RegisterExternalError::Rejected(pending)) = result else {
        panic!("submission rejection expected")
    };
    assert_eq!(task.state_name(), StateName::Running);
    assert_eq!(runtime.cleanup_len(), 0);
    let pending = pending.invoke_decoder();
    assert_eq!(&*events.lock().unwrap(), &["decode"]);
    assert!(matches!(
        pending.invoke_continuation(),
        Ok(NativeOutcome::Return(_))
    ));
    assert_eq!(&*events.lock().unwrap(), &["decode", "failed"]);
    assert_eq!(runtime.active_len(), 0);
}

#[test]
fn wait_submit_rejection_cancels_interruptible_resource_before_resume() {
    for (result, expected_cleanup) in [
        (CancelResult::Reaped, 0),
        (CancelResult::PendingReap, 1),
        (CancelResult::Error, 1),
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let executor = Arc::new(FakeExecutor {
            mode: FakeSubmit::Reject,
            failure: None,
        });
        let mut runtime = WaitRuntime::new(executor).unwrap();
        let mut ids = Ids::new();
        let mut task = task(&mut ids);
        task.start().unwrap();
        let result = runtime.register_external(
            &mut task,
            interruptible_suspend_with_hook(
                Arc::clone(&events),
                RecordingHook {
                    result,
                    calls: Arc::clone(&calls),
                    edge: None,
                    trace_ok: true,
                },
            ),
            TaskContextHandle::default(),
        );
        let Err(RegisterExternalError::Rejected(pending)) = result else {
            panic!("submission rejection expected")
        };

        assert_eq!(&*calls.lock().unwrap(), &["cancel"]);
        assert_eq!(runtime.cleanup_len(), expected_cleanup);
        pending.invoke_decoder().invoke_continuation().unwrap();
    }
}

#[test]
fn wait_submit_rejection_drops_unadmitted_quarantine() {
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Reject,
        failure: None,
    });
    let mut runtime = WaitRuntime::new(executor).unwrap();
    let mut ids = Ids::new();
    let mut task = task(&mut ids);
    task.start().unwrap();
    let result = runtime.register_external(
        &mut task,
        external_suspend(Arc::new(Mutex::new(Vec::new()))),
        TaskContextHandle::default(),
    );
    assert!(matches!(result, Err(RegisterExternalError::Rejected(_))));
    assert_eq!(runtime.cleanup_len(), 0);
}

#[test]
fn wait_trace_includes_cleanup_resource_edges_and_short_circuits() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    });
    let mut runtime = WaitRuntime::new(executor).unwrap();
    let mut ids = Ids::new();
    let mut task = task(&mut ids);
    task.start().unwrap();
    let key = runtime
        .register_external(
            &mut task,
            interruptible_suspend_with_hook(
                Arc::new(Mutex::new(Vec::new())),
                RecordingHook {
                    result: CancelResult::PendingReap,
                    calls,
                    edge: Some(Value::string("cleanup edge")),
                    trace_ok: true,
                },
            ),
            TaskContextHandle::default(),
        )
        .unwrap();
    task.request_cancellation(CancelReason::Explicit);
    runtime.cancel(&mut task, key, Instant::now()).unwrap();

    let mut edges = 0;
    assert!(runtime.trace(&mut |_| edges += 1));
    assert_eq!(edges, 1);
}

#[test]
fn wait_trace_propagates_cleanup_resource_trace_failure() {
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    });
    let mut runtime = WaitRuntime::new(executor).unwrap();
    let mut ids = Ids::new();
    let mut task = task(&mut ids);
    task.start().unwrap();
    let key = runtime
        .register_external(
            &mut task,
            interruptible_suspend_with_hook(
                Arc::new(Mutex::new(Vec::new())),
                RecordingHook {
                    result: CancelResult::PendingReap,
                    calls: Arc::new(Mutex::new(Vec::new())),
                    edge: Some(Value::string("cleanup edge")),
                    trace_ok: false,
                },
            ),
            TaskContextHandle::default(),
        )
        .unwrap();
    task.request_cancellation(CancelReason::Explicit);
    runtime.cancel(&mut task, key, Instant::now()).unwrap();

    let mut edges = 0;
    assert!(!runtime.trace(&mut |_| edges += 1));
    assert_eq!(edges, 1);
}

#[test]
fn wait_completion_for_wrong_task_preserves_active_wait() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    });
    let mut runtime = WaitRuntime::new(executor).unwrap();
    let mut ids = Ids::new();
    let mut owner = task(&mut ids);
    let mut stranger = task(&mut ids);
    owner.start().unwrap();
    stranger.start().unwrap();
    runtime
        .register_external(
            &mut owner,
            external_suspend(events),
            TaskContextHandle::default(),
        )
        .unwrap();

    assert!(runtime.drain_one(&mut stranger).is_none());
    assert_eq!(runtime.active_len(), 1);
    assert_eq!(owner.state_name(), StateName::Waiting);
    assert!(matches!(
        runtime.drain_one(&mut owner),
        Some((CompletionRoute::Active, Some(_)))
    ));
    assert_eq!(owner.state_name(), StateName::Ready);
}

#[test]
fn wait_trace_reports_exact_owned_edges_and_fails_on_borrowed_context() {
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    });
    let mut runtime = WaitRuntime::new(executor).unwrap();
    let mut ids = Ids::new();
    let mut task = task(&mut ids);
    task.start().unwrap();
    let context = TaskContextHandle::default();
    context
        .borrow_mut()
        .insert(Rc::new(EdgeLocal(Value::string("context"))));
    runtime
        .register_external(&mut task, edge_suspend(), context.clone())
        .unwrap();
    let mut edges = 0;
    assert!(runtime.trace(&mut |_| edges += 1));
    assert_eq!(edges, 3);

    let Some((CompletionRoute::Active, Some(pending))) = runtime.drain_one(&mut task) else {
        panic!("pending resume expected")
    };
    edges = 0;
    assert!(pending.trace(&mut |_| edges += 1));
    assert_eq!(edges, 3);
    let pending = pending.invoke_decoder();
    edges = 0;
    assert!(pending.trace(&mut |_| edges += 1));
    assert_eq!(edges, 3);

    let borrow = context.borrow_mut();
    edges = 0;
    assert!(!pending.trace(&mut |_| edges += 1));
    assert_eq!(edges, 1);
    drop(borrow);
}

#[test]
fn wait_cancel_requires_canonical_request_and_exact_owner_key() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    });
    let mut runtime = WaitRuntime::new(executor).unwrap();
    let mut ids = Ids::new();
    let mut owner = task(&mut ids);
    let mut stranger = task(&mut ids);
    owner.start().unwrap();
    stranger.start().unwrap();
    let key = runtime
        .register_external(
            &mut owner,
            external_suspend(events),
            TaskContextHandle::default(),
        )
        .unwrap();
    let _ = ids.wait_key();
    let stale = ids.wait_key();

    assert!(runtime.cancel(&mut owner, key, Instant::now()).is_none());
    stranger.request_cancellation(CancelReason::Explicit);
    assert!(runtime.cancel(&mut stranger, key, Instant::now()).is_none());
    owner.request_cancellation(CancelReason::Timeout);
    assert!(runtime.cancel(&mut owner, stale, Instant::now()).is_none());
    assert_eq!(runtime.active_len(), 1);
    assert_eq!(owner.state_name(), StateName::Waiting);
    assert!(runtime.cancel(&mut owner, key, Instant::now()).is_some());
    assert_eq!(runtime.active_len(), 0);
    assert_eq!(owner.state_name(), StateName::Ready);
}

#[test]
fn wait_constructor_preserves_executor_attach_error() {
    let error = ExecutorAttachError::ShuttingDown;
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: Some(error),
    });
    assert_eq!(
        WaitRuntime::new(executor).err(),
        Some(RuntimeCreateError::ExecutorAttach(error))
    );
}

#[test]
fn wait_cancel_uses_first_task_reason_and_only_quarantine_completion_reaps_cleanup() {
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    });
    let mut ids = Ids::new();

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut quarantine = WaitRuntime::new(executor.clone()).unwrap();
    let mut quarantine_task = task(&mut ids);
    quarantine_task.start().unwrap();
    let Ok(key) = quarantine.register_external(
        &mut quarantine_task,
        external_suspend(events.clone()),
        TaskContextHandle::default(),
    ) else {
        panic!("registration accepted")
    };
    assert!(quarantine_task.request_cancellation(CancelReason::Timeout));
    assert!(!quarantine_task.request_cancellation(CancelReason::Explicit));
    let pending = quarantine
        .cancel(&mut quarantine_task, key, Instant::now())
        .unwrap();
    assert_eq!(quarantine_task.state_name(), StateName::Ready);
    assert!(matches!(
        pending.invoke_continuation(),
        Ok(NativeOutcome::Return(_))
    ));
    assert_eq!(
        quarantine
            .drain_one(&mut quarantine_task)
            .map(|item| item.0),
        Some(CompletionRoute::Cleanup)
    );
    assert_eq!(quarantine.quarantine_reaped(), 1);
    assert_eq!(quarantine.late_completions(), 0);

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut interruptible = WaitRuntime::new(executor).unwrap();
    let mut interruptible_task = task(&mut ids);
    interruptible_task.start().unwrap();
    let Ok(key) = interruptible.register_external(
        &mut interruptible_task,
        interruptible_suspend(events),
        TaskContextHandle::default(),
    ) else {
        panic!("registration accepted")
    };
    interruptible_task.request_cancellation(CancelReason::Owner);
    interruptible
        .cancel(&mut interruptible_task, key, Instant::now())
        .unwrap();
    assert_eq!(
        interruptible
            .drain_one(&mut interruptible_task)
            .map(|item| item.0),
        Some(CompletionRoute::Late)
    );
    assert_eq!(interruptible.cleanup_len(), 1);
    assert_eq!(interruptible.quarantine_reaped(), 0);
}

#[test]
fn exact_quarantine_removal_leaves_one_charged_tombstone_without_scanning_predecessors() {
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    });
    let mut runtime = WaitRuntime::new(executor).unwrap();
    let mut ids = Ids::new();
    let mut last_key = None;

    for _ in 0..1_024 {
        let mut task = task(&mut ids);
        task.start().unwrap();
        let key = runtime
            .register_external(
                &mut task,
                external_suspend(Arc::new(Mutex::new(Vec::new()))),
                TaskContextHandle::default(),
            )
            .unwrap();
        task.request_cancellation(CancelReason::Explicit);
        runtime.cancel(&mut task, key, Instant::now()).unwrap();
        last_key = Some(key);
    }

    assert!(runtime.remove_cleanup_exact_for_test(last_key.unwrap()));
    assert_eq!(runtime.cleanup_len(), 1_023);
    assert_eq!(runtime.cleanup_tombstones(), 1);
}

#[derive(Clone)]
struct FakeClock {
    origin: Instant,
    elapsed: Rc<Cell<Duration>>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            elapsed: Rc::new(Cell::new(Duration::ZERO)),
        }
    }

    fn advance(&self, duration: Duration) {
        self.elapsed.set(self.elapsed.get() + duration);
    }
}

impl RuntimeClock for FakeClock {
    fn now(&self) -> Instant {
        self.origin + self.elapsed.get()
    }
}

#[test]
fn timer_same_deadline_is_fifo_and_zero_duration_is_immediately_due() {
    let clock = FakeClock::new();
    let executor = Arc::new(FakeExecutor {
        mode: FakeSubmit::Inline,
        failure: None,
    });
    let runtime = WaitRuntime::new(executor).unwrap();
    let mut timers = TimerQueue::new();
    let first = runtime.issue_internal_wait().unwrap();
    let second = runtime.issue_internal_wait().unwrap();
    let zero = runtime.issue_internal_wait().unwrap();
    timers.insert(clock.now() + Duration::from_millis(5), first);
    timers.insert(clock.now() + Duration::from_millis(5), second);
    timers.insert(clock.now(), zero);

    assert_eq!(timers.pop_due(clock.now()), Some(zero));
    clock.advance(Duration::from_millis(5));
    assert_eq!(timers.pop_due(clock.now()), Some(first));
    assert_eq!(timers.pop_due(clock.now()), Some(second));
    assert_eq!(timers.pop_due(clock.now()), None);
}

#[test]
fn timer_cancel_removes_only_exact_generation_and_updates_deadline() {
    let clock = FakeClock::new();
    let mut ids = Ids::new();
    let mut timers = TimerQueue::new();
    let old = ids.wait_key();
    let replacement = WaitKey {
        runtime: old.runtime,
        id: old.id,
        generation: ids.generations.allocate().expect("generation available"),
    };
    let absent_generation = ids.generations.allocate().expect("generation available");
    timers.insert(clock.now() + Duration::from_secs(1), old);
    timers.insert(clock.now() + Duration::from_secs(2), replacement);

    assert!(!timers.cancel(WaitKey {
        runtime: old.runtime,
        id: old.id,
        generation: absent_generation,
    }));
    assert!(timers.cancel(old));
    assert_eq!(
        timers.next_deadline(),
        Some(clock.now() + Duration::from_secs(2))
    );
    clock.advance(Duration::from_secs(2));
    assert_eq!(timers.pop_due(clock.now()), Some(replacement));
}

#[test]
fn timer_cancel_physically_removes_entry_without_tombstones() {
    let clock = FakeClock::new();
    let mut ids = Ids::new();
    let mut timers = TimerQueue::new();
    let key = ids.wait_key();
    timers.insert(clock.now() + Duration::from_secs(1), key);

    assert!(timers.cancel(key));
    assert_eq!(timers.scheduled_len(), 0);
    assert_eq!(timers.next_deadline(), None);
}

#[test]
fn runtime_registers_and_fires_timers_with_source_cap_and_idle_deadline() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let deadline = clock.now() + Duration::from_millis(5);
    let first = runtime
        .submit_test_root(TestPreparedTask::timer_returned(deadline, Value::int(1)))
        .unwrap();
    let second = runtime
        .submit_test_root(TestPreparedTask::timer_returned(deadline, Value::int(2)))
        .unwrap();
    runtime.drive(&drive_budget(8)).unwrap();

    assert_eq!(runtime.timer_count_for_test(), 2);
    assert!(matches!(
        runtime.drive(&drive_budget(8)).unwrap(),
        super::DriveState::Idle { next_deadline: Some(next), .. } if next == deadline
    ));

    clock.advance(Duration::from_millis(5));
    let mut budget = drive_budget(8);
    budget.timer_limit = std::num::NonZeroUsize::new(1).unwrap();
    runtime.drive(&budget).unwrap();
    assert_eq!(
        runtime.timer_count_for_test(),
        1,
        "one timer source item per turn"
    );
    assert!(matches!(first.poll_result(), RootPoll::Pending));
    assert!(matches!(second.poll_result(), RootPoll::Pending));

    for _ in 0..4 {
        runtime.drive(&budget).unwrap();
    }
    assert!(matches!(first.poll_result(), RootPoll::Ready(_)));
    assert!(matches!(second.poll_result(), RootPoll::Ready(_)));
}

#[test]
fn runtime_cancellation_removes_registered_timer_and_settles_once() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let handle = runtime
        .submit_test_root(TestPreparedTask::timer_returned(
            clock.now() + Duration::from_secs(1),
            Value::int(1),
        ))
        .unwrap();
    runtime.drive(&drive_budget(8)).unwrap();
    assert_eq!(runtime.timer_count_for_test(), 1);

    assert!(handle.cancel(CancelReason::Explicit));
    runtime.drive(&drive_budget(8)).unwrap();
    assert_eq!(runtime.timer_count_for_test(), 0);
    runtime.drive(&drive_budget(8)).unwrap();
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("cancelled timer settles root")
    };
    assert!(matches!(
        settlement.outcome,
        TaskOutcome::Cancelled(CancelReason::Explicit)
    ));
    clock.advance(Duration::from_secs(1));
    runtime.drive(&drive_budget(8)).unwrap();
    assert!(matches!(handle.poll_result(), RootPoll::Ready(_)));
}

fn drive_budget(limit: usize) -> DriveBudget {
    DriveBudget {
        work_item_limit: std::num::NonZeroUsize::new(limit).unwrap(),
        completion_limit: std::num::NonZeroUsize::new(3).unwrap(),
        timer_limit: std::num::NonZeroUsize::new(3).unwrap(),
        root_visit_limit: std::num::NonZeroUsize::new(2).unwrap(),
        cleanup_limit: std::num::NonZeroUsize::new(2).unwrap(),
        instruction_limit_per_task: std::num::NonZeroUsize::new(5).unwrap(),
        wall_clock_limit: Duration::from_secs(1),
    }
}

#[test]
fn drive_limits_reserve_root_visit_under_completion_and_timer_backlogs() {
    let clock = FakeClock::new();
    let mut driver = BoundedDriver::new(Rc::new(clock));
    driver.add_completions(10);
    driver.add_timers(10);
    driver.add_cleanup(10);
    driver.add_ready_roots(3);

    let report = driver.drive(&drive_budget(5));
    assert!(report.work_items <= 5);
    assert_eq!(report.root_visits, 2);
    assert_eq!(report.completions, 1);
    assert_eq!(report.timers, 1);
    assert_eq!(report.cleanup, 1);
    assert!(report.ready_remaining);
}

#[test]
fn drive_limits_wall_clock_is_checked_between_items() {
    let clock = FakeClock::new();
    let mut driver = BoundedDriver::new(Rc::new(clock.clone()));
    driver.add_completions(10);
    driver.set_after_item(move || clock.advance(Duration::from_millis(2)));
    let mut budget = drive_budget(10);
    budget.wall_clock_limit = Duration::from_millis(1);

    let report = driver.drive(&budget);
    assert_eq!(report.work_items, 1);
}

#[test]
fn drive_does_not_consume_unvisited_reserved_roots() {
    let clock = FakeClock::new();
    let mut driver = BoundedDriver::new(Rc::new(clock));
    driver.add_completions(2);
    driver.add_ready_roots(3);
    let mut budget = drive_budget(1);
    budget.root_visit_limit = std::num::NonZeroUsize::new(3).unwrap();

    let first = driver.drive(&budget);
    let second = driver.drive(&budget);

    assert_eq!(first.work_items, 1);
    assert_eq!(second.work_items, 1);
    assert!(first.ready_remaining || second.ready_remaining);
    assert_eq!(driver.pending_ready_roots(), 2);
}

#[test]
fn drive_rotation_persists_for_repeated_one_credit_calls() {
    let clock = FakeClock::new();
    let mut driver = BoundedDriver::new(Rc::new(clock));
    driver.add_completions(8);
    driver.add_timers(8);
    driver.add_cleanup(8);
    driver.add_ready_roots(8);
    let budget = drive_budget(1);
    let reports: Vec<_> = (0..8).map(|_| driver.drive(&budget)).collect();

    assert!(reports.iter().any(|report| report.completions == 1));
    assert!(reports.iter().any(|report| report.timers == 1));
    assert!(reports.iter().any(|report| report.cleanup == 1));
    assert!(reports.iter().any(|report| report.root_visits == 1));
}

struct Ids {
    runtime: RuntimeId,
    tasks: IdCounter<TaskId>,
    roots: RuntimeScopedIdCounter<RootId>,
    scopes: IdCounter<ScopeId>,
    waits: IdCounter<WaitId>,
    generations: IdCounter<WaitGeneration>,
    settlements: IdCounter<SettlementSeq>,
}

impl Ids {
    fn new() -> Self {
        let (runtime, issuers) = runtime_issuers();
        let (root_ids, _, _) = issuers.into_parts();
        Self {
            runtime,
            tasks: IdCounter::new(),
            roots: root_ids,
            scopes: IdCounter::new(),
            waits: IdCounter::new(),
            generations: IdCounter::new(),
            settlements: IdCounter::new(),
        }
    }

    fn wait_key(&mut self) -> WaitKey {
        WaitKey {
            runtime: self.runtime,
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
