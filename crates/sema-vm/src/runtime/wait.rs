use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;

use sema_core::runtime::{
    CancellationView, CompletionDecoder, CompletionDelivery, CompletionKind, CompletionRegistrar,
    CompletionSender, ExecutorAttachError, ExecutorCancelHandle, ExecutorLease, ExternalCompletion,
    ExternalFailure, IoExecutor, NativeCallContext, NativeContinuation, NativeResult,
    NativeSuspend, OperationId, ResourceClass, ResumeInput, RuntimeId, TaskContextHandle, TaskId,
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
    task_id: TaskId,
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
    quarantine: bool,
}

pub struct PendingResume {
    task_id: TaskId,
    decoder: Option<Box<dyn CompletionDecoder>>,
    continuation: Box<dyn NativeContinuation>,
    context: TaskContextHandle,
    raw: Option<Result<sema_core::runtime::SendPayload, ExternalFailure>>,
    input: Option<ResumeInput>,
    cancellation: CancellationView,
}

impl PendingResume {
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    pub fn invoke_decoder(mut self) -> Self {
        if let Some(decoder) = self.decoder.take() {
            let mut task_context = self.context.borrow_mut();
            let mut context = NativeCallContext {
                task_context: &mut task_context,
                cancellation: self.cancellation.clone(),
            };
            let decoded = decoder.decode(
                &mut context,
                self.raw
                    .take()
                    .expect("decoder invocation owns raw completion"),
            );
            self.input = Some(decoded.map_or_else(ResumeInput::Failed, ResumeInput::Returned));
        }
        self
    }

    pub fn invoke_continuation(self) -> NativeResult {
        let mut task_context = self.context.borrow_mut();
        let mut context = NativeCallContext {
            task_context: &mut task_context,
            cancellation: self.cancellation,
        };
        self.continuation.resume(
            &mut context,
            self.input.expect("decoder is charged before continuation"),
        )
    }
}

pub struct WaitRuntime {
    runtime_id: RuntimeId,
    registrar: CompletionRegistrar,
    lease: Arc<dyn ExecutorLease>,
    inbox: Receiver<ExternalCompletion>,
    active: HashMap<WaitKey, RegisteredExternalWait>,
    cleanup: HashMap<WaitKey, CleanupEntry>,
    cleanup_order: VecDeque<WaitKey>,
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
            cleanup_order: VecDeque::new(),
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
    ) -> Result<WaitKey, Box<PendingResume>> {
        let WaitKind::External(prepared) = suspend.wait else {
            panic!("external wait required");
        };
        let kind = prepared.completion_kind();
        let identity = self
            .registrar
            .issue_identity(kind)
            .expect("wait identity available in task harness");
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
            .expect("runtime-issued identity binds its declared kind");
        let (runtime, submission) = binding.split();
        let (decoder, resource, queue_cancel) = runtime.into_parts();
        task.wait(key).expect("running task accepts external wait");
        self.active.insert(
            key,
            RegisteredExternalWait {
                identity: retained,
                task_id: task.id(),
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
            task.reject_wait(key)
                .expect("rejection restores waiting task");
            return Err(Box::new(self.finish_rejection(key, wait)));
        }
        Ok(key)
    }

    fn finish_rejection(
        &mut self,
        key: WaitKey,
        mut wait: RegisteredExternalWait,
    ) -> PendingResume {
        let _ = wait.queue_cancel.cancel_before_start();
        let cancellation = wait.resource.cancel();
        let quarantine = wait.resource.bound().is_some();
        let retain = quarantine
            || matches!(
                cancellation,
                Some(Ok(sema_core::runtime::CancelDisposition::PendingReap) | Err(_))
            );
        let last_error = cancellation
            .and_then(Result::err)
            .map(|error| error.to_string());
        if retain {
            self.insert_cleanup(key, &wait.identity, wait.resource, quarantine, last_error);
        }
        PendingResume {
            task_id: wait.task_id,
            decoder: Some(wait.decoder),
            continuation: wait.continuation,
            context: wait.context,
            raw: Some(Err(ExternalFailure::rejected())),
            input: None,
            cancellation: CancellationView::default(),
        }
    }

    pub fn drain_one(&mut self) -> Option<(CompletionRoute, Option<PendingResume>)> {
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
            return Some((
                CompletionRoute::Active,
                Some(PendingResume {
                    task_id: wait.task_id,
                    decoder: Some(wait.decoder),
                    continuation: wait.continuation,
                    context: wait.context,
                    raw: Some(completion.result),
                    input: None,
                    cancellation: CancellationView::default(),
                }),
            ));
        }
        if self.cleanup.get(&key).is_some_and(|entry| {
            entry.quarantine && {
                entry.runtime_id == completion.runtime_id
                    && entry.operation_id == completion.operation_id
                    && entry.kind == completion.kind
            }
        }) {
            self.cleanup.remove(&key);
            self.quarantine_reaped += 1;
            return Some((CompletionRoute::Cleanup, None));
        }
        self.late_completions += 1;
        Some((CompletionRoute::Late, None))
    }

    pub fn cancel(&mut self, task: &TaskRecord, key: WaitKey) -> Option<PendingResume> {
        let mut wait = self.active.remove(&key)?;
        debug_assert_eq!(wait.task_id, task.id());
        let _ = wait.queue_cancel.cancel_before_start();
        let reason = task.cancellation()?.reason;
        let cancellation = wait.resource.cancel();
        let quarantine = wait.resource.bound().is_some();
        let retain = match &cancellation {
            Some(Ok(sema_core::runtime::CancelDisposition::Reaped)) => false,
            Some(Ok(sema_core::runtime::CancelDisposition::PendingReap) | Err(_)) => true,
            None => wait.resource.bound().is_some(),
        };
        if retain {
            let last_error = cancellation
                .and_then(Result::err)
                .map(|error| error.to_string());
            self.insert_cleanup(key, &wait.identity, wait.resource, quarantine, last_error);
        }
        drop(wait.decoder);
        Some(PendingResume {
            task_id: wait.task_id,
            decoder: None,
            continuation: wait.continuation,
            context: wait.context,
            raw: None,
            input: Some(ResumeInput::Cancelled(reason)),
            cancellation: CancellationView::new(true, Some(reason)),
        })
    }

    fn insert_cleanup(
        &mut self,
        key: WaitKey,
        identity: &RetainedIdentity,
        resource: ResourceClass,
        quarantine: bool,
        last_error: Option<String>,
    ) {
        self.cleanup.insert(
            key,
            CleanupEntry {
                runtime_id: identity.runtime_id,
                operation_id: identity.operation_id,
                kind: identity.kind,
                resource,
                reap_attempts: 0,
                last_error,
                quarantine,
            },
        );
        self.cleanup_order.push_back(key);
    }

    pub fn reap_cleanup(&mut self, limit: usize) -> usize {
        let mut reaped = 0;
        for _ in 0..limit.min(self.cleanup_order.len()) {
            let key = self
                .cleanup_order
                .pop_front()
                .expect("bounded by queue length");
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
            if self.cleanup.contains_key(&key) {
                self.cleanup_order.push_back(key);
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
