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
}

impl BillableUsage {
    pub(super) fn usage(&self) -> &Usage {
        &self.usage
    }

    pub(super) fn origin(&self) -> ResponseOrigin {
        self.origin
    }
}

/// Derive the usage to charge from the usage a result reports (`recorded`) and the
/// result's origin. `CacheHit` charges zero tokens and keeps only the model name
/// (so `llm/last-usage` still names the model); `CassetteReplay` and `Provider`
/// charge `recorded` unchanged.
pub(super) fn billable_usage(origin: ResponseOrigin, recorded: &Usage) -> BillableUsage {
    let usage = match origin {
        ResponseOrigin::CacheHit => Usage {
            model: recorded.model.clone(),
            ..Usage::default()
        },
        ResponseOrigin::Provider | ResponseOrigin::CassetteReplay => recorded.clone(),
    };
    BillableUsage { usage, origin }
}

/// A completion result with its origin, as returned by the synchronous completion
/// functions (`do_complete`, `do_complete_streaming`, `run_completion`,
/// `stream_with_dispatch`) and by the runtime prep stage for an inline result.
#[derive(Debug, Clone)]
pub(super) struct Completion {
    pub(super) response: ChatResponse,
    pub(super) origin: ResponseOrigin,
}

impl Completion {
    pub(super) fn new(response: ChatResponse, origin: ResponseOrigin) -> Self {
        Self { response, origin }
    }

    pub(super) fn billable(&self) -> BillableUsage {
        billable_usage(self.origin, &self.response.usage)
    }

    /// Charge this completion through [`track_usage`] and return the response.
    pub(super) fn charge(self) -> Result<ChatResponse, SemaError> {
        track_usage(&self.billable())?;
        Ok(self.response)
    }
}

/// Charge `billable` against the leaf-usage frame and budget frame captured when the
/// request was dispatched, instead of the frames active now. Runtime finalizers use
/// this: they run after the task's dynamic scope may have been swapped out.
///
/// The leaf frame is folded here (priced as served by `serving_provider`), and the
/// ambient fold inside [`track_usage`] is suppressed so the completion is counted
/// once. The captured budget frame is installed around [`track_usage`] and the
/// previous frame is restored afterwards.
pub(super) fn charge_dispatch_frames(
    billable: &BillableUsage,
    serving_provider: &str,
    usage_accum_slot: Option<&Rc<RefCell<LeafUsage>>>,
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
) -> Result<(), SemaError> {
    if let Some(slot) = usage_accum_slot {
        let cost = pricing::calculate_cost_for(serving_provider, billable.usage());
        accumulate_into(slot, billable, cost);
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
