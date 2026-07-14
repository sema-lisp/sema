use std::any::type_name;
use std::num::NonZeroU64;
use std::time::Duration;

use sema_core::runtime::{
    downcast_send_payload, CompletionDelivery, ExecutorAttachError, ExternalCompletion,
    ExternalFailureCode, QuarantineBound, QuarantineBoundDescriptor,
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
