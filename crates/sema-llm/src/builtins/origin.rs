//! Typed origin of an LLM result, and the accounting rules that follow from it.
//!
//! A completion (or embedding) result reaches a builtin from one of three places.
//! Each place has its own accounting rule:
//!
//! | [`ResponseOrigin`] | Provider call | Charged usage            | Response cache write | Cassette record |
//! |--------------------|---------------|--------------------------|----------------------|-----------------|
//! | `Provider`         | yes           | the usage it reported    | yes                  | yes (record mode) |
//! | `CacheHit`         | no            | zero                     | no                   | no              |
//! | `CassetteReplay`   | no            | the recorded usage       | no                   | no              |
//!
//! A cassette replay charges the recorded usage so that budgets, session cost, and
//! leaf tallies behave the same on replay as on the recorded run. A cache hit made
//! no provider call, so it charges nothing (AGENTS.md "Accounting invariant").
//!
//! Further rules that follow from the origin:
//!
//! - A result the output policy rejects is charged before the policy error is
//!   returned, for every origin ([`Completion::checked`]).
//! - A cache hit is not checked against the budget limits, so a scope that is over
//!   its limit still serves cached results (`track_usage`).
//! - Only `CacheHit` counts as a cache hit in a leaf tally. A provider result or a
//!   replay that reports zero tokens is still a call (`accumulate_into`).
//! - Each charge carries the provider name its cost is priced with: the serving
//!   provider, or for a replay the provider stored on the tape. No charge reads the
//!   provider from thread-local state.
//!
//! Workflow memo replay (`workflow/step` on resume) is not an origin here: it returns
//! the stored leaf value in `sema-stdlib` before any LLM builtin runs, so no result
//! from that path reaches this module.
//!
//! The only way to charge usage is [`track_usage`] / [`charge_dispatch_frames`],
//! and both take a [`BillableUsage`]. A `BillableUsage` can be built only by
//! [`billable_usage`], which applies the table above.

use super::*;

/// Where an LLM result came from. Decides how the result is charged, and whether
/// it is written to the response cache or the cassette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponseOrigin {
    /// A live provider call. Charged at the usage the provider reported.
    Provider,
    /// Served by `llm/with-cache`. No provider call was made; charged zero.
    CacheHit,
    /// Served from a cassette tape. Charged at the usage stored on the tape.
    CassetteReplay,
}

impl ResponseOrigin {
    /// True when the result came from a live provider call. Only such a result is
    /// written to the response cache or recorded onto a cassette.
    pub(super) fn is_provider(self) -> bool {
        matches!(self, Self::Provider)
    }

    /// The provider name the `chat` span and the leaf-tally pricing use for a
    /// result that did not come from a live provider. `None` for `Provider`: the
    /// caller uses the serving provider.
    pub(super) fn telemetry_provider(self) -> Option<&'static str> {
        match self {
            Self::Provider => None,
            Self::CacheHit => Some(""),
            Self::CassetteReplay => Some("cassette"),
        }
    }
}

/// Usage that may be charged to session usage, session cost, budget frames, and
/// leaf-usage frames. Built only by [`billable_usage`].
#[derive(Debug, Clone)]
pub(super) struct BillableUsage {
    usage: Usage,
    origin: ResponseOrigin,
    pricing_provider: String,
}

impl BillableUsage {
    pub(super) fn usage(&self) -> &Usage {
        &self.usage
    }

    pub(super) fn origin(&self) -> ResponseOrigin {
        self.origin
    }

    /// The cost of this usage, priced as served by the provider the charge was built
    /// with (`pricing::calculate_cost_for`). An empty provider name means the
    /// canonical first-party price.
    pub(super) fn cost(&self) -> Option<f64> {
        pricing::calculate_cost_for(&self.pricing_provider, &self.usage)
    }
}

/// Derive the usage to charge from the usage a result reports (`recorded`) and the
/// result's origin. `CacheHit` charges zero tokens and keeps only the model name
/// (so `llm/last-usage` still names the model); `CassetteReplay` and `Provider`
/// charge `recorded` unchanged.
///
/// `pricing_provider` names the provider whose price list prices the usage: the
/// serving provider for `Provider`, the provider stored on the tape for
/// `CassetteReplay`. A cache hit charges zero tokens, so its provider does not
/// change the cost. The caller passes the name explicitly; no charge reads it from
/// thread-local state, so a provider name left by an earlier call cannot price
/// this one.
pub(super) fn billable_usage(
    origin: ResponseOrigin,
    recorded: &Usage,
    pricing_provider: &str,
) -> BillableUsage {
    let usage = match origin {
        ResponseOrigin::CacheHit => Usage {
            model: recorded.model.clone(),
            ..Usage::default()
        },
        ResponseOrigin::Provider | ResponseOrigin::CassetteReplay => recorded.clone(),
    };
    BillableUsage {
        usage,
        origin,
        pricing_provider: pricing_provider.to_string(),
    }
}

/// A completion result with its origin, as returned by the synchronous completion
/// functions (`do_complete`, `do_complete_streaming`, `run_completion`,
/// `stream_with_dispatch`) and by the runtime prep stage for an inline result.
#[derive(Debug, Clone)]
pub(super) struct Completion {
    pub(super) response: ChatResponse,
    pub(super) origin: ResponseOrigin,
    /// The provider that served the result (`Provider`), the provider stored on the
    /// tape (`CassetteReplay`), or empty (`CacheHit`). Prices the charge.
    pub(super) provider: String,
}

impl Completion {
    pub(super) fn new(
        response: ChatResponse,
        origin: ResponseOrigin,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            response,
            origin,
            provider: provider.into(),
        }
    }

    pub(super) fn billable(&self) -> BillableUsage {
        billable_usage(self.origin, &self.response.usage, &self.provider)
    }

    /// Charge this completion through [`track_usage`] and return the response.
    pub(super) fn charge(self) -> Result<ChatResponse, SemaError> {
        track_usage(&self.billable())?;
        Ok(self.response)
    }

    /// Run the output policy on the response. A rejected result is charged before
    /// the policy error is returned, for every origin: the result was produced (or,
    /// for a replay, stands in for a produced result) before the policy refused it.
    pub(super) fn checked(mut self, source: PolicySource) -> Result<Self, SemaError> {
        if let Err(error) = apply_output_policy_to_response(&mut self.response, source) {
            track_usage(&self.billable())?;
            return Err(error);
        }
        Ok(self)
    }
}

/// Charge `billable` against the leaf-usage frame and budget frame captured when the
/// request was dispatched, instead of the frames active now. Runtime finalizers use
/// this: they run after the task's dynamic scope may have been swapped out.
///
/// The leaf frame is folded here (priced by `billable`'s pricing provider), and the
/// ambient fold inside [`track_usage`] is suppressed so the completion is counted
/// once. The captured budget frame is installed around [`track_usage`] and the
/// previous frame is restored afterwards.
pub(super) fn charge_dispatch_frames(
    billable: &BillableUsage,
    usage_accum_slot: Option<&Rc<RefCell<LeafUsage>>>,
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
) -> Result<(), SemaError> {
    if let Some(slot) = usage_accum_slot {
        accumulate_into(slot, billable, billable.cost());
    }
    let previous_budget =
        ACTIVE_BUDGET.with(|active| std::mem::replace(&mut *active.borrow_mut(), budget_slot));
    let result = USAGE_ACCUM_SUPPRESS.with(|suppress| {
        suppress.set(true);
        let result = track_usage(billable);
        suppress.set(false);
        result
    });
    ACTIVE_BUDGET.with(|active| *active.borrow_mut() = previous_budget);
    result
}
