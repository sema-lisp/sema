use std::any::type_name;
use std::num::NonZeroU64;
use std::time::Duration;

use sema_core::runtime::{
    downcast_send_payload, CompletionDelivery, ExecutorAttachError, ExecutorLease,
    ExecutorShutdown, ExecutorSnapshot, ExternalCompletion, ExternalFailure, ExternalFailureCode,
    IoExecutor, QuarantineBound, QuarantineBoundDescriptor,
};

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
fn producer_failures_are_limited_to_deadline_and_bound() {
    assert_eq!(
        ExternalFailure::deadline_exceeded("deadline").code(),
        ExternalFailureCode::DeadlineExceeded
    );
    assert_eq!(
        ExternalFailure::bound_exceeded("bound").code(),
        ExternalFailureCode::BoundExceeded
    );
}

#[test]
fn executor_attachment_contract_is_implementable_cross_crate() {
    struct FakeExecutor;
    struct FakeLease;
    impl IoExecutor for FakeExecutor {
        fn attach_runtime(
            &self,
            _runtime_id: sema_core::runtime::RuntimeId,
        ) -> Result<std::sync::Arc<dyn ExecutorLease>, ExecutorAttachError> {
            Ok(std::sync::Arc::new(FakeLease))
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
            let operation_id = submission.operation_id();
            drop(submission.into_dispatch());
            Ok(sema_core::runtime::RunningSubmission::new(operation_id))
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
}
