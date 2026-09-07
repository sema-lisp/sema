use super::*;

/// Policy boundary recorded by the workflow journal sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyBoundary {
    Model,
    Tool,
    LlmInput,
    LlmOutput,
}

impl PolicyBoundary {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Tool => "tool",
            Self::LlmInput => "llm.input",
            Self::LlmOutput => "llm.output",
        }
    }
}

/// Where a policy-checked model result is about to come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySource {
    Request,
    Cache,
    Cassette,
}

impl PolicySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Cache => "cache",
            Self::Cassette => "cassette",
        }
    }
}

/// The journal-facing result of checking or bypassing one policy layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyObservation {
    pub kind: PolicyObservationKind,
    pub policy: String,
    pub policy_digest: String,
    pub boundary: PolicyBoundary,
    pub subject: String,
    pub subject_digest: Option<String>,
    pub rule: String,
    pub label: Option<String>,
    pub count: Option<usize>,
    pub action: Option<String>,
    pub reason: Option<String>,
    pub source: PolicySource,
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyObservationKind {
    Checked,
    Flagged,
    Redacted,
    Violation,
    Bypassed,
}

/// Sink installed by `workflow/run`; it captures only a weak workflow context.
pub type PolicyDecisionSink = Rc<dyn Fn(PolicyObservation)>;

#[derive(Clone)]
pub(super) struct ActivePolicy {
    policy: Rc<sema_policy::CompiledPolicy>,
    workspace_root: PathBuf,
    sink: PolicyDecisionSink,
}

/// Effective result of checking all active policy layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PolicyGate<T> {
    Allow,
    Deny(PolicyDecision<T>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PolicyDecision<T> {
    action: T,
    denial: PolicyDenial,
}

/// RAII guard for one workflow or step policy layer.
pub struct PolicyScope {
    previous: Option<Vec<ActivePolicy>>,
}

impl Drop for PolicyScope {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            ACTIVE_POLICIES.with(|policies| *policies.borrow_mut() = previous);
        }
    }
}

/// RAII guard for a trusted lexical policy bypass.
pub struct PolicyBypassScope {
    previous: Option<Vec<String>>,
}

impl Drop for PolicyBypassScope {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            POLICY_BYPASS.with(|bypass| *bypass.borrow_mut() = previous);
        }
    }
}

/// RAII guard for step-level journal attribution.
pub struct PolicyAttributionScope {
    previous: Option<String>,
}

impl Drop for PolicyAttributionScope {
    fn drop(&mut self) {
        POLICY_AGENT_ID.with(|agent| *agent.borrow_mut() = self.previous.take());
    }
}

/// Install compiled policy layers atomically for the dynamic extent of a workflow or step.
pub fn open_policy_scopes(
    policies_to_add: Vec<Rc<sema_policy::CompiledPolicy>>,
    workspace_root: PathBuf,
    sink: PolicyDecisionSink,
) -> PolicyScope {
    let previous = ACTIVE_POLICIES.with(|policies| {
        let previous = policies.borrow().clone();
        policies
            .borrow_mut()
            .extend(policies_to_add.into_iter().map(|policy| ActivePolicy {
                policy,
                workspace_root: workspace_root.clone(),
                sink: sink.clone(),
            }));
        previous
    });
    PolicyScope {
        previous: Some(previous),
    }
}

/// Disable active policies for a trusted lexical extent while retaining audit
/// observations and all sandbox capability checks.
pub fn open_policy_bypass(reason: String) -> PolicyBypassScope {
    let previous = POLICY_BYPASS.with(|bypass| {
        let previous = bypass.borrow().clone();
        bypass.borrow_mut().push(reason);
        previous
    });
    PolicyBypassScope {
        previous: Some(previous),
    }
}

/// Attribute policy observations to one workflow step.
pub fn open_policy_attribution(agent_id: String) -> PolicyAttributionScope {
    let previous = POLICY_AGENT_ID.with(|agent| agent.borrow_mut().replace(agent_id));
    PolicyAttributionScope { previous }
}

/// Whether at least one policy layer is currently active.
pub fn policy_active() -> bool {
    ACTIVE_POLICIES.with(|policies| !policies.borrow().is_empty())
}

/// Stable digest of the ordered effective policy stack and bypass state.
pub fn effective_policy_fingerprint() -> String {
    let policies = ACTIVE_POLICIES.with(|policies| policies.borrow().clone());
    if policies.is_empty() {
        return String::new();
    }
    let bypass = POLICY_BYPASS.with(|bypass| bypass.borrow().last().cloned());
    let mut hasher = Sha256::new();
    hasher.update(b"sema-effective-policy-v1\0");
    for layer in policies {
        hasher.update(layer.policy.fingerprint().as_bytes());
        hasher.update(b"\0");
    }
    if let Some(reason) = bypass {
        hasher.update(b"bypass\0");
        hasher.update(reason.as_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
}

/// Check all active model policy layers. `minimum_action` upgrades `:skip`
/// outside a real fallback selection so the journal records the action that
/// enforcement will actually take.
pub(super) fn check_model_policy(
    provider: &str,
    model: &str,
    source: PolicySource,
    minimum_action: sema_policy::ModelDenyAction,
) -> PolicyGate<sema_policy::ModelDenyAction> {
    let subject = format!("{provider}/{model}");
    check_active_policies(
        PolicyBoundary::Model,
        &subject,
        None,
        source,
        |layer| layer.policy.check_model(provider, model),
        |layer| layer.policy.model_action().max(Some(minimum_action)),
    )
}

pub(super) fn policy_denied(denial: PolicyDenial) -> SemaError {
    SemaError::policy_denied(denial)
}

pub(super) fn unnamed_policy_denial(
    boundary: PolicyBoundary,
    subject: impl Into<String>,
    rule: impl Into<String>,
    reason: impl Into<String>,
    action: impl Into<String>,
    source: PolicySource,
) -> PolicyDenial {
    PolicyDenial {
        policy: None,
        boundary: boundary.as_str().to_string(),
        subject: subject.into(),
        rule: rule.into(),
        reason: reason.into(),
        action: action.into(),
        source: source.as_str().to_string(),
    }
}

/// Check a resolved model target. `Ok(false)` means a fallback-only `:skip`
/// denial; every other denial is a hard error.
pub(super) fn model_target_allowed(
    provider: &str,
    model: &str,
    source: PolicySource,
    fallback_target: bool,
) -> Result<bool, SemaError> {
    let minimum_action = if fallback_target {
        sema_policy::ModelDenyAction::Skip
    } else {
        sema_policy::ModelDenyAction::Fail
    };
    match check_model_policy(provider, model, source, minimum_action) {
        PolicyGate::Allow => Ok(true),
        PolicyGate::Deny(decision)
            if decision.action == sema_policy::ModelDenyAction::Skip && fallback_target =>
        {
            Ok(false)
        }
        PolicyGate::Deny(decision) => Err(policy_denied(decision.denial)),
    }
}

/// Resolve and check every batch target before the provider starts any request.
pub(super) fn resolve_batch_models(
    provider: &dyn LlmProvider,
    requests: impl IntoIterator<Item = ChatRequest>,
) -> Result<Vec<ChatRequest>, SemaError> {
    requests
        .into_iter()
        .map(|mut request| {
            apply_input_policy_to_request(&mut request)?;
            if request.model.is_empty() {
                request.model = provider.default_model().to_string();
            }
            model_target_allowed(
                provider.name(),
                &request.model,
                PolicySource::Request,
                false,
            )?;
            Ok(request)
        })
        .collect()
}

pub(super) fn enforce_stored_model_policy(
    provider: &str,
    model: &str,
    source: PolicySource,
) -> Result<(), SemaError> {
    if !policy_active() {
        return Ok(());
    }
    if provider.is_empty() {
        return Err(policy_denied(unnamed_policy_denial(
            PolicyBoundary::Model,
            model,
            format!("{}.missing-provider", source.as_str()),
            "stored model metadata does not identify a provider",
            "fail",
            source,
        )));
    }
    model_target_allowed(provider, model, source, false).map(|_| ())
}

pub(super) fn preflight_tool_calls(
    calls: &[ToolCall],
    tools: &[Value],
) -> Result<BTreeMap<String, String>, SemaError> {
    let mut denied = BTreeMap::new();
    let mut hard_denial = None;
    for call in calls {
        let definition = tools.iter().find_map(|tool| {
            tool.as_tool_def_rc()
                .filter(|definition| definition.name == call.name)
        });
        let policy_subjects = definition
            .as_deref()
            .map_or(&[][..], |definition| definition.policy_subjects.as_slice());
        match check_tool_policy(
            &call.name,
            &call.arguments,
            policy_subjects,
            sema_policy::ToolDenyAction::ToolError,
        ) {
            PolicyGate::Allow => {}
            PolicyGate::Deny(decision)
                if decision.action == sema_policy::ToolDenyAction::ToolError =>
            {
                denied.insert(
                    call.id.clone(),
                    format!(
                        "tool '{}' was blocked: {}",
                        call.name, decision.denial.reason
                    ),
                );
            }
            PolicyGate::Deny(decision) => {
                hard_denial.get_or_insert(decision.denial);
            }
        }
    }
    if let Some(denial) = hard_denial {
        return Err(policy_denied(denial));
    }
    Ok(denied)
}

pub(super) fn enforce_direct_tool_policy(
    tool: &str,
    arguments: &serde_json::Value,
    policy_subjects: &[sema_core::ToolPolicySubject],
) -> Result<(), SemaError> {
    match check_tool_policy(
        tool,
        arguments,
        policy_subjects,
        sema_policy::ToolDenyAction::Fail,
    ) {
        PolicyGate::Allow => Ok(()),
        PolicyGate::Deny(decision) => Err(policy_denied(decision.denial)),
    }
}

/// Check all active tool policy layers.
pub(super) fn check_tool_policy(
    tool: &str,
    arguments: &serde_json::Value,
    policy_subjects: &[sema_core::ToolPolicySubject],
    minimum_action: sema_policy::ToolDenyAction,
) -> PolicyGate<sema_policy::ToolDenyAction> {
    let subject_digest = policy_value_digest(arguments);
    check_active_policies(
        PolicyBoundary::Tool,
        tool,
        Some(subject_digest),
        PolicySource::Request,
        |layer| {
            layer
                .policy
                .check_tool(tool, arguments, policy_subjects, &layer.workspace_root)
        },
        |layer| layer.policy.tool_action().max(Some(minimum_action)),
    )
}

pub(super) trait PolicyAction: Copy + Ord {
    fn name(self) -> &'static str;
}

impl PolicyAction for sema_policy::ModelDenyAction {
    fn name(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Fail => "fail",
        }
    }
}

impl PolicyAction for sema_policy::ToolDenyAction {
    fn name(self) -> &'static str {
        match self {
            Self::ToolError => "tool-error",
            Self::Fail => "fail",
        }
    }
}

pub(super) fn check_active_policies<T: PolicyAction>(
    boundary: PolicyBoundary,
    subject: &str,
    subject_digest: Option<String>,
    source: PolicySource,
    check: impl Fn(&ActivePolicy) -> sema_policy::PolicyCheck,
    action: impl Fn(&ActivePolicy) -> Option<T>,
) -> PolicyGate<T> {
    let policies = ACTIVE_POLICIES.with(|policies| policies.borrow().clone());
    if policies.is_empty() {
        return PolicyGate::Allow;
    }
    let agent_id = POLICY_AGENT_ID.with(|agent| agent.borrow().clone());
    if let Some(reason) = POLICY_BYPASS.with(|bypass| bypass.borrow().last().cloned()) {
        let fingerprint = effective_policy_fingerprint();
        let observation = PolicyObservation {
            kind: PolicyObservationKind::Bypassed,
            policy: "effective-policy".to_string(),
            policy_digest: fingerprint,
            boundary,
            subject: subject.to_string(),
            subject_digest,
            rule: "policy.without".to_string(),
            label: None,
            count: None,
            action: Some("bypass".to_string()),
            reason: Some(reason),
            source,
            agent_id,
        };
        (policies.last().expect("nonempty policy stack").sink)(observation);
        return PolicyGate::Allow;
    }

    let decisions: Vec<_> = policies
        .iter()
        .map(|layer| {
            let result = check(layer);
            let configured_action = (!result.allowed).then(|| action(layer)).flatten();
            (result, configured_action)
        })
        .collect();
    let denied_action = decisions.iter().filter_map(|(_, action)| *action).max();
    let denial = denied_action.and_then(|effective_action| {
        policies
            .iter()
            .zip(&decisions)
            .find(|(_, (result, configured_action))| {
                !result.allowed && *configured_action == Some(effective_action)
            })
            .map(|(layer, (result, _))| PolicyDecision {
                action: effective_action,
                denial: PolicyDenial {
                    policy: Some(layer.policy.name().to_string()),
                    boundary: boundary.as_str().to_string(),
                    subject: subject.to_string(),
                    rule: result.rule.clone(),
                    reason: result
                        .reason
                        .clone()
                        .unwrap_or_else(|| "the active policy denied this operation".to_string()),
                    action: effective_action.name().to_string(),
                    source: source.as_str().to_string(),
                },
            })
    });

    for (layer, (result, configured_action)) in policies.iter().zip(decisions) {
        let observation = PolicyObservation {
            kind: if result.allowed {
                PolicyObservationKind::Checked
            } else {
                PolicyObservationKind::Violation
            },
            policy: layer.policy.name().to_string(),
            policy_digest: layer.policy.fingerprint().to_string(),
            boundary,
            subject: subject.to_string(),
            subject_digest: subject_digest.clone(),
            rule: result.rule,
            label: None,
            count: None,
            // Journal what the boundary will actually do. A stricter denial in
            // any active layer upgrades every violation observation to that
            // effective action.
            action: configured_action
                .and(denied_action)
                .map(|action| action.name().to_string()),
            reason: result.reason,
            source,
            agent_id: agent_id.clone(),
        };
        (layer.sink)(observation);
    }
    denial.map_or(PolicyGate::Allow, PolicyGate::Deny)
}

pub(super) fn policy_value_digest(value: &serde_json::Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(value).unwrap_or_default());
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn policy_text_digest(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn output_policy_active() -> bool {
    ACTIVE_POLICIES.with(|policies| {
        policies
            .borrow()
            .iter()
            .any(|layer| layer.policy.has_output_policy())
    })
}

pub(super) fn apply_text_policy(
    text: &str,
    boundary: PolicyBoundary,
    subject: &str,
    source: PolicySource,
    output_stage: Option<sema_policy::OutputStage>,
) -> Result<String, SemaError> {
    let policies = ACTIVE_POLICIES.with(|policies| policies.borrow().clone());
    let policies: Vec<_> = policies
        .into_iter()
        .filter(|layer| match boundary {
            PolicyBoundary::LlmInput => layer.policy.has_input_policy(),
            PolicyBoundary::LlmOutput => layer.policy.has_output_policy(),
            PolicyBoundary::Model | PolicyBoundary::Tool => false,
        })
        .collect();
    if policies.is_empty() {
        return Ok(text.to_string());
    }
    if text.len() > sema_policy::content::INPUT_BYTE_CAP {
        return Err(policy_denied(unnamed_policy_denial(
            boundary,
            subject,
            "content.input-too-large",
            format!(
                "content exceeds the {}-byte policy limit",
                sema_policy::content::INPUT_BYTE_CAP
            ),
            "block",
            source,
        )));
    }
    let subject_digest = Some(policy_text_digest(text));
    let agent_id = POLICY_AGENT_ID.with(|agent| agent.borrow().clone());
    if let Some(reason) = POLICY_BYPASS.with(|bypass| bypass.borrow().last().cloned()) {
        let observation = PolicyObservation {
            kind: PolicyObservationKind::Bypassed,
            policy: "effective-policy".to_string(),
            policy_digest: effective_policy_fingerprint(),
            boundary,
            subject: subject.to_string(),
            subject_digest,
            rule: "policy.without".to_string(),
            label: None,
            count: None,
            action: Some("bypass".to_string()),
            reason: Some(reason),
            source,
            agent_id,
        };
        (policies.last().expect("nonempty content policy stack").sink)(observation);
        return Ok(text.to_string());
    }

    let outcomes: Vec<_> = policies
        .iter()
        .map(|layer| {
            let outcome = match boundary {
                PolicyBoundary::LlmInput => layer.policy.check_input(text),
                PolicyBoundary::LlmOutput => layer.policy.check_output(
                    text,
                    output_stage.unwrap_or(sema_policy::OutputStage::Final),
                ),
                PolicyBoundary::Model | PolicyBoundary::Tool => {
                    unreachable!("content policy called for non-content boundary")
                }
            };
            (layer, outcome)
        })
        .collect();
    let effective_action = outcomes
        .iter()
        .map(|(_, outcome)| outcome.action)
        .max()
        .unwrap_or(sema_policy::ContentAction::Allow);

    for (layer, outcome) in &outcomes {
        if outcome.findings.is_empty() {
            (layer.sink)(PolicyObservation {
                kind: PolicyObservationKind::Checked,
                policy: layer.policy.name().to_string(),
                policy_digest: layer.policy.fingerprint().to_string(),
                boundary,
                subject: subject.to_string(),
                subject_digest: subject_digest.clone(),
                rule: format!("{}.checked", boundary.as_str()),
                label: None,
                count: None,
                action: None,
                reason: None,
                source,
                agent_id: agent_id.clone(),
            });
            continue;
        }
        for finding in &outcome.findings {
            let kind = match outcome.action {
                sema_policy::ContentAction::Block => PolicyObservationKind::Violation,
                sema_policy::ContentAction::Redact => PolicyObservationKind::Redacted,
                sema_policy::ContentAction::Audit => PolicyObservationKind::Flagged,
                sema_policy::ContentAction::Allow => PolicyObservationKind::Checked,
            };
            (layer.sink)(PolicyObservation {
                kind,
                policy: layer.policy.name().to_string(),
                policy_digest: layer.policy.fingerprint().to_string(),
                boundary,
                subject: subject.to_string(),
                subject_digest: subject_digest.clone(),
                rule: finding.rule_id.clone(),
                label: Some(finding.label.clone()),
                count: Some(finding.count),
                action: Some(outcome.action.as_str().to_string()),
                reason: (outcome.action == sema_policy::ContentAction::Block)
                    .then(|| "deterministic content policy matched".to_string()),
                source,
                agent_id: agent_id.clone(),
            });
        }
    }

    match effective_action {
        sema_policy::ContentAction::Block => {
            let denial = outcomes
                .iter()
                .filter(|(_, outcome)| outcome.action == sema_policy::ContentAction::Block)
                .find_map(|(layer, outcome)| {
                    outcome.findings.first().map(|finding| PolicyDenial {
                        policy: Some(layer.policy.name().to_string()),
                        boundary: boundary.as_str().to_string(),
                        subject: subject.to_string(),
                        rule: finding.rule_id.clone(),
                        reason: format!(
                            "content matched {} ({} {})",
                            finding.label,
                            finding.count,
                            if finding.count == 1 {
                                "finding"
                            } else {
                                "findings"
                            }
                        ),
                        action: effective_action.as_str().to_string(),
                        source: source.as_str().to_string(),
                    })
                })
                .unwrap_or_else(|| {
                    unnamed_policy_denial(
                        boundary,
                        subject,
                        "content.denied",
                        "content policy blocked this value",
                        effective_action.as_str(),
                        source,
                    )
                });
            Err(policy_denied(denial))
        }
        sema_policy::ContentAction::Redact => {
            let redactions = outcomes
                .iter()
                .flat_map(|(_, outcome)| outcome.redactions.iter().cloned())
                .collect::<Vec<_>>();
            Ok(sema_policy::content::redact(text, &redactions))
        }
        sema_policy::ContentAction::Allow | sema_policy::ContentAction::Audit => {
            Ok(text.to_string())
        }
    }
}

pub(super) fn apply_input_policy_to_request(request: &mut ChatRequest) -> Result<(), SemaError> {
    let mut system = request.system.clone();
    if let Some(value) = &mut system {
        *value = apply_text_policy(
            value,
            PolicyBoundary::LlmInput,
            "system",
            PolicySource::Request,
            None,
        )?;
    }
    let mut messages = request.messages.clone();
    for (message_index, message) in messages.iter_mut().enumerate() {
        let subject = format!("message.{message_index}.{}", message.role);
        match &mut message.content {
            MessageContent::Text(text) => {
                *text = apply_text_policy(
                    text,
                    PolicyBoundary::LlmInput,
                    &subject,
                    PolicySource::Request,
                    None,
                )?;
            }
            MessageContent::Blocks(blocks) => {
                for (block_index, block) in blocks.iter_mut().enumerate() {
                    if let ContentBlock::Text { text } = block {
                        *text = apply_text_policy(
                            text,
                            PolicyBoundary::LlmInput,
                            &format!("{subject}.block.{block_index}"),
                            PolicySource::Request,
                            None,
                        )?;
                    }
                }
            }
        }
    }
    request.system = system;
    request.messages = messages;
    Ok(())
}

pub(super) fn apply_output_policy_to_response(
    response: &mut ChatResponse,
    source: PolicySource,
) -> Result<(), SemaError> {
    let stage = if response.tool_calls.is_empty() {
        sema_policy::OutputStage::Final
    } else {
        sema_policy::OutputStage::Round
    };
    let subject = match stage {
        sema_policy::OutputStage::Round => "assistant.round",
        sema_policy::OutputStage::Final => "assistant.final",
    };
    response.content = apply_text_policy(
        &response.content,
        PolicyBoundary::LlmOutput,
        subject,
        source,
        Some(stage),
    )?;
    Ok(())
}

pub(super) fn apply_input_policy_to_texts(
    texts: &mut [String],
    subject_prefix: &str,
) -> Result<(), SemaError> {
    let transformed = texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            apply_text_policy(
                text,
                PolicyBoundary::LlmInput,
                &format!("{subject_prefix}[{index}]"),
                PolicySource::Request,
                None,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (text, safe) in texts.iter_mut().zip(transformed) {
        *text = safe;
    }
    Ok(())
}
