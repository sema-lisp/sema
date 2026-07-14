use std::collections::{HashMap, HashSet, VecDeque};

use sema_core::runtime::{RootId, TaskId};

#[derive(Default)]
pub struct ReadyScheduler {
    roots: VecDeque<RootId>,
    root_membership: HashSet<RootId>,
    tasks_by_root: HashMap<RootId, VecDeque<TaskId>>,
    task_membership: HashSet<TaskId>,
}

impl ReadyScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, root: RootId, task: TaskId) -> bool {
        if !self.task_membership.insert(task) {
            self.assert_invariants();
            return false;
        }

        self.tasks_by_root.entry(root).or_default().push_back(task);
        if self.root_membership.insert(root) {
            self.roots.push_back(root);
        }
        self.assert_invariants();
        true
    }

    pub fn dequeue(&mut self) -> Option<(RootId, TaskId)> {
        let root = self.roots.pop_front()?;
        let queue = self
            .tasks_by_root
            .get_mut(&root)
            .expect("active root has a ready queue");
        let task = queue.pop_front().expect("active root has a ready task");
        self.task_membership.remove(&task);

        if queue.is_empty() {
            self.tasks_by_root.remove(&root);
            self.root_membership.remove(&root);
        } else {
            self.roots.push_back(root);
        }
        self.assert_invariants();
        Some((root, task))
    }

    pub fn remove_root(&mut self, root: RootId) -> Vec<TaskId> {
        let removed = self.tasks_by_root.remove(&root).unwrap_or_default();
        for task in &removed {
            self.task_membership.remove(task);
        }
        if self.root_membership.remove(&root) {
            self.roots.retain(|queued| *queued != root);
        }
        self.assert_invariants();
        removed.into()
    }

    #[cfg(test)]
    fn assert_invariants(&self) {
        let queued_roots: HashSet<_> = self.roots.iter().copied().collect();
        assert_eq!(queued_roots.len(), self.roots.len());
        assert_eq!(queued_roots, self.root_membership);
        assert_eq!(
            self.tasks_by_root.keys().copied().collect::<HashSet<_>>(),
            self.root_membership
        );
        assert!(self.tasks_by_root.values().all(|queue| !queue.is_empty()));

        let queued_tasks: HashSet<_> = self
            .tasks_by_root
            .values()
            .flat_map(|queue| queue.iter().copied())
            .collect();
        let queued_task_count = self
            .tasks_by_root
            .values()
            .map(VecDeque::len)
            .sum::<usize>();
        assert_eq!(queued_tasks.len(), queued_task_count);
        assert_eq!(queued_tasks, self.task_membership);
    }

    #[cfg(not(test))]
    fn assert_invariants(&self) {}
}
