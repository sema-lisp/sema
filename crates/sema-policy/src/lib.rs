//! Deterministic workflow policy compilation and boundary matching.
//!
//! This crate is deliberately runtime-agnostic. It compiles immutable Sema maps
//! into Rust-only policy data, evaluates resolved model identities and
//! model-supplied tool arguments, and returns decisions for the workflow/LLM
//! integration layers to enforce and journal.

pub mod content;

use globset::{GlobBuilder, GlobMatcher};
use regex::Regex;
use sema_core::{suggest_similar, FileAccess, ToolPolicySubject, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use url::{Host, Url};

const POLICY_VERSION: i64 = 1;

/// A policy definition or matcher compilation error.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{message}")]
pub struct PolicyError {
    message: String,
    hint: Option<String>,
}

impl PolicyError {
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
}

/// The default effect when no explicit rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultEffect {
    Allow,
    Deny,
}

/// What a model gate does with a denied fallback target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelDenyAction {
    Skip,
    Fail,
}

/// What an agent loop does with a denied tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolDenyAction {
    ToolError,
    Fail,
}

/// The result of checking one policy layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyCheck {
    pub allowed: bool,
    pub rule: String,
    pub reason: Option<String>,
}

impl PolicyCheck {
    fn allow(rule: impl Into<String>) -> Self {
        Self {
            allowed: true,
            rule: rule.into(),
            reason: None,
        }
    }

    fn deny(rule: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            rule: rule.into(),
            reason: Some(reason.into()),
        }
    }
}

/// Strictness lattice for deterministic content findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContentAction {
    Allow,
    Audit,
    Redact,
    Block,
}

impl ContentAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Audit => "audit",
            Self::Redact => "redact",
            Self::Block => "block",
        }
    }
}

/// Whether output validation is running on an intermediate tool-call round or
/// the terminal assistant response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStage {
    Round,
    Final,
}

/// Safe, aggregate content finding. It contains no matched text or byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyFinding {
    pub rule_id: String,
    pub label: String,
    pub action: ContentAction,
    pub count: usize,
}

/// Result of evaluating one policy layer against one original text value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentOutcome {
    pub action: ContentAction,
    pub findings: Vec<PolicyFinding>,
    /// Private transformation data for the enforcement layer; never journal it.
    pub redactions: Vec<content::Finding>,
}

impl ContentOutcome {
    fn allow() -> Self {
        Self {
            action: ContentAction::Allow,
            findings: Vec::new(),
            redactions: Vec::new(),
        }
    }
}

/// One compiled, named policy layer.
#[derive(Debug)]
pub struct CompiledPolicy {
    name: String,
    fingerprint: String,
    models: Option<ModelPolicy>,
    tools: Option<ToolPolicy>,
    subjects: Option<SubjectPolicy>,
    input: Option<DetectorPolicy>,
    output: Option<OutputPolicy>,
    metadata: Option<MetadataPolicy>,
    completion: Option<CompletionPolicy>,
}

impl CompiledPolicy {
    /// Compile a `defpolicy` map or an inline policy map.
    pub fn compile(value: &Value) -> Result<Self, PolicyError> {
        let map = require_map(value, "policy")?;
        reject_unknown_keys(
            map,
            &[
                "__policy-name",
                "__policy-version",
                "models",
                "tools",
                "subjects",
                "input",
                "output",
                "metadata",
                "completion",
            ],
            "policy",
        )?;

        let name = get(map, "__policy-name")
            .and_then(value_name)
            .unwrap_or_else(|| "inline-policy".to_string());
        if name.trim().is_empty() {
            return Err(invalid("policy name must not be empty"));
        }

        if let Some(version) = get(map, "__policy-version") {
            let version = version
                .as_int()
                .ok_or_else(|| invalid(":__policy-version must be an integer"))?;
            if version != POLICY_VERSION {
                return Err(invalid(format!(
                    "unsupported policy version {version}; expected {POLICY_VERSION}"
                )));
            }
        }

        let models = get(map, "models").map(ModelPolicy::compile).transpose()?;
        let tools = get(map, "tools").map(ToolPolicy::compile).transpose()?;
        let subjects = get(map, "subjects")
            .map(SubjectPolicy::compile)
            .transpose()?;
        let input = get(map, "input")
            .map(|value| DetectorPolicy::compile(value, ":input", true))
            .transpose()?;
        let output = get(map, "output").map(OutputPolicy::compile).transpose()?;
        let metadata = get(map, "metadata")
            .map(MetadataPolicy::compile)
            .transpose()?;
        let completion = get(map, "completion")
            .map(CompletionPolicy::compile)
            .transpose()?;
        let fingerprint = fingerprint(value, &name);

        Ok(Self {
            name,
            fingerprint,
            models,
            tools,
            subjects,
            input,
            output,
            metadata,
            completion,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Stable SHA-256 digest over the policy name, version, and canonical map.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The configured deny action, or `None` when no models section is present.
    pub fn model_action(&self) -> Option<ModelDenyAction> {
        self.models.as_ref().map(|policy| policy.on_deny)
    }

    /// The configured deny action, or `None` when no tools section is present.
    pub fn tool_action(&self) -> Option<ToolDenyAction> {
        self.tools.as_ref().map(|policy| policy.on_deny)
    }

    /// Check a fully resolved provider/model pair.
    pub fn check_model(&self, provider: &str, model: &str) -> PolicyCheck {
        self.models.as_ref().map_or_else(
            || PolicyCheck::allow("models.unrestricted"),
            |policy| policy.check(provider, model),
        )
    }

    /// Check a named tool and its model-supplied JSON arguments.
    pub fn check_tool(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
        policy_subjects: &[ToolPolicySubject],
        workspace_root: &Path,
    ) -> PolicyCheck {
        let named = self.tools.as_ref().map_or_else(
            || PolicyCheck::allow("tools.unrestricted"),
            |policy| policy.check(tool, arguments, workspace_root),
        );
        if !named.allowed {
            return named;
        }
        self.subjects.as_ref().map_or_else(
            || PolicyCheck::allow("subjects.unrestricted"),
            |policy| policy.check(policy_subjects, arguments, workspace_root),
        )
    }

    pub fn has_input_policy(&self) -> bool {
        self.input.is_some()
    }

    pub fn has_output_policy(&self) -> bool {
        self.output.is_some()
    }

    pub fn check_input(&self, text: &str) -> ContentOutcome {
        self.input
            .as_ref()
            .map_or_else(ContentOutcome::allow, |policy| policy.check(text))
    }

    pub fn check_output(&self, text: &str, stage: OutputStage) -> ContentOutcome {
        self.output
            .as_ref()
            .map_or_else(ContentOutcome::allow, |policy| policy.check(text, stage))
    }

    pub fn required_metadata(&self) -> impl Iterator<Item = &str> {
        self.metadata
            .iter()
            .flat_map(|policy| policy.required.iter().map(String::as_str))
    }

    pub fn required_completion_events(&self) -> impl Iterator<Item = &str> {
        self.completion
            .iter()
            .flat_map(|policy| policy.required_events.iter().map(String::as_str))
    }
}

#[derive(Debug)]
struct MetadataPolicy {
    required: BTreeSet<String>,
}

impl MetadataPolicy {
    fn compile(value: &Value) -> Result<Self, PolicyError> {
        let map = require_map(value, ":metadata")?;
        reject_unknown_keys(map, &["require"], ":metadata")?;
        let required = parse_name_set(get(map, "require"), ":metadata :require")?;
        if required.is_empty() {
            return Err(invalid(":metadata :require must not be empty"));
        }
        Ok(Self { required })
    }
}

#[derive(Debug)]
struct CompletionPolicy {
    required_events: BTreeSet<String>,
}

impl CompletionPolicy {
    fn compile(value: &Value) -> Result<Self, PolicyError> {
        let map = require_map(value, ":completion")?;
        reject_unknown_keys(map, &["require-events"], ":completion")?;
        let required_events =
            parse_name_set(get(map, "require-events"), ":completion :require-events")?;
        if required_events.is_empty() {
            return Err(invalid(":completion :require-events must not be empty"));
        }
        const SUPPORTED_EVENTS: &[&str] = &[
            "run.started",
            "phase.started",
            "phase.ended",
            "agent.started",
            "agent.result",
            "agent.tool_call",
            "agent.tool_result",
            "checkpoint",
            "budget",
            "auth.required",
            "auth.granted",
            "auth.failed",
            "approval.requested",
            "approval.granted",
            "approval.rejected",
            "approval.applied",
            "policy.checked",
            "policy.flagged",
            "policy.redacted",
            "policy.violation",
            "policy.bypassed",
        ];
        for event in &required_events {
            if !SUPPORTED_EVENTS.contains(&event.as_str()) {
                return Err(invalid_with_hint(
                    format!(":completion requires unsupported event {event:?}"),
                    format!("valid events are {}", SUPPORTED_EVENTS.join(", ")),
                ));
            }
        }
        Ok(Self { required_events })
    }
}

#[derive(Debug)]
struct ModelPolicy {
    default: DefaultEffect,
    allow: Vec<ModelPattern>,
    deny: Vec<ModelPattern>,
    on_deny: ModelDenyAction,
}

impl ModelPolicy {
    fn compile(value: &Value) -> Result<Self, PolicyError> {
        let map = require_map(value, ":models")?;
        reject_unknown_keys(map, &["default", "allow", "deny", "on-deny"], ":models")?;
        Ok(Self {
            default: parse_default(get(map, "default"), ":models")?,
            allow: parse_model_patterns(get(map, "allow"), ":models :allow")?,
            deny: parse_model_patterns(get(map, "deny"), ":models :deny")?,
            on_deny: match get(map, "on-deny") {
                None => ModelDenyAction::Fail,
                Some(value) => match value_name(value).as_deref() {
                    Some("fail") => ModelDenyAction::Fail,
                    Some("skip") => ModelDenyAction::Skip,
                    Some(other) => {
                        return Err(invalid_with_hint(
                            format!(":models :on-deny has unsupported value :{other}"),
                            "valid values are :fail and :skip",
                        ))
                    }
                    None => {
                        return Err(invalid(format!(
                            ":models :on-deny must be a keyword or string, got {}",
                            value.type_name()
                        )))
                    }
                },
            },
        })
    }

    fn check(&self, provider: &str, model: &str) -> PolicyCheck {
        if self
            .deny
            .iter()
            .any(|pattern| pattern.matches(provider, model))
        {
            return PolicyCheck::deny(
                "models.deny",
                format!("model {provider}/{model} matches a deny rule"),
            );
        }
        if self
            .allow
            .iter()
            .any(|pattern| pattern.matches(provider, model))
        {
            return PolicyCheck::allow("models.allow");
        }
        match self.default {
            DefaultEffect::Allow => PolicyCheck::allow("models.default-allow"),
            DefaultEffect::Deny => PolicyCheck::deny(
                "models.default-deny",
                format!("model {provider}/{model} is not allowlisted"),
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelPattern {
    provider: String,
    model: Option<String>,
}

impl ModelPattern {
    fn parse(value: &str) -> Result<Self, PolicyError> {
        let (provider, model) = value.split_once('/').ok_or_else(|| {
            invalid(format!(
                "model rule {value:?} must use provider/model syntax"
            ))
        })?;
        if provider.is_empty() || model.is_empty() {
            return Err(invalid(format!(
                "model rule {value:?} must have a nonempty provider and model"
            )));
        }
        if provider.contains('*') {
            return Err(invalid(format!(
                "model rule {value:?} cannot wildcard the provider"
            )));
        }
        if model.contains('*') && model != "*" {
            return Err(invalid(format!(
                "model rule {value:?} only supports the provider/* wildcard"
            )));
        }
        Ok(Self {
            provider: provider.to_string(),
            model: (model != "*").then(|| model.to_string()),
        })
    }

    fn matches(&self, provider: &str, model: &str) -> bool {
        self.provider == provider && self.model.as_deref().is_none_or(|rule| rule == model)
    }
}

#[derive(Debug)]
struct DetectorPolicy {
    rules: Vec<(content::DetectorKind, ContentAction)>,
    rule_prefix: &'static str,
}

impl DetectorPolicy {
    fn compile(value: &Value, context: &'static str, input: bool) -> Result<Self, PolicyError> {
        let map = require_map(value, context)?;
        reject_unknown_keys(map, &["detect", "actions"], context)?;
        Self::compile_from_map(map, context, input)
    }

    fn compile_from_map(
        map: &BTreeMap<Value, Value>,
        context: &'static str,
        input: bool,
    ) -> Result<Self, PolicyError> {
        let detectors = parse_detectors(get(map, "detect"), &format!("{context} :detect"))?;
        let actions = match get(map, "actions") {
            None => BTreeMap::new(),
            Some(value) => {
                let action_map = require_map(value, &format!("{context} :actions"))?;
                let mut actions = BTreeMap::new();
                for (key, value) in action_map {
                    let detector = parse_detector(key, &format!("{context} :actions"))?;
                    if !detectors.contains(&detector) {
                        return Err(invalid(format!(
                            "{context} :actions key :{} is not declared in :detect",
                            detector.as_str()
                        )));
                    }
                    let action = parse_content_action(value, &format!("{context} :actions"))?;
                    if action == ContentAction::Allow {
                        return Err(invalid(format!(
                            "{context} detector actions must be :audit, :redact, or :block"
                        )));
                    }
                    actions.insert(detector, action);
                }
                actions
            }
        };
        Ok(Self {
            rules: detectors
                .into_iter()
                .map(|detector| {
                    let action = actions
                        .get(&detector)
                        .copied()
                        .unwrap_or(ContentAction::Block);
                    (detector, action)
                })
                .collect(),
            rule_prefix: if input {
                "input.detect"
            } else {
                "output.detect"
            },
        })
    }

    fn check(&self, text: &str) -> ContentOutcome {
        let mut outcome = ContentOutcome::allow();
        for (detector, action) in &self.rules {
            let findings = content::scan(text, *detector);
            if findings.is_empty() {
                continue;
            }
            outcome.action = outcome.action.max(*action);
            let mut counts = BTreeMap::new();
            for finding in &findings {
                *counts.entry(finding.label).or_insert(0usize) += 1;
            }
            outcome
                .findings
                .extend(counts.into_iter().map(|(label, count)| PolicyFinding {
                    rule_id: format!("{}.{}", self.rule_prefix, detector.as_str()),
                    label: label.to_string(),
                    action: *action,
                    count,
                }));
            if *action == ContentAction::Redact {
                outcome.redactions.extend(findings);
            }
        }
        outcome
    }
}

#[derive(Debug)]
struct OutputPolicy {
    detectors: DetectorPolicy,
    schema: Option<OutputSchema>,
    required: BTreeSet<String>,
    max_length: Option<usize>,
    forbid: Vec<ForbidRule>,
    action: ContentAction,
}

impl OutputPolicy {
    fn compile(value: &Value) -> Result<Self, PolicyError> {
        let map = require_map(value, ":output")?;
        reject_unknown_keys(
            map,
            &[
                "detect",
                "actions",
                "schema",
                "require",
                "max-length",
                "forbid",
                "action",
            ],
            ":output",
        )?;
        let action = get(map, "action")
            .map(|value| parse_content_action(value, ":output :action"))
            .transpose()?
            .unwrap_or(ContentAction::Block);
        if !matches!(action, ContentAction::Audit | ContentAction::Block) {
            return Err(invalid(
                ":output :action must be :audit or :block; only detector spans may be redacted",
            ));
        }
        let max_length = get(map, "max-length")
            .map(|value| {
                value
                    .as_int()
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .ok_or_else(|| {
                        invalid(format!(
                            ":output :max-length must be a positive integer, got {value}"
                        ))
                    })
            })
            .transpose()?;
        Ok(Self {
            detectors: DetectorPolicy::compile_from_map(map, ":output", false)?,
            schema: get(map, "schema").map(OutputSchema::compile).transpose()?,
            required: parse_name_set(get(map, "require"), ":output :require")?,
            max_length,
            forbid: parse_forbid_rules(get(map, "forbid"))?,
            action,
        })
    }

    fn check(&self, text: &str, stage: OutputStage) -> ContentOutcome {
        let mut outcome = self.detectors.check(text);
        for rule in &self.forbid {
            let count = rule.count(text);
            if count == 0 {
                continue;
            }
            outcome.action = outcome.action.max(self.action);
            outcome.findings.push(PolicyFinding {
                rule_id: format!("output.forbid.{}", rule.id),
                label: rule.id.clone(),
                action: self.action,
                count,
            });
        }
        if stage == OutputStage::Round {
            return outcome;
        }

        if self
            .max_length
            .is_some_and(|maximum| text.chars().count() > maximum)
        {
            push_structural_finding(&mut outcome, "output.max-length", "max-length", self.action);
        }

        if self.schema.is_none() && self.required.is_empty() {
            return outcome;
        }
        let parsed = match serde_json::from_str::<serde_json::Value>(text) {
            Ok(parsed) => parsed,
            Err(_) => {
                push_structural_finding(
                    &mut outcome,
                    "output.schema.json",
                    "invalid-json",
                    self.action,
                );
                return outcome;
            }
        };
        if let Some(schema) = &self.schema {
            for finding in schema.validate(&parsed) {
                push_structural_finding(
                    &mut outcome,
                    &format!("output.schema.{finding}"),
                    &finding,
                    self.action,
                );
            }
        }
        let Some(object) = parsed.as_object() else {
            if !self.required.is_empty() {
                push_structural_finding(
                    &mut outcome,
                    "output.require.object",
                    "required-fields",
                    self.action,
                );
            }
            return outcome;
        };
        for key in &self.required {
            if object.get(key).is_none_or(is_empty_json) {
                push_structural_finding(
                    &mut outcome,
                    &format!("output.require.{key}"),
                    key,
                    self.action,
                );
            }
        }
        outcome
    }
}

fn push_structural_finding(
    outcome: &mut ContentOutcome,
    rule_id: &str,
    label: &str,
    action: ContentAction,
) {
    outcome.action = outcome.action.max(action);
    outcome.findings.push(PolicyFinding {
        rule_id: rule_id.to_string(),
        label: label.to_string(),
        action,
        count: 1,
    });
}

#[derive(Debug)]
struct ForbidRule {
    id: String,
    matcher: ForbidMatcher,
}

impl ForbidRule {
    fn compile(value: &Value, index: usize) -> Result<Self, PolicyError> {
        let context = format!(":output :forbid entry {}", index + 1);
        let map = require_map(value, &context)?;
        reject_unknown_keys(map, &["id", "contains", "regex"], &context)?;
        let id = get(map, "id")
            .and_then(value_name)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| invalid(format!("{context} requires a nonempty :id")))?;
        let contains = get(map, "contains").and_then(Value::as_str);
        let regex = get(map, "regex").and_then(Value::as_str);
        let matcher = match (contains, regex) {
            (Some(literal), None) if !literal.is_empty() => {
                ForbidMatcher::Contains(literal.to_string())
            }
            (None, Some(pattern)) if !pattern.is_empty() => ForbidMatcher::Regex(
                Regex::new(pattern)
                    .map_err(|error| invalid(format!("{context} invalid regex: {error}")))?,
            ),
            _ => {
                return Err(invalid(format!(
                    "{context} requires exactly one nonempty :contains or :regex"
                )))
            }
        };
        Ok(Self { id, matcher })
    }

    fn count(&self, text: &str) -> usize {
        match &self.matcher {
            ForbidMatcher::Contains(literal) => text.match_indices(literal).count(),
            ForbidMatcher::Regex(regex) => regex.find_iter(text).count(),
        }
    }
}

#[derive(Debug)]
enum ForbidMatcher {
    Contains(String),
    Regex(Regex),
}

fn parse_forbid_rules(value: Option<&Value>) -> Result<Vec<ForbidRule>, PolicyError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let rules = require_seq(value, ":output :forbid")?;
    let mut ids = BTreeSet::new();
    rules
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let rule = ForbidRule::compile(value, index)?;
            if !ids.insert(rule.id.clone()) {
                return Err(invalid(format!(
                    ":output :forbid has duplicate :id {:?}",
                    rule.id
                )));
            }
            Ok(rule)
        })
        .collect()
}

#[derive(Debug)]
struct OutputSchema {
    fields: BTreeMap<String, SchemaField>,
}

impl OutputSchema {
    fn compile(value: &Value) -> Result<Self, PolicyError> {
        let map = require_map(value, ":output :schema")?;
        let mut fields = BTreeMap::new();
        for (key, value) in map {
            let key = value_name(key)
                .filter(|key| !key.is_empty())
                .ok_or_else(|| invalid(":output :schema field names must be names or strings"))?;
            if fields
                .insert(key.clone(), SchemaField::compile(value, &key)?)
                .is_some()
            {
                return Err(invalid(format!(
                    ":output :schema has duplicate field {key:?}"
                )));
            }
        }
        Ok(Self { fields })
    }

    fn validate(&self, value: &serde_json::Value) -> Vec<String> {
        let Some(object) = value.as_object() else {
            return vec!["object".to_string()];
        };
        self.fields
            .iter()
            .filter_map(|(name, field)| match object.get(name) {
                None if !field.optional => Some(format!("{name}.missing")),
                Some(value) if !field.kind.matches(value) => Some(format!("{name}.type")),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug)]
struct SchemaField {
    kind: SchemaKind,
    optional: bool,
}

impl SchemaField {
    fn compile(value: &Value, name: &str) -> Result<Self, PolicyError> {
        if let Some(kind) = value_name(value) {
            return Ok(Self {
                kind: SchemaKind::parse(&kind, name)?,
                optional: false,
            });
        }
        let map = require_map(value, &format!(":output :schema field {name:?}"))?;
        reject_unknown_keys(
            map,
            &["type", "optional"],
            &format!(":output :schema field {name:?}"),
        )?;
        let kind = get(map, "type")
            .and_then(value_name)
            .ok_or_else(|| invalid(format!(":output :schema field {name:?} requires :type")))?;
        let optional = get(map, "optional")
            .map(|value| {
                value.as_bool().ok_or_else(|| {
                    invalid(format!(
                        ":output :schema field {name:?} :optional must be boolean"
                    ))
                })
            })
            .transpose()?
            .unwrap_or(false);
        Ok(Self {
            kind: SchemaKind::parse(&kind, name)?,
            optional,
        })
    }
}

#[derive(Debug)]
enum SchemaKind {
    String,
    Number,
    Boolean,
    List,
}

impl SchemaKind {
    fn parse(value: &str, name: &str) -> Result<Self, PolicyError> {
        match value {
            "string" => Ok(Self::String),
            "number" => Ok(Self::Number),
            "boolean" => Ok(Self::Boolean),
            "list" | "array" => Ok(Self::List),
            _ => Err(invalid(format!(
                ":output :schema field {name:?} has unsupported type {value:?}"
            ))),
        }
    }

    fn matches(&self, value: &serde_json::Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::List => value.is_array(),
        }
    }
}

fn is_empty_json(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(value) => value.trim().is_empty(),
        serde_json::Value::Array(value) => value.is_empty(),
        serde_json::Value::Object(value) => value.is_empty(),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => false,
    }
}

fn parse_detectors(
    value: Option<&Value>,
    context: &str,
) -> Result<BTreeSet<content::DetectorKind>, PolicyError> {
    let Some(value) = value else {
        return Ok(BTreeSet::new());
    };
    require_seq(value, context)?
        .iter()
        .enumerate()
        .map(|(index, value)| parse_detector(value, &format!("{context} entry {}", index + 1)))
        .collect()
}

fn parse_detector(value: &Value, context: &str) -> Result<content::DetectorKind, PolicyError> {
    match value_name(value).as_deref() {
        Some("secret") => Ok(content::DetectorKind::Secret),
        Some("email") => Ok(content::DetectorKind::Email),
        Some("phone") => Ok(content::DetectorKind::Phone),
        Some("ipv4") => Ok(content::DetectorKind::Ipv4),
        Some("payment-card") => Ok(content::DetectorKind::PaymentCard),
        Some(other) => Err(invalid_with_hint(
            format!("{context} has unsupported detector :{other}"),
            "valid detectors are :secret, :email, :phone, :ipv4, and :payment-card",
        )),
        None => Err(invalid(format!(
            "{context} must be a keyword or string, got {}",
            value.type_name()
        ))),
    }
}

fn parse_content_action(value: &Value, context: &str) -> Result<ContentAction, PolicyError> {
    match value_name(value).as_deref() {
        Some("allow") => Ok(ContentAction::Allow),
        Some("audit") => Ok(ContentAction::Audit),
        Some("redact") => Ok(ContentAction::Redact),
        Some("block") => Ok(ContentAction::Block),
        Some(other) => Err(invalid_with_hint(
            format!("{context} action has unsupported value :{other}"),
            "valid actions are :allow, :audit, :redact, and :block",
        )),
        None => Err(invalid(format!(
            "{context} action must be a keyword or string, got {}",
            value.type_name()
        ))),
    }
}

#[derive(Debug)]
struct ToolPolicy {
    default: DefaultEffect,
    allow: BTreeMap<String, ToolRule>,
    deny: BTreeSet<String>,
    on_deny: ToolDenyAction,
}

impl ToolPolicy {
    fn compile(value: &Value) -> Result<Self, PolicyError> {
        let map = require_map(value, ":tools")?;
        reject_unknown_keys(map, &["default", "allow", "deny", "on-deny"], ":tools")?;
        let allow = match get(map, "allow") {
            None => BTreeMap::new(),
            Some(value) => {
                let rules = require_map(value, ":tools :allow")?;
                let mut compiled = BTreeMap::new();
                for (name, rule) in rules {
                    let name = value_name(name)
                        .ok_or_else(|| invalid(":tools :allow keys must be tool names"))?;
                    if compiled.contains_key(&name) {
                        return Err(invalid(format!(
                            ":tools :allow has duplicate tool name {name:?}"
                        )));
                    }
                    compiled.insert(name.clone(), ToolRule::compile(&name, rule)?);
                }
                compiled
            }
        };
        let deny = parse_string_set(get(map, "deny"), ":tools :deny")?;
        let on_deny = match get(map, "on-deny") {
            None => ToolDenyAction::Fail,
            Some(value) => match value_name(value).as_deref() {
                Some("fail") => ToolDenyAction::Fail,
                Some("tool-error") => ToolDenyAction::ToolError,
                Some(other) => {
                    return Err(invalid_with_hint(
                        format!(":tools :on-deny has unsupported value :{other}"),
                        "valid values are :fail and :tool-error",
                    ))
                }
                None => {
                    return Err(invalid(format!(
                        ":tools :on-deny must be a keyword or string, got {}",
                        value.type_name()
                    )))
                }
            },
        };
        Ok(Self {
            default: parse_default(get(map, "default"), ":tools")?,
            allow,
            deny,
            on_deny,
        })
    }

    fn check(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
        workspace_root: &Path,
    ) -> PolicyCheck {
        if self.deny.contains(tool) {
            return PolicyCheck::deny(
                format!("tools.{tool}.deny"),
                format!("tool {tool} matches an explicit deny rule"),
            );
        }
        if let Some(rule) = self.allow.get(tool) {
            return rule.check(tool, arguments, workspace_root);
        }
        match self.default {
            DefaultEffect::Allow => PolicyCheck::allow("tools.default-allow"),
            DefaultEffect::Deny => PolicyCheck::deny(
                "tools.default-deny",
                format!("tool {tool} is not allowlisted"),
            ),
        }
    }
}

#[derive(Debug)]
struct SubjectPolicy {
    default: DefaultEffect,
    allow: Vec<SubjectRule>,
    deny: Vec<SubjectRule>,
}

impl SubjectPolicy {
    fn compile(value: &Value) -> Result<Self, PolicyError> {
        let map = require_map(value, ":subjects")?;
        reject_unknown_keys(map, &["default", "allow", "deny"], ":subjects")?;
        Ok(Self {
            default: parse_default(get(map, "default"), ":subjects")?,
            allow: parse_subject_rules(get(map, "allow"), ":subjects :allow")?,
            deny: parse_subject_rules(get(map, "deny"), ":subjects :deny")?,
        })
    }

    fn check(
        &self,
        specs: &[ToolPolicySubject],
        arguments: &serde_json::Value,
        workspace_root: &Path,
    ) -> PolicyCheck {
        let Some(arguments) = arguments.as_object() else {
            return PolicyCheck::deny("subjects.arguments", "tool arguments must be a JSON object");
        };
        if specs.is_empty() {
            return match self.default {
                DefaultEffect::Allow => PolicyCheck::allow("subjects.default-allow"),
                DefaultEffect::Deny => {
                    PolicyCheck::deny("subjects.missing", "tool has no declared policy subjects")
                }
            };
        }

        for spec in specs {
            let subject = match ResolvedSubject::resolve(spec, arguments) {
                Ok(subject) => subject,
                Err(reason) => {
                    return PolicyCheck::deny("subjects.arguments", reason);
                }
            };
            if self
                .deny
                .iter()
                .any(|rule| rule.matches(&subject, workspace_root, true))
            {
                return PolicyCheck::deny(
                    format!("subjects.{}.deny", subject.kind().as_str()),
                    format!(
                        "{} subject matches an explicit deny rule",
                        subject.kind().as_str()
                    ),
                );
            }
            if self
                .allow
                .iter()
                .any(|rule| rule.matches(&subject, workspace_root, false))
            {
                continue;
            }
            if self.default == DefaultEffect::Deny {
                return PolicyCheck::deny(
                    format!("subjects.{}.default-deny", subject.kind().as_str()),
                    format!("{} subject is not allowlisted", subject.kind().as_str()),
                );
            }
        }
        PolicyCheck::allow("subjects.allow")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubjectKind {
    FileRead,
    FileWrite,
    FileDelete,
    NetworkRequest,
    Command,
    ExternalAction,
}

impl SubjectKind {
    fn parse(value: &Value, context: &str) -> Result<Self, PolicyError> {
        match value_name(value).as_deref() {
            Some("file-read") => Ok(Self::FileRead),
            Some("file-write") => Ok(Self::FileWrite),
            Some("file-delete") => Ok(Self::FileDelete),
            Some("network-request") => Ok(Self::NetworkRequest),
            Some("command") => Ok(Self::Command),
            Some("external-action") => Ok(Self::ExternalAction),
            Some(other) => Err(invalid_with_hint(
                format!("{context} has unsupported :kind :{other}"),
                "valid kinds are :file-read, :file-write, :file-delete, :network-request, :command, and :external-action",
            )),
            None => Err(invalid(format!(
                "{context} :kind must be a keyword or string, got {}",
                value.type_name()
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::FileRead => "file-read",
            Self::FileWrite => "file-write",
            Self::FileDelete => "file-delete",
            Self::NetworkRequest => "network-request",
            Self::Command => "command",
            Self::ExternalAction => "external-action",
        }
    }
}

#[derive(Debug)]
struct SubjectRule {
    kind: SubjectKind,
    constraint: Option<CompiledConstraint>,
    methods: BTreeSet<String>,
    actions: BTreeSet<String>,
}

impl SubjectRule {
    fn compile(value: &Value, context: &str) -> Result<Self, PolicyError> {
        let map = require_map(value, context)?;
        reject_unknown_keys(
            map,
            &["kind", "paths", "domains", "commands", "methods", "actions"],
            context,
        )?;
        let kind = SubjectKind::parse(
            get(map, "kind").ok_or_else(|| invalid(format!("{context} requires :kind")))?,
            context,
        )?;
        let constraint = match kind {
            SubjectKind::FileRead | SubjectKind::FileWrite | SubjectKind::FileDelete => {
                get(map, "paths")
                    .map(|value| compile_subject_constraint(value, ConstraintKind::Path, context))
                    .transpose()?
            }
            SubjectKind::NetworkRequest => get(map, "domains")
                .map(|value| compile_subject_constraint(value, ConstraintKind::Domain, context))
                .transpose()?,
            SubjectKind::Command => get(map, "commands")
                .map(|value| compile_subject_constraint(value, ConstraintKind::Command, context))
                .transpose()?,
            SubjectKind::ExternalAction => None,
        };
        if kind != SubjectKind::NetworkRequest && get(map, "methods").is_some() {
            return Err(invalid(format!(
                "{context} :methods is valid only for :network-request"
            )));
        }
        if kind != SubjectKind::ExternalAction && get(map, "actions").is_some() {
            return Err(invalid(format!(
                "{context} :actions is valid only for :external-action"
            )));
        }
        Ok(Self {
            kind,
            constraint,
            methods: parse_name_set(get(map, "methods"), &format!("{context} :methods"))?
                .into_iter()
                .map(|method| method.to_ascii_uppercase())
                .collect(),
            actions: parse_name_set(get(map, "actions"), &format!("{context} :actions"))?,
        })
    }

    /// Decide whether this rule covers `subject`.
    ///
    /// `on_unevaluatable` is the answer for a subject the rule cannot inspect at all
    /// (`ConstraintMiss::evaluated` false): an unparsable or non-allowlisted-scheme URL,
    /// a path that escapes the workspace, or an absent request method. A deny rule
    /// passes `true` so an argument it cannot read still denies; an allow rule passes
    /// `false` so that argument is never allowlisted. Both directions fail closed.
    ///
    /// A value that *was* compared and simply is not covered stays a non-match for both
    /// polarities, so a deny rule naming one host does not deny every other host.
    ///
    /// Treating an un-evaluatable subject as a plain non-match made deny rules fail
    /// open: a `:domains` rule defaults `:schemes` to `["https"]`, so an `http://` URL
    /// produced a scheme error, did not match, and was allowed by `:default :allow`.
    fn matches(
        &self,
        subject: &ResolvedSubject,
        workspace_root: &Path,
        on_unevaluatable: bool,
    ) -> bool {
        if self.kind != subject.kind() {
            return false;
        }
        let constraint_matches = |value: &serde_json::Value| {
            self.constraint.as_ref().is_none_or(|constraint| {
                constraint
                    .check(value, workspace_root)
                    .map_or_else(|miss| !miss.evaluated && on_unevaluatable, |()| true)
            })
        };
        match subject {
            ResolvedSubject::File { path, .. } => constraint_matches(path),
            ResolvedSubject::NetworkRequest { method, url } => {
                let method_matches = self.methods.is_empty()
                    || match method.as_deref() {
                        Some(method) => self.methods.contains(method),
                        None => on_unevaluatable,
                    };
                method_matches && constraint_matches(url)
            }
            ResolvedSubject::Command(command) => constraint_matches(command),
            ResolvedSubject::ExternalAction { action, .. } => {
                self.actions.is_empty() || self.actions.contains(action)
            }
        }
    }
}

#[derive(Debug)]
enum ResolvedSubject {
    File {
        kind: SubjectKind,
        path: serde_json::Value,
    },
    NetworkRequest {
        method: Option<String>,
        url: serde_json::Value,
    },
    Command(serde_json::Value),
    ExternalAction {
        action: String,
        _target: Option<serde_json::Value>,
    },
}

impl ResolvedSubject {
    fn resolve(
        spec: &ToolPolicySubject,
        arguments: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, String> {
        let required = |name: &str| {
            arguments
                .get(name)
                .cloned()
                .ok_or_else(|| format!("required policy subject argument {name:?} is missing"))
        };
        match spec {
            ToolPolicySubject::File { access, path_arg } => Ok(Self::File {
                kind: match access {
                    FileAccess::Read => SubjectKind::FileRead,
                    FileAccess::Write => SubjectKind::FileWrite,
                    FileAccess::Delete => SubjectKind::FileDelete,
                },
                path: required(path_arg)?,
            }),
            ToolPolicySubject::NetworkRequest { method, url_arg } => Ok(Self::NetworkRequest {
                method: method.as_ref().map(|method| method.to_ascii_uppercase()),
                url: required(url_arg)?,
            }),
            ToolPolicySubject::Command { command_arg } => Ok(Self::Command(required(command_arg)?)),
            ToolPolicySubject::ExternalAction { action, target_arg } => Ok(Self::ExternalAction {
                action: action.clone(),
                _target: target_arg.as_deref().map(required).transpose()?,
            }),
        }
    }

    fn kind(&self) -> SubjectKind {
        match self {
            Self::File { kind, .. } => *kind,
            Self::NetworkRequest { .. } => SubjectKind::NetworkRequest,
            Self::Command(_) => SubjectKind::Command,
            Self::ExternalAction { .. } => SubjectKind::ExternalAction,
        }
    }
}

fn parse_subject_rules(
    value: Option<&Value>,
    context: &str,
) -> Result<Vec<SubjectRule>, PolicyError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    require_seq(value, context)?
        .iter()
        .enumerate()
        .map(|(index, value)| SubjectRule::compile(value, &format!("{context}[{index}]")))
        .collect()
}

fn compile_subject_constraint(
    value: &Value,
    kind: ConstraintKind,
    context: &str,
) -> Result<CompiledConstraint, PolicyError> {
    let selector = if let Some(map) = value.as_map_ref() {
        map.clone()
    } else {
        let values = require_seq(value, context)?;
        if !values.iter().all(|value| value.as_str().is_some()) {
            return Err(invalid(format!(
                "{context} constraint must be a selector map or string sequence"
            )));
        }
        shorthand_selector(&values)
    };
    let allowed_keys: &[&str] = match kind {
        ConstraintKind::Path | ConstraintKind::Command => &["allow", "deny"],
        ConstraintKind::Domain => &["allow", "deny", "schemes", "ports"],
    };
    reject_unknown_keys(&selector, allowed_keys, context)?;
    match kind {
        ConstraintKind::Path => PathConstraint::compile(&selector).map(CompiledConstraint::Path),
        ConstraintKind::Domain => {
            DomainConstraint::compile(&selector).map(CompiledConstraint::Domain)
        }
        ConstraintKind::Command => {
            CommandConstraint::compile(&selector).map(CompiledConstraint::Command)
        }
    }
}

#[derive(Debug, Default)]
struct ToolRule {
    constraints: Vec<ArgumentConstraint>,
}

impl ToolRule {
    fn compile(tool: &str, value: &Value) -> Result<Self, PolicyError> {
        let map = require_map(value, &format!("tool rule {tool:?}"))?;
        reject_unknown_keys(
            map,
            &["paths", "domains", "commands"],
            &format!("tool rule {tool:?}"),
        )?;
        let mut constraints = Vec::new();
        if let Some(value) = get(map, "paths") {
            constraints.extend(compile_constraints(
                value,
                "path",
                ConstraintKind::Path,
                tool,
            )?);
        }
        if let Some(value) = get(map, "domains") {
            constraints.extend(compile_constraints(
                value,
                "url",
                ConstraintKind::Domain,
                tool,
            )?);
        }
        if let Some(value) = get(map, "commands") {
            constraints.extend(compile_constraints(
                value,
                "command",
                ConstraintKind::Command,
                tool,
            )?);
        }
        Ok(Self { constraints })
    }

    fn check(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
        workspace_root: &Path,
    ) -> PolicyCheck {
        let Some(arguments) = arguments.as_object() else {
            return PolicyCheck::deny(
                format!("tools.{tool}.arguments"),
                "tool arguments must be a JSON object",
            );
        };
        for constraint in &self.constraints {
            if let Err(reason) = constraint.check(arguments, workspace_root) {
                return PolicyCheck::deny(
                    format!(
                        "tools.{tool}.{}.{}",
                        constraint.kind.label(),
                        constraint.argument
                    ),
                    reason,
                );
            }
        }
        PolicyCheck::allow(format!("tools.{tool}.allow"))
    }
}

#[derive(Debug, Clone, Copy)]
enum ConstraintKind {
    Path,
    Domain,
    Command,
}

impl ConstraintKind {
    fn label(self) -> &'static str {
        match self {
            Self::Path => "paths",
            Self::Domain => "domains",
            Self::Command => "commands",
        }
    }
}

#[derive(Debug)]
struct ArgumentConstraint {
    argument: String,
    kind: CompiledConstraint,
}

impl ArgumentConstraint {
    fn check(
        &self,
        arguments: &serde_json::Map<String, serde_json::Value>,
        workspace_root: &Path,
    ) -> Result<(), String> {
        let value = arguments
            .get(&self.argument)
            .ok_or_else(|| format!("required argument {:?} is missing", self.argument))?;
        // The tools section is an allowlist: every miss denies, so only the reason matters.
        self.kind
            .check(value, workspace_root)
            .map_err(|miss| miss.reason)
    }
}

#[derive(Debug)]
enum CompiledConstraint {
    Path(PathConstraint),
    Domain(DomainConstraint),
    Command(CommandConstraint),
}

/// Why a constraint did not accept a value.
///
/// `evaluated` is false when the value could not be compared against the rule's lists
/// at all: a non-string argument, an unparsable URL, a scheme or port outside the
/// selector, or a path that escapes the workspace. It is true when the comparison ran
/// and the value is simply not covered.
///
/// Subject matching needs the distinction. An allow rule must not allowlist a value it
/// could not read, and a deny rule must still deny one. Collapsing both cases into
/// "does not match" let an `http://` URL evade a `:domains` deny rule, because
/// `:schemes` defaults to `["https"]` and the scheme error read as "no match".
#[derive(Debug)]
struct ConstraintMiss {
    reason: String,
    evaluated: bool,
}

impl ConstraintMiss {
    /// The value could not be compared against the rule's lists.
    fn unevaluatable(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            evaluated: false,
        }
    }

    /// The comparison ran; the value is not covered by the rule.
    fn no_match(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            evaluated: true,
        }
    }
}

impl CompiledConstraint {
    fn label(&self) -> &'static str {
        match self {
            Self::Path(_) => "paths",
            Self::Domain(_) => "domains",
            Self::Command(_) => "commands",
        }
    }

    fn check(
        &self,
        value: &serde_json::Value,
        workspace_root: &Path,
    ) -> Result<(), ConstraintMiss> {
        match self {
            Self::Path(rule) => rule.check(value, workspace_root),
            Self::Domain(rule) => rule.check(value),
            Self::Command(rule) => rule.check(value),
        }
    }
}

#[derive(Debug)]
struct PathConstraint {
    allow: Vec<GlobMatcher>,
    deny: Vec<GlobMatcher>,
}

impl PathConstraint {
    fn compile(selector: &BTreeMap<Value, Value>) -> Result<Self, PolicyError> {
        let allow = parse_string_list(get(selector, "allow"), "path :allow")?;
        let deny = parse_string_list(get(selector, "deny"), "path :deny")?;
        if allow.is_empty() {
            return Err(invalid("path constraint requires a nonempty :allow list"));
        }
        Ok(Self {
            allow: compile_path_globs(&allow)?,
            deny: compile_path_globs(&deny)?,
        })
    }

    fn check(
        &self,
        value: &serde_json::Value,
        workspace_root: &Path,
    ) -> Result<(), ConstraintMiss> {
        let path = value
            .as_str()
            .ok_or_else(|| ConstraintMiss::unevaluatable("path argument must be a string"))?;
        let relative =
            normalize_policy_path(workspace_root, path).map_err(ConstraintMiss::unevaluatable)?;
        if self.deny.iter().any(|pattern| pattern.is_match(&relative)) {
            return Err(ConstraintMiss::no_match("path matches a deny pattern"));
        }
        self.allow
            .iter()
            .any(|pattern| pattern.is_match(&relative))
            .then_some(())
            .ok_or_else(|| ConstraintMiss::no_match("path is not allowlisted"))
    }
}

#[derive(Debug)]
struct DomainConstraint {
    allow: Vec<HostPattern>,
    deny: Vec<HostPattern>,
    schemes: BTreeSet<String>,
    ports: Option<BTreeSet<u16>>,
}

impl DomainConstraint {
    fn compile(selector: &BTreeMap<Value, Value>) -> Result<Self, PolicyError> {
        let allow = parse_string_list(get(selector, "allow"), "domain :allow")?
            .into_iter()
            .map(|host| HostPattern::parse(&host))
            .collect::<Result<Vec<_>, _>>()?;
        if allow.is_empty() {
            return Err(invalid("domain constraint requires a nonempty :allow list"));
        }
        let deny = parse_string_list(get(selector, "deny"), "domain :deny")?
            .into_iter()
            .map(|host| HostPattern::parse(&host))
            .collect::<Result<Vec<_>, _>>()?;
        let schemes = match get(selector, "schemes") {
            Some(value) => parse_string_list(Some(value), "domain :schemes")?
                .into_iter()
                .map(|scheme| scheme.to_ascii_lowercase())
                .collect(),
            None => BTreeSet::from(["https".to_string()]),
        };
        if schemes.is_empty() {
            return Err(invalid("domain :schemes must not be empty"));
        }
        if schemes
            .iter()
            .any(|scheme| scheme != "http" && scheme != "https")
        {
            return Err(invalid(
                "domain :schemes supports only \"http\" and \"https\"",
            ));
        }
        let ports = get(selector, "ports")
            .map(|value| {
                let values = require_seq(value, "domain :ports")?;
                values
                    .iter()
                    .map(|value| {
                        value
                            .as_int()
                            .and_then(|port| u16::try_from(port).ok())
                            .ok_or_else(|| {
                                invalid("domain :ports entries must be integers 0-65535")
                            })
                    })
                    .collect::<Result<BTreeSet<_>, _>>()
            })
            .transpose()?;
        Ok(Self {
            allow,
            deny,
            schemes,
            ports,
        })
    }

    fn check(&self, value: &serde_json::Value) -> Result<(), ConstraintMiss> {
        let raw = value
            .as_str()
            .ok_or_else(|| ConstraintMiss::unevaluatable("URL argument must be a string"))?;
        let url = Url::parse(raw)
            .map_err(|_| ConstraintMiss::unevaluatable("URL argument is invalid"))?;
        if !self.schemes.contains(url.scheme()) {
            return Err(ConstraintMiss::unevaluatable(
                "URL scheme is not allowlisted",
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ConstraintMiss::unevaluatable(
                "URL credentials are not allowed",
            ));
        }
        let host = match url.host() {
            Some(Host::Domain(domain)) => domain.to_ascii_lowercase(),
            Some(Host::Ipv4(ip)) => ip.to_string(),
            Some(Host::Ipv6(ip)) => ip.to_string(),
            None => return Err(ConstraintMiss::unevaluatable("URL must have a host")),
        };
        if let Some(ports) = &self.ports {
            let port = url
                .port_or_known_default()
                .ok_or_else(|| ConstraintMiss::unevaluatable("URL port cannot be resolved"))?;
            if !ports.contains(&port) {
                return Err(ConstraintMiss::unevaluatable("URL port is not allowlisted"));
            }
        }
        if self.deny.iter().any(|pattern| pattern.matches(&host)) {
            return Err(ConstraintMiss::no_match("URL host matches a deny rule"));
        }
        self.allow
            .iter()
            .any(|pattern| pattern.matches(&host))
            .then_some(())
            .ok_or_else(|| ConstraintMiss::no_match("URL host is not allowlisted"))
    }
}

#[derive(Debug)]
struct HostPattern {
    host: String,
    include_subdomains: bool,
}

impl HostPattern {
    fn parse(value: &str) -> Result<Self, PolicyError> {
        if value.contains("://") || value.contains('/') || value.contains('@') {
            return Err(invalid(format!(
                "domain rule {value:?} must be a hostname, not a URL"
            )));
        }
        let (host, include_subdomains) = match value.strip_prefix("*.") {
            Some(host) => (host, true),
            None => (value, false),
        };
        if host.is_empty() || host.contains('*') {
            return Err(invalid(format!(
                "domain rule {value:?} only supports a leading *. wildcard"
            )));
        }
        let host = match Host::parse(host) {
            Ok(Host::Domain(domain)) => domain.to_ascii_lowercase(),
            Ok(Host::Ipv4(ip)) => ip.to_string(),
            Ok(Host::Ipv6(ip)) => ip.to_string(),
            Err(_) => {
                return Err(invalid(format!(
                    "domain rule {value:?} must contain only a valid hostname"
                )))
            }
        };
        Ok(Self {
            host,
            include_subdomains,
        })
    }

    fn matches(&self, candidate: &str) -> bool {
        if !self.include_subdomains {
            return candidate == self.host;
        }
        candidate
            .strip_suffix(&self.host)
            .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1)
    }
}

#[derive(Debug)]
struct CommandConstraint {
    allow: BTreeSet<String>,
    deny: BTreeSet<String>,
}

impl CommandConstraint {
    fn compile(selector: &BTreeMap<Value, Value>) -> Result<Self, PolicyError> {
        let allow = parse_string_set(get(selector, "allow"), "command :allow")?;
        let deny = parse_string_set(get(selector, "deny"), "command :deny")?;
        if allow.is_empty() {
            return Err(invalid(
                "command constraint requires a nonempty :allow list",
            ));
        }
        if allow
            .iter()
            .chain(deny.iter())
            .any(|command| command.contains('*') || command.contains('?') || command.contains('['))
        {
            return Err(invalid(
                "command rules are exact strings; wildcard syntax is not supported",
            ));
        }
        Ok(Self { allow, deny })
    }

    fn check(&self, value: &serde_json::Value) -> Result<(), ConstraintMiss> {
        let command = value
            .as_str()
            .ok_or_else(|| ConstraintMiss::unevaluatable("command argument must be a string"))?;
        if self.deny.contains(command) {
            return Err(ConstraintMiss::no_match(
                "command matches an explicit deny rule",
            ));
        }
        self.allow
            .contains(command)
            .then_some(())
            .ok_or_else(|| ConstraintMiss::no_match("command is not allowlisted"))
    }
}

fn compile_constraints(
    value: &Value,
    default_argument: &str,
    kind: ConstraintKind,
    tool: &str,
) -> Result<Vec<ArgumentConstraint>, PolicyError> {
    if let Some(map) = value.as_map_ref() {
        return compile_selector(map, default_argument, kind, tool).map(|rule| vec![rule]);
    }
    let values = require_seq(value, &format!("tool {tool:?} {}", kind.label()))?;
    if values.iter().all(|value| value.as_str().is_some()) {
        let selector = shorthand_selector(&values);
        return compile_selector(&selector, default_argument, kind, tool).map(|rule| vec![rule]);
    }
    values
        .iter()
        .map(|value| {
            let map = require_map(value, &format!("tool {tool:?} {} selector", kind.label()))?;
            compile_selector(map, default_argument, kind, tool)
        })
        .collect()
}

fn compile_selector(
    selector: &BTreeMap<Value, Value>,
    default_argument: &str,
    kind: ConstraintKind,
    tool: &str,
) -> Result<ArgumentConstraint, PolicyError> {
    let allowed_keys: &[&str] = match kind {
        ConstraintKind::Path | ConstraintKind::Command => &["arg", "allow", "deny"],
        ConstraintKind::Domain => &["arg", "allow", "deny", "schemes", "ports"],
    };
    reject_unknown_keys(
        selector,
        allowed_keys,
        &format!("tool {tool:?} {} selector", kind.label()),
    )?;
    let argument = get(selector, "arg")
        .map(|value| {
            value_name(value).ok_or_else(|| {
                invalid(format!(
                    "tool {tool:?} {} :arg must be a keyword or string",
                    kind.label()
                ))
            })
        })
        .transpose()?
        .unwrap_or_else(|| default_argument.to_string());
    if argument.is_empty() {
        return Err(invalid(format!(
            "tool {tool:?} {} :arg must not be empty",
            kind.label()
        )));
    }
    let kind = match kind {
        ConstraintKind::Path => CompiledConstraint::Path(PathConstraint::compile(selector)?),
        ConstraintKind::Domain => CompiledConstraint::Domain(DomainConstraint::compile(selector)?),
        ConstraintKind::Command => {
            CompiledConstraint::Command(CommandConstraint::compile(selector)?)
        }
    };
    Ok(ArgumentConstraint { argument, kind })
}

fn shorthand_selector(values: &[Value]) -> BTreeMap<Value, Value> {
    BTreeMap::from([(Value::keyword("allow"), Value::vector(values.to_vec()))])
}

fn compile_path_globs(patterns: &[String]) -> Result<Vec<GlobMatcher>, PolicyError> {
    patterns
        .iter()
        .map(|pattern| {
            validate_path_pattern(pattern)?;
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .backslash_escape(false)
                .build()
                .map(|glob| glob.compile_matcher())
                .map_err(|error| invalid(format!("invalid path pattern {pattern:?}: {error}")))
        })
        .collect()
}

fn validate_path_pattern(pattern: &str) -> Result<(), PolicyError> {
    if pattern.is_empty()
        || pattern.starts_with('/')
        || pattern.contains('\\')
        || pattern.contains(['[', ']', '{', '}', '?'])
    {
        return Err(invalid(format!(
            "path pattern {pattern:?} must be a relative literal/*/** pattern"
        )));
    }
    for component in pattern.split('/') {
        if component == ".." || (component.contains("**") && component != "**") {
            return Err(invalid(format!(
                "path pattern {pattern:?} has an invalid component {component:?}"
            )));
        }
    }
    Ok(())
}

fn normalize_policy_path(workspace_root: &Path, input: &str) -> Result<String, String> {
    if input.is_empty() || input.contains('\0') {
        return Err("path must be a nonempty string without NUL bytes".to_string());
    }
    let input_path = Path::new(input);
    if input_path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }

    let root = absolute_lexical(workspace_root)?;
    let joined = normalize_lexical(&root.join(input_path));
    if !joined.starts_with(&root) {
        return Err("path escapes the workflow root".to_string());
    }
    let canonical_root = canonical_or_lexical(&root);
    let resolved = canonicalize_existing_prefix(&joined);
    if !resolved.starts_with(&canonical_root) {
        return Err("path resolves outside the workflow root".to_string());
    }
    let relative = resolved
        .strip_prefix(&canonical_root)
        .map_err(|_| "path cannot be made root-relative".to_string())?;
    Ok(relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

fn absolute_lexical(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(normalize_lexical(path));
    }
    // The workspace root should always be provided as an absolute path by the enforcement
    // layer. Resolving a relative path with `current_dir()` introduces CWD-dependent
    // non-determinism, so we canonicalize it instead of accepting process-global state.
    std::fs::canonicalize(path).map_err(|error| format!("cannot resolve workspace root: {error}"))
}

fn canonical_or_lexical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize_lexical(path))
}

fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut suffix = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
            break;
        };
        suffix.push(name);
        if !existing.pop() {
            break;
        }
    }
    let mut resolved = canonical_or_lexical(&existing);
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    normalize_lexical(&resolved)
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn fingerprint(value: &Value, name: &str) -> String {
    let json = sema_core::value_to_json_lossy(value);
    let encoded = serde_json::to_vec(&json).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(b"sema-policy-v1\0");
    hasher.update(name.as_bytes());
    hasher.update(b"\0");
    hasher.update(encoded);
    format!("sha256:{:x}", hasher.finalize())
}

fn parse_default(value: Option<&Value>, context: &str) -> Result<DefaultEffect, PolicyError> {
    let Some(value) = value else {
        return Ok(DefaultEffect::Deny);
    };
    match value_name(value).as_deref() {
        Some("deny") => Ok(DefaultEffect::Deny),
        Some("allow") => Ok(DefaultEffect::Allow),
        Some(other) => Err(invalid_with_hint(
            format!("{context} :default has unsupported value :{other}"),
            "valid values are :allow and :deny",
        )),
        None => Err(invalid(format!(
            "{context} :default must be a keyword or string, got {}",
            value.type_name()
        ))),
    }
}

fn parse_model_patterns(
    value: Option<&Value>,
    context: &str,
) -> Result<Vec<ModelPattern>, PolicyError> {
    parse_string_list(value, context)?
        .into_iter()
        .map(|value| ModelPattern::parse(&value))
        .collect()
}

fn parse_string_set(value: Option<&Value>, context: &str) -> Result<BTreeSet<String>, PolicyError> {
    parse_string_list(value, context).map(|values| values.into_iter().collect())
}

fn parse_name_set(value: Option<&Value>, context: &str) -> Result<BTreeSet<String>, PolicyError> {
    let Some(value) = value else {
        return Ok(BTreeSet::new());
    };
    require_seq(value, context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value_name(value).ok_or_else(|| {
                invalid(format!(
                    "{context} entry {} must be a keyword or string, got {}",
                    index + 1,
                    value.type_name()
                ))
            })
        })
        .collect()
}

fn parse_string_list(value: Option<&Value>, context: &str) -> Result<Vec<String>, PolicyError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    require_seq(value, context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                invalid(format!(
                    "{context} entry {} must be a string, got {}",
                    index + 1,
                    value.type_name()
                ))
            })
        })
        .collect()
}

fn require_seq(value: &Value, context: &str) -> Result<Vec<Value>, PolicyError> {
    value.as_seq().map(|values| values.to_vec()).ok_or_else(|| {
        invalid(format!(
            "{context} must be a list or vector, got {}",
            value.type_name()
        ))
    })
}

fn require_map<'a>(
    value: &'a Value,
    context: &str,
) -> Result<&'a BTreeMap<Value, Value>, PolicyError> {
    value.as_map_ref().ok_or_else(|| {
        invalid(format!(
            "{context} must be a map, got {}",
            value.type_name()
        ))
    })
}

fn get<'a>(map: &'a BTreeMap<Value, Value>, key: &str) -> Option<&'a Value> {
    map.iter().find_map(|(candidate, value)| {
        (value_name(candidate).as_deref() == Some(key)).then_some(value)
    })
}

fn value_name(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_keyword())
        .or_else(|| value.as_symbol())
}

fn reject_unknown_keys(
    map: &BTreeMap<Value, Value>,
    allowed: &[&str],
    context: &str,
) -> Result<(), PolicyError> {
    let mut seen = BTreeSet::new();
    for key in map.keys() {
        let key_name = value_name(key).ok_or_else(|| {
            invalid(format!(
                "{context} keys must be keywords, strings, or symbols, got {}",
                key.type_name()
            ))
        })?;
        if !allowed.contains(&key_name.as_str()) {
            let hint = suggest_similar(&key_name, allowed)
                .map(|candidate| format!("did you mean :{candidate}?"))
                .unwrap_or_else(|| {
                    format!(
                        "valid keys are {}",
                        allowed
                            .iter()
                            .map(|key| format!(":{key}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                });
            return Err(invalid_with_hint(
                format!("{context} has unknown key :{key_name}"),
                hint,
            ));
        }
        if !seen.insert(key_name.clone()) {
            return Err(invalid(format!("{context} has duplicate key :{key_name}")));
        }
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> PolicyError {
    PolicyError {
        message: message.into(),
        hint: None,
    }
}

fn invalid_with_hint(message: impl Into<String>, hint: impl Into<String>) -> PolicyError {
    PolicyError {
        message: message.into(),
        hint: Some(hint.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        Value::map(
            entries
                .into_iter()
                .map(|(key, value)| (Value::keyword(key), value))
                .collect(),
        )
    }

    fn strings(values: &[&str]) -> Value {
        Value::vector(values.iter().map(|value| Value::string(value)).collect())
    }

    fn named_policy(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        let mut values: BTreeMap<Value, Value> = entries
            .into_iter()
            .map(|(key, value)| (Value::keyword(key), value))
            .collect();
        values.insert(Value::keyword("__policy-name"), Value::symbol("safe"));
        values.insert(
            Value::keyword("__policy-version"),
            Value::int(POLICY_VERSION),
        );
        Value::map(values)
    }

    #[test]
    fn model_rules_match_resolved_pairs_and_provider_wildcard() {
        let policy = CompiledPolicy::compile(&named_policy([(
            "models",
            map([
                ("allow", strings(&["openai/gpt-5", "ollama/*"])),
                ("deny", strings(&["ollama/unsafe"])),
                ("on-deny", Value::keyword("skip")),
            ]),
        )]))
        .unwrap();

        assert!(policy.check_model("openai", "gpt-5").allowed);
        assert!(policy.check_model("ollama", "qwen").allowed);
        assert!(!policy.check_model("ollama", "unsafe").allowed);
        assert!(!policy.check_model("openai", "gpt-4").allowed);
        assert_eq!(policy.model_action(), Some(ModelDenyAction::Skip));
    }

    #[test]
    fn invalid_model_globs_are_rejected() {
        let error = CompiledPolicy::compile(&named_policy([(
            "models",
            map([("allow", strings(&["*/gpt-5"]))]),
        )]))
        .unwrap_err();
        assert!(error.to_string().contains("cannot wildcard the provider"));
    }

    #[test]
    fn present_sections_default_to_deny_and_hard_fail() {
        let policy =
            CompiledPolicy::compile(&named_policy([("models", map([])), ("tools", map([]))]))
                .unwrap();
        assert!(!policy.check_model("fake", "model").allowed);
        assert_eq!(policy.model_action(), Some(ModelDenyAction::Fail));
        assert!(
            !policy
                .check_tool("read-file", &serde_json::json!({}), &[], Path::new("."))
                .allowed
        );
        assert_eq!(policy.tool_action(), Some(ToolDenyAction::Fail));
    }

    #[test]
    fn semantic_subject_rules_use_declared_argument_mappings() {
        let root = sema_core::testing::unique_temp_dir("policy-subject");
        fs::create_dir_all(root.join("src")).unwrap();
        let policy = CompiledPolicy::compile(&named_policy([(
            "subjects",
            map([(
                "allow",
                Value::vector(vec![map([
                    ("kind", Value::keyword("file-read")),
                    ("paths", strings(&["src/**"])),
                ])]),
            )]),
        )]))
        .unwrap();
        let subjects = [ToolPolicySubject::File {
            access: FileAccess::Read,
            path_arg: "location".to_string(),
        }];

        assert!(
            policy
                .check_tool(
                    "arbitrary-name",
                    &serde_json::json!({"location":"src/lib.rs"}),
                    &subjects,
                    &root,
                )
                .allowed
        );
        assert!(
            !policy
                .check_tool(
                    "arbitrary-name",
                    &serde_json::json!({"location":"Cargo.toml"}),
                    &subjects,
                    &root,
                )
                .allowed
        );
        assert!(
            !policy
                .check_tool(
                    "arbitrary-name",
                    &serde_json::json!({"location":"src/lib.rs"}),
                    &[],
                    &root,
                )
                .allowed
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn input_detectors_compile_to_safe_aggregate_findings() {
        let policy = CompiledPolicy::compile(&named_policy([(
            "input",
            map([
                (
                    "detect",
                    Value::vector(vec![Value::keyword("secret"), Value::keyword("email")]),
                ),
                (
                    "actions",
                    map([
                        ("secret", Value::keyword("block")),
                        ("email", Value::keyword("redact")),
                    ]),
                ),
            ]),
        )]))
        .unwrap();
        let text = "send me@example.com and AKIAIOSFODNN7EXAMPLE";
        let outcome = policy.check_input(text);

        assert_eq!(outcome.action, ContentAction::Block);
        assert!(outcome
            .findings
            .iter()
            .all(|finding| !finding.label.contains('@')));
        assert_eq!(
            content::redact(text, &outcome.redactions),
            "send «redacted:email» and AKIAIOSFODNN7EXAMPLE"
        );
    }

    #[test]
    fn terminal_output_enforces_schema_required_length_and_patterns() {
        let policy = CompiledPolicy::compile(&named_policy([(
            "output",
            map([
                (
                    "schema",
                    map([
                        ("answer", Value::keyword("string")),
                        (
                            "citations",
                            map([
                                ("type", Value::keyword("list")),
                                ("optional", Value::bool(true)),
                            ]),
                        ),
                    ]),
                ),
                ("require", Value::vector(vec![Value::keyword("citations")])),
                ("max-length", Value::int(120)),
                (
                    "forbid",
                    Value::vector(vec![map([
                        ("id", Value::keyword("absolute")),
                        ("contains", Value::string("guaranteed")),
                    ])]),
                ),
            ]),
        )]))
        .unwrap();

        let round = policy.check_output("guaranteed", OutputStage::Round);
        assert_eq!(round.action, ContentAction::Block);
        let final_outcome =
            policy.check_output(r#"{"answer":"ok","citations":[]}"#, OutputStage::Final);
        assert_eq!(final_outcome.action, ContentAction::Block);
        assert!(final_outcome
            .findings
            .iter()
            .any(|finding| finding.rule_id == "output.require.citations"));
        assert_eq!(
            policy
                .check_output(
                    r#"{"answer":"ok","citations":["source-1"]}"#,
                    OutputStage::Final,
                )
                .action,
            ContentAction::Allow
        );
    }

    #[test]
    fn metadata_and_completion_requirements_are_strict_and_stable() {
        let policy = CompiledPolicy::compile(&named_policy([
            (
                "metadata",
                map([("require", strings(&["owner", "risk-tier"]))]),
            ),
            (
                "completion",
                map([(
                    "require-events",
                    strings(&["agent.tool_result", "approval.applied", "checkpoint"]),
                )]),
            ),
        ]))
        .unwrap();
        assert_eq!(
            policy.required_metadata().collect::<Vec<_>>(),
            vec!["owner", "risk-tier"]
        );
        assert_eq!(
            policy.required_completion_events().collect::<Vec<_>>(),
            vec!["agent.tool_result", "approval.applied", "checkpoint"]
        );

        let error = CompiledPolicy::compile(&named_policy([(
            "completion",
            map([("require-events", strings(&["made.up"]))]),
        )]))
        .unwrap_err();
        assert!(error.to_string().contains("unsupported event"));
    }

    #[test]
    fn unknown_keys_are_rejected_instead_of_being_ignored() {
        let error = CompiledPolicy::compile(&named_policy([(
            "models",
            map([("alow", strings(&["fake/*"]))]),
        )]))
        .unwrap_err();
        assert!(error.to_string().contains("unknown key :alow"));
        assert_eq!(error.hint(), Some("did you mean :allow?"));
    }

    #[test]
    fn list_errors_include_one_based_index_and_actual_type() {
        let error = CompiledPolicy::compile(&named_policy([(
            "models",
            map([(
                "allow",
                Value::vector(vec![Value::string("fake/*"), Value::int(42)]),
            )]),
        )]))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            ":models :allow entry 2 must be a string, got int"
        );
    }

    #[test]
    fn enum_errors_use_keyword_notation_and_list_valid_values() {
        let error = CompiledPolicy::compile(&named_policy([(
            "tools",
            map([("on-deny", Value::keyword("ignore"))]),
        )]))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            ":tools :on-deny has unsupported value :ignore"
        );
        assert_eq!(error.hint(), Some("valid values are :fail and :tool-error"));
    }

    #[test]
    fn structural_string_and_symbol_keys_are_enforced() {
        let models = Value::map(BTreeMap::from([
            (Value::string("default"), Value::keyword("deny")),
            (Value::symbol("allow"), strings(&["openai/allowed-model"])),
        ]));
        let policy = Value::map(BTreeMap::from([(Value::string("models"), models)]));
        let policy = CompiledPolicy::compile(&policy).unwrap();

        assert!(policy.check_model("openai", "allowed-model").allowed);
        assert!(!policy.check_model("openai", "blocked-model").allowed);
    }

    #[test]
    fn string_constraint_keys_do_not_create_unconstrained_tool_rules() {
        let tool_rule = Value::map(BTreeMap::from([(
            Value::string("paths"),
            strings(&["safe/**"]),
        )]));
        let allow = Value::map(BTreeMap::from([(Value::string("read-file"), tool_rule)]));
        let tools = Value::map(BTreeMap::from([
            (Value::string("default"), Value::keyword("deny")),
            (Value::string("allow"), allow),
        ]));
        let policy = Value::map(BTreeMap::from([(Value::string("tools"), tools)]));
        let policy = CompiledPolicy::compile(&policy).unwrap();

        assert!(
            !policy
                .check_tool(
                    "read-file",
                    &serde_json::json!({"path":"../outside"}),
                    &[],
                    Path::new(".")
                )
                .allowed
        );
    }

    #[test]
    fn subject_deny_rules_fail_closed_on_unevaluatable_arguments() {
        // A deny rule must still deny an argument it cannot inspect. `:domains` defaults
        // `:schemes` to ["https"], so an `http://` URL cannot be evaluated against the
        // rule; treating that as "no match" let plain HTTP evade the deny list.
        let subjects = Value::map(BTreeMap::from([
            (Value::string("default"), Value::keyword("allow")),
            (
                Value::string("deny"),
                Value::vector(vec![Value::map(BTreeMap::from([
                    (Value::string("kind"), Value::keyword("network-request")),
                    (Value::string("domains"), strings(&["evil.example.com"])),
                ]))]),
            ),
        ]));
        let policy = CompiledPolicy::compile(&Value::map(BTreeMap::from([(
            Value::string("subjects"),
            subjects,
        )])))
        .unwrap();
        let spec = [ToolPolicySubject::NetworkRequest {
            method: None,
            url_arg: "url".to_string(),
        }];

        for url in [
            "https://evil.example.com/x",
            // Same host, scheme the rule cannot evaluate — must not become an allow.
            "http://evil.example.com/x",
        ] {
            let check = policy.check_tool(
                "fetch",
                &serde_json::json!({ "url": url }),
                &spec,
                Path::new("."),
            );
            assert!(!check.allowed, "{url} was allowed by a deny rule");
        }

        // A host the rule does not name is still allowed by `:default :allow`.
        assert!(
            policy
                .check_tool(
                    "fetch",
                    &serde_json::json!({"url":"https://ok.example.com/x"}),
                    &spec,
                    Path::new(".")
                )
                .allowed
        );
    }

    #[test]
    fn subject_deny_paths_fail_closed_outside_the_workspace() {
        // `normalize_policy_path` errors for absolute and root-escaping paths, so a
        // `:paths` deny rule could not evaluate them and let them through.
        let subjects = Value::map(BTreeMap::from([
            (Value::string("default"), Value::keyword("allow")),
            (
                Value::string("deny"),
                Value::vector(vec![Value::map(BTreeMap::from([
                    (Value::string("kind"), Value::keyword("file-write")),
                    (Value::string("paths"), strings(&["**"])),
                ]))]),
            ),
        ]));
        let policy = CompiledPolicy::compile(&Value::map(BTreeMap::from([(
            Value::string("subjects"),
            subjects,
        )])))
        .unwrap();
        let spec = [ToolPolicySubject::File {
            access: FileAccess::Write,
            path_arg: "path".to_string(),
        }];

        for path in ["src/x.rs", "/tmp/evil.txt", "../evil.txt"] {
            let check = policy.check_tool(
                "write-file",
                &serde_json::json!({ "path": path }),
                &spec,
                Path::new("."),
            );
            assert!(!check.allowed, "{path} was allowed by a deny rule");
        }
    }

    #[test]
    fn duplicate_normalized_keys_and_tool_names_are_rejected() {
        let duplicate_sections = Value::map(BTreeMap::from([
            (Value::keyword("models"), map([])),
            (Value::string("models"), map([])),
        ]));
        assert!(CompiledPolicy::compile(&duplicate_sections)
            .unwrap_err()
            .to_string()
            .contains("duplicate key"));

        let duplicate_tools = map([(
            "tools",
            map([(
                "allow",
                Value::map(BTreeMap::from([
                    (Value::keyword("read-file"), map([])),
                    (Value::string("read-file"), map([])),
                ])),
            )]),
        )]);
        assert!(CompiledPolicy::compile(&duplicate_tools)
            .unwrap_err()
            .to_string()
            .contains("duplicate tool name"));
    }

    #[test]
    fn tool_default_deny_and_explicit_deny_win() {
        let policy = CompiledPolicy::compile(&named_policy([(
            "tools",
            map([
                (
                    "allow",
                    Value::map(BTreeMap::from([(Value::string("read-file"), map([]))])),
                ),
                ("deny", strings(&["read-file"])),
            ]),
        )]))
        .unwrap();
        assert!(
            !policy
                .check_tool("read-file", &serde_json::json!({}), &[], Path::new("."))
                .allowed
        );
        assert!(
            !policy
                .check_tool("write-file", &serde_json::json!({}), &[], Path::new("."))
                .allowed
        );
    }

    #[test]
    fn shorthand_and_explicit_path_arguments_match() {
        let root = sema_core::testing::unique_temp_dir("policy-path");
        fs::create_dir_all(root.join("src")).unwrap();
        let policy = CompiledPolicy::compile(&named_policy([(
            "tools",
            map([(
                "allow",
                Value::map(BTreeMap::from([
                    (
                        Value::string("read-file"),
                        map([("paths", strings(&["src/**"]))]),
                    ),
                    (
                        Value::string("copy-file"),
                        map([(
                            "paths",
                            Value::vector(vec![
                                map([
                                    ("arg", Value::keyword("source")),
                                    ("allow", strings(&["src/**"])),
                                ]),
                                map([
                                    ("arg", Value::keyword("destination")),
                                    ("allow", strings(&["tmp/**"])),
                                ]),
                            ]),
                        )]),
                    ),
                ])),
            )]),
        )]))
        .unwrap();

        assert!(
            policy
                .check_tool(
                    "read-file",
                    &serde_json::json!({"path":"src/lib.rs"}),
                    &[],
                    &root
                )
                .allowed
        );
        assert!(
            !policy
                .check_tool(
                    "read-file",
                    &serde_json::json!({"path":"Cargo.toml"}),
                    &[],
                    &root
                )
                .allowed
        );
        assert!(
            policy
                .check_tool(
                    "copy-file",
                    &serde_json::json!({"source":"src/lib.rs","destination":"tmp/lib.rs"}),
                    &[],
                    &root
                )
                .allowed
        );
        assert!(
            !policy
                .check_tool(
                    "copy-file",
                    &serde_json::json!({"source":"src/lib.rs"}),
                    &[],
                    &root
                )
                .allowed
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn path_traversal_and_absolute_paths_are_denied() {
        let root = sema_core::testing::unique_temp_dir("policy-traversal");
        fs::create_dir_all(root.join("src")).unwrap();
        let rule = PathConstraint {
            allow: compile_path_globs(&["**".to_string()]).unwrap(),
            deny: Vec::new(),
        };
        assert!(rule.check(&serde_json::json!("../outside"), &root).is_err());
        assert!(rule
            .check(&serde_json::json!("/tmp/outside"), &root)
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn unix_backslash_is_not_treated_as_a_path_separator() {
        let root = sema_core::testing::unique_temp_dir("policy-backslash");
        fs::write(root.join(r"safe\secret"), "not in the safe directory").unwrap();
        let rule = PathConstraint {
            allow: compile_path_globs(&["safe/**".to_string()]).unwrap(),
            deny: Vec::new(),
        };

        assert!(rule
            .check(&serde_json::json!(r"safe\secret"), &root)
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_denied() {
        use std::os::unix::fs::symlink;

        let root = sema_core::testing::unique_temp_dir("policy-symlink");
        let outside = sema_core::testing::unique_temp_dir("policy-outside");
        symlink(&outside, root.join("escape")).unwrap();
        let rule = PathConstraint {
            allow: compile_path_globs(&["**".to_string()]).unwrap(),
            deny: Vec::new(),
        };
        assert!(rule
            .check(&serde_json::json!("escape/new.txt"), &root)
            .is_err());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn domains_normalize_idna_and_reject_credentials() {
        let selector = BTreeMap::from([(
            Value::keyword("allow"),
            strings(&["münchen.de", "*.example.com"]),
        )]);
        let rule = DomainConstraint::compile(&selector).unwrap();
        assert!(rule
            .check(&serde_json::json!("https://münchen.de/a/../b"))
            .is_ok());
        assert!(rule
            .check(&serde_json::json!("https://api.example.com/v1"))
            .is_ok());
        assert!(rule
            .check(&serde_json::json!("https://example.com/v1"))
            .is_err());
        assert!(rule
            .check(&serde_json::json!("http://api.example.com/v1"))
            .is_err());
        assert!(rule
            .check(&serde_json::json!("https://u:p@api.example.com/v1"))
            .is_err());
    }

    #[test]
    fn domain_rules_reject_url_components_that_would_be_discarded() {
        for host in [
            "example.com:443",
            "example.com?tenant=other",
            "example.com#fragment",
        ] {
            let selector = BTreeMap::from([(Value::keyword("allow"), strings(&[host]))]);
            let error = DomainConstraint::compile(&selector).unwrap_err();
            assert!(
                error.to_string().contains("only a valid hostname"),
                "unexpected error for {host:?}: {error}"
            );
        }
    }

    #[test]
    fn commands_are_exact_and_wildcards_are_rejected() {
        let selector = BTreeMap::from([(
            Value::keyword("allow"),
            strings(&["cargo test", "git diff"]),
        )]);
        let rule = CommandConstraint::compile(&selector).unwrap();
        assert!(rule.check(&serde_json::json!("cargo test")).is_ok());
        assert!(rule.check(&serde_json::json!("cargo test -p x")).is_err());

        let wildcard = BTreeMap::from([(Value::keyword("allow"), strings(&["cargo test *"]))]);
        assert!(CommandConstraint::compile(&wildcard).is_err());
    }

    #[test]
    fn fingerprint_is_stable_and_includes_name() {
        let first = CompiledPolicy::compile(&named_policy([(
            "tools",
            map([("default", Value::keyword("allow"))]),
        )]))
        .unwrap();
        let second = CompiledPolicy::compile(&named_policy([(
            "tools",
            map([("default", Value::keyword("allow"))]),
        )]))
        .unwrap();
        assert_eq!(first.fingerprint(), second.fingerprint());

        let other = CompiledPolicy::compile(&map([(
            "tools",
            map([("default", Value::keyword("allow"))]),
        )]))
        .unwrap();
        assert_ne!(first.fingerprint(), other.fingerprint());
    }
}
