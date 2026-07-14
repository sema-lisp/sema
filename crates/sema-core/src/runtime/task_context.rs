use std::any::{Any, TypeId};
use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use hashbrown::HashMap;

use crate::cycle::GcEdge;

use super::Trace;

pub trait TaskLocalValue: Trace {
    fn inherit(&self) -> Rc<dyn TaskLocalValue>;
    fn as_any(&self) -> &dyn Any;
}

#[derive(Default)]
pub struct TaskContext {
    extensions: HashMap<TypeId, Rc<dyn TaskLocalValue>>,
}

impl TaskContext {
    #[doc(hidden)]
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn insert<T: TaskLocalValue + 'static>(
        &mut self,
        value: Rc<T>,
    ) -> Option<Rc<dyn TaskLocalValue>> {
        self.extensions.insert(TypeId::of::<T>(), value)
    }

    pub fn get<T: TaskLocalValue + 'static>(&self) -> Option<&T> {
        self.extensions
            .get(&TypeId::of::<T>())
            .and_then(|value| value.as_any().downcast_ref())
    }

    pub fn remove<T: TaskLocalValue + 'static>(&mut self) -> Option<Rc<dyn TaskLocalValue>> {
        self.extensions.remove(&TypeId::of::<T>())
    }

    pub fn inherit_for_child(&self) -> Self {
        Self {
            extensions: self
                .extensions
                .iter()
                .map(|(type_id, value)| (*type_id, value.inherit()))
                .collect(),
        }
    }
}

impl Trace for TaskContext {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        self.extensions.values().all(|value| value.trace(sink))
    }
}

#[derive(Clone, Default)]
pub struct TaskContextHandle(Rc<RefCell<TaskContext>>);

impl TaskContextHandle {
    pub fn borrow(&self) -> Ref<'_, TaskContext> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, TaskContext> {
        self.0.borrow_mut()
    }

    pub fn inherit_for_child(&self) -> Self {
        Self(Rc::new(RefCell::new(self.borrow().inherit_for_child())))
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::Cell;
    use std::rc::Rc;

    use crate::cycle::GcEdge;

    use super::*;

    struct Shared {
        value: Rc<String>,
        inherit_calls: Rc<Cell<usize>>,
    }

    impl Trace for Shared {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl TaskLocalValue for Shared {
        fn inherit(&self) -> Rc<dyn TaskLocalValue> {
            self.inherit_calls.set(self.inherit_calls.get() + 1);
            Rc::new(Self {
                value: Rc::clone(&self.value),
                inherit_calls: Rc::clone(&self.inherit_calls),
            })
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct Resettable {
        value: u32,
        inherit_calls: Rc<Cell<usize>>,
    }

    impl Trace for Resettable {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl TaskLocalValue for Resettable {
        fn inherit(&self) -> Rc<dyn TaskLocalValue> {
            self.inherit_calls.set(self.inherit_calls.get() + 1);
            Rc::new(Self {
                value: 0,
                inherit_calls: Rc::clone(&self.inherit_calls),
            })
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn task_context_typed_operations_keep_type_ids_separate() {
        let mut context = TaskContext::default();
        context.insert(Rc::new(Shared {
            value: Rc::new("shared".into()),
            inherit_calls: Rc::new(Cell::new(0)),
        }));
        context.insert(Rc::new(Resettable {
            value: 7,
            inherit_calls: Rc::new(Cell::new(0)),
        }));

        assert_eq!(context.get::<Shared>().unwrap().value.as_str(), "shared");
        assert_eq!(context.get::<Resettable>().unwrap().value, 7);
        assert!(context.remove::<Shared>().is_some());
        assert!(context.get::<Shared>().is_none());
        assert_eq!(context.get::<Resettable>().unwrap().value, 7);
    }

    #[test]
    fn task_context_child_applies_each_extension_policy_once() {
        let shared = Rc::new("shared".to_owned());
        let shared_calls = Rc::new(Cell::new(0));
        let reset_calls = Rc::new(Cell::new(0));
        let mut parent = TaskContext::default();
        parent.insert(Rc::new(Shared {
            value: Rc::clone(&shared),
            inherit_calls: Rc::clone(&shared_calls),
        }));
        parent.insert(Rc::new(Resettable {
            value: 9,
            inherit_calls: Rc::clone(&reset_calls),
        }));

        let child = parent.inherit_for_child();

        assert!(Rc::ptr_eq(&child.get::<Shared>().unwrap().value, &shared));
        assert_eq!(child.get::<Resettable>().unwrap().value, 0);
        assert_eq!(parent.get::<Resettable>().unwrap().value, 9);
        assert_eq!(shared_calls.get(), 1);
        assert_eq!(reset_calls.get(), 1);
    }

    struct CountTrace {
        calls: Rc<Cell<usize>>,
        succeeds: bool,
    }

    impl Trace for CountTrace {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            self.calls.set(self.calls.get() + 1);
            self.succeeds
        }
    }

    impl TaskLocalValue for CountTrace {
        fn inherit(&self) -> Rc<dyn TaskLocalValue> {
            Rc::new(Self {
                calls: Rc::clone(&self.calls),
                succeeds: self.succeeds,
            })
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct TraceAlias(CountTrace);

    impl Trace for TraceAlias {
        fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            self.0.trace(sink)
        }
    }

    impl TaskLocalValue for TraceAlias {
        fn inherit(&self) -> Rc<dyn TaskLocalValue> {
            Rc::new(Self(CountTrace {
                calls: Rc::clone(&self.0.calls),
                succeeds: self.0.succeeds,
            }))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn task_context_trace_visits_each_entry_once() {
        let first = Rc::new(Cell::new(0));
        let second = Rc::new(Cell::new(0));
        let mut context = TaskContext::default();
        context.insert(Rc::new(CountTrace {
            calls: Rc::clone(&first),
            succeeds: true,
        }));
        context.insert(Rc::new(TraceAlias(CountTrace {
            calls: Rc::clone(&second),
            succeeds: true,
        })));

        assert!(context.trace(&mut |_| {}));
        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 1);
    }

    #[test]
    fn task_context_trace_short_circuits_after_failure() {
        let failure = Rc::new(Cell::new(0));
        let other = Rc::new(Cell::new(0));
        let mut context = TaskContext::default();
        context.insert(Rc::new(CountTrace {
            calls: Rc::clone(&failure),
            succeeds: false,
        }));
        context.insert(Rc::new(TraceAlias(CountTrace {
            calls: Rc::clone(&other),
            succeeds: true,
        })));

        assert!(!context.trace(&mut |_| {}));
        assert_eq!(failure.get(), 1);
        assert!(other.get() <= 1);
    }
}
