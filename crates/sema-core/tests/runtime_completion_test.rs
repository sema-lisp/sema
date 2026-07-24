use std::any::type_name;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    downcast_send_payload, CompletionDecoder, CompletionDelivery, CompletionKind,
    CompletionRegistrar, CompletionSender, ExecutorAttachError, ExecutorDispatch, ExecutorLease,
    ExecutorShutdown, ExecutorSnapshot, ExecutorTerminal, ExternalCompletion, ExternalFailure,
    ExternalFailureCode, IoExecutor, NativeCallContext, PreparedExternalOperation, QuarantineBound,
    QuarantineBoundDescriptor, RunningSubmission, SubmissionRejected, SubmitErrorKind, Trace,
};
use sema_core::Value;

fn assert_send<T: Send>() {}

#[test]
fn completion_envelope_is_send() {
    assert_send::<ExternalCompletion>();
}

#[test]
fn completion_payload_downcasts_or_names_the_expected_type() {
    let value =
        downcast_send_payload::<u32>(Box::new(42_u32), "test/read").expect("matching payload type");
    assert_eq!(value, 42);

    let failure = downcast_send_payload::<u32>(Box::new("wrong"), "test/read")
        .expect_err("mismatched payload type");
    assert_eq!(failure.code(), ExternalFailureCode::Decode);
    assert_eq!(failure.operation(), Some("test/read"));
    assert_eq!(failure.expected_type(), Some(type_name::<u32>()));
}

#[test]
fn quarantine_bounds_are_exact_and_nonzero() {
    assert!(QuarantineBound::hard_deadline(Duration::ZERO).is_err());
    let deadline =
        QuarantineBound::hard_deadline(Duration::from_millis(25)).expect("nonzero deadline");
    assert_eq!(
        deadline.hard_deadline_value(),
        Some(Duration::from_millis(25))
    );
    assert_eq!(deadline.finite_work_value(), None);
    assert_eq!(
        deadline.descriptor(),
        QuarantineBoundDescriptor::HardDeadline(Duration::from_millis(25))
    );

    let maximum = NonZeroU64::new(7).expect("nonzero fixture");
    let finite = QuarantineBound::finite_work("records", maximum);
    assert_eq!(finite.finite_work_value(), Some(("records", maximum)));
    assert_eq!(finite.hard_deadline_value(), None);
}

#[test]
fn delivery_and_attachment_failures_are_structured() {
    assert_ne!(
        CompletionDelivery::Delivered,
        CompletionDelivery::InboxClosed
    );
    assert!(matches!(
        ExecutorAttachError::ShuttingDown,
        ExecutorAttachError::ShuttingDown
    ));
}

#[test]
fn cross_crate_failures_cover_jobs_and_runtime_rejection() {
    assert_eq!(
        ExternalFailure::deadline_exceeded("deadline").code(),
        ExternalFailureCode::DeadlineExceeded
    );
    assert_eq!(
        ExternalFailure::bound_exceeded("bound").code(),
        ExternalFailureCode::BoundExceeded
    );

    // A downstream runtime synthesizes this only after silent submission rollback.
    assert_eq!(
        ExternalFailure::rejected().code(),
        ExternalFailureCode::Rejected
    );
}

#[test]
fn executor_attachment_contract_is_implementable_cross_crate() {
    struct Decoder;
    impl Trace for Decoder {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }
    impl CompletionDecoder for Decoder {
        fn decode(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            result: Result<Box<dyn std::any::Any + Send>, ExternalFailure>,
        ) -> Result<Value, sema_core::SemaError> {
            result
                .map(|_| Value::nil())
                .map_err(|error| sema_core::SemaError::eval(error.message()))
        }
    }
    struct Sender(Mutex<Vec<ExternalCompletion>>);
    impl CompletionSender for Sender {
        fn send(&self, completion: ExternalCompletion) -> CompletionDelivery {
            self.0.lock().unwrap().push(completion);
            CompletionDelivery::Delivered
        }
    }
    struct FakeExecutor;
    struct FakeLease {
        queue: Arc<Mutex<Vec<ExecutorDispatch>>>,
        reject: bool,
    }
    impl IoExecutor for FakeExecutor {
        fn attach_runtime(
            &self,
            _runtime_id: sema_core::runtime::RuntimeId,
        ) -> Result<std::sync::Arc<dyn ExecutorLease>, ExecutorAttachError> {
            Ok(Arc::new(FakeLease {
                queue: Arc::default(),
                reject: false,
            }))
        }
        fn snapshot(&self) -> ExecutorSnapshot {
            ExecutorSnapshot::default()
        }
    }
    impl ExecutorLease for FakeLease {
        fn submit(
            &self,
            submission: sema_core::runtime::ExecutorSubmission,
        ) -> Result<sema_core::runtime::RunningSubmission, sema_core::runtime::SubmissionRejected>
        {
            if self.reject {
                return Err(submission.reject(SubmitErrorKind::Capacity));
            }
            let receipt = RunningSubmission::new(submission.operation_id());
            self.queue.lock().unwrap().push(submission.into_dispatch());
            Ok(receipt)
        }
        fn snapshot(&self) -> ExecutorSnapshot {
            ExecutorSnapshot::default()
        }
        fn shutdown(&self, _deadline: std::time::Instant) -> ExecutorShutdown {
            ExecutorShutdown::Drained(ExecutorSnapshot::default())
        }
    }
    fn assert_executor(_: &dyn IoExecutor) {}
    assert_executor(&FakeExecutor);

    let make_submission = |sender: Arc<Sender>| {
        let (_, registrar, _) = CompletionRegistrar::register(sender).unwrap();
        let kind = CompletionKind::try_from_raw(1).unwrap();
        let identity = registrar.issue_identity(kind).unwrap();
        let prepared = PreparedExternalOperation::quarantined_blocking(
            kind,
            Box::new(Decoder),
            QuarantineBound::finite_work("test", NonZeroU64::new(1).unwrap()),
            || Ok(Box::new(1_u8)),
        );
        let (binding, submission) = registrar.bind(identity, prepared).unwrap().split();
        (binding, submission)
    };

    let rejected_sender = Arc::new(Sender(Mutex::new(Vec::new())));
    let (_binding, submission) = make_submission(Arc::clone(&rejected_sender));
    let rejecting = FakeLease {
        queue: Arc::default(),
        reject: true,
    };
    let rejection: SubmissionRejected = rejecting.submit(submission).unwrap_err();
    assert_eq!(rejection.rollback(), SubmitErrorKind::Capacity);
    assert!(rejected_sender.0.lock().unwrap().is_empty());

    let sender = Arc::new(Sender(Mutex::new(Vec::new())));
    let (_binding, submission) = make_submission(Arc::clone(&sender));
    let queue = Arc::new(Mutex::new(Vec::new()));
    let lease = FakeLease {
        queue: Arc::clone(&queue),
        reject: false,
    };
    let expected = submission.operation_id();
    let receipt = match lease.submit(submission) {
        Ok(receipt) => receipt,
        Err(_) => panic!("accepting fake lease rejected submission"),
    };
    assert_eq!(receipt.operation_id(), expected);
    let ExecutorDispatch::Blocking(dispatch) = queue.lock().unwrap().pop().unwrap() else {
        panic!("blocking fixture")
    };
    let report = dispatch.run();
    assert_eq!(report.terminal, ExecutorTerminal::Completed);
    assert_eq!(sender.0.lock().unwrap().len(), 1);

    let failed_sender = Arc::new(Sender(Mutex::new(Vec::new())));
    let (_binding, submission) = make_submission(Arc::clone(&failed_sender));
    drop(submission.into_dispatch());
    let completion = failed_sender.0.lock().unwrap().pop().unwrap();
    assert_eq!(
        completion.result.unwrap_err().code(),
        ExternalFailureCode::Cancelled
    );
}
