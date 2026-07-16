use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Instant;

use sema_core::runtime::{
    CancellationView, CompletionDecoder, CompletionDelivery, CompletionKind, CompletionRegistrar,
    CompletionSender, ExecutorAttachError, ExecutorCancelHandle, ExecutorLease, ExternalCompletion,
    ExternalFailure, IoExecutor, NativeCallContext, NativeContinuation, NativeResult,
    NativeSuspend, OperationId, ResourceClass, ResumeInput, RuntimeId, TaskContextHandle, TaskId,
    Trace, WaitGeneration, WaitId, WaitKind,
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

pub enum RegisterExternalError {
    IdExhausted(&'static str, NativeSuspend),
    Rejected(Box<PendingResume>),
}

impl std::fmt::Debug for RegisterExternalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::IdExhausted(_, _) => "IdExhausted(..)",
            Self::Rejected(_) => "Rejected(..)",
        })
    }
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

impl Trace for RegisteredExternalWait {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.decoder.trace(sink)
            && self.resource.trace(sink)
            && self.continuation.trace(sink)
            && self.context.trace(sink)
    }
}

struct CleanupEntry {
    runtime_id: RuntimeId,
    operation_id: OperationId,
    kind: CompletionKind,
    resource: ResourceClass,
    reap_attempts: usize,
    last_error: Option<String>,
    quarantine: bool,
    transferred_at: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupDiagnostic {
    pub wait: WaitKey,
    pub operation: OperationId,
    pub resource: String,
    pub reap_attempts: usize,
    pub last_error: Option<String>,
    pub suppressed_hook_error: Option<String>,
    pub quarantine: bool,
    pub bound_expired: bool,
}

pub struct PendingResume {
    key: WaitKey,
    task_id: TaskId,
    decoder: Option<Box<dyn CompletionDecoder>>,
    continuation: Box<dyn NativeContinuation>,
    context: TaskContextHandle,
    raw: Option<Result<sema_core::runtime::SendPayload, ExternalFailure>>,
    input: Option<ResumeInput>,
    cancellation: CancellationView,
}

impl Trace for PendingResume {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.decoder
            .as_ref()
            .is_none_or(|decoder| decoder.trace(sink))
            && self.continuation.trace(sink)
            && self.context.trace(sink)
            && self.input.as_ref().is_none_or(|input| input.trace(sink))
    }
}

impl PendingResume {
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    pub fn wait_key(&self) -> WaitKey {
        self.key
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

    /// Surrender the continuation so the runtime can resume it through the
    /// cancellation-reconciling path when a sticky cancellation landed after the
    /// completion woke the task. The decoded completion value is discarded.
    pub fn into_continuation(self) -> Box<dyn NativeContinuation> {
        self.continuation
    }
}

pub struct WaitRuntime {
    runtime_id: RuntimeId,
    registrar: Option<CompletionRegistrar>,
    lease: Option<Arc<dyn ExecutorLease>>,
    inbox: Option<Receiver<ExternalCompletion>>,
    deferred: VecDeque<ExternalCompletion>,
    active: HashMap<WaitKey, RegisteredExternalWait>,
    cleanup: HashMap<WaitKey, CleanupEntry>,
    cleanup_order: VecDeque<WaitKey>,
    cleanup_tombstones: usize,
    late_completions: usize,
    quarantine_reaped: usize,
    #[cfg(test)]
    force_wait_exhaustion: bool,
    #[cfg(test)]
    force_operation_exhaustion: bool,
}

impl Trace for WaitRuntime {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.active.values().all(|wait| wait.trace(sink))
            && self
                .cleanup
                .values()
                .all(|entry| entry.resource.trace(sink))
    }
}

impl WaitRuntime {
    pub fn new(executor: Arc<dyn IoExecutor>) -> Result<Self, RuntimeCreateError> {
        Self::new_with_issuers(executor).map(|(runtime, _)| runtime)
    }

    pub(crate) fn new_with_issuers(
        executor: Arc<dyn IoExecutor>,
    ) -> Result<(Self, sema_core::runtime::RuntimeScopedIdIssuers), RuntimeCreateError> {
        let (sender, inbox) = mpsc::channel();
        let (runtime_id, registrar, issuers) =
            CompletionRegistrar::register(Arc::new(InboxSender(sender)))
                .map_err(|_| RuntimeCreateError::IdExhausted)?;
        let lease = executor
            .attach_runtime(runtime_id)
            .map_err(RuntimeCreateError::ExecutorAttach)?;
        Ok((
            Self {
                runtime_id,
                registrar: Some(registrar),
                lease: Some(lease),
                inbox: Some(inbox),
                deferred: VecDeque::new(),
                active: HashMap::new(),
                cleanup: HashMap::new(),
                cleanup_order: VecDeque::new(),
                cleanup_tombstones: 0,
                late_completions: 0,
                quarantine_reaped: 0,
                #[cfg(test)]
                force_wait_exhaustion: false,
                #[cfg(test)]
                force_operation_exhaustion: false,
            },
            issuers,
        ))
    }

    pub fn runtime_id(&self) -> RuntimeId {
        self.runtime_id
    }

    pub fn issue_internal_wait(&self) -> Result<WaitKey, sema_core::runtime::IdExhausted> {
        #[cfg(test)]
        if self.force_wait_exhaustion {
            return Err(sema_core::runtime::IdExhausted);
        }
        self.registrar
            .as_ref()
            .expect("open wait runtime has registrar")
            .issue_wait_identity()
            .map(|(id, generation)| WaitKey {
                runtime: self.runtime_id,
                id,
                generation,
            })
    }

    pub fn register_external(
        &mut self,
        task: &mut TaskRecord,
        suspend: NativeSuspend,
        context: TaskContextHandle,
    ) -> Result<WaitKey, RegisterExternalError> {
        let WaitKind::External(prepared) = &suspend.wait else {
            panic!("external wait required");
        };
        let kind = prepared.completion_kind();
        #[cfg(test)]
        if self.force_wait_exhaustion {
            return Err(RegisterExternalError::IdExhausted("wait", suspend));
        }
        #[cfg(test)]
        if self.force_operation_exhaustion {
            return Err(RegisterExternalError::IdExhausted("operation", suspend));
        }
        let identity = match self
            .registrar
            .as_ref()
            .expect("closed wait runtime rejects admission before registration")
            .issue_identity(kind)
        {
            Ok(identity) => identity,
            Err(_) => return Err(RegisterExternalError::IdExhausted("wait", suspend)),
        };
        let WaitKind::External(prepared) = suspend.wait else {
            unreachable!("wait kind checked before identity issuance")
        };
        let key = WaitKey {
            runtime: self.runtime_id,
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
            .as_ref()
            .expect("registrar retained through binding")
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
        if let Err(rejected) = self
            .lease
            .as_ref()
            .expect("lease retained through submission")
            .submit(submission)
        {
            let _ = rejected.rollback();
            let wait = self.active.remove(&key).expect("registered before submit");
            task.reject_wait(key)
                .expect("rejection restores waiting task");
            return Err(RegisterExternalError::Rejected(Box::new(
                self.finish_rejection(key, wait),
            )));
        }
        Ok(key)
    }

    fn finish_rejection(&mut self, key: WaitKey, wait: RegisteredExternalWait) -> PendingResume {
        let _ = wait.queue_cancel.cancel_before_start();
        self.cancel_or_transfer_resource(key, &wait.identity, wait.resource, false, Instant::now());
        PendingResume {
            key,
            task_id: wait.task_id,
            decoder: Some(wait.decoder),
            continuation: wait.continuation,
            context: wait.context,
            raw: Some(Err(ExternalFailure::rejected())),
            input: None,
            cancellation: CancellationView::default(),
        }
    }

    pub fn drain_one(
        &mut self,
        task: &mut TaskRecord,
    ) -> Option<(CompletionRoute, Option<PendingResume>)> {
        let completion = match self.deferred.pop_front().map_or_else(
            || {
                self.inbox
                    .as_ref()
                    .map_or(Err(TryRecvError::Disconnected), Receiver::try_recv)
            },
            Ok,
        ) {
            Ok(completion) => completion,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return None,
        };
        let key = WaitKey {
            runtime: self.runtime_id,
            id: completion.wait_id,
            generation: completion.generation,
        };
        if let Some(wait) = self.active.get(&key) {
            if !identity_matches(&wait.identity, &completion) {
                self.late_completions += 1;
                return Some((CompletionRoute::Late, None));
            }
            if wait.task_id != task.id() || task.wait_key() != Some(key) {
                self.deferred.push_front(completion);
                return None;
            }
            task.wake(key).ok()?;
            let wait = self.active.remove(&key).expect("identity checked");
            return Some((
                CompletionRoute::Active,
                Some(PendingResume {
                    key,
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
            self.remove_cleanup(key);
            self.quarantine_reaped += 1;
            return Some((CompletionRoute::Cleanup, None));
        }
        self.late_completions += 1;
        Some((CompletionRoute::Late, None))
    }

    /// Block the calling (VM) thread until a completion is available on the
    /// inbox or `deadline` elapses, buffering any received completion so the next
    /// drive turn delivers it. Returns `true` if a completion is now buffered.
    /// A `None` deadline blocks until a completion arrives or the inbox is
    /// closed — used when a task is parked on an external op with no timer bound,
    /// where a worker completion is guaranteed to arrive. Never busy-spins.
    pub fn block_on_inbox(&mut self, deadline: Option<Instant>) -> bool {
        if !self.deferred.is_empty() {
            return true;
        }
        let Some(inbox) = self.inbox.as_ref() else {
            return false;
        };
        let received = match deadline {
            Some(deadline) => {
                let timeout = deadline.saturating_duration_since(Instant::now());
                inbox.recv_timeout(timeout).ok()
            }
            None => inbox.recv().ok(),
        };
        match received {
            Some(completion) => {
                self.deferred.push_back(completion);
                true
            }
            None => false,
        }
    }

    pub fn next_completion_task_id(&mut self) -> Option<TaskId> {
        if self.deferred.is_empty() {
            if let Ok(completion) = self
                .inbox
                .as_ref()
                .map_or(Err(TryRecvError::Disconnected), Receiver::try_recv)
            {
                self.deferred.push_back(completion);
            }
        }
        let completion = self.deferred.front();
        completion.and_then(|completion| {
            let key = WaitKey {
                runtime: self.runtime_id,
                id: completion.wait_id,
                generation: completion.generation,
            };
            self.active.get(&key).map(|wait| wait.task_id)
        })
    }

    pub fn drain_unowned_one(&mut self) -> bool {
        if self.next_completion_task_id().is_some() {
            return false;
        }
        let Some(completion) = self.deferred.pop_front() else {
            return false;
        };
        let key = WaitKey {
            runtime: self.runtime_id,
            id: completion.wait_id,
            generation: completion.generation,
        };
        if self.cleanup.get(&key).is_some_and(|entry| {
            entry.quarantine
                && entry.runtime_id == completion.runtime_id
                && entry.operation_id == completion.operation_id
                && entry.kind == completion.kind
        }) {
            self.remove_cleanup(key);
            self.quarantine_reaped += 1;
        } else {
            self.late_completions += 1;
        }
        true
    }

    pub fn cancel(
        &mut self,
        task: &mut TaskRecord,
        key: WaitKey,
        now: Instant,
    ) -> Option<PendingResume> {
        let wait = self.active.get(&key)?;
        if wait.task_id != task.id() || task.wait_key() != Some(key) {
            return None;
        }
        let reason = task.cancellation()?.reason;
        task.wake(key).ok()?;
        let wait = self.active.remove(&key).expect("validated active wait");
        let _ = wait.queue_cancel.cancel_before_start();
        self.cancel_or_transfer_resource(key, &wait.identity, wait.resource, true, now);
        drop(wait.decoder);
        Some(PendingResume {
            key,
            task_id: wait.task_id,
            decoder: None,
            continuation: wait.continuation,
            context: wait.context,
            raw: None,
            input: Some(ResumeInput::Cancelled(reason)),
            cancellation: CancellationView::new(true, Some(reason)),
        })
    }

    fn cancel_or_transfer_resource(
        &mut self,
        key: WaitKey,
        identity: &RetainedIdentity,
        mut resource: ResourceClass,
        admitted: bool,
        now: Instant,
    ) {
        let cancellation = resource.cancel();
        let quarantine = resource.bound().is_some();
        let retain = match &cancellation {
            Some(Ok(sema_core::runtime::CancelDisposition::Reaped)) => false,
            Some(Ok(sema_core::runtime::CancelDisposition::PendingReap) | Err(_)) => true,
            None => quarantine && admitted,
        };
        if retain {
            let last_error = cancellation
                .and_then(Result::err)
                .map(|error| error.to_string());
            self.insert_cleanup(key, identity, resource, quarantine, last_error, now);
        }
    }

    fn insert_cleanup(
        &mut self,
        key: WaitKey,
        identity: &RetainedIdentity,
        resource: ResourceClass,
        quarantine: bool,
        last_error: Option<String>,
        transferred_at: Instant,
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
                transferred_at,
            },
        );
        self.cleanup_order.push_back(key);
    }

    fn remove_cleanup(&mut self, key: WaitKey) -> Option<CleanupEntry> {
        let entry = self.cleanup.remove(&key)?;
        // Exact completions leave a charged tombstone so their hot path never
        // scans unrelated cleanup entries. A later bounded reap turn consumes it.
        self.cleanup_tombstones += 1;
        Some(entry)
    }

    pub fn reap_cleanup(&mut self, limit: usize) -> usize {
        let mut reaped = 0;
        for _ in 0..limit.min(self.cleanup_order.len()) {
            let key = self
                .cleanup_order
                .pop_front()
                .expect("bounded by queue length");
            let Some(entry) = self.cleanup.get_mut(&key) else {
                self.cleanup_tombstones = self.cleanup_tombstones.saturating_sub(1);
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

    /// Whether `key` names a live external wait (an in-flight offloaded op). Used
    /// to classify a cancelled Waiting task's kind at cancellation-request time so
    /// its executor abort runs immediately, not at the next drive scan.
    pub fn is_active(&self, key: WaitKey) -> bool {
        self.active.contains_key(&key)
    }
    pub fn cleanup_len(&self) -> usize {
        self.cleanup.len()
    }

    pub fn take_lease(&mut self) -> Option<Arc<dyn ExecutorLease>> {
        self.registrar.take();
        self.lease.take()
    }

    pub fn close_inbox(&mut self) {
        self.inbox.take();
    }
    pub fn is_closed(&self) -> bool {
        self.lease.is_none()
    }
    pub fn late_completions(&self) -> usize {
        self.late_completions
    }
    pub fn quarantine_reaped(&self) -> usize {
        self.quarantine_reaped
    }
    pub fn cleanup_diagnostics(&self) -> Vec<CleanupDiagnostic> {
        self.cleanup_diagnostics_at(Instant::now())
    }

    pub fn cleanup_diagnostics_at(&self, now: Instant) -> Vec<CleanupDiagnostic> {
        let mut diagnostics = self
            .cleanup
            .iter()
            .map(|(wait, entry)| CleanupDiagnostic {
                wait: *wait,
                operation: entry.operation_id,
                resource: entry.resource.kind().to_owned(),
                reap_attempts: entry.reap_attempts,
                last_error: entry.last_error.clone(),
                suppressed_hook_error: entry.last_error.clone(),
                quarantine: entry.quarantine,
                bound_expired: cleanup_bound_expired(entry, now),
            })
            .collect::<Vec<_>>();
        diagnostics.sort_by_key(|item| (item.wait.id, item.wait.generation));
        diagnostics
    }

    pub fn expired_quarantine(&self, now: Instant) -> Option<WaitKey> {
        self.cleanup
            .iter()
            .find_map(|(key, entry)| cleanup_bound_expired(entry, now).then_some(*key))
    }
    #[cfg(test)]
    pub fn cleanup_tombstones(&self) -> usize {
        self.cleanup_tombstones
    }
    #[cfg(test)]
    pub fn remove_cleanup_exact_for_test(&mut self, key: WaitKey) -> bool {
        self.remove_cleanup(key).is_some()
    }

    #[cfg(test)]
    pub fn force_identity_exhaustion_for_test(&mut self, kind: &str) {
        match kind {
            "wait" => self.force_wait_exhaustion = true,
            "operation" => self.force_operation_exhaustion = true,
            _ => panic!("unknown completion identity kind: {kind}"),
        }
    }

    #[cfg(test)]
    pub fn forge_active_completion_for_test(
        &mut self,
        key: WaitKey,
        mutation: ForgedCompletionMutation,
        result: Result<sema_core::runtime::SendPayload, ExternalFailure>,
    ) {
        let identity = self.active.get(&key).expect("active wait identity");
        let mut completion = ExternalCompletion {
            runtime_id: identity.identity.runtime_id,
            wait_id: identity.identity.wait_id,
            generation: identity.identity.generation,
            operation_id: identity.identity.operation_id,
            kind: identity.identity.kind,
            result,
        };
        match mutation {
            ForgedCompletionMutation::None => {}
            ForgedCompletionMutation::Runtime(id) => completion.runtime_id = id,
            ForgedCompletionMutation::Operation(id) => completion.operation_id = id,
            ForgedCompletionMutation::Kind(kind) => completion.kind = kind,
            ForgedCompletionMutation::Generation(generation) => {
                completion.generation = generation;
            }
        }
        self.deferred.push_front(completion);
    }

    #[cfg(test)]
    pub fn first_active_key_for_test(&self) -> WaitKey {
        *self.active.keys().next().expect("active wait")
    }
}

#[cfg(test)]
pub enum ForgedCompletionMutation {
    None,
    Runtime(RuntimeId),
    Operation(OperationId),
    Kind(CompletionKind),
    Generation(WaitGeneration),
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

fn cleanup_bound_expired(entry: &CleanupEntry, now: Instant) -> bool {
    match entry.resource.bound() {
        Some(sema_core::runtime::QuarantineBoundDescriptor::HardDeadline(duration)) => {
            now.saturating_duration_since(entry.transferred_at) >= duration
        }
        Some(sema_core::runtime::QuarantineBoundDescriptor::FiniteWork {
            maximum_units, ..
        }) => entry.reap_attempts as u64 >= maximum_units.get(),
        None => false,
    }
}
