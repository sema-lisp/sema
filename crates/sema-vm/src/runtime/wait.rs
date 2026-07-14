use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;

use sema_core::runtime::{
    CancellationView, CompletionDecoder, CompletionDelivery, CompletionKind, CompletionRegistrar,
    CompletionSender, ExecutorAttachError, ExecutorCancelHandle, ExecutorLease, ExternalCompletion,
    ExternalFailure, IoExecutor, NativeCallContext, NativeContinuation, NativeResult,
    NativeSuspend, OperationId, ResourceClass, ResumeInput, RuntimeId, TaskContextHandle,
    WaitGeneration, WaitId, WaitKind,
};

use super::{TaskRecord, WaitKey};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeCreateError {
    IdExhausted,
    ExecutorAttach(ExecutorAttachError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionRoute {
    Active,
    Cleanup,
    Late,
}

struct InboxSender(Sender<ExternalCompletion>);

impl CompletionSender for InboxSender {
    fn send(&self, completion: ExternalCompletion) -> CompletionDelivery {
        self.0
            .send(completion)
            .map(|()| CompletionDelivery::Delivered)
            .unwrap_or(CompletionDelivery::InboxClosed)
    }
}

struct RegisteredExternalWait {
    identity: RetainedIdentity,
    decoder: Box<dyn CompletionDecoder>,
    resource: ResourceClass,
    queue_cancel: ExecutorCancelHandle,
    continuation: Box<dyn NativeContinuation>,
    context: TaskContextHandle,
}

struct CleanupEntry {
    runtime_id: RuntimeId,
    operation_id: OperationId,
    kind: CompletionKind,
    resource: ResourceClass,
    reap_attempts: usize,
    last_error: Option<String>,
}

pub struct WaitRuntime {
    runtime_id: RuntimeId,
    registrar: CompletionRegistrar,
    lease: Arc<dyn ExecutorLease>,
    inbox: Receiver<ExternalCompletion>,
    active: HashMap<WaitKey, RegisteredExternalWait>,
    cleanup: HashMap<WaitKey, CleanupEntry>,
    late_completions: usize,
    quarantine_reaped: usize,
}

impl WaitRuntime {
    pub fn new(executor: Arc<dyn IoExecutor>) -> Result<Self, RuntimeCreateError> {
        let (sender, inbox) = mpsc::channel();
        let (runtime_id, registrar) = CompletionRegistrar::register(Arc::new(InboxSender(sender)))
            .map_err(|_| RuntimeCreateError::IdExhausted)?;
        let lease = executor
            .attach_runtime(runtime_id)
            .map_err(RuntimeCreateError::ExecutorAttach)?;
        Ok(Self {
            runtime_id,
            registrar,
            lease,
            inbox,
            active: HashMap::new(),
            cleanup: HashMap::new(),
            late_completions: 0,
            quarantine_reaped: 0,
        })
    }

    pub fn runtime_id(&self) -> RuntimeId {
        self.runtime_id
    }

    pub fn register_external(
        &mut self,
        task: &mut TaskRecord,
        suspend: NativeSuspend,
        context: TaskContextHandle,
    ) -> Result<WaitKey, NativeResult> {
        let WaitKind::External(prepared) = suspend.wait else {
            return Err(Err(sema_core::SemaError::eval("external wait required")));
        };
        let kind = prepared.completion_kind();
        let identity = self
            .registrar
            .issue_identity(kind)
            .map_err(|_| Err(sema_core::SemaError::eval("wait identity exhausted")))?;
        let key = WaitKey {
            id: identity.wait_id(),
            generation: identity.generation(),
        };
        let retained = RetainedIdentity {
            runtime_id: identity.runtime_id(),
            wait_id: identity.wait_id(),
            generation: identity.generation(),
            operation_id: identity.operation_id(),
            kind: identity.kind(),
        };
        let binding = self
            .registrar
            .bind(identity, *prepared)
            .map_err(|error| Err(sema_core::SemaError::eval(error.to_string())))?;
        let (runtime, submission) = binding.split();
        let (decoder, resource, queue_cancel) = runtime.into_parts();
        task.wait(key)
            .map_err(|error| Err(sema_core::SemaError::eval(format!("{error:?}"))))?;
        self.active.insert(
            key,
            RegisteredExternalWait {
                identity: retained,
                decoder,
                resource,
                queue_cancel,
                continuation: suspend.continuation,
                context,
            },
        );
        if let Err(rejected) = self.lease.submit(submission) {
            let _ = rejected.rollback();
            let wait = self.active.remove(&key).expect("registered before submit");
            let result = self.finish_rejection(wait);
            return Err(result);
        }
        Ok(key)
    }

    fn finish_rejection(&mut self, mut wait: RegisteredExternalWait) -> NativeResult {
        let _ = wait.queue_cancel.cancel_before_start();
        let _ = wait.resource.cancel();
        let mut task_context = wait.context.borrow_mut();
        let mut context = NativeCallContext {
            task_context: &mut task_context,
            cancellation: CancellationView::default(),
        };
        let decoded = wait
            .decoder
            .decode(&mut context, Err(ExternalFailure::rejected()));
        wait.continuation.resume(
            &mut context,
            decoded.map_or_else(ResumeInput::Failed, ResumeInput::Returned),
        )
    }

    pub fn drain_one(&mut self) -> Option<(CompletionRoute, Option<NativeResult>)> {
        let completion = match self.inbox.try_recv() {
            Ok(completion) => completion,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return None,
        };
        let key = WaitKey {
            id: completion.wait_id,
            generation: completion.generation,
        };
        if self
            .active
            .get(&key)
            .is_some_and(|wait| identity_matches(&wait.identity, &completion))
        {
            let wait = self.active.remove(&key).expect("identity checked");
            let mut task_context = wait.context.borrow_mut();
            let mut context = NativeCallContext {
                task_context: &mut task_context,
                cancellation: CancellationView::default(),
            };
            let decoded = wait.decoder.decode(&mut context, completion.result);
            let result = wait.continuation.resume(
                &mut context,
                decoded.map_or_else(ResumeInput::Failed, ResumeInput::Returned),
            );
            return Some((CompletionRoute::Active, Some(result)));
        }
        if self.cleanup.get(&key).is_some_and(|entry| {
            entry.runtime_id == completion.runtime_id
                && entry.operation_id == completion.operation_id
                && entry.kind == completion.kind
        }) {
            self.cleanup.remove(&key);
            self.quarantine_reaped += 1;
            return Some((CompletionRoute::Cleanup, None));
        }
        self.late_completions += 1;
        Some((CompletionRoute::Late, None))
    }

    pub fn cancel(&mut self, key: WaitKey) -> Option<NativeResult> {
        let mut wait = self.active.remove(&key)?;
        let _ = wait.queue_cancel.cancel_before_start();
        let reason = sema_core::runtime::CancelReason::Explicit;
        let retain = match wait.resource.cancel() {
            Some(Ok(sema_core::runtime::CancelDisposition::Reaped)) => false,
            Some(Ok(sema_core::runtime::CancelDisposition::PendingReap) | Err(_)) => true,
            None => wait.resource.bound().is_some(),
        };
        if retain {
            self.cleanup.insert(
                key,
                CleanupEntry {
                    runtime_id: wait.identity.runtime_id,
                    operation_id: wait.identity.operation_id,
                    kind: wait.identity.kind,
                    resource: wait.resource,
                    reap_attempts: 0,
                    last_error: None,
                },
            );
        }
        let mut task_context = wait.context.borrow_mut();
        let mut context = NativeCallContext {
            task_context: &mut task_context,
            cancellation: CancellationView::new(true, Some(reason)),
        };
        Some(
            wait.continuation
                .resume(&mut context, ResumeInput::Cancelled(reason)),
        )
    }

    pub fn reap_cleanup(&mut self, limit: usize) -> usize {
        let keys: Vec<_> = self.cleanup.keys().copied().take(limit).collect();
        let mut reaped = 0;
        for key in keys {
            let Some(entry) = self.cleanup.get_mut(&key) else {
                continue;
            };
            entry.reap_attempts += 1;
            match entry.resource.reap() {
                Some(Ok(sema_core::runtime::CancelDisposition::Reaped)) => {
                    self.cleanup.remove(&key);
                    reaped += 1;
                }
                Some(Err(error)) => entry.last_error = Some(error.to_string()),
                _ => {}
            }
        }
        reaped
    }

    pub fn active_len(&self) -> usize {
        self.active.len()
    }
    pub fn cleanup_len(&self) -> usize {
        self.cleanup.len()
    }
    pub fn late_completions(&self) -> usize {
        self.late_completions
    }
    pub fn quarantine_reaped(&self) -> usize {
        self.quarantine_reaped
    }
}

struct RetainedIdentity {
    runtime_id: RuntimeId,
    wait_id: WaitId,
    generation: WaitGeneration,
    operation_id: OperationId,
    kind: CompletionKind,
}

fn identity_matches(identity: &RetainedIdentity, completion: &ExternalCompletion) -> bool {
    identity.runtime_id == completion.runtime_id
        && identity.wait_id == completion.wait_id
        && identity.generation == completion.generation
        && identity.operation_id == completion.operation_id
        && identity.kind == completion.kind
}
