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
    completions: usize,
    timers: usize,
    cleanup: usize,
    ready_roots: usize,
    after_item: Option<Box<dyn FnMut()>>,
}

impl BoundedDriver {
    pub fn new(clock: Rc<dyn RuntimeClock>) -> Self {
        Self {
            clock,
            completions: 0,
            timers: 0,
            cleanup: 0,
            ready_roots: 0,
            after_item: None,
        }
    }

    pub fn add_completions(&mut self, count: usize) {
        self.completions += count;
    }
    pub fn add_timers(&mut self, count: usize) {
        self.timers += count;
    }
    pub fn add_cleanup(&mut self, count: usize) {
        self.cleanup += count;
    }
    pub fn add_ready_roots(&mut self, count: usize) {
        self.ready_roots += count;
    }
    pub fn set_after_item(&mut self, callback: impl FnMut() + 'static) {
        self.after_item = Some(Box::new(callback));
    }

    pub fn drive(&mut self, budget: &DriveBudget) -> DriveReport {
        let start = self.clock.now();
        let limit = budget.work_item_limit.get();
        let mut report = DriveReport::default();
        let reserved_roots = self
            .ready_roots
            .min(budget.root_visit_limit.get())
            .min(limit);
        self.ready_roots -= reserved_roots;
        report.root_visits = reserved_roots;
        report.work_items = reserved_roots;

        let mut source = 0;
        while report.work_items < limit {
            let progressed = match source % 3 {
                0 if report.completions < budget.completion_limit.get() && self.completions > 0 => {
                    self.completions -= 1;
                    report.completions += 1;
                    true
                }
                1 if report.timers < budget.timer_limit.get() && self.timers > 0 => {
                    self.timers -= 1;
                    report.timers += 1;
                    true
                }
                2 if report.cleanup < budget.cleanup_limit.get() && self.cleanup > 0 => {
                    self.cleanup -= 1;
                    report.cleanup += 1;
                    true
                }
                _ => false,
            };
            source += 1;
            if !progressed {
                if source >= 6 {
                    break;
                }
                continue;
            }
            report.work_items += 1;
            if let Some(callback) = &mut self.after_item {
                callback();
            }
            if self.clock.now().saturating_duration_since(start) >= budget.wall_clock_limit {
                break;
            }
        }
        report.ready_remaining = self.ready_roots > 0;
        report
    }
}
