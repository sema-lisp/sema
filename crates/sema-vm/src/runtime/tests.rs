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
    RootHandle, RootPoll, Runtime, TestPreparedTask,
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

/// Compile a Sema source expression and submit it as a real VM-backed root.
fn submit_vm_expr(runtime: &Runtime, src: &str) -> RootHandle {
    let vals = sema_reader::read_many(src).expect("parse");
    let prog = crate::compile_program(&vals, None).expect("compile");
    let mut vm = crate::VM::new_for_task_with_native_fns(
        Rc::new(sema_core::Env::new()),
        Rc::new(prog.functions),
        Rc::new(Vec::new()),
    );
    vm.seed_main_frame(prog.closure);
    runtime.submit_root(vm).expect("root admitted")
}

fn submit_debuggable_vm_expr(runtime: &Runtime, src: &str, source: &str) -> RootHandle {
    let (vals, spans) = sema_reader::read_many_with_spans(src).expect("parse");
    let prog =
        crate::compile_program_with_spans(&vals, &spans, Some(std::path::PathBuf::from(source)))
            .expect("compile");
    let mut vm = crate::VM::new(
        Rc::new(sema_core::Env::new()),
        prog.functions,
        &prog.native_table,
        prog.main_cache_slots,
    )
    .expect("VM construction");
    vm.seed_main_frame(prog.closure);
    runtime.submit_root(vm).expect("root admitted")
}

fn drive_root_to_int(runtime: &Runtime, handle: &RootHandle) -> i64 {
    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(64)).unwrap();
        guard += 1;
        assert!(guard < 100, "real vm root did not settle");
    }
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("real vm root settles");
    };
    match &settlement.outcome {
        TaskOutcome::Returned(v) => v.as_int().expect("integer result"),
        other => panic!("expected Returned, got {other:?}"),
    }
}

#[test]
fn active_debugger_stops_only_its_target_root() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let ordinary = submit_debuggable_vm_expr(
        &runtime,
        "(begin (define ordinary-only 40) (+ ordinary-only 2))",
        "ordinary.sema",
    );
    let debugged = submit_debuggable_vm_expr(
        &runtime,
        "(begin (define debug-only 40) (+ debug-only 2))",
        "debugged.sema",
    );

    let mut debug = crate::DebugState::new_headless();
    debug.step_mode = crate::StepMode::StepInto;
    debug.instructions_remaining = 500_000;
    let _active = crate::ActiveDebugGuard::enter_for_root(&mut debug, debugged.id());

    let stopped_root = loop {
        match runtime.drive(&drive_budget(64)).expect("debug drive") {
            super::DriveState::DebugStopped { root, .. } => break root,
            _ => {
                assert!(
                    matches!(debugged.poll_result(), RootPoll::Pending),
                    "debug root must stop before settling"
                );
            }
        }
    };
    assert_eq!(
        stopped_root,
        debugged.id(),
        "the debugger must not claim the earlier ordinary root"
    );
    assert!(runtime.is_debug_paused_for(debugged.id()));
    assert!(!runtime.is_debug_paused_for(ordinary.id()));
    assert!(
        runtime.with_paused_root_vm(ordinary.id(), |_| ()).is_none(),
        "an unrelated root cannot inspect the paused VM"
    );
    assert!(!runtime.debug_resume_root(ordinary.id()));
    assert!(!runtime.debug_cancel_paused_root(ordinary.id()));
    assert!(
        runtime.is_debug_paused_for(debugged.id()),
        "mismatched controls leave the target barrier intact"
    );
    assert!(runtime.debug_cancel_paused_root(debugged.id()));
    assert_eq!(
        drive_root_to_int(&runtime, &ordinary),
        42,
        "the ordinary root completes outside the target debugger"
    );
}

#[test]
fn runtime_executes_a_real_vm_root_to_a_returned_value() {
    // The unified runtime drives a real compiled Sema root via run_quantum and
    // settles with its value — the first end-to-end evaluation through the runtime.
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = submit_vm_expr(&runtime, "(+ 1 2)");
    assert_eq!(drive_root_to_int(&runtime, &handle), 3);
}

#[test]
fn runtime_interleaves_two_real_vm_roots_independently() {
    // Two real roots submitted before driving settle independently with their
    // own values — the runtime evaluates multiple concurrent roots.
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let a = submit_vm_expr(&runtime, "(+ 1 2)");
    let b = submit_vm_expr(&runtime, "(+ 20 22)");
    while matches!(a.poll_result(), RootPoll::Pending)
        || matches!(b.poll_result(), RootPoll::Pending)
    {
        runtime.drive(&drive_budget(64)).unwrap();
    }
    assert_eq!(drive_root_to_int(&runtime, &a), 3);
    assert_eq!(drive_root_to_int(&runtime, &b), 42);
}

#[test]
fn root_filtered_drive_leaves_an_earlier_foreign_root_queued() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let foreign = submit_vm_expr(&runtime, "(+ 1 2)");
    let owned = submit_vm_expr(&runtime, "(+ 20 22)");

    while matches!(owned.poll_result(), RootPoll::Pending) {
        runtime
            .drive_roots(&drive_budget(64), &[owned.id()])
            .expect("owned root drives");
    }

    assert!(matches!(foreign.poll_result(), RootPoll::Pending));
    assert_eq!(drive_root_to_int(&runtime, &owned), 42);
    assert_eq!(drive_root_to_int(&runtime, &foreign), 3);
}

#[test]
fn root_filtered_drive_leaves_a_later_foreign_root_queued() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let owned = submit_vm_expr(&runtime, "(+ 20 22)");
    let foreign = submit_vm_expr(&runtime, "(+ 1 2)");

    while matches!(owned.poll_result(), RootPoll::Pending) {
        runtime
            .drive_roots(&drive_budget(64), &[owned.id()])
            .expect("owned root drives");
    }

    assert!(matches!(foreign.poll_result(), RootPoll::Pending));
    assert_eq!(drive_root_to_int(&runtime, &owned), 42);
    assert_eq!(drive_root_to_int(&runtime, &foreign), 3);
}

#[test]
fn root_filtered_drive_does_not_report_a_foreign_debug_stop() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let foreign = runtime
        .submit_test_root(TestPreparedTask::debug_stop())
        .expect("foreign debug root admitted");
    let selected = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("selected root admitted");

    assert!(matches!(
        runtime
            .drive_roots(&drive_budget(8), &[foreign.id()])
            .expect("foreign root reaches its debug stop"),
        super::DriveState::DebugStopped { root, .. } if root == foreign.id()
    ));

    assert!(matches!(
        runtime
            .drive_roots(&drive_budget(8), &[selected.id()])
            .expect("selected drive remains blocked by the runtime barrier"),
        super::DriveState::Idle {
            next_deadline: None,
            inbox_wakeup_required: false,
        }
    ));
    assert!(matches!(selected.poll_result(), RootPoll::Pending));
}

#[test]
fn root_filtered_idle_ignores_a_foreign_timer_deadline() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let foreign = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::Timer(Duration::from_secs(800)),
                continuation: Box::new(CountingContinuation(Arc::new(Mutex::new(Vec::new())))),
            },
        ))))
        .expect("foreign timer root admitted");
    let channel = runtime.create_channel_for_test(0);
    let selected = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Receive { channel }),
                continuation: Box::new(CountingContinuation(Arc::new(Mutex::new(Vec::new())))),
            },
        ))))
        .expect("selected channel root admitted");
    let one = drive_budget(1);

    while runtime.timer_count_for_test() == 0 {
        runtime.drive_roots(&one, &[foreign.id()]).unwrap();
    }
    while runtime.channel_receiver_queue_len_for_test(channel) == 0 {
        runtime.drive_roots(&one, &[selected.id()]).unwrap();
    }

    assert!(matches!(
        runtime.drive_roots(&one, &[selected.id()]).unwrap(),
        super::DriveState::Idle {
            next_deadline: None,
            inbox_wakeup_required: false,
        }
    ));
    assert!(matches!(selected.poll_result(), RootPoll::Pending));
    assert_eq!(runtime.timer_count_for_test(), 1);
}

#[test]
fn root_filtered_idle_ignores_a_foreign_external_wait() {
    let runtime = Runtime::new(
        Rc::new(sema_core::EvalContext::new()),
        Rc::new(FakeClock::new()),
        Arc::new(PendingExecutor),
    )
    .expect("runtime");
    let foreign = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(Arc::new(Mutex::new(Vec::new()))),
        ))))
        .expect("foreign external root admitted");
    let channel = runtime.create_channel_for_test(0);
    let selected = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Receive { channel }),
                continuation: Box::new(CountingContinuation(Arc::new(Mutex::new(Vec::new())))),
            },
        ))))
        .expect("selected channel root admitted");
    let one = drive_budget(1);

    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive_roots(&one, &[foreign.id()]).unwrap();
    }
    while runtime.channel_receiver_queue_len_for_test(channel) == 0 {
        runtime.drive_roots(&one, &[selected.id()]).unwrap();
    }

    assert!(matches!(
        runtime.drive_roots(&one, &[selected.id()]).unwrap(),
        super::DriveState::Idle {
            next_deadline: None,
            inbox_wakeup_required: false,
        }
    ));
    assert!(matches!(selected.poll_result(), RootPoll::Pending));
    assert_eq!(runtime.active_wait_count_for_test(), 1);
}

#[test]
fn root_filtered_mixed_promise_wake_preserves_the_foreign_stage_position() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let promise = runtime.create_pending_promise_for_test();
    let promise_wait = || {
        TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::PromiseSetWait {
                wait: sema_core::runtime::PromiseSetWait {
                    promises: vec![promise],
                    mode: sema_core::runtime::PromiseSetMode::Race,
                },
                continuation: Box::new(CaptureRuntimeContinuation(Rc::new(RefCell::new(
                    Vec::new(),
                )))),
            },
        )))
    };
    let foreign = runtime
        .submit_test_root(promise_wait())
        .expect("foreign promise observer admitted");
    let selected = runtime
        .submit_test_root(promise_wait())
        .expect("selected promise observer admitted");
    let one = drive_budget(1);

    while runtime.protocol_wait_count_for_test() < 1 {
        runtime.drive_roots(&one, &[foreign.id()]).unwrap();
    }
    while runtime.protocol_wait_count_for_test() < 2 {
        runtime.drive_roots(&one, &[selected.id()]).unwrap();
    }
    runtime.settle_promise_for_test(promise, TaskOutcome::Returned(Value::int(7)));

    let later = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(9)))
        .expect("later root admitted");
    while runtime
        .pending_stage_positions_for_root_for_test(later.id())
        .is_empty()
    {
        runtime.drive_roots(&one, &[later.id()]).unwrap();
    }

    assert_eq!(
        runtime.pending_stage_positions_for_root_for_test(foreign.id()),
        vec![0]
    );
    assert_eq!(
        runtime.pending_stage_positions_for_root_for_test(selected.id()),
        vec![0]
    );
    assert_eq!(
        runtime.pending_stage_positions_for_root_for_test(later.id()),
        vec![1]
    );

    runtime.drive_roots(&one, &[selected.id()]).unwrap();

    assert_eq!(
        runtime.pending_stage_positions_for_root_for_test(foreign.id()),
        vec![0],
        "the foreign remainder retains the mixed wake's original queue position"
    );
    assert_eq!(
        runtime.pending_stage_positions_for_root_for_test(later.id()),
        vec![1],
        "the later stage cannot overtake the foreign wake"
    );
}

#[test]
fn root_filtered_mixed_channel_close_preserves_the_foreign_stage_position() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let channel = runtime.create_channel_for_test(0);
    let receive = || {
        TestPreparedTask::native(Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Receive { channel }),
            continuation: Box::new(CountingContinuation(Arc::new(Mutex::new(Vec::new())))),
        })))
    };
    let foreign = runtime
        .submit_test_root(receive())
        .expect("foreign channel receiver admitted");
    let selected = runtime
        .submit_test_root(receive())
        .expect("selected channel receiver admitted");
    let one = drive_budget(1);

    while runtime.channel_receiver_queue_len_for_test(channel) < 1 {
        runtime.drive_roots(&one, &[foreign.id()]).unwrap();
    }
    while runtime.channel_receiver_queue_len_for_test(channel) < 2 {
        runtime.drive_roots(&one, &[selected.id()]).unwrap();
    }

    let closer = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::ChannelOp {
                channel,
                operation: sema_core::runtime::ChannelOperation::Close,
                continuation: Box::new(CountingContinuation(Arc::new(Mutex::new(Vec::new())))),
            },
        ))))
        .expect("channel closer admitted");
    while runtime.channel_receiver_queue_len_for_test(channel) > 0 {
        runtime.drive_roots(&one, &[closer.id()]).unwrap();
    }

    assert_eq!(
        runtime.pending_stage_positions_for_root_for_test(foreign.id()),
        vec![0]
    );
    assert_eq!(
        runtime.pending_stage_positions_for_root_for_test(selected.id()),
        vec![0]
    );
    assert_eq!(
        runtime.pending_stage_positions_for_root_for_test(closer.id()),
        vec![1]
    );

    runtime.drive_roots(&one, &[selected.id()]).unwrap();

    assert_eq!(
        runtime.pending_stage_positions_for_root_for_test(foreign.id()),
        vec![0],
        "the foreign remainder retains the channel close's original queue position"
    );
    assert_eq!(
        runtime.pending_stage_positions_for_root_for_test(closer.id()),
        vec![1],
        "the close response cannot overtake the foreign receiver wake"
    );
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
fn rendezvous_wake_survives_receiver_shutdown_cancellation() {
    // A committed channel rendezvous delivers its value even when the receiver is
    // then cancelled by shutdown: the value is delivered, nothing is dropped, and
    // shutdown reports clean. Guards the dropped_protocol_completions invariant on
    // the rendezvous+cancellation path (see docs/bugs for the UCR-3 race this
    // diagnostic is designed to catch under seeded interleavings).
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let channel = runtime.create_channel_for_test(0);
    let received = Arc::new(Mutex::new(Vec::new()));
    let _receiver = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Receive { channel }),
                continuation: Box::new(CountingContinuation(Arc::clone(&received))),
            },
        ))))
        .unwrap();
    runtime.drive(&drive_budget(8)).unwrap();

    let sender = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Send {
                    channel,
                    value: Value::int(42),
                }),
                continuation: Box::new(CountingContinuation(Arc::new(Mutex::new(Vec::new())))),
            },
        ))))
        .unwrap();
    // Drive until the sender settles: that proves the rendezvous committed (the
    // receiver was matched and told Sent) and leaves the receiver's wake queued.
    let mut guard = 0;
    while matches!(sender.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(1)).unwrap();
        guard += 1;
        assert!(guard < 200, "sender never matched the receiver");
    }
    // Shutdown requests cancellation on the (already-matched) receiver and drives
    // to quiescence.
    let report = runtime
        .shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(8),
        })
        .expect("bounded shutdown");
    assert!(report.clean, "{report:?}");
    assert_eq!(
        runtime.dropped_protocol_completions_for_test(),
        0,
        "a committed rendezvous value was dropped"
    );
    assert_eq!(
        received.lock().unwrap().len(),
        1,
        "receiver did not observe its committed rendezvous value"
    );
}

#[test]
fn channel_close_staged_wake_survives_receiver_cancellation() {
    // `rendezvous_wake_survives_receiver_shutdown_cancellation` above now
    // exercises the INLINE rendezvous fast path (a matched send/receive pair
    // resumes within one `install_channel_wait` work item, never touching
    // `PendingStage::ChannelWake`/`ChannelClose`). That leaves the genuinely
    // STAGED wake path — `PendingStage::ChannelClose` -> `consume_channel_wake`
    // -> `finish_protocol_wait` -> `finish_protocol_wait_now` — with no test
    // covering its drop guard: a committed completion delivered to a
    // cancelled/reaped waiter must not be silently dropped.
    //
    // `channel/close`, dispatched via `RuntimeRequest::ChannelOp` (not a
    // `NativeSuspend`), takes the staged path unconditionally: it moves the
    // parked receiver out of the channel registry's own waiter queue and into
    // a `ChannelClose` object pushed onto `state.pending` — a real
    // pending-queue hop with a genuine window between "wake computed" and
    // "wake delivered" for a later `drive()` work item to process. Landing a
    // cancellation in that window is the point of this test:
    // `deliver_cancel_teardown` leaves a `Channel` wait alone (nothing to
    // eagerly abort), and the periodic `cancel_waiting` scan also skips it
    // because it is no longer queued in the channel's own registry (the
    // UCR-3 `has_wait` guard) — so its `protocol_waits` entry survives for the
    // staged wake to deliver instead of being cancel-dropped ahead of it.
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let channel = runtime.create_channel_for_test(0);
    let received = Arc::new(Mutex::new(Vec::new()));
    let receiver = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Receive { channel }),
                continuation: Box::new(CountingContinuation(Arc::clone(&received))),
            },
        ))))
        .unwrap();
    runtime.drive(&drive_budget(8)).unwrap();
    assert_eq!(
        runtime.channel_receiver_queue_len_for_test(channel),
        1,
        "receiver did not park on the empty channel"
    );

    let _closer = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::ChannelOp {
                channel,
                operation: sema_core::runtime::ChannelOperation::Close,
                continuation: Box::new(CountingContinuation(Arc::new(Mutex::new(Vec::new())))),
            },
        ))))
        .unwrap();

    // Drive one work item at a time until `channel/close`'s dispatch runs: it
    // is the single work item that moves the parked receiver out of the
    // channel registry's queue (into the `ChannelClose` handed to
    // `state.pending`) without yet delivering its wake. That transition is
    // the staged window this test needs to land the cancellation in.
    let mut guard = 0;
    while runtime.channel_receiver_queue_len_for_test(channel) > 0 {
        runtime.drive(&drive_budget(1)).unwrap();
        guard += 1;
        assert!(
            guard < 200,
            "channel/close never dequeued the parked receiver"
        );
    }

    // The receiver's `ChannelWake` (Closed) is queued in `state.pending` but
    // not yet consumed. Cancel the receiver here, in that window.
    assert!(receiver.cancel(CancelReason::Explicit));

    let report = runtime
        .shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(8),
        })
        .expect("bounded shutdown");
    assert!(report.clean, "{report:?}");
    assert_eq!(
        runtime.dropped_protocol_completions_for_test(),
        0,
        "a committed close wake was dropped on the staged path"
    );
    // The receiver's continuation must still be resumed exactly once, proving
    // the staged wake (`consume_channel_wake` -> `finish_protocol_wait`) was
    // actually delivered rather than silently swallowed once the wait landed
    // in `state.pending`. A cancelled task always observes its resume as
    // `Cancelled` regardless of the response that was in flight (the
    // sticky-cancellation override in `resume_continuation_value`), so this
    // can't assert on the resume's *content* — only that the resume happened
    // at all, which a silently-dropped wake would never produce (the receiver
    // would hang and `shutdown` above would report `clean: false`).
    assert_eq!(
        received.lock().unwrap().len(),
        1,
        "receiver did not observe its staged close wake despite cancellation"
    );
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

/// A continuation that records the shape of the resume input it observes and
/// then returns `value`, so the parked task settles with `value`. Used to assert
/// that a Timer suspension resumes with `ResumeInput::Returned(nil)`.
struct TimerResumeContinuation {
    observed: Rc<RefCell<Vec<&'static str>>>,
    value: Value,
}

impl Trace for TimerResumeContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Value(&self.value));
        true
    }
}

impl NativeContinuation for TimerResumeContinuation {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, input: ResumeInput) -> NativeResult {
        self.observed.borrow_mut().push(match &input {
            ResumeInput::Returned(v) if *v == Value::NIL => "returned-nil",
            ResumeInput::Returned(_) => "returned-other",
            ResumeInput::Failed(_) => "failed",
            ResumeInput::Cancelled(_) => "cancelled",
            ResumeInput::Runtime(_) => "runtime",
        });
        Ok(NativeOutcome::Return(self.value))
    }
}

#[test]
fn runtime_timer_suspension_resumes_continuation_with_nil_after_deadline() {
    // A native returning `Suspend { wait: Timer(d) }` parks its continuation on a
    // runtime timer; when the deadline elapses the continuation resumes with
    // `Returned(nil)` and its own return value settles the task.
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let observed = Rc::new(RefCell::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::Timer(Duration::from_millis(5)),
                continuation: Box::new(TimerResumeContinuation {
                    observed: Rc::clone(&observed),
                    value: Value::int(7),
                }),
            },
        ))))
        .unwrap();

    runtime.drive(&drive_budget(8)).unwrap();
    assert_eq!(
        runtime.timer_count_for_test(),
        1,
        "timer suspension arms one runtime timer"
    );
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    assert!(
        observed.borrow().is_empty(),
        "continuation must not resume before the deadline"
    );

    clock.advance(Duration::from_millis(5));
    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 100, "timer task settles after the deadline");
    }
    assert_eq!(*observed.borrow(), vec!["returned-nil"]);
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("timer task settles");
    };
    assert!(matches!(&settlement.outcome, TaskOutcome::Returned(v) if v.as_int() == Some(7)));
    assert_eq!(runtime.timer_count_for_test(), 0);
}

/// Drives a real compiled Sema source to settlement and returns the value it
/// returned — used to obtain a spawn-able VM closure thunk for the Spawn test.
fn settle_vm_value(runtime: &Runtime, src: &str) -> Value {
    let handle = submit_vm_expr(runtime, src);
    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(64)).unwrap();
        guard += 1;
        assert!(guard < 100, "value root settles");
    }
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("value root settles");
    };
    match &settlement.outcome {
        TaskOutcome::Returned(value) => value.clone(),
        other => panic!("expected Returned, got {other:?}"),
    }
}

/// Spawn continuation: records the promise the runtime allocated and then awaits
/// it, so the parked task settles with the child's returned value.
struct SpawnAwaitContinuation(Rc<Cell<Option<sema_core::runtime::PromiseId>>>);

impl Trace for SpawnAwaitContinuation {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for SpawnAwaitContinuation {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, input: ResumeInput) -> NativeResult {
        let ResumeInput::Runtime(sema_core::runtime::RuntimeResponse::Promise(promise)) = input
        else {
            panic!("spawn resumes with a canonical promise");
        };
        self.0.set(Some(promise));
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::Promise(promise),
            continuation: Box::new(SettlementValueContinuation),
        }))
    }
}

struct SettlementValueContinuation;

impl Trace for SettlementValueContinuation {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for SettlementValueContinuation {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, input: ResumeInput) -> NativeResult {
        let ResumeInput::Runtime(sema_core::runtime::RuntimeResponse::Settlement(Some(settlement))) =
            input
        else {
            panic!("await resumes with the child settlement");
        };
        match &settlement.outcome {
            TaskOutcome::Returned(value) => Ok(NativeOutcome::Return(value.clone())),
            other => panic!("child returned, got {other:?}"),
        }
    }
}

#[test]
fn runtime_spawn_request_yields_promise_that_settles_with_child_result() {
    // A native returning `Runtime(Spawn { .. })` admits a detached child through
    // the canonical PromiseRegistry and resumes with `Promise(id)`. Awaiting that
    // promise yields the child's returned value once the child task settles.
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let thunk = settle_vm_value(&runtime, "(fn () 42)");
    let promise = Rc::new(Cell::new(None));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::Spawn {
                callable: thunk,
                continuation: Box::new(SpawnAwaitContinuation(Rc::clone(&promise))),
            },
        ))))
        .unwrap();

    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 100, "spawn root settles");
    }
    assert!(
        promise.get().is_some(),
        "spawn allocated a canonical registry promise"
    );
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("spawn root settles");
    };
    assert!(matches!(&settlement.outcome, TaskOutcome::Returned(v) if v.as_int() == Some(42)));
}

/// Timeout continuation: asserts it resumes with a failure and records the
/// structured condition's `:type` keyword.
struct CaptureTimeoutContinuation(Rc<RefCell<Option<String>>>);

impl Trace for CaptureTimeoutContinuation {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for CaptureTimeoutContinuation {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, input: ResumeInput) -> NativeResult {
        let ResumeInput::Failed(error) = input else {
            panic!("timeout deadline resumes with a failure");
        };
        let kind = match &error {
            SemaError::Condition(value) => value
                .as_map_ref()
                .and_then(|map| map.get(&Value::keyword("type")))
                .and_then(Value::as_keyword),
            _ => None,
        };
        *self.0.borrow_mut() = kind;
        Ok(NativeOutcome::Return(Value::NIL))
    }
}

#[test]
fn runtime_promise_timeout_resolves_to_settlement_when_promise_wins() {
    // A settled promise wins the timeout race; the deadline timer is deregistered
    // and the observed settlement is delivered.
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let promise = runtime.create_pending_promise_for_test();
    let responses = Rc::new(RefCell::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::PromiseSetWait {
                wait: sema_core::runtime::PromiseSetWait {
                    promises: vec![promise],
                    mode: sema_core::runtime::PromiseSetMode::Timeout(Duration::from_millis(5)),
                },
                continuation: Box::new(CaptureRuntimeContinuation(Rc::clone(&responses))),
            },
        ))))
        .unwrap();
    runtime.drive(&drive_budget(8)).unwrap();
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    assert_eq!(
        runtime.timer_count_for_test(),
        1,
        "timeout arms a deadline timer while the promise is pending"
    );
    let settlement = runtime.settle_promise_for_test(promise, TaskOutcome::Returned(Value::int(3)));
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(
        runtime.timer_count_for_test(),
        0,
        "the winning promise deregisters the deadline timer"
    );
    let sema_core::runtime::RuntimeResponse::Settlement(Some(observed)) = &responses.borrow()[0]
    else {
        panic!("timeout returns the winning settlement");
    };
    assert!(Rc::ptr_eq(observed, &settlement));
}

#[test]
fn runtime_promise_timeout_raises_structured_condition_when_deadline_wins() {
    // A pending promise at the deadline raises a structured `:timeout` condition
    // and leaves the supplied promise untouched (still settleable afterward).
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let promise = runtime.create_pending_promise_for_test();
    let kind = Rc::new(RefCell::new(None));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::PromiseSetWait {
                wait: sema_core::runtime::PromiseSetWait {
                    promises: vec![promise],
                    mode: sema_core::runtime::PromiseSetMode::Timeout(Duration::from_millis(5)),
                },
                continuation: Box::new(CaptureTimeoutContinuation(Rc::clone(&kind))),
            },
        ))))
        .unwrap();
    runtime.drive(&drive_budget(8)).unwrap();
    assert!(matches!(handle.poll_result(), RootPoll::Pending));

    clock.advance(Duration::from_millis(5));
    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 100, "timeout task settles after the deadline");
    }
    assert_eq!(kind.borrow().as_deref(), Some("timeout"));
    assert_eq!(runtime.timer_count_for_test(), 0);
    // The observed promise is left untouched — it is still pending and settleable.
    runtime.settle_promise_for_test(promise, TaskOutcome::Returned(Value::int(9)));
}

#[test]
fn deadlocked_root_deregisters_promise_wait_before_later_dependency_wake() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let promise = runtime.create_pending_promise_for_test();
    let responses = Rc::new(RefCell::new(Vec::new()));
    let deadlocked = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::Promise(promise),
                continuation: Box::new(CaptureRuntimeContinuation(Rc::clone(&responses))),
            },
        ))))
        .expect("deadlocked root admitted");
    while runtime.protocol_wait_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }

    assert!(runtime
        .settle_deadlocked_root(deadlocked.id())
        .expect("deadlocked root settlement succeeds"));
    let RootPoll::Ready(settlement) = deadlocked.poll_result() else {
        panic!("deadlocked root settles failed")
    };
    assert!(matches!(settlement.outcome, TaskOutcome::Failed(_)));

    runtime.settle_promise_for_test(promise, TaskOutcome::Returned(Value::int(42)));
    let fresh = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(9)))
        .expect("persistent runtime accepts a later root");
    let mut turns = 0;
    while matches!(fresh.poll_result(), RootPoll::Pending) {
        runtime
            .drive(&drive_budget(8))
            .expect("the later wake must not target the removed deadlocked task");
        turns += 1;
        assert!(turns < 20, "later root settles");
    }

    let RootPoll::Ready(settlement) = fresh.poll_result() else {
        panic!("later root settles")
    };
    assert!(
        matches!(&settlement.outcome, TaskOutcome::Returned(value) if value.as_int() == Some(9))
    );
    assert!(responses.borrow().is_empty());
    assert_eq!(runtime.protocol_wait_count_for_test(), 0);
}

#[test]
fn forced_deadlock_settlement_aborts_external_wait_without_arming_a_resume() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let blocked = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            interruptible_suspend_with_hook(
                Arc::clone(&events),
                RecordingHook {
                    result: CancelResult::Reaped,
                    calls: Arc::clone(&calls),
                    edge: None,
                    trace_ok: true,
                },
            ),
        ))))
        .expect("external root admitted");
    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }

    assert!(runtime
        .settle_deadlocked_root(blocked.id())
        .expect("forced settlement tears down the external wait"));
    assert_eq!(runtime.active_wait_count_for_test(), 0);
    assert_eq!(*calls.lock().unwrap(), vec!["cancel"]);
    assert!(events.lock().unwrap().is_empty());
    let RootPoll::Ready(settlement) = blocked.poll_result() else {
        panic!("externally parked root settles")
    };
    assert!(matches!(settlement.outcome, TaskOutcome::Failed(_)));

    let fresh = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(11)))
        .expect("persistent runtime accepts a later root");
    let mut turns = 0;
    while matches!(fresh.poll_result(), RootPoll::Pending) {
        runtime
            .drive(&drive_budget(8))
            .expect("cancelled external completion cannot resume the removed task");
        turns += 1;
        assert!(turns < 20, "later root settles");
    }
    assert_eq!(*calls.lock().unwrap(), vec!["cancel"]);
    assert!(events.lock().unwrap().is_empty());
}

fn force_deadlocked_root(runtime: &Runtime, handle: &RootHandle) {
    assert!(runtime
        .settle_deadlocked_root(handle.id())
        .expect("forced deadlock cleanup succeeds"));
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("forced deadlock root settles")
    };
    assert!(matches!(settlement.outcome, TaskOutcome::Failed(_)));
}

fn assert_persistent_runtime_accepts_fresh_root(runtime: &Runtime, expected: i64) {
    let fresh = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(expected)))
        .expect("persistent runtime accepts a fresh root");
    assert_eq!(drive_root_to_int(runtime, &fresh), expected);
    for _ in 0..8 {
        runtime
            .drive(&drive_budget(8))
            .expect("late cleanup wake is harmless");
    }
}

#[test]
fn forced_deadlock_cleanup_matrix_deregisters_every_protocol_wait_kind() {
    // PromiseSet::Timeout owns both promise observations and a timer entry.
    {
        let clock = Rc::new(FakeClock::new());
        let runtime = runtime_with_inline_executor(clock.clone());
        let first = runtime.create_pending_promise_for_test();
        let second = runtime.create_pending_promise_for_test();
        let responses = Rc::new(RefCell::new(Vec::new()));
        let blocked = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
                sema_core::runtime::RuntimeRequest::PromiseSetWait {
                    wait: sema_core::runtime::PromiseSetWait {
                        promises: vec![first, second],
                        mode: sema_core::runtime::PromiseSetMode::Timeout(Duration::from_secs(60)),
                    },
                    continuation: Box::new(CaptureRuntimeContinuation(Rc::clone(&responses))),
                },
            ))))
            .expect("promise-set root admitted");
        while runtime.protocol_wait_count_for_test() == 0 || runtime.timer_count_for_test() == 0 {
            runtime.drive(&drive_budget(1)).unwrap();
        }

        force_deadlocked_root(&runtime, &blocked);
        assert_eq!(runtime.protocol_wait_count_for_test(), 0);
        assert_eq!(runtime.timer_count_for_test(), 0);
        runtime.settle_promise_for_test(first, TaskOutcome::Returned(Value::int(1)));
        runtime.settle_promise_for_test(second, TaskOutcome::Returned(Value::int(2)));
        clock.advance(Duration::from_secs(120));
        assert_persistent_runtime_accepts_fresh_root(&runtime, 31);
        assert!(responses.borrow().is_empty());
    }

    // A bare protocol timer must be physically removed from the timer queue.
    {
        let clock = Rc::new(FakeClock::new());
        let runtime = runtime_with_inline_executor(clock.clone());
        let events = Arc::new(Mutex::new(Vec::new()));
        let blocked = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
                NativeSuspend {
                    wait: WaitKind::Timer(Duration::from_secs(60)),
                    continuation: Box::new(CountingContinuation(Arc::clone(&events))),
                },
            ))))
            .expect("timer root admitted");
        while runtime.timer_count_for_test() == 0 {
            runtime.drive(&drive_budget(1)).unwrap();
        }

        force_deadlocked_root(&runtime, &blocked);
        assert_eq!(runtime.protocol_wait_count_for_test(), 0);
        assert_eq!(runtime.timer_count_for_test(), 0);
        clock.advance(Duration::from_secs(120));
        assert_persistent_runtime_accepts_fresh_root(&runtime, 32);
        assert!(events.lock().unwrap().is_empty());
    }

    // A channel waiter must leave the receiver FIFO before a later close.
    {
        let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
        let channel = runtime.create_channel_for_test(0);
        let events = Arc::new(Mutex::new(Vec::new()));
        let blocked = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
                NativeSuspend {
                    wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Receive { channel }),
                    continuation: Box::new(CountingContinuation(Arc::clone(&events))),
                },
            ))))
            .expect("channel root admitted");
        while runtime.channel_receiver_queue_len_for_test(channel) == 0 {
            runtime.drive(&drive_budget(1)).unwrap();
        }

        force_deadlocked_root(&runtime, &blocked);
        assert_eq!(runtime.protocol_wait_count_for_test(), 0);
        assert_eq!(runtime.channel_receiver_queue_len_for_test(channel), 0);
        let close_events = Arc::new(Mutex::new(Vec::new()));
        let closer = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
                sema_core::runtime::RuntimeRequest::ChannelOp {
                    channel,
                    operation: sema_core::runtime::ChannelOperation::Close,
                    continuation: Box::new(CountingContinuation(Arc::clone(&close_events))),
                },
            ))))
            .expect("channel closer admitted");
        while matches!(closer.poll_result(), RootPoll::Pending) {
            runtime.drive(&drive_budget(8)).unwrap();
        }
        assert_persistent_runtime_accepts_fresh_root(&runtime, 33);
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(*close_events.lock().unwrap(), vec!["runtime"]);
    }

    // A queued resource-slot waiter must leave the gate FIFO before release.
    {
        let clock = Rc::new(FakeClock::new());
        let runtime = runtime_with_inline_executor(clock.clone());
        let events = Arc::new(Mutex::new(Vec::new()));
        let gate_slot = Rc::new(Cell::new(None));
        let owner = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
                sema_core::runtime::RuntimeRequest::CreateResourceGate {
                    continuation: Box::new(GateHoldOwner {
                        gate_slot: Rc::clone(&gate_slot),
                        events: Arc::clone(&events),
                        stage: 0,
                        gate: None,
                    }),
                },
            ))))
            .expect("gate owner admitted");
        while gate_slot.get().is_none()
            || runtime
                .resource_gate_owner_for_test(gate_slot.get().expect("gate created"))
                .is_none()
            || runtime.timer_count_for_test() == 0
        {
            runtime.drive(&drive_budget(1)).unwrap();
        }
        let gate = gate_slot.get().expect("gate created");
        let blocked = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
                NativeSuspend {
                    wait: WaitKind::ResourceSlot(gate),
                    continuation: Box::new(RecordGateGrant {
                        label: "deadlocked",
                        events: Arc::clone(&events),
                    }),
                },
            ))))
            .expect("resource waiter admitted");
        while runtime.protocol_wait_count_for_test() < 2 {
            runtime.drive(&drive_budget(1)).unwrap();
        }

        force_deadlocked_root(&runtime, &blocked);
        assert_eq!(
            runtime.protocol_wait_count_for_test(),
            1,
            "only the owner's hold timer remains"
        );
        clock.advance(Duration::from_secs(7200));
        let mut turns = 0;
        while matches!(owner.poll_result(), RootPoll::Pending) {
            runtime.drive(&drive_budget(8)).unwrap();
            turns += 1;
            assert!(turns < 32, "gate owner releases and settles");
        }
        assert_eq!(runtime.resource_gate_owner_for_test(gate), None);
        assert_eq!(runtime.protocol_wait_count_for_test(), 0);
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .all(|event| event != "deadlocked-granted"));
        assert_persistent_runtime_accepts_fresh_root(&runtime, 34);
    }

    // An origin barrier owns only its protocol entry and the aggregate count.
    {
        let clock = Rc::new(FakeClock::new());
        let runtime = runtime_with_inline_executor(clock.clone());
        let barrier_events = Arc::new(Mutex::new(Vec::new()));
        let child_events = Arc::new(Mutex::new(Vec::new()));
        let blocked = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
                sema_core::runtime::RuntimeRequest::OriginBarrier {
                    continuation: Box::new(BarrierReleaseCont {
                        events: Arc::clone(&barrier_events),
                    }),
                },
            ))))
            .expect("barrier root admitted");
        runtime.submit_test_child_under_root(
            blocked.id(),
            TestPreparedTask::native(Ok(NativeOutcome::Suspend(NativeSuspend {
                wait: WaitKind::Timer(Duration::from_secs(60)),
                continuation: Box::new(CountingContinuation(Arc::clone(&child_events))),
            }))),
        );
        while runtime.origin_barrier_wait_count_for_test() == 0
            || runtime.timer_count_for_test() == 0
        {
            runtime.drive(&drive_budget(1)).unwrap();
        }

        force_deadlocked_root(&runtime, &blocked);
        assert_eq!(runtime.origin_barrier_wait_count_for_test(), 0);
        assert_eq!(
            runtime.protocol_wait_count_for_test(),
            1,
            "only the child timer remains"
        );
        clock.advance(Duration::from_secs(120));
        let mut turns = 0;
        while runtime.live_task_count() > 0 {
            runtime.drive(&drive_budget(8)).unwrap();
            turns += 1;
            assert!(turns < 32, "the surviving child timer settles");
        }
        assert_eq!(runtime.protocol_wait_count_for_test(), 0);
        assert!(barrier_events.lock().unwrap().is_empty());
        assert_eq!(*child_events.lock().unwrap(), vec!["returned"]);
        assert_persistent_runtime_accepts_fresh_root(&runtime, 35);
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

#[test]
fn simultaneous_runtimes_route_colliding_local_roots_to_their_own_output_sinks() {
    let runtime_a = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let root_a_slot = Rc::new(Cell::new(None));
    let root_a_for_call = Rc::clone(&root_a_slot);
    let handle_a = runtime_a
        .submit_test_root(TestPreparedTask::native_call(move || {
            let previous = sema_core::set_current_root(root_a_for_call.get());
            sema_core::write_stdout("A-only");
            let _ = sema_core::set_current_root(previous);
            Ok(NativeOutcome::Return(Value::int(1)))
        }))
        .expect("root A admitted");
    root_a_slot.set(Some(handle_a.id()));
    sema_core::mark_root_capturing(handle_a.id());

    // Both scoped counters start at local root 1. The runtime component is the
    // only identity that can select the correct sink while A and B coexist.
    let runtime_b = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let root_b_slot = Rc::new(Cell::new(None));
    let root_b_for_call = Rc::clone(&root_b_slot);
    let handle_b = runtime_b
        .submit_test_root(TestPreparedTask::native_call(move || {
            let previous = sema_core::set_current_root(root_b_for_call.get());
            sema_core::write_stdout("B-only");
            let _ = sema_core::set_current_root(previous);
            Ok(NativeOutcome::Return(Value::int(2)))
        }))
        .expect("root B admitted");
    root_b_slot.set(Some(handle_b.id()));
    sema_core::mark_root_capturing(handle_b.id());
    let root_a = handle_a.id();
    let root_b = handle_b.id();
    assert_eq!(root_a.local(), root_b.local());
    assert_ne!(root_a.runtime(), root_b.runtime());

    while matches!(handle_a.poll_result(), RootPoll::Pending) {
        runtime_a.drive(&drive_budget(8)).expect("runtime A drives");
    }
    let events_a = runtime_a.take_captured_output();
    assert!(matches!(
        events_a.as_slice(),
        [super::OutputEvent::Stdout { root, text }]
            if *root == root_a && text == "A-only"
    ));

    // A is still marked because its settled root handle has not been reaped.
    // Dropping its final Runtime owner must clean up A without removing B's
    // marker or route.
    drop(handle_a);
    drop(runtime_a);
    assert_eq!(sema_core::capturing_root_count(), 1);

    while matches!(handle_b.poll_result(), RootPoll::Pending) {
        runtime_b.drive(&drive_budget(8)).expect("runtime B drives");
    }
    let events_b = runtime_b.take_captured_output();
    assert!(matches!(
        events_b.as_slice(),
        [super::OutputEvent::Stdout { root, text }]
            if *root == root_b && text == "B-only"
    ));
}

/// A `Runtime` dropped directly (bypassing `Interpreter::drop`'s bounded
/// `shutdown`, which drives every task to cancellation/reap before tearing
/// down) closes its inbox without driving any VM quantum. An abandoned root
/// therefore never reaches the normal `cleanup_one` reap path, so Runtime
/// teardown itself must unregister that runtime's capturing-root markers.
#[test]
fn dropped_runtime_with_live_capturing_root_does_not_leak_into_next_runtime_on_thread() {
    {
        let clock = Rc::new(FakeClock::new());
        let runtime_a = runtime_with_inline_executor(clock);
        let handle_a = runtime_a
            .submit_test_root(TestPreparedTask::yield_forever())
            .expect("root A admitted");
        // Opt this root into output capture exactly as `submit_root_with_options`
        // would for `RootOptions { capture_output: true, .. }` — this is the
        // one-liner that flips `write_stdout`/`write_stderr` into the capture
        // path for this root.
        sema_core::mark_root_capturing(handle_a.id);
        assert_eq!(sema_core::capturing_root_count(), 1);

        // Never drive `runtime_a` — root A's task stays parked forever
        // (`YieldForever`), so it can never become reap-eligible. Dropping
        // `handle_a` then `runtime_a` here exercises exactly the raw
        // `Runtime::drop` -> `close_for_interpreter_drop` path, which does
        // not drive a single quantum.
    }

    // A second `Runtime` on this same thread must not inherit runtime A's
    // abandoned capturing-root entry.
    let clock_b = Rc::new(FakeClock::new());
    let _runtime_b = runtime_with_inline_executor(clock_b);
    assert_eq!(
        sema_core::capturing_root_count(),
        0,
        "a fresh runtime must not inherit a dead runtime's stale capturing-root count"
    );
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

// P6-1 Task 2: `RuntimeCommandHandle`, the runtime's only `Send + Sync`
// control surface. These tests exercise the real cross-thread path (a
// spawned OS thread holding only the handle, never the `Runtime` itself,
// which is `!Send`) rather than calling `WaitRuntime` internals directly, so
// they cover the same wiring a host (CLI Ctrl-C, notebook cancel) will use.

#[test]
fn runtime_command_handle_cancel_root_lands_within_one_drive_turn() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let root = handle.id();
    let commands = runtime.command_handle();
    // Only the `Send + Sync` handle crosses the thread boundary — `Runtime`
    // itself holds an `Rc` and could not be moved here.
    let delivered = std::thread::spawn(move || commands.cancel_root(root))
        .join()
        .expect("spawned thread does not panic");
    assert!(
        delivered,
        "command channel is open while the runtime is alive"
    );
    // One drive turn: `apply_pending_commands` drains the queued command at
    // the top of `drive`, before source rotation, and routes it through the
    // existing `cancel_root` — same settlement path as a same-thread cancel.
    runtime
        .drive(&drive_budget(8))
        .expect("drive applies the queued command");
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("cancelled root should settle after one drive turn");
    };
    assert!(matches!(
        settlement.outcome,
        TaskOutcome::Cancelled(CancelReason::HostStop)
    ));
}

#[test]
fn runtime_command_handle_cancel_all_lands_within_one_drive_turn() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let a = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let b = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let commands = runtime.command_handle();
    let delivered = std::thread::spawn(move || commands.cancel_all())
        .join()
        .expect("spawned thread does not panic");
    assert!(delivered);
    runtime
        .drive(&drive_budget(8))
        .expect("drive applies the queued command");
    for handle in [&a, &b] {
        let RootPoll::Ready(settlement) = handle.poll_result() else {
            panic!("cancel_all settles every live root after one drive turn");
        };
        assert!(matches!(
            settlement.outcome,
            TaskOutcome::Cancelled(CancelReason::HostStop)
        ));
    }
}

#[test]
fn runtime_command_handle_cancel_root_settles_debug_paused_root_without_resume() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::debug_stop())
        .expect("root admitted");
    let root = handle.id();
    assert!(matches!(
        runtime
            .drive(&drive_budget(8))
            .expect("drive to debug stop"),
        super::DriveState::DebugStopped { .. }
    ));
    assert!(runtime.is_debug_paused());

    let commands = runtime.command_handle();
    assert!(std::thread::spawn(move || commands.cancel_root(root))
        .join()
        .expect("command thread does not panic"));
    runtime
        .drive(&drive_budget(8))
        .expect("ordinary root cancellation clears the debug barrier");

    assert!(!runtime.is_debug_paused());
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("debug-paused root settles without debug_resume")
    };
    assert!(matches!(
        settlement.outcome,
        TaskOutcome::Cancelled(CancelReason::HostStop)
    ));
}

#[test]
fn runtime_command_handle_cancel_root_settles_a_debug_paused_child() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let root = handle.id();
    let child = runtime.submit_test_child_under_root(root, TestPreparedTask::debug_stop());

    let mut turns = 0;
    loop {
        match runtime
            .drive(&drive_budget(1))
            .expect("drive reaches the child debug stop")
        {
            super::DriveState::DebugStopped { task, .. } => {
                assert_eq!(task, child, "the child, not the root main, is paused");
                break;
            }
            _ => {
                turns += 1;
                assert!(turns < 32, "child reaches its debug stop");
            }
        }
    }

    let commands = runtime.command_handle();
    assert!(std::thread::spawn(move || commands.cancel_root(root))
        .join()
        .expect("command thread does not panic"));
    let mut turns = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) || runtime.live_task_count() > 0 {
        runtime
            .drive(&drive_budget(8))
            .expect("command cancellation clears the child-owned debug barrier");
        turns += 1;
        assert!(turns < 32, "root main and paused child both settle");
    }

    assert!(!runtime.is_debug_paused());
    assert_eq!(runtime.live_task_count(), 0);
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("root settles after its paused child is cancelled")
    };
    assert!(matches!(
        settlement.outcome,
        TaskOutcome::Cancelled(CancelReason::HostStop)
    ));
}

#[test]
fn runtime_command_handle_cancel_all_settles_debug_paused_and_sibling_roots() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let paused = runtime
        .submit_test_root(TestPreparedTask::debug_stop())
        .expect("paused root admitted");
    let sibling = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("sibling root admitted");
    assert!(matches!(
        runtime
            .drive(&drive_budget(1))
            .expect("drive to debug stop"),
        super::DriveState::DebugStopped { .. }
    ));

    let commands = runtime.command_handle();
    assert!(std::thread::spawn(move || commands.cancel_all())
        .join()
        .expect("command thread does not panic"));
    let mut turns = 0;
    while [&paused, &sibling]
        .iter()
        .any(|handle| matches!(handle.poll_result(), RootPoll::Pending))
    {
        runtime
            .drive(&drive_budget(8))
            .expect("cancel_all clears the debug barrier and drains roots");
        turns += 1;
        assert!(turns < 20, "cancel_all settles every root");
    }

    assert!(!runtime.is_debug_paused());
    for handle in [&paused, &sibling] {
        let RootPoll::Ready(settlement) = handle.poll_result() else {
            panic!("cancel_all settles every root")
        };
        assert!(matches!(
            settlement.outcome,
            TaskOutcome::Cancelled(CancelReason::HostStop)
        ));
    }
}

#[test]
fn runtime_command_handle_outliving_runtime_returns_false_without_panic() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let root = handle.id();
    let commands = runtime.command_handle();
    drop(handle);
    drop(runtime);
    assert!(
        !commands.cancel_root(root),
        "cancel_root reports false once the runtime is gone"
    );
    assert!(
        !commands.cancel_all(),
        "cancel_all reports false once the runtime is gone"
    );
}

// ── CANCEL-ROOT-CASCADE-1 ───────────────────────────────────────────────
//
// `cancel_root` used to cancel only the root's main task and rely on the
// LIVE `cancellation_parent` chain (walked by `cancel_descendants`, the
// `async/cancel`/`CancelPromise` path — never called by `cancel_root`
// itself — plus the per-drive-turn `cancel_waiting` scan) to reach
// descendants. That chain is broken the moment an intermediate task
// settles and is removed from `state.tasks`: a fire-and-forget grandchild
// it spawned before returning is then unreachable from the root and leaks
// (runs to completion / stays parked forever in a persistent runtime).
// The fix sweeps every live task by `origin_root` instead — a field that
// survives an intermediate spawner's removal, unlike `cancellation_parent`.
//
// Helpers used below: `submit_test_child_under_root` (cancellation-parented
// directly on the root, the `async/spawn`-from-main shape) and the new
// `submit_test_child_under_task` (cancellation-parented on another TASK,
// the shape a detached descendant of an intermediate spawner has).

/// THE reproduction (CANCEL-ROOT-CASCADE-1): a fire-and-forget grandchild
/// whose spawning intermediate task has ALREADY SETTLED (and been removed
/// from `state.tasks`, breaking its `cancellation_parent` chain to the
/// root) must still be reaped by `cancel_root`. Pre-fix this asserts
/// `live_task_count() == 1` (the orphaned grandchild); post-fix, 0.
#[test]
fn cancel_root_reaps_a_fire_and_forget_grandchild_of_an_already_settled_task() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock);
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let root = handle.id();

    // Intermediate task: allocated but never inserted into `state.tasks` —
    // models a same-origin-root task that has ALREADY settled and been
    // reaped (a handler/task that spawned a detached child and returned).
    // `TestPreparedTask`'s synthetic settlement path only supports a root's
    // MAIN task (`self.settle`, which asserts `WrongMainTask` for anything
    // else — a real detached child settles via `settle_task`, which needs a
    // genuine VM closure to reach), so this test models "already settled
    // and gone" directly rather than driving a synthetic task through it.
    let intermediate = runtime.allocate_task_id_for_test();

    // Grandchild: cancellation-parented to the INTERMEDIATE (not the root),
    // parked on a far-future Timer that never fires on its own — models a
    // subprocess/request the intermediate detached before returning. Its
    // live `cancellation_parent` chain to the root is broken from the
    // moment it is created: `intermediate` was never a real task. A Timer
    // (not External) is deliberate: the `Inline` fake executor resolves an
    // External wait on its own within a handful of drive turns, which would
    // reap this task via its own NORMAL completion regardless of whether
    // `cancel_root`'s sweep works — silently defeating this test's RED/GREEN
    // power. A far-future Timer under a never-advanced `FakeClock`
    // genuinely never resolves except via cancellation.
    let _grandchild = runtime.submit_test_child_under_task(
        root,
        intermediate,
        TestPreparedTask::native(Ok(NativeOutcome::Suspend(far_future_timer_suspend()))),
    );

    // Drive with a work-item budget of 1 until the Timer wait is
    // registered, stopping EXACTLY there.
    while runtime.timer_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(
        runtime.live_task_count(),
        2,
        "main (yield-forever) + parked grandchild only"
    );

    assert!(
        runtime.cancel_root(root, CancelReason::Explicit),
        "cancel_root accepts the live root"
    );

    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 100, "cancelled root settles");
    }
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("cancelled root settles");
    };
    assert!(matches!(
        settlement.outcome,
        TaskOutcome::Cancelled(CancelReason::Explicit)
    ));
    assert_eq!(
        runtime.live_task_count(),
        0,
        "cancel_root must reap the fire-and-forget grandchild too — no leak \
         (CANCEL-ROOT-CASCADE-1)"
    );
}

#[test]
fn root_handle_cancel_reaps_a_fire_and_forget_grandchild() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let root = handle.id();
    let intermediate = runtime.allocate_task_id_for_test();
    let _grandchild = runtime.submit_test_child_under_task(
        root,
        intermediate,
        TestPreparedTask::native(Ok(NativeOutcome::Suspend(far_future_timer_suspend()))),
    );
    while runtime.timer_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(runtime.live_task_count(), 2, "main + parked grandchild");

    assert!(handle.cancel(CancelReason::Explicit));
    let mut turns = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) || runtime.live_task_count() > 0 {
        runtime.drive(&drive_budget(8)).unwrap();
        turns += 1;
        assert!(turns < 100, "handle cancellation settles the whole root");
    }

    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("cancelled root settles")
    };
    assert!(matches!(
        settlement.outcome,
        TaskOutcome::Cancelled(CancelReason::Explicit)
    ));
    assert_eq!(runtime.live_task_count(), 0);
}

/// Regression: a plain single-task root is still fully reaped.
#[test]
fn cancel_root_reaps_a_plain_single_task_root() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let root = handle.id();
    assert!(runtime.cancel_root(root, CancelReason::Explicit));
    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 50, "single-task root settles");
    }
    assert_eq!(runtime.live_task_count(), 0);
}

/// Regression: a root with a directly root-parented parked sibling (the
/// already-working shape, reachable from main WITHOUT needing the
/// `origin_root` sweep) stays fully reaped.
#[test]
fn cancel_root_reaps_a_directly_parked_sibling_child() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let root = handle.id();
    // A far-future Timer (never fires under a never-advanced `FakeClock`),
    // not External — the `Inline` fake executor resolves an External wait
    // on its own within a few drive turns regardless of cancellation.
    let _child = runtime.submit_test_child_under_root(
        root,
        TestPreparedTask::native(Ok(NativeOutcome::Suspend(far_future_timer_suspend()))),
    );
    while runtime.timer_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(runtime.live_task_count(), 2, "main + parked sibling");

    assert!(runtime.cancel_root(root, CancelReason::Explicit));
    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) || runtime.live_task_count() > 0 {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 100, "root with parked sibling settles and drains");
    }
    assert_eq!(
        runtime.live_task_count(),
        0,
        "parked sibling reaped alongside main"
    );
}

/// Regression: cancelling an unknown/already-settled root still returns
/// `false` (unchanged contract).
#[test]
fn cancel_root_on_a_settled_root_returns_false() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(1)))
        .expect("root admitted");
    let root = handle.id();
    runtime.drive(&drive_budget(8)).unwrap();
    assert!(
        matches!(handle.poll_result(), RootPoll::Ready(_)),
        "root settles on its own"
    );
    assert!(
        !runtime.cancel_root(root, CancelReason::Explicit),
        "cancel_root on an already-settled root is a no-op"
    );
}

/// Regression: `cancel_root` is idempotent — a second call on an
/// already-cancelled root returns `false` and does not double-tear-down the
/// swept descendant (would otherwise panic/misbehave in
/// `deliver_cancel_teardown`).
#[test]
fn cancel_root_is_idempotent_second_call_returns_false_no_panic() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let root = handle.id();
    let intermediate = runtime.allocate_task_id_for_test();
    // A far-future Timer (never fires under a never-advanced `FakeClock`),
    // not External — see `cancel_root_reaps_a_directly_parked_sibling_child`.
    let _grandchild = runtime.submit_test_child_under_task(
        root,
        intermediate,
        TestPreparedTask::native(Ok(NativeOutcome::Suspend(far_future_timer_suspend()))),
    );
    while runtime.timer_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(runtime.live_task_count(), 2, "main + parked grandchild");

    assert!(
        runtime.cancel_root(root, CancelReason::Explicit),
        "first cancel newly cancels"
    );
    assert!(
        !runtime.cancel_root(root, CancelReason::Timeout),
        "second cancel is a no-op"
    );
    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) || runtime.live_task_count() > 0 {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 100, "root settles once and every task drains");
    }
    assert_eq!(runtime.live_task_count(), 0);
}

/// CRITICAL — multi-root isolation: the `origin_root` sweep must not
/// over-reach into a sibling root's tasks. Cancelling root A (which has the
/// same orphaned-grandchild shape as the headline repro) must leave root
/// B's independent, still-live detached task fully untouched (never
/// cancelled, never reaped) while B keeps running/settling normally. Both
/// descendants park on a far-future `Timer` (not External — the `Inline`
/// fake executor resolves an External wait on its own after a few drive
/// turns, which would make root B's "stays alive" half of this test flaky
/// against the number of turns root A's settlement happens to take; a
/// `Timer` under a never-advanced `FakeClock` genuinely never fires).
#[test]
fn cancel_root_sweep_does_not_reach_a_sibling_roots_tasks() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock);

    // Root A: main (yield-forever) + intermediate (already gone) +
    // grandchild (parked, cancellation-parented to the now-settled
    // intermediate) — the same orphaned shape as the headline repro.
    let handle_a = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root A admitted");
    let root_a = handle_a.id();
    let intermediate_a = runtime.allocate_task_id_for_test();
    let _grandchild_a = runtime.submit_test_child_under_task(
        root_a,
        intermediate_a,
        TestPreparedTask::native(Ok(NativeOutcome::Suspend(far_future_timer_suspend()))),
    );

    // Root B: fully independent. Main settles immediately; a directly
    // root-parented sibling stays parked on its own Timer wait.
    let handle_b = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(99)))
        .expect("root B admitted");
    let root_b = handle_b.id();
    let _sibling_b = runtime.submit_test_child_under_root(
        root_b,
        TestPreparedTask::native(Ok(NativeOutcome::Suspend(far_future_timer_suspend()))),
    );

    // Drive until root A's grandchild has parked and root B's main has
    // settled (root B's detached sibling does NOT block root B's
    // settlement — a detached child never transitions its root).
    let mut guard = 0;
    while runtime.live_task_count() > 3 || matches!(handle_b.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 100, "two-root setup converges");
    }
    assert_eq!(
        runtime.live_task_count(),
        3,
        "main_a (yield-forever) + grandchild_a (parked) + sibling_b (parked)"
    );
    assert!(
        matches!(handle_b.poll_result(), RootPoll::Ready(_)),
        "root B settles independently of root A's setup"
    );

    assert!(runtime.cancel_root(root_a, CancelReason::Explicit));
    let mut guard = 0;
    while matches!(handle_a.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 100, "root A settles");
    }

    assert_eq!(
        runtime.live_task_count(),
        1,
        "root A's main + grandchild reaped; root B's detached sibling left \
         completely alone"
    );
    let RootPoll::Ready(settlement_b) = handle_b.poll_result() else {
        panic!("root B stays settled")
    };
    assert!(
        matches!(&settlement_b.outcome, TaskOutcome::Returned(v) if v.as_int() == Some(99)),
        "root B's own settlement is unperturbed by an unrelated root's cancel"
    );
}

/// Double-teardown safety: a grandchild parked on a real External wait,
/// reached BOTH by the new `origin_root` sweep (which pushes it onto
/// `pending_cancel_waits` and delivers C2 eager teardown) and by the
/// existing per-drive-turn `cancel_waiting` scan, must have its abort hook
/// fire EXACTLY ONCE — `deliver_cancel_teardown` removes the wait
/// registration itself, so the scan finds nothing left to double-abort.
#[test]
fn cancel_root_sweep_aborts_an_external_grandchild_exactly_once() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::yield_forever())
        .expect("root admitted");
    let root = handle.id();
    let intermediate = runtime.allocate_task_id_for_test();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let _grandchild = runtime.submit_test_child_under_task(
        root,
        intermediate,
        TestPreparedTask::native(Ok(NativeOutcome::Suspend(interruptible_suspend_with_hook(
            events,
            RecordingHook {
                result: CancelResult::Reaped,
                calls: Arc::clone(&calls),
                edge: None,
                trace_ok: true,
            },
        )))),
    );
    // Drive with a work-item budget of 1 until the External wait is
    // registered, stopping EXACTLY there — a bigger budget can also drain
    // the (Inline-executor-resolved) completion in the same turn, which
    // would resume the grandchild normally before cancellation ever
    // observes it as `Waiting` (mirrors
    // `cancelling_external_parked_task_runs_abort_hook_at_request_time`'s
    // proven pattern above).
    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert_eq!(runtime.live_task_count(), 2, "main + parked grandchild");
    assert!(
        calls.lock().unwrap().is_empty(),
        "the abort hook must not fire before cancellation"
    );

    assert!(runtime.cancel_root(root, CancelReason::Explicit));
    // Keep driving (letting the per-drive-turn `cancel_waiting` scan run
    // repeatedly) to prove it never re-aborts what the sweep's eager
    // teardown already tore down.
    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) || runtime.live_task_count() > 0 {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 100, "root settles and every task drains");
    }
    assert_eq!(runtime.live_task_count(), 0);
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["cancel"],
        "the abort hook must fire exactly once, not double-aborted by the \
         drive-turn scan"
    );
}

#[test]
fn runtime_command_wakes_a_thread_parked_in_block_on_inbox() {
    // The driving thread parks in `block_on_inbox` (as `drive_vm_on_runtime`
    // does when `DriveState::Idle { inbox_wakeup_required: true, .. }`) with
    // no deadline — it can only return via a channel arrival. A command sent
    // from another thread must wake it, since `CommandChannel` rides the same
    // channel and dirty flag as an executor completion.
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let commands = runtime.command_handle();
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        assert!(commands.cancel_all());
    });
    let start = Instant::now();
    let woke = runtime.block_on_inbox(None);
    let elapsed = start.elapsed();
    sender.join().expect("spawned thread does not panic");
    assert!(woke, "block_on_inbox returns true when a command arrives");
    assert!(
        elapsed < Duration::from_secs(1),
        "command should wake block_on_inbox promptly, took {elapsed:?}"
    );
}

#[test]
fn block_on_inbox_returns_immediately_when_a_command_is_already_buffered() {
    // Reproduces the buffer-then-park bug: a command that arrives mid-turn is
    // pumped off the channel into `self.commands` (not `self.deferred`) and
    // the dirty flag is cleared, so nothing is left on the channel for a
    // *subsequent* `block_on_inbox` call to observe. The first call below
    // blocks until the command arrives and buffers it; the second call must
    // see that already-buffered command and return `true` immediately,
    // instead of blocking out its full deadline waiting on a channel that
    // will never receive anything else.
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let commands = runtime.command_handle();
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        assert!(commands.cancel_all());
    });
    let woke_first = runtime.block_on_inbox(None);
    sender.join().expect("spawned thread does not panic");
    assert!(woke_first, "first call wakes when the command arrives");

    let start = Instant::now();
    let woke_second = runtime.block_on_inbox(Some(start + Duration::from_millis(400)));
    let elapsed = start.elapsed();
    assert!(
        woke_second,
        "second call must see the already-buffered command and return true"
    );
    assert!(
        elapsed < Duration::from_millis(100),
        "buffered command should make block_on_inbox return immediately, took {elapsed:?}"
    );
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
fn root_filtered_drive_does_not_invoke_a_foreign_pending_call() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let native_events = Arc::clone(&events);
    let foreign = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
            NativeCall {
                callable: Value::native_fn(sema_core::NativeFn::simple_result(
                    "foreign-pending-native",
                    move |_| {
                        native_events.lock().unwrap().push("foreign-invoke");
                        Ok(NativeOutcome::Return(Value::int(8)))
                    },
                )),
                args: Vec::new(),
                continuation: Box::new(RecordingContinuation(Arc::clone(&events))),
            },
        ))))
        .expect("foreign root admitted");
    let owned = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(42)))
        .expect("owned root admitted");
    let one = drive_budget(1);

    // Advance the foreign root through Action -> Apply, leaving its Invoke as
    // an already-staged item at the head of the runtime-wide pending queue.
    for _ in 0..3 {
        runtime.drive_roots(&one, &[foreign.id()]).unwrap();
    }
    assert!(events.lock().unwrap().is_empty());

    while matches!(owned.poll_result(), RootPoll::Pending) {
        runtime.drive_roots(&one, &[owned.id()]).unwrap();
    }
    assert_eq!(
        *events.lock().unwrap(),
        Vec::<&'static str>::new(),
        "an owned drive must leave the foreign Invoke staged"
    );

    while matches!(foreign.poll_result(), RootPoll::Pending) {
        runtime.drive_roots(&one, &[foreign.id()]).unwrap();
    }
    assert_eq!(
        *events.lock().unwrap(),
        vec!["foreign-invoke", "resume-returned"]
    );
}

#[test]
fn panicking_runtime_native_restores_quantum_thread_local() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let invoked = Rc::new(Cell::new(false));
    let observed = Rc::clone(&invoked);
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
            NativeCall {
                callable: Value::native_fn(sema_core::NativeFn::simple_result(
                    "panicking-native",
                    move |_| {
                        observed.set(true);
                        assert!(sema_core::in_runtime_quantum());
                        panic!("test native panic");
                    },
                )),
                args: Vec::new(),
                continuation: Box::new(RecordingContinuation(Arc::new(Mutex::new(Vec::new())))),
            },
        ))))
        .unwrap();
    assert!(!sema_core::in_runtime_quantum());

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        while !invoked.get() {
            runtime.drive(&drive_budget(1)).unwrap();
        }
    }));

    assert!(panic.is_err());
    assert!(invoked.get());
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    assert!(!sema_core::in_runtime_quantum());
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

struct ForwardReturnedValue;

impl Trace for ForwardReturnedValue {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ForwardReturnedValue {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => {
                Err(SemaError::eval(format!("call was cancelled ({reason:?})")))
            }
            ResumeInput::Runtime(_) => Err(SemaError::eval("unexpected runtime response")),
        }
    }
}

struct ReturnAfterTimer;

impl Trace for ReturnAfterTimer {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ReturnAfterTimer {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        assert!(matches!(input, ResumeInput::Returned(_)));
        Ok(NativeOutcome::Return(Value::int(73)))
    }
}

struct ReturnDispatchKeyAfterTimer;

impl Trace for ReturnDispatchKeyAfterTimer {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ReturnDispatchKeyAfterTimer {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        assert!(matches!(input, ResumeInput::Returned(_)));
        Ok(NativeOutcome::Return(Value::keyword("selected")))
    }
}

#[test]
fn native_call_multimethod_dispatch_and_selected_handler_suspend() {
    let clock = Rc::new(FakeClock::new());
    let context = Rc::new(sema_core::EvalContext::new());
    let runtime = Runtime::new(
        context,
        clock.clone(),
        Arc::new(FakeExecutor {
            mode: FakeSubmit::Inline,
            failure: None,
        }),
    )
    .expect("runtime");

    let dispatch = Value::native_fn(sema_core::NativeFn::simple_result(
        "test/multimethod-dispatch",
        |_| {
            Ok(NativeOutcome::Suspend(NativeSuspend {
                wait: WaitKind::Timer(Duration::from_millis(3)),
                continuation: Box::new(ReturnDispatchKeyAfterTimer),
            }))
        },
    ));
    let selected = Value::native_fn(sema_core::NativeFn::simple_result(
        "test/suspending-method",
        |_| {
            Ok(NativeOutcome::Suspend(NativeSuspend {
                wait: WaitKind::Timer(Duration::from_millis(5)),
                continuation: Box::new(ReturnAfterTimer),
            }))
        },
    ));
    let mut methods = std::collections::BTreeMap::new();
    methods.insert(Value::keyword("selected"), selected);
    let multimethod = Value::multimethod(sema_core::MultiMethod {
        name: sema_core::intern("test/runtime-multimethod"),
        dispatch_fn: dispatch,
        methods: RefCell::new(methods),
        default: RefCell::new(None),
    });
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
            NativeCall {
                callable: multimethod,
                args: vec![Value::int(1)],
                continuation: Box::new(ForwardReturnedValue),
            },
        ))))
        .expect("root admitted");

    runtime.drive(&drive_budget(32)).expect("park dispatch");
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    assert_eq!(runtime.timer_count_for_test(), 1);

    clock.advance(Duration::from_millis(3));
    runtime.drive(&drive_budget(32)).expect("park method");
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    assert_eq!(runtime.timer_count_for_test(), 1);

    clock.advance(Duration::from_millis(5));
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).expect("settle method");
    }
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("multimethod call settles");
    };
    assert!(matches!(
        settlement.outcome,
        TaskOutcome::Returned(ref value) if value.as_int() == Some(73)
    ));
}

/// `channel/send`/`channel/recv`'s continuation for the in-place handoff fast
/// path (`try_channel_handoff`, state.rs) — `SendCont`/`RecvCont` in
/// `sema-stdlib`'s `async_ops.rs` — only ever `resume()` to
/// `Ok(NativeOutcome::Return(_))` or `Err(_)`, so no Sema-source channel op
/// can drive the `ChannelHandoffOutcome::Deferred(other)` arm (state.rs:2999):
/// a resumed continuation that itself composes into a further
/// `Call`/`Suspend`/`Runtime` instead of settling outright. Pin that cold
/// path directly with a synthetic `NativeContinuation` that does compose
/// further (into a second, external suspend) — the only way to reach it.
struct ChannelDeferContinuation(Arc<Mutex<Vec<&'static str>>>);

impl Trace for ChannelDeferContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ChannelDeferContinuation {
    fn resume(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        assert!(
            matches!(
                input,
                ResumeInput::Runtime(sema_core::runtime::RuntimeResponse::Send(
                    sema_core::runtime::ChannelSend::Sent
                ))
            ),
            "channel handoff must resume the continuation with a Sent runtime response"
        );
        let key = Value::keyword("inline-channel-context-seam");
        assert_eq!(
            context.eval_context.context_get(&key),
            Some(Value::keyword("native")),
            "inline channel continuation observes the installed task state"
        );
        context
            .eval_context
            .context_set(key, Value::keyword("continuation"));
        self.0.lock().unwrap().push("channel-composed");
        // Compose into a further suspend instead of returning/erroring — this
        // is exactly what makes `resume_this_inline`'s match fall into the
        // `other => ChannelHandoffOutcome::Deferred(other)` arm.
        Ok(NativeOutcome::Suspend(external_suspend(Arc::clone(
            &self.0,
        ))))
    }
}

fn channel_defer_native(
    channel: sema_core::runtime::ChannelId,
    events: Arc<Mutex<Vec<&'static str>>>,
) -> sema_core::NativeFn {
    sema_core::NativeFn::with_context_result("chan-send-defer", move |context, _args| {
        context.eval_context.context_set(
            Value::keyword("inline-channel-context-seam"),
            Value::keyword("native"),
        );
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Send {
                channel,
                value: Value::int(1),
            }),
            continuation: Box::new(ChannelDeferContinuation(Arc::clone(&events))),
        }))
    })
}

#[test]
fn channel_handoff_continuation_composing_further_takes_the_deferred_cold_path() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    // Capacity 1, freshly allocated: `would_resolve_immediately` says yes for
    // a send with no receiver required, so the quantum-loop's fast path in
    // `run_parked_quantum` reaches `try_channel_handoff` instead of genuinely
    // parking.
    let channel = runtime.create_channel_for_test(1);
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    let globals = Rc::new(sema_core::Env::new());
    globals.set(
        sema_core::intern("chan-send-defer"),
        Value::native_fn(channel_defer_native(channel, Arc::clone(&events))),
    );
    let known = [sema_core::intern("chan-send-defer")].into_iter().collect();
    let forms = sema_reader::read_many("(lambda () (chan-send-defer))").unwrap();
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

    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
            NativeCall {
                callable,
                args: Vec::new(),
                continuation: Box::new(RecordingContinuation(Arc::clone(&events))),
            },
        ))))
        .unwrap();

    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(
            guard < 100,
            "deferred channel handoff composition did not settle"
        );
    }
    assert!(matches!(
        handle.poll_result(),
        RootPoll::Ready(settlement)
            if matches!(settlement.outcome, TaskOutcome::Returned(ref value) if value.as_int() == Some(9))
    ));
    assert_eq!(
        *events.lock().unwrap(),
        vec!["channel-composed", "decode", "returned", "resume-returned"],
        "the Deferred outcome must drive its composed external suspend to \
         completion and resume the outer VM call — proving the cold path is \
         handled, not just matched"
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
    // The continuation observed cancellation and returned; shutdown cancellation
    // cannot be defeated by that return, so the task settles Cancelled.
    assert!(matches!(&settlement.outcome, TaskOutcome::Cancelled(_)));
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
fn external_completion_after_cancellation_request_settles_cancelled_not_returned() {
    // Regression: a cancellation requested after an external completion has been
    // drained (task Ready with a pending resume) but before the task is visited
    // must still be observed by the continuation and settle the task Cancelled.
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_suspend(Arc::clone(&events)),
        ))))
        .expect("root admitted");
    // Register the external wait; the inline job's completion now sits in the
    // inbox while the task is still Waiting.
    runtime.drive(&drive_budget(2)).expect("register wait");
    assert!(matches!(handle.poll_result(), RootPoll::Pending));

    // Cancellation is requested while that completion is still queued.
    handle.cancel(CancelReason::Explicit);

    // Draining the completion wakes the task; the resume must reconcile the
    // sticky cancellation rather than resuming with the stale completion value.
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
    }
    let RootPoll::Ready(settlement) = handle.poll_result() else {
        panic!("cancelled root settles");
    };
    assert!(
        matches!(settlement.outcome, TaskOutcome::Cancelled(_)),
        "cancelled task must not settle Returned: {:?}",
        settlement.outcome
    );
    assert_eq!(
        events.lock().unwrap().last().copied(),
        Some("cancelled"),
        "continuation observed the wrong resume input: {:?}",
        events.lock().unwrap()
    );
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

struct PendingExecutor;

impl IoExecutor for PendingExecutor {
    fn attach_runtime(
        &self,
        _runtime_id: RuntimeId,
    ) -> Result<Arc<dyn ExecutorLease>, ExecutorAttachError> {
        Ok(Arc::new(PendingLease::default()))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }
}

#[derive(Default)]
struct PendingLease {
    submissions: Mutex<Vec<ExecutorDispatch>>,
}

impl ExecutorLease for PendingLease {
    fn submit(
        &self,
        submission: sema_core::runtime::ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        let operation = submission.operation_id();
        self.submissions
            .lock()
            .unwrap()
            .push(submission.into_dispatch());
        Ok(RunningSubmission::new(operation))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }

    fn shutdown(&self, _deadline: Instant) -> ExecutorShutdown {
        ExecutorShutdown::Drained(self.snapshot())
    }
}

struct CountingDecoder(Arc<Mutex<Vec<&'static str>>>);
impl Trace for CountingDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

struct ContextIdentityDecoder(Rc<RefCell<Vec<(&'static str, usize)>>>);

impl Trace for ContextIdentityDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl CompletionDecoder for ContextIdentityDecoder {
    fn decode(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        _result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> Result<Value, SemaError> {
        let key = Value::keyword("runtime-context-seam");
        assert_eq!(
            context.eval_context.context_get(&key),
            Some(Value::keyword("native")),
            "external decoder observes the task state while NativeCallContext owns its mutable loan"
        );
        context
            .eval_context
            .context_set(key, Value::keyword("decoder"));
        self.0.borrow_mut().push((
            "decode",
            context.eval_context as *const sema_core::EvalContext as usize,
        ));
        Ok(Value::NIL)
    }
}

struct ContextIdentityContinuation {
    stage: &'static str,
    seen: Rc<RefCell<Vec<(&'static str, usize)>>>,
}

impl Trace for ContextIdentityContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ContextIdentityContinuation {
    fn resume(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        assert!(matches!(input, ResumeInput::Returned(_)));
        let key = Value::keyword("runtime-context-seam");
        let (expected, next) = match self.stage {
            "external-resume" => ("decoder", "external-continuation"),
            "outer-resume" => ("external-continuation", "outer-continuation"),
            stage => panic!("unexpected context-identity continuation stage: {stage}"),
        };
        assert_eq!(
            context.eval_context.context_get(&key),
            Some(Value::keyword(expected)),
            "continuation observes the task state while NativeCallContext owns its mutable loan"
        );
        context.eval_context.context_set(key, Value::keyword(next));
        self.seen.borrow_mut().push((
            self.stage,
            context.eval_context as *const sema_core::EvalContext as usize,
        ));
        Ok(NativeOutcome::Return(Value::NIL))
    }
}

fn context_identity_suspend(seen: Rc<RefCell<Vec<(&'static str, usize)>>>) -> NativeSuspend {
    NativeSuspend {
        wait: WaitKind::External(Box::new(PreparedExternalOperation::quarantined_blocking(
            CompletionKind::try_from_raw(91).unwrap(),
            Box::new(ContextIdentityDecoder(Rc::clone(&seen))),
            sema_core::runtime::QuarantineBound::hard_deadline(Duration::from_secs(1)).unwrap(),
            || Ok(Box::new(())),
        ))),
        continuation: Box::new(ContextIdentityContinuation {
            stage: "external-resume",
            seen,
        }),
    }
}

#[test]
fn runtime_native_external_decoder_and_continuations_keep_owning_eval_context() {
    let context_a = Rc::new(sema_core::EvalContext::new());
    let context_b = Rc::new(sema_core::EvalContext::new());
    let expected_a = Rc::as_ptr(&context_a) as usize;
    let expected_b = Rc::as_ptr(&context_b) as usize;
    let runtime_a = Runtime::new(
        Rc::clone(&context_a),
        Rc::new(FakeClock::new()),
        Arc::new(FakeExecutor {
            mode: FakeSubmit::Inline,
            failure: None,
        }),
    )
    .expect("runtime A");
    let runtime_b = Runtime::new(
        Rc::clone(&context_b),
        Rc::new(FakeClock::new()),
        Arc::new(FakeExecutor {
            mode: FakeSubmit::Inline,
            failure: None,
        }),
    )
    .expect("runtime B");

    let submit = |runtime: &Runtime| {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let native_seen = Rc::clone(&seen);
        let suspend_seen = Rc::clone(&seen);
        let native = Value::native_fn(sema_core::NativeFn::with_context_result(
            "context-identity",
            move |context, _| {
                let key = Value::keyword("runtime-context-seam");
                assert_eq!(context.eval_context.context_get(&key), None);
                context
                    .eval_context
                    .context_set(key, Value::keyword("native"));
                native_seen.borrow_mut().push((
                    "native",
                    context.eval_context as *const sema_core::EvalContext as usize,
                ));
                Ok(NativeOutcome::Suspend(context_identity_suspend(Rc::clone(
                    &suspend_seen,
                ))))
            },
        ));
        let handle = runtime
            .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Call(
                NativeCall {
                    callable: native,
                    args: Vec::new(),
                    continuation: Box::new(ContextIdentityContinuation {
                        stage: "outer-resume",
                        seen: Rc::clone(&seen),
                    }),
                },
            ))))
            .expect("root admitted");
        (handle, seen)
    };
    let (handle_a, seen_a) = submit(&runtime_a);
    let (handle_b, seen_b) = submit(&runtime_b);

    while matches!(handle_a.poll_result(), RootPoll::Pending)
        || matches!(handle_b.poll_result(), RootPoll::Pending)
    {
        runtime_b.drive(&drive_budget(1)).expect("runtime B drives");
        runtime_a.drive(&drive_budget(1)).expect("runtime A drives");
    }

    for (seen, expected) in [(&seen_a, expected_a), (&seen_b, expected_b)] {
        assert_eq!(
            &*seen.borrow(),
            &[
                ("native", expected),
                ("decode", expected),
                ("external-resume", expected),
                ("outer-resume", expected),
            ]
        );
    }
    for context in [&context_a, &context_b] {
        assert_eq!(
            context.context_get(&Value::keyword("runtime-context-seam")),
            Some(Value::keyword("outer-continuation")),
            "the root publishes the state touched by every external seam"
        );
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

struct ExternalCallContinuation(Arc<Mutex<Vec<&'static str>>>);

impl Trace for ExternalCallContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ExternalCallContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        assert!(matches!(input, ResumeInput::Returned(_)));
        let native_events = Arc::clone(&self.0);
        Ok(NativeOutcome::Call(NativeCall {
            callable: Value::native_fn(sema_core::NativeFn::simple_result(
                "external-foreign-native",
                move |_| {
                    native_events.lock().unwrap().push("external-invoke");
                    Ok(NativeOutcome::Return(Value::int(8)))
                },
            )),
            args: Vec::new(),
            continuation: Box::new(RecordingContinuation(self.0)),
        }))
    }
}

fn external_call_suspend(events: Arc<Mutex<Vec<&'static str>>>) -> NativeSuspend {
    NativeSuspend {
        wait: WaitKind::External(Box::new(PreparedExternalOperation::quarantined_blocking(
            CompletionKind::try_from_raw(92).unwrap(),
            Box::new(CountingDecoder(Arc::clone(&events))),
            sema_core::runtime::QuarantineBound::hard_deadline(Duration::from_secs(1)).unwrap(),
            || Ok(Box::new(())),
        ))),
        continuation: Box::new(ExternalCallContinuation(events)),
    }
}

#[test]
fn root_filtered_drive_stages_but_does_not_decode_a_foreign_external_completion() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let foreign = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            external_call_suspend(Arc::clone(&events)),
        ))))
        .expect("foreign root admitted");
    let owned = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(42)))
        .expect("owned root admitted");
    let one = drive_budget(1);

    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive_roots(&one, &[foreign.id()]).unwrap();
    }
    while matches!(owned.poll_result(), RootPoll::Pending) {
        runtime.drive_roots(&one, &[owned.id()]).unwrap();
    }
    assert!(
        events.lock().unwrap().is_empty(),
        "the owned drive may stage a foreign completion but cannot decode or resume it"
    );

    while matches!(foreign.poll_result(), RootPoll::Pending) {
        runtime.drive_roots(&one, &[foreign.id()]).unwrap();
    }
    assert_eq!(
        *events.lock().unwrap(),
        vec!["decode", "external-invoke", "resume-returned"]
    );
}

/// A `WaitKind::Timer` parked FAR in the future under a `FakeClock` that is
/// never advanced — unlike an External wait (which the `Inline` fake
/// executor resolves on its own after a few drive turns, making it
/// unsuitable for a test that needs a descendant to stay parked across MANY
/// turns), this genuinely never fires on its own, however long the test
/// keeps driving. Used where a test needs a same-origin-root task to stay
/// alive-and-parked for the duration, not to prove the External-wait abort
/// hook specifically (that is `interruptible_suspend_with_hook`'s job).
fn far_future_timer_suspend() -> NativeSuspend {
    NativeSuspend {
        wait: WaitKind::Timer(Duration::from_secs(3600)),
        continuation: Box::new(TimerResumeContinuation {
            observed: Rc::new(RefCell::new(Vec::new())),
            value: Value::NIL,
        }),
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

// --- DECISION C2: eager cancellation delivery ------------------------------

/// (a) Cancelling a task parked on an External wait must run the offloaded job's
/// executor/resource abort hook SYNCHRONOUSLY at cancellation-REQUEST time, not
/// deferred to a later per-drive-turn scan — so a settled root followed by
/// process exit never leaves an in-flight subprocess/request un-aborted
/// (ASYNC-TIMEOUT-CANCEL-1). The `RecordingHook` records exactly when the abort
/// fires; it must be recorded by `handle.cancel` itself, before any further drive.
#[test]
fn cancelling_external_parked_task_runs_abort_hook_at_request_time() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock);
    let events = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let hook = RecordingHook {
        result: CancelResult::PendingReap,
        calls: Arc::clone(&calls),
        edge: None,
        trace_ok: true,
    };
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            interruptible_suspend_with_hook(Arc::clone(&events), hook),
        ))))
        .expect("root admitted");
    // Drive only until the external wait is registered — the task is now Waiting
    // on its in-flight offloaded job (do NOT drain the completion yet).
    while runtime.active_wait_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    assert!(
        calls.lock().unwrap().is_empty(),
        "the abort hook must not fire before cancellation"
    );
    // Request cancellation: the abort hook must fire RIGHT NOW, synchronously.
    assert!(handle.cancel(CancelReason::Explicit));
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["cancel"],
        "the in-flight job's abort hook must fire at cancellation-REQUEST time"
    );
    assert_eq!(
        runtime.active_wait_count_for_test(),
        0,
        "the external wait is deregistered exactly once at request time"
    );
}

struct GateHoldOwner {
    gate_slot: Rc<Cell<Option<sema_core::runtime::ResourceGateId>>>,
    events: Arc<Mutex<Vec<String>>>,
    stage: u8,
    gate: Option<sema_core::runtime::ResourceGateId>,
}
impl Trace for GateHoldOwner {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}
impl NativeContinuation for GateHoldOwner {
    fn resume(
        mut self: Box<Self>,
        _: &mut NativeCallContext<'_>,
        _input: ResumeInput,
    ) -> NativeResult {
        match self.stage {
            // Stage 0: the freshly-created gate arrived — record it, then acquire.
            0 => {
                let ResumeInput::Runtime(sema_core::runtime::RuntimeResponse::ResourceGate(handle)) =
                    _input
                else {
                    panic!("gate owner stage 0 expected a ResourceGate response");
                };
                let gate = handle.id();
                self.gate_slot.set(Some(gate));
                self.gate = Some(gate);
                self.stage = 1;
                Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::ResourceSlot(gate),
                    continuation: self,
                }))
            }
            // Stage 1: acquired the free gate — park on a long Timer to HOLD it
            // while the two waiters queue behind us (a Timer never fires until the
            // clock is advanced, and produces no completion to accidentally drain,
            // so we control exactly when the gate is released).
            1 => {
                self.stage = 2;
                Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::Timer(Duration::from_secs(3600)),
                    continuation: self,
                }))
            }
            // Stage 2: the hold timer fired — release the gate (granting the FIFO head).
            _ => {
                self.events.lock().unwrap().push("A-released".to_string());
                Ok(NativeOutcome::Runtime(
                    sema_core::runtime::RuntimeRequest::ReleaseResourceGate {
                        gate: self.gate.expect("owner knows its gate"),
                        continuation: Box::new(GateDone),
                    },
                ))
            }
        }
    }
}

struct GateDone;
impl Trace for GateDone {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}
impl NativeContinuation for GateDone {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, _: ResumeInput) -> NativeResult {
        Ok(NativeOutcome::Return(Value::NIL))
    }
}

struct RecordCreatedGate(Rc<Cell<Option<sema_core::runtime::ResourceGateId>>>);

impl Trace for RecordCreatedGate {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for RecordCreatedGate {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, input: ResumeInput) -> NativeResult {
        if let ResumeInput::Runtime(sema_core::runtime::RuntimeResponse::ResourceGate(handle)) =
            input
        {
            self.0.set(Some(handle.id()));
        }
        Ok(NativeOutcome::Return(Value::NIL))
    }
}

/// A cancellation recorded after gate allocation but before its allocation
/// response reaches the caller must not strand an unnameable registry entry.
#[test]
fn cancelling_between_resource_gate_allocation_and_store_closes_the_gate() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let delivered = Rc::new(Cell::new(None));
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::CreateResourceGate {
                continuation: Box::new(RecordCreatedGate(Rc::clone(&delivered))),
            },
        ))))
        .expect("gate creator admitted");

    let mut turns = 0;
    while runtime.resource_gate_count_for_test() == 0 {
        runtime.drive(&drive_budget(1)).expect("allocate gate");
        turns += 1;
        assert!(turns < 32, "gate allocation must be staged");
    }
    assert_eq!(
        delivered.get(),
        None,
        "allocation response is still pending"
    );

    assert!(handle.cancel(CancelReason::Explicit));
    while matches!(handle.poll_result(), RootPoll::Pending) {
        runtime
            .drive(&drive_budget(1))
            .expect("settle cancellation");
    }

    assert_eq!(
        delivered.get(),
        None,
        "cancelled caller never stores the gate"
    );
    assert_eq!(
        runtime.resource_gate_count_for_test(),
        0,
        "the allocation delivery guard closes its owned gate"
    );
}

#[test]
fn resource_gate_handle_reports_a_dropped_runtime_instead_of_hiding_it() {
    let handle = {
        let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
        let handle = runtime.create_resource_gate_handle_for_test();
        assert_eq!(runtime.resource_gate_count(), 1);
        handle
    };

    assert_eq!(
        handle.close(),
        Err(sema_core::runtime::ResourceGateCloseError::RuntimeUnavailable)
    );
}

struct RecordGateGrant {
    label: &'static str,
    events: Arc<Mutex<Vec<String>>>,
}
impl Trace for RecordGateGrant {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}
impl NativeContinuation for RecordGateGrant {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, input: ResumeInput) -> NativeResult {
        let outcome = match input {
            ResumeInput::Runtime(sema_core::runtime::RuntimeResponse::Value(_)) => "granted",
            ResumeInput::Cancelled(_) => "cancelled",
            ResumeInput::Failed(_) => "failed",
            _ => "other",
        };
        self.events
            .lock()
            .unwrap()
            .push(format!("{}-{}", self.label, outcome));
        Ok(NativeOutcome::Return(Value::NIL))
    }
}

#[test]
fn command_cancellation_settles_resource_waiter_after_close_wake_is_committed() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock);
    let events = Arc::new(Mutex::new(Vec::new()));
    let gate_slot = Rc::new(Cell::new(None));

    let owner = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::CreateResourceGate {
                continuation: Box::new(GateHoldOwner {
                    gate_slot: Rc::clone(&gate_slot),
                    events: Arc::clone(&events),
                    stage: 0,
                    gate: None,
                }),
            },
        ))))
        .expect("gate owner admitted");
    while gate_slot.get().is_none()
        || runtime
            .resource_gate_owner_for_test(gate_slot.get().expect("gate created"))
            .is_none()
        || runtime.timer_count_for_test() == 0
    {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    let gate = gate_slot.get().expect("gate created");

    let waiter = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::ResourceSlot(gate),
                continuation: Box::new(RecordGateGrant {
                    label: "waiter",
                    events: Arc::clone(&events),
                }),
            },
        ))))
        .expect("gate waiter admitted");
    while runtime.protocol_wait_count_for_test() < 2 {
        runtime.drive(&drive_budget(1)).unwrap();
    }

    let closer = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::CloseResourceGate {
                gate,
                continuation: Box::new(GateDone),
            },
        ))))
        .expect("gate closer admitted");
    let mut guard = 0;
    while runtime.resource_gate_owner_for_test(gate).is_some() {
        runtime.drive(&drive_budget(1)).unwrap();
        guard += 1;
        assert!(guard < 64, "close request must remove the gate");
    }
    assert_eq!(
        runtime.resource_gate_count(),
        0,
        "close removes the gate before any Closed wake is delivered"
    );
    assert!(
        matches!(waiter.poll_result(), RootPoll::Pending),
        "the one-item close turn leaves its Closed wake pending"
    );

    let commands = runtime.command_handle();
    let waiter_root = waiter.id();
    assert!(
        std::thread::spawn(move || commands.cancel_root(waiter_root))
            .join()
            .expect("command thread does not panic")
    );

    let mut turns = 0;
    while matches!(waiter.poll_result(), RootPoll::Pending) {
        runtime
            .drive(&drive_budget(1))
            .expect("the committed Closed wake is a harmless late no-op");
        turns += 1;
        assert!(
            turns < 64,
            "command cancellation must not strand the closed-gate waiter"
        );
    }
    let RootPoll::Ready(settlement) = waiter.poll_result() else {
        panic!("cancelled gate waiter settles")
    };
    assert!(matches!(
        settlement.outcome,
        TaskOutcome::Cancelled(CancelReason::HostStop)
    ));

    let fresh = runtime
        .submit_test_root(TestPreparedTask::returned(Value::int(23)))
        .expect("persistent runtime accepts another root");
    let mut turns = 0;
    while matches!(fresh.poll_result(), RootPoll::Pending) {
        runtime
            .drive(&drive_budget(8))
            .expect("late Closed wake cannot fault the persistent runtime");
        turns += 1;
        assert!(turns < 20, "fresh root settles");
    }
    assert!(matches!(closer.poll_result(), RootPoll::Ready(_)));
    assert!(owner.cancel(CancelReason::Explicit));
}

#[test]
fn wrong_runtime_resource_slot_teardown_aborts_instead_of_stranding() {
    let runtime = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let gate_slot = Rc::new(Cell::new(None));
    let owner = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::CreateResourceGate {
                continuation: Box::new(GateHoldOwner {
                    gate_slot: Rc::clone(&gate_slot),
                    events: Arc::clone(&events),
                    stage: 0,
                    gate: None,
                }),
            },
        ))))
        .expect("gate owner admitted");
    while gate_slot.get().is_none()
        || runtime
            .resource_gate_owner_for_test(gate_slot.get().expect("gate created"))
            .is_none()
        || runtime.timer_count_for_test() == 0
    {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    let gate = gate_slot.get().expect("gate created");

    let waiter = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::ResourceSlot(gate),
                continuation: Box::new(RecordGateGrant {
                    label: "waiter",
                    events,
                }),
            },
        ))))
        .expect("gate waiter admitted");
    while runtime.protocol_wait_count_for_test() < 2 {
        runtime.drive(&drive_budget(1)).unwrap();
    }

    let foreign = runtime_with_inline_executor(Rc::new(FakeClock::new()));
    let foreign_gate = foreign.create_resource_gate_for_test();
    runtime.forge_resource_slot_gate_for_test(
        waiter.main_task_for_test().expect("waiter task is live"),
        foreign_gate,
    );

    assert!(waiter.cancel(CancelReason::Explicit));
    let RootPoll::Aborted(super::RuntimeFault::Invariant { message }) = waiter.poll_result() else {
        panic!("wrong-runtime teardown aborts the root with its invariant")
    };
    assert!(message.contains("WrongRuntime"), "{message}");
    assert!(matches!(
        owner.poll_result(),
        RootPoll::Aborted(super::RuntimeFault::Invariant { .. })
    ));
    assert!(matches!(
        runtime.drive(&drive_budget(1)),
        Err(super::RuntimeFault::Invariant { message }) if message.contains("WrongRuntime")
    ));
}

/// (c) A resource-gate acquirer that is cancelled AFTER being granted the slot but
/// BEFORE its acquire continuation runs (the granted-but-not-run window) must
/// release the gate — its continuation raises on the cancellation without
/// releasing, so the runtime must release for it. Proof: a THIRD acquirer, queued
/// behind the cancelled one, proceeds (is granted the slot) rather than deadlocking
/// on a leaked gate.
#[test]
fn cancelling_a_granted_but_not_run_gate_acquirer_releases_the_gate() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock.clone());
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let gate_slot = Rc::new(Cell::new(None));

    // Owner A: create a gate, acquire it, and hold it (parked on a long Timer).
    let owner = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::CreateResourceGate {
                continuation: Box::new(GateHoldOwner {
                    gate_slot: Rc::clone(&gate_slot),
                    events: Arc::clone(&events),
                    stage: 0,
                    gate: None,
                }),
            },
        ))))
        .expect("owner root admitted");
    // Drive until A owns the gate and is parked on its hold timer.
    while gate_slot.get().is_none()
        || runtime
            .resource_gate_owner_for_test(gate_slot.get().unwrap())
            .is_none()
        || runtime.timer_count_for_test() == 0
    {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    let gate = gate_slot.get().expect("gate created");

    // Waiter B queues behind A.
    let waiter_b = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::ResourceSlot(gate),
                continuation: Box::new(RecordGateGrant {
                    label: "B",
                    events: Arc::clone(&events),
                }),
            },
        ))))
        .expect("waiter B admitted");
    // Waiter C queues behind B.
    let waiter_c = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Suspend(
            NativeSuspend {
                wait: WaitKind::ResourceSlot(gate),
                continuation: Box::new(RecordGateGrant {
                    label: "C",
                    events: Arc::clone(&events),
                }),
            },
        ))))
        .expect("waiter C admitted");
    // Drive until both B and C are parked in the gate's FIFO queue behind A.
    for _ in 0..24 {
        runtime.drive(&drive_budget(1)).unwrap();
    }
    let b_task = waiter_b.main_task_for_test().expect("B task live");
    assert_eq!(
        runtime.resource_gate_owner_for_test(gate),
        owner.main_task_for_test(),
        "A still owns the gate while B and C are queued"
    );

    // Fire A's hold timer so it releases the gate, granting B. Drive one step at a
    // time until ownership transfers to B — B is now GRANTED but has not yet run
    // its acquire continuation (the granted-but-not-run window).
    clock.advance(Duration::from_secs(7200));
    let mut guard = 0;
    while runtime.resource_gate_owner_for_test(gate) != Some(b_task) {
        runtime.drive(&drive_budget(1)).unwrap();
        guard += 1;
        assert!(guard < 64, "gate never transferred to B");
    }
    assert!(
        !events.lock().unwrap().iter().any(|e| e.starts_with("B-")),
        "B must not have run its continuation yet (granted-but-not-run): {:?}",
        events.lock().unwrap()
    );

    // Cancel B in the granted-but-not-run window. The runtime must release the
    // gate on B's behalf so C — queued behind B — proceeds.
    assert!(waiter_b.cancel(CancelReason::Explicit));
    for _ in 0..40 {
        runtime.drive(&drive_budget(4)).unwrap();
    }

    let log = events.lock().unwrap().clone();
    assert!(
        log.iter().any(|e| e == "C-granted"),
        "C (queued behind the cancelled B) must be granted the released gate: {log:?}"
    );
    assert!(
        !log.iter().any(|e| e == "B-granted"),
        "the cancelled granted-but-not-run acquirer must not run its grant: {log:?}"
    );
    let RootPoll::Ready(c_settle) = waiter_c.poll_result() else {
        panic!("C settles after acquiring the released gate");
    };
    assert!(
        matches!(c_settle.outcome, TaskOutcome::Returned(_)),
        "C acquires and returns: {:?}",
        c_settle.outcome
    );
}

/// Gate holder that acquires a resource gate and then parks FOREVER on an empty
/// channel receive — holding the slot while blocked on a cycle-forming wait. This
/// is the pathological topology behind the Reviewer-2 hole: a slot holder that is
/// itself excluded from an `async/run` barrier.
struct GateHoldOnChannel {
    gate_slot: Rc<Cell<Option<sema_core::runtime::ResourceGateId>>>,
    channel: sema_core::runtime::ChannelId,
    stage: u8,
    gate: Option<sema_core::runtime::ResourceGateId>,
}
impl Trace for GateHoldOnChannel {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}
impl NativeContinuation for GateHoldOnChannel {
    fn resume(
        mut self: Box<Self>,
        _: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match self.stage {
            0 => {
                let ResumeInput::Runtime(sema_core::runtime::RuntimeResponse::ResourceGate(handle)) =
                    input
                else {
                    panic!("gate holder stage 0 expected a ResourceGate response");
                };
                let gate = handle.id();
                self.gate_slot.set(Some(gate));
                self.gate = Some(gate);
                self.stage = 1;
                Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::ResourceSlot(gate),
                    continuation: self,
                }))
            }
            // Slot granted — park on an empty channel receive that never resolves,
            // holding the gate forever.
            _ => Ok(NativeOutcome::Suspend(NativeSuspend {
                wait: WaitKind::Channel(sema_core::runtime::ChannelWait::Receive {
                    channel: self.channel,
                }),
                continuation: self,
            })),
        }
    }
}

/// Barrier caller: `async/run`, then record its release and return.
struct BarrierReleaseCont {
    events: Arc<Mutex<Vec<String>>>,
}
impl Trace for BarrierReleaseCont {
    fn trace(&self, _: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}
impl NativeContinuation for BarrierReleaseCont {
    fn resume(self: Box<Self>, _: &mut NativeCallContext<'_>, input: ResumeInput) -> NativeResult {
        self.events.lock().unwrap().push(
            match input {
                ResumeInput::Returned(_) => "M-released",
                ResumeInput::Failed(_) => "M-failed",
                ResumeInput::Cancelled(_) => "M-cancelled",
                ResumeInput::Runtime(_) => "M-runtime",
            }
            .to_string(),
        );
        Ok(NativeOutcome::Return(Value::NIL))
    }
}

/// Hang-detection (Reviewer-2 hole, closed): `WaitKind::ResourceSlot` MUST be
/// CYCLE-FORMING for the `async/run` barrier. A same-origin-root sibling parked on
/// a `ResourceSlot` whose holder never releases (it is blocked forever on a
/// channel) must NOT keep the barrier waiting — `(async/run)` releases because the
/// slot waiter is excluded. Bounded by a drive-turn guard: were `ResourceSlot`
/// classified self-resolving, the barrier would wait on the slot waiter forever
/// and the guard would trip (a regression surfaces as a hang, not a pass).
#[test]
fn async_run_barrier_releases_over_resource_slot_cycle() {
    let clock = Rc::new(FakeClock::new());
    let runtime = runtime_with_inline_executor(clock);
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let gate_slot = Rc::new(Cell::new(None));
    // An empty channel the gate holder blocks on forever.
    let channel = runtime.create_channel_for_test(1);

    // Holder A (its own root): create a gate, acquire it, then block forever on
    // the empty channel while HOLDING the slot.
    runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::CreateResourceGate {
                continuation: Box::new(GateHoldOnChannel {
                    gate_slot: Rc::clone(&gate_slot),
                    channel,
                    stage: 0,
                    gate: None,
                }),
            },
        ))))
        .expect("holder root admitted");
    // Drive until A owns the gate and is parked on the channel.
    let mut guard = 0;
    while gate_slot.get().is_none()
        || runtime
            .resource_gate_owner_for_test(gate_slot.get().unwrap())
            .is_none()
    {
        runtime.drive(&drive_budget(4)).unwrap();
        guard += 1;
        assert!(guard < 64, "holder never acquired the gate");
    }
    let gate = gate_slot.get().expect("gate created");

    // Root R: main task M is the `async/run` barrier caller.
    let barrier_root = runtime
        .submit_test_root(TestPreparedTask::native(Ok(NativeOutcome::Runtime(
            sema_core::runtime::RuntimeRequest::OriginBarrier {
                continuation: Box::new(BarrierReleaseCont {
                    events: Arc::clone(&events),
                }),
            },
        ))))
        .expect("barrier root admitted");
    // Sibling B under R (same origin root) parks on the busy gate's ResourceSlot.
    runtime.submit_test_child_under_root(
        barrier_root.id(),
        TestPreparedTask::native(Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::ResourceSlot(gate),
            continuation: Box::new(RecordGateGrant {
                label: "B",
                events: Arc::clone(&events),
            }),
        }))),
    );

    // Drive until R settles — the barrier must release despite B being parked on
    // the never-granted ResourceSlot. Bounded: a hang regression trips the guard.
    let mut guard = 0;
    while matches!(barrier_root.poll_result(), RootPoll::Pending) {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(
            guard < 256,
            "async/run barrier hung on a ResourceSlot cycle (regression: ResourceSlot must be cycle-forming); events: {:?}",
            events.lock().unwrap()
        );
    }
    let RootPoll::Ready(settlement) = barrier_root.poll_result() else {
        panic!("barrier root settles");
    };
    assert!(
        matches!(settlement.outcome, TaskOutcome::Returned(_)),
        "barrier releases and returns: {:?}",
        settlement.outcome
    );
    let log = events.lock().unwrap().clone();
    assert!(
        log.iter().any(|e| e == "M-released"),
        "the barrier caller resumed with nil: {log:?}"
    );
    assert!(
        !log.iter().any(|e| e == "B-granted"),
        "B is never granted the slot (its holder blocks forever): {log:?}"
    );
}

#[test]
fn wait_inline_completion_observes_registered_state_then_consumes_owners_once() {
    let eval_context = sema_core::EvalContext::new();
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
    let pending = pending.invoke_decoder(&eval_context);
    assert_eq!(&*events.lock().unwrap(), &["decode"]);
    assert!(matches!(
        pending.invoke_continuation(&eval_context),
        Ok(NativeOutcome::Return(_))
    ));
    assert_eq!(&*events.lock().unwrap(), &["decode", "returned"]);
    assert_eq!(runtime.active_len(), 0);
    assert!(runtime.drain_one(&mut task).is_none());
}

#[test]
fn wait_submit_rejection_traverses_decoder_then_continuation() {
    let eval_context = sema_core::EvalContext::new();
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
    let pending = pending.invoke_decoder(&eval_context);
    assert_eq!(&*events.lock().unwrap(), &["decode"]);
    assert!(matches!(
        pending.invoke_continuation(&eval_context),
        Ok(NativeOutcome::Return(_))
    ));
    assert_eq!(&*events.lock().unwrap(), &["decode", "failed"]);
    assert_eq!(runtime.active_len(), 0);
}

#[test]
fn wait_submit_rejection_cancels_interruptible_resource_before_resume() {
    let eval_context = sema_core::EvalContext::new();
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
        pending
            .invoke_decoder(&eval_context)
            .invoke_continuation(&eval_context)
            .unwrap();
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
    let eval_context = sema_core::EvalContext::new();
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
    let pending = pending.invoke_decoder(&eval_context);
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
    let eval_context = sema_core::EvalContext::new();
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
        pending.invoke_continuation(&eval_context),
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
fn drive_reservation_leaves_a_credit_for_other_sources() {
    // A ready-root storm must not starve completions/timers/cleanup: even when
    // root_visit_limit >= work_item_limit, root reservation must leave at least
    // one work item per turn for the other drive sources (spec 223-226).
    let clock = FakeClock::new();
    let mut driver = BoundedDriver::new(Rc::new(clock));
    driver.add_ready_roots(4);
    driver.add_completions(4);
    let mut budget = drive_budget(4);
    budget.root_visit_limit = std::num::NonZeroUsize::new(4).unwrap();

    let report = driver.drive(&budget);
    assert!(
        report.completions >= 1,
        "root reservation starved the completion source: {report:?}"
    );
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

// ─────────────────────────────────────────────────────────────────────────────
// SRV-1 liveness spike — the re-arming External wait is deadlock-free.
//
// Proves the runtime primitive the concurrent `http/serve` accept loop needs
// (docs/deferred.md §SRV-1, "the first thing to prove"): a task may park on a
// `WaitKind::External` that stays idle indefinitely, re-arm onto a fresh
// External each time one completes (the accept-loop ping-pong), coexist with a
// second independently-parked task, and be torn down cleanly by shutdown — all
// without a false Quiescent/deadlock, a busy-spin, a panic, or an orphaned wait
// left in `active_len`.
//
// Uses SYNTHETIC externals: a `HoldExecutor` that accepts a submission and holds
// it un-run (modelling a real `rx.recv()` that has not yet produced a request),
// plus `deliver_oldest_held` which runs a held submission on demand (modelling a
// request arriving). No HTTP/tokio machinery — this proves the RUNTIME
// primitive, not the HTTP feature.

type HeldSubmissions = Arc<Mutex<Vec<sema_core::runtime::ExecutorSubmission>>>;

struct HoldLease {
    held: HeldSubmissions,
}
impl ExecutorLease for HoldLease {
    fn submit(
        &self,
        submission: sema_core::runtime::ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        let operation = submission.operation_id();
        // Hold the submission un-dispatched: the external stays "in flight" (its
        // job never runs, no completion is delivered) so the owning task parks
        // idle, exactly like an accept loop awaiting the next request.
        self.held.lock().unwrap().push(submission);
        Ok(RunningSubmission::new(operation))
    }
    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }
    fn shutdown(&self, _deadline: Instant) -> ExecutorShutdown {
        // Drop every held submission: each delivers a Cancelled completion to the
        // (closing) inbox, discarded by the runtime — clean teardown.
        self.held.lock().unwrap().clear();
        ExecutorShutdown::Drained(ExecutorSnapshot::default())
    }
}

struct HoldExecutor {
    held: HeldSubmissions,
}
impl IoExecutor for HoldExecutor {
    fn attach_runtime(
        &self,
        _runtime_id: RuntimeId,
    ) -> Result<Arc<dyn ExecutorLease>, ExecutorAttachError> {
        Ok(Arc::new(HoldLease {
            held: Arc::clone(&self.held),
        }))
    }
    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }
}

fn hold_runtime_with_clock(clock: Rc<dyn RuntimeClock>) -> (Runtime, HeldSubmissions) {
    let held: HeldSubmissions = Arc::new(Mutex::new(Vec::new()));
    let runtime = Runtime::new(
        Rc::new(sema_core::EvalContext::new()),
        clock,
        Arc::new(HoldExecutor {
            held: Arc::clone(&held),
        }),
    )
    .expect("hold runtime");
    (runtime, held)
}

fn hold_runtime() -> (Runtime, HeldSubmissions) {
    hold_runtime_with_clock(Rc::new(FakeClock::new()))
}

/// Run the OLDEST held submission's blocking dispatch, delivering its completion
/// to the runtime inbox — models one request arriving on the accept channel.
/// Returns false if nothing is held.
fn deliver_oldest_held(held: &HeldSubmissions) -> bool {
    let submission = {
        let mut guard = held.lock().unwrap();
        if guard.is_empty() {
            None
        } else {
            Some(guard.remove(0))
        }
    };
    match submission {
        Some(submission) => {
            match submission.into_dispatch() {
                ExecutorDispatch::Blocking(dispatch) => {
                    dispatch.run();
                }
                ExecutorDispatch::Async(_) => panic!("spike externals are blocking"),
            }
            true
        }
        None => false,
    }
}

/// A cancel hook that reaps immediately (models a client-disconnect teardown
/// that fully releases the parked accept/recv wait) and counts its firings.
struct SpikeReapHook {
    cancelled: Arc<Mutex<usize>>,
}
impl Trace for SpikeReapHook {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}
impl sema_core::runtime::CancelHook for SpikeReapHook {
    fn cancel(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        *self.cancelled.lock().unwrap() += 1;
        Ok(sema_core::runtime::CancelDisposition::Reaped)
    }
    fn reap(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        Ok(sema_core::runtime::CancelDisposition::Reaped)
    }
}

/// Build one interruptible External wait (models a single `rx.recv()`), whose
/// cancel hook shares `cancelled` so a test can assert teardown fired.
fn spike_external_wait(
    events: Arc<Mutex<Vec<&'static str>>>,
    cancelled: Arc<Mutex<usize>>,
) -> WaitKind {
    WaitKind::External(Box::new(PreparedExternalOperation::interruptible_blocking(
        CompletionKind::try_from_raw(2).unwrap(),
        Box::new(CountingDecoder(events)),
        InterruptibleResource::new("spike-accept", Box::new(SpikeReapHook { cancelled })),
        || Ok(Box::new(7_i32)),
    )))
}

/// Accept-loop continuation: on each External completion, re-arm onto a fresh
/// External (the ping-pong) up to `remaining` more times, then settle. Records
/// each re-arm so a test can assert every intermediate park was idle.
struct AcceptLoopSpikeCont {
    remaining: usize,
    parks: Arc<Mutex<usize>>,
    events: Arc<Mutex<Vec<&'static str>>>,
    cancelled: Arc<Mutex<usize>>,
}
impl Trace for AcceptLoopSpikeCont {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}
impl NativeContinuation for AcceptLoopSpikeCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(_) => {
                if self.remaining == 0 {
                    Ok(NativeOutcome::Return(Value::int(99)))
                } else {
                    *self.parks.lock().unwrap() += 1;
                    Ok(NativeOutcome::Suspend(NativeSuspend {
                        wait: spike_external_wait(
                            Arc::clone(&self.events),
                            Arc::clone(&self.cancelled),
                        ),
                        continuation: Box::new(AcceptLoopSpikeCont {
                            remaining: self.remaining - 1,
                            parks: self.parks,
                            events: self.events,
                            cancelled: self.cancelled,
                        }),
                    }))
                }
            }
            _ => Err(SemaError::eval("spike accept loop resumed unexpectedly")),
        }
    }
}

fn spike_accept_root(remaining: usize) -> (NativeOutcome, Arc<Mutex<usize>>, Arc<Mutex<usize>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let cancelled = Arc::new(Mutex::new(0));
    let parks = Arc::new(Mutex::new(0));
    let outcome = NativeOutcome::Suspend(NativeSuspend {
        wait: spike_external_wait(Arc::clone(&events), Arc::clone(&cancelled)),
        continuation: Box::new(AcceptLoopSpikeCont {
            remaining,
            parks: Arc::clone(&parks),
            events,
            cancelled: Arc::clone(&cancelled),
        }),
    });
    (outcome, parks, cancelled)
}

/// (a) An idle External wait — zero completions arriving — must let the runtime
/// report `Idle { inbox_wakeup_required: true }` (never Quiescent, never a false
/// deadlock), make no progress, and never panic across many drive turns.
#[test]
fn srv1_spike_idle_external_wait_reports_idle_without_busy_spin() {
    let (runtime, held) = hold_runtime();
    let (outcome, _parks, _cancelled) = spike_accept_root(0);
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(outcome)))
        .expect("root admitted");

    // First drive parks the root on its External.
    runtime.drive(&drive_budget(8)).unwrap();
    assert_eq!(runtime.active_wait_count_for_test(), 1);
    assert!(matches!(handle.poll_result(), RootPoll::Pending));
    assert_eq!(held.lock().unwrap().len(), 1, "the External is in flight");

    // No completion ever arrives: repeated drives must all report Idle, make no
    // progress, and leave the parked wait untouched — no busy-spin, no panic.
    for _ in 0..32 {
        let state = runtime.drive(&drive_budget(8)).unwrap();
        assert!(
            matches!(
                state,
                super::DriveState::Idle {
                    inbox_wakeup_required: true,
                    ..
                }
            ),
            "an idle External must keep the runtime awaiting its inbox, got {state:?}"
        );
        assert!(matches!(handle.poll_result(), RootPoll::Pending));
        assert_eq!(runtime.active_wait_count_for_test(), 1);
    }
}

/// (b) Two tasks each parked on their own External wait must coexist: completing
/// one settles only its owner and leaves the other parked and idle.
#[test]
fn srv1_spike_two_parked_tasks_coexist_and_complete_independently() {
    let (runtime, held) = hold_runtime();
    let (outcome_a, _pa, _ca) = spike_accept_root(0);
    let (outcome_b, _pb, _cb) = spike_accept_root(0);
    let root_a = runtime
        .submit_test_root(TestPreparedTask::native(Ok(outcome_a)))
        .expect("root a admitted");
    let root_b = runtime
        .submit_test_root(TestPreparedTask::native(Ok(outcome_b)))
        .expect("root b admitted");

    // Park both (two root visits per drive; drive twice to be safe).
    runtime.drive(&drive_budget(8)).unwrap();
    runtime.drive(&drive_budget(8)).unwrap();
    assert_eq!(runtime.active_wait_count_for_test(), 2);
    assert!(matches!(root_a.poll_result(), RootPoll::Pending));
    assert!(matches!(root_b.poll_result(), RootPoll::Pending));

    // A single request arrives: exactly one owner settles, the other stays parked.
    assert!(deliver_oldest_held(&held));
    let mut guard = 0;
    while !(matches!(root_a.poll_result(), RootPoll::Ready(_))
        || matches!(root_b.poll_result(), RootPoll::Ready(_)))
    {
        runtime.drive(&drive_budget(8)).unwrap();
        guard += 1;
        assert!(guard < 64, "the completed root settles");
    }
    let a_ready = matches!(root_a.poll_result(), RootPoll::Ready(_));
    let b_ready = matches!(root_b.poll_result(), RootPoll::Ready(_));
    assert!(a_ready ^ b_ready, "exactly one root settled, not both");
    assert_eq!(
        runtime.active_wait_count_for_test(),
        1,
        "the other task is still parked, undisturbed"
    );
    let state = runtime.drive(&drive_budget(8)).unwrap();
    assert!(matches!(
        state,
        super::DriveState::Idle {
            inbox_wakeup_required: true,
            ..
        }
    ));
}

/// The genuinely-novel bit: a continuation whose `resume` returns another
/// `Suspend(External)` re-arms indefinitely (the accept-loop ping-pong). Each
/// iteration parks idle on exactly one External; delivering a request re-arms
/// onto a fresh one; after the last, the root settles. No deadlock, no leak.
#[test]
fn srv1_spike_external_wait_rearms_across_multiple_idle_parks() {
    let (runtime, held) = hold_runtime();
    let (outcome, parks, _cancelled) = spike_accept_root(3);
    let handle = runtime
        .submit_test_root(TestPreparedTask::native(Ok(outcome)))
        .expect("root admitted");

    // Park on the first External.
    runtime.drive(&drive_budget(8)).unwrap();

    let mut guard = 0;
    while matches!(handle.poll_result(), RootPoll::Pending) {
        // Parked idle on exactly one External between requests.
        assert_eq!(runtime.active_wait_count_for_test(), 1);
        let state = runtime.drive(&drive_budget(8)).unwrap();
        assert!(
            matches!(
                state,
                super::DriveState::Idle {
                    inbox_wakeup_required: true,
                    ..
                }
            ),
            "each intermediate park must be idle, got {state:?}"
        );
        // A request arrives → deliver → the continuation re-arms (or settles).
        assert!(
            deliver_oldest_held(&held),
            "an External must be in flight to complete"
        );
        // Drain the completion + run the continuation's re-arm/settle.
        for _ in 0..6 {
            runtime.drive(&drive_budget(8)).unwrap();
        }
        guard += 1;
        assert!(guard < 16, "re-arm loop must terminate");
    }

    assert!(matches!(handle.poll_result(), RootPoll::Ready(_)));
    assert_eq!(
        *parks.lock().unwrap(),
        3,
        "the continuation re-armed exactly 3 times before settling"
    );
    assert_eq!(
        runtime.active_wait_count_for_test(),
        0,
        "no wait is left after the accept loop settles"
    );
}

/// (c) Shutdown while two tasks are parked on External waits must tear down
/// cleanly: cancel hooks fire, no wait is orphaned in `active_len`, and the
/// report is clean — no panic, no hang.
#[test]
fn srv1_spike_shutdown_tears_down_parked_waits_cleanly() {
    let clock = Rc::new(FakeClock::new());
    let (runtime, _held) = hold_runtime_with_clock(clock.clone());
    let (outcome_a, _pa, cancelled_a) = spike_accept_root(0);
    let (outcome_b, _pb, cancelled_b) = spike_accept_root(0);
    let root_a = runtime
        .submit_test_root(TestPreparedTask::native(Ok(outcome_a)))
        .expect("root a admitted");
    let root_b = runtime
        .submit_test_root(TestPreparedTask::native(Ok(outcome_b)))
        .expect("root b admitted");

    // Park both.
    runtime.drive(&drive_budget(8)).unwrap();
    runtime.drive(&drive_budget(8)).unwrap();
    assert_eq!(runtime.active_wait_count_for_test(), 2);

    // Shut down while both are parked.
    let report = runtime
        .shutdown(&super::ShutdownOptions {
            deadline: clock.now() + Duration::from_secs(1),
            drive_budget: drive_budget(16),
        })
        .expect("bounded shutdown");

    assert!(report.clean, "shutdown must be clean: {report:?}");
    assert_eq!(report.active_waits, 0, "no External wait left orphaned");
    assert_eq!(report.live_tasks, 0, "no task left live");
    assert_eq!(
        *cancelled_a.lock().unwrap() + *cancelled_b.lock().unwrap(),
        2,
        "each parked wait's cancel hook fired exactly once"
    );
    assert!(matches!(root_a.poll_result(), RootPoll::Ready(_)));
    assert!(matches!(root_b.poll_result(), RootPoll::Ready(_)));
}
