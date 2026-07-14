use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub trait RuntimeClock {
    fn now(&self) -> Instant;
}

#[derive(Clone, Debug)]
pub struct DriveBudget {
    pub work_item_limit: NonZeroUsize,
    pub completion_limit: NonZeroUsize,
    pub timer_limit: NonZeroUsize,
    pub root_visit_limit: NonZeroUsize,
    pub cleanup_limit: NonZeroUsize,
    pub instruction_limit_per_task: NonZeroUsize,
    pub wall_clock_limit: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriveState {
    Progress {
        work_items: usize,
        instructions: usize,
        ready_remaining: bool,
    },
    Idle {
        next_deadline: Option<Instant>,
        inbox_wakeup_required: bool,
        legacy_io_wakeup_required: bool,
    },
    Quiescent,
    ShutdownComplete,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DriveReport {
    pub work_items: usize,
    pub completions: usize,
    pub timers: usize,
    pub cleanup: usize,
    pub root_visits: usize,
    pub ready_remaining: bool,
}

pub struct BoundedDriver {
    clock: Rc<dyn RuntimeClock>,
    completions: VecDeque<()>,
    timers: VecDeque<()>,
    cleanup: VecDeque<()>,
    ready_roots: VecDeque<()>,
    after_item: Option<Box<dyn FnMut()>>,
    source_cursor: usize,
}

impl BoundedDriver {
    pub fn new(clock: Rc<dyn RuntimeClock>) -> Self {
        Self {
            clock,
            completions: VecDeque::new(),
            timers: VecDeque::new(),
            cleanup: VecDeque::new(),
            ready_roots: VecDeque::new(),
            after_item: None,
            source_cursor: 0,
        }
    }

    pub fn add_completions(&mut self, count: usize) {
        self.completions.extend(std::iter::repeat_n((), count));
    }
    pub fn add_timers(&mut self, count: usize) {
        self.timers.extend(std::iter::repeat_n((), count));
    }
    pub fn add_cleanup(&mut self, count: usize) {
        self.cleanup.extend(std::iter::repeat_n((), count));
    }
    pub fn add_ready_roots(&mut self, count: usize) {
        self.ready_roots.extend(std::iter::repeat_n((), count));
    }
    pub fn set_after_item(&mut self, callback: impl FnMut() + 'static) {
        self.after_item = Some(Box::new(callback));
    }

    pub fn pending_ready_roots(&self) -> usize {
        self.ready_roots.len()
    }

    pub fn drive(&mut self, budget: &DriveBudget) -> DriveReport {
        let start = self.clock.now();
        let limit = budget.work_item_limit.get();
        let mut report = DriveReport::default();
        let reserved_roots = self.ready_roots.len().min(budget.root_visit_limit.get());
        let mut no_progress = 0;
        while report.work_items < limit {
            let unvisited_reserved = reserved_roots - report.root_visits;
            let remaining_credits = limit - report.work_items;
            let source =
                if limit > 1 && unvisited_reserved > 0 && remaining_credits <= unvisited_reserved {
                    3
                } else {
                    let source = self.source_cursor % 4;
                    self.source_cursor = (self.source_cursor + 1) % 4;
                    source
                };
            let progressed = match source {
                0 if report.completions < budget.completion_limit.get()
                    && !self.completions.is_empty() =>
                {
                    self.completions.pop_front();
                    report.completions += 1;
                    true
                }
                1 if report.timers < budget.timer_limit.get() && !self.timers.is_empty() => {
                    self.timers.pop_front();
                    report.timers += 1;
                    true
                }
                2 if report.cleanup < budget.cleanup_limit.get() && !self.cleanup.is_empty() => {
                    self.cleanup.pop_front();
                    report.cleanup += 1;
                    true
                }
                3 if report.root_visits < reserved_roots && !self.ready_roots.is_empty() => {
                    self.ready_roots.pop_front();
                    report.root_visits += 1;
                    true
                }
                _ => false,
            };
            if !progressed {
                no_progress += 1;
                if no_progress == 4 {
                    break;
                }
                continue;
            }
            no_progress = 0;
            report.work_items += 1;
            if let Some(callback) = &mut self.after_item {
                callback();
            }
            if self.clock.now().saturating_duration_since(start) >= budget.wall_clock_limit {
                break;
            }
        }
        report.ready_remaining = !self.ready_roots.is_empty();
        report
    }
}
