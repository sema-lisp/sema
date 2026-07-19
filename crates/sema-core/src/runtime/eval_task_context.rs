use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use crate::cycle::GcEdge;
use crate::runtime::{IdCounter, IdExhausted, ScopeId, TaskLocalValue, Trace};
use crate::Value;

#[derive(Clone, Debug)]
struct Scoped<T> {
    id: Option<ScopeId>,
    value: T,
}

impl<T> Scoped<T> {
    fn inherited(value: T) -> Self {
        Self { id: None, value }
    }

    fn owned(id: ScopeId, value: T) -> Self {
        Self {
            id: Some(id),
            value,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct ModuleState {
    scope_ids: IdCounter<ScopeId>,
    current_files: Vec<Scoped<PathBuf>>,
    loading: Vec<Scoped<PathBuf>>,
    exports: Vec<Scoped<Option<Vec<String>>>>,
}

#[derive(Debug)]
pub struct ModuleTaskState {
    inner: RefCell<ModuleState>,
}

impl Default for ModuleTaskState {
    fn default() -> Self {
        Self {
            inner: RefCell::new(ModuleState::default()),
        }
    }
}

impl ModuleTaskState {
    pub fn from_snapshot(
        current_files: Vec<PathBuf>,
        loading: Vec<PathBuf>,
        exports: Vec<Option<Vec<String>>>,
    ) -> Self {
        Self {
            inner: RefCell::new(ModuleState {
                scope_ids: IdCounter::new(),
                current_files: current_files.into_iter().map(Scoped::inherited).collect(),
                loading: loading.into_iter().map(Scoped::inherited).collect(),
                exports: exports.into_iter().map(Scoped::inherited).collect(),
            }),
        }
    }

    pub fn current_file(&self) -> Option<PathBuf> {
        self.inner
            .borrow()
            .current_files
            .last()
            .map(|entry| entry.value.clone())
    }

    pub fn push_current_file(&self, path: PathBuf) -> Result<ScopeId, IdExhausted> {
        let mut inner = self.inner.borrow_mut();
        let id = inner.scope_ids.allocate()?;
        inner.current_files.push(Scoped::owned(id, path));
        Ok(id)
    }

    pub fn remove_current_file(&self, id: ScopeId) -> bool {
        remove_scope(&mut self.inner.borrow_mut().current_files, id).is_some()
    }

    pub fn loading(&self) -> Vec<PathBuf> {
        self.inner
            .borrow()
            .loading
            .iter()
            .map(|entry| entry.value.clone())
            .collect()
    }

    pub fn push_loading(&self, path: PathBuf) -> Result<ScopeId, IdExhausted> {
        let mut inner = self.inner.borrow_mut();
        let id = inner.scope_ids.allocate()?;
        inner.loading.push(Scoped::owned(id, path));
        Ok(id)
    }

    pub fn remove_loading(&self, id: ScopeId) -> bool {
        remove_scope(&mut self.inner.borrow_mut().loading, id).is_some()
    }

    pub fn exports(&self) -> Vec<Option<Vec<String>>> {
        self.inner
            .borrow()
            .exports
            .iter()
            .map(|entry| entry.value.clone())
            .collect()
    }

    pub fn push_exports(&self, exports: Option<Vec<String>>) -> Result<ScopeId, IdExhausted> {
        let mut inner = self.inner.borrow_mut();
        let id = inner.scope_ids.allocate()?;
        inner.exports.push(Scoped::owned(id, exports));
        Ok(id)
    }

    pub fn set_current_exports(&self, exports: Vec<String>) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(current) = inner.exports.last_mut() else {
            return false;
        };
        current.value = Some(exports);
        true
    }

    pub fn take_exports(&self, id: ScopeId) -> Option<Option<Vec<String>>> {
        remove_scope(&mut self.inner.borrow_mut().exports, id)
    }

    pub fn remove_exports(&self, id: ScopeId) -> bool {
        self.take_exports(id).is_some()
    }
}

impl Trace for ModuleTaskState {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        self.inner.try_borrow().is_ok()
    }
}

impl TaskLocalValue for ModuleTaskState {
    fn inherit(&self) -> Rc<dyn TaskLocalValue> {
        Rc::new(Self {
            inner: RefCell::new(self.inner.borrow().clone()),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DynamicMutation {
    UserSet(Value, Value),
    UserRemove(Value),
    UserClear,
    HiddenSet(Value, Value),
    StackPush(Value, Value),
    StackPop(Value),
}

#[derive(Clone, Debug)]
struct DynamicState {
    scope_ids: IdCounter<ScopeId>,
    user_frames: Vec<Scoped<BTreeMap<Value, Value>>>,
    hidden_frames: Vec<Scoped<BTreeMap<Value, Value>>>,
    stacks: BTreeMap<Value, Vec<Scoped<Value>>>,
    mutations: Option<Vec<DynamicMutation>>,
}

#[derive(Debug)]
pub struct DynamicTaskState {
    inner: RefCell<DynamicState>,
}

impl DynamicTaskState {
    pub fn root(
        mut user_frames: Vec<BTreeMap<Value, Value>>,
        mut hidden_frames: Vec<BTreeMap<Value, Value>>,
        stacks: BTreeMap<Value, Vec<Value>>,
    ) -> Self {
        if user_frames.is_empty() {
            user_frames.push(BTreeMap::new());
        }
        if hidden_frames.is_empty() {
            hidden_frames.push(BTreeMap::new());
        }
        Self {
            inner: RefCell::new(DynamicState {
                scope_ids: IdCounter::new(),
                user_frames: user_frames.into_iter().map(Scoped::inherited).collect(),
                hidden_frames: hidden_frames.into_iter().map(Scoped::inherited).collect(),
                stacks: stacks
                    .into_iter()
                    .map(|(key, values)| (key, values.into_iter().map(Scoped::inherited).collect()))
                    .collect(),
                mutations: Some(Vec::new()),
            }),
        }
    }

    pub fn user_get(&self, key: &Value) -> Option<Value> {
        self.inner
            .borrow()
            .user_frames
            .iter()
            .rev()
            .find_map(|frame| frame.value.get(key).cloned())
    }

    pub fn user_all(&self) -> BTreeMap<Value, Value> {
        self.inner
            .borrow()
            .user_frames
            .iter()
            .flat_map(|frame| frame.value.iter())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    pub fn user_set(&self, key: Value, value: Value) {
        let mut inner = self.inner.borrow_mut();
        let publishes = inner.user_frames.len() == 1;
        if let Some(frame) = inner.user_frames.last_mut() {
            frame.value.insert(key.clone(), value.clone());
        }
        if publishes {
            record(&mut inner, DynamicMutation::UserSet(key, value));
        }
    }

    pub fn user_remove(&self, key: &Value) -> Option<Value> {
        let mut inner = self.inner.borrow_mut();
        let publishes = inner
            .user_frames
            .first()
            .is_some_and(|frame| frame.value.contains_key(key));
        let mut removed = None;
        for frame in inner.user_frames.iter_mut().rev() {
            if let Some(value) = frame.value.remove(key) {
                if removed.is_none() {
                    removed = Some(value);
                }
            }
        }
        if publishes {
            record(&mut inner, DynamicMutation::UserRemove(key.clone()));
        }
        removed
    }

    pub fn user_clear(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.user_frames.clear();
        inner.user_frames.push(Scoped::inherited(BTreeMap::new()));
        record(&mut inner, DynamicMutation::UserClear);
    }

    pub fn push_user_frame(
        &self,
        bindings: BTreeMap<Value, Value>,
    ) -> Result<ScopeId, IdExhausted> {
        let mut inner = self.inner.borrow_mut();
        let id = inner.scope_ids.allocate()?;
        inner.user_frames.push(Scoped::owned(id, bindings));
        Ok(id)
    }

    pub fn remove_user_frame(&self, id: ScopeId) -> bool {
        remove_scope(&mut self.inner.borrow_mut().user_frames, id).is_some()
    }

    pub fn hidden_get(&self, key: &Value) -> Option<Value> {
        self.inner
            .borrow()
            .hidden_frames
            .iter()
            .rev()
            .find_map(|frame| frame.value.get(key).cloned())
    }

    pub fn hidden_set(&self, key: Value, value: Value) {
        let mut inner = self.inner.borrow_mut();
        let publishes = inner.hidden_frames.len() == 1;
        if let Some(frame) = inner.hidden_frames.last_mut() {
            frame.value.insert(key.clone(), value.clone());
        }
        if publishes {
            record(&mut inner, DynamicMutation::HiddenSet(key, value));
        }
    }

    pub fn push_hidden_frame(
        &self,
        bindings: BTreeMap<Value, Value>,
    ) -> Result<ScopeId, IdExhausted> {
        let mut inner = self.inner.borrow_mut();
        let id = inner.scope_ids.allocate()?;
        inner.hidden_frames.push(Scoped::owned(id, bindings));
        Ok(id)
    }

    pub fn remove_hidden_frame(&self, id: ScopeId) -> bool {
        remove_scope(&mut self.inner.borrow_mut().hidden_frames, id).is_some()
    }

    pub fn stack_get(&self, key: &Value) -> Vec<Value> {
        self.inner
            .borrow()
            .stacks
            .get(key)
            .map(|values| values.iter().map(|entry| entry.value.clone()).collect())
            .unwrap_or_default()
    }

    pub fn stack_push(&self, key: Value, value: Value) -> Result<ScopeId, IdExhausted> {
        let mut inner = self.inner.borrow_mut();
        let id = inner.scope_ids.allocate()?;
        inner
            .stacks
            .entry(key.clone())
            .or_default()
            .push(Scoped::owned(id, value.clone()));
        record(&mut inner, DynamicMutation::StackPush(key, value));
        Ok(id)
    }

    pub fn stack_pop(&self, key: &Value) -> Option<Value> {
        let mut inner = self.inner.borrow_mut();
        let value = inner.stacks.get_mut(key)?.pop()?.value;
        if inner.stacks.get(key).is_some_and(Vec::is_empty) {
            inner.stacks.remove(key);
        }
        record(&mut inner, DynamicMutation::StackPop(key.clone()));
        Some(value)
    }

    pub fn remove_stack_value(&self, key: &Value, id: ScopeId) -> Option<Value> {
        let mut inner = self.inner.borrow_mut();
        let stack = inner.stacks.get_mut(key)?;
        let index = stack.iter().position(|entry| entry.id == Some(id))?;
        let removed = stack.remove(index).value;
        let above: Vec<Value> = stack[index..]
            .iter()
            .map(|entry| entry.value.clone())
            .collect();
        if stack.is_empty() {
            inner.stacks.remove(key);
        }
        for _ in 0..=above.len() {
            record(&mut inner, DynamicMutation::StackPop(key.clone()));
        }
        for value in above {
            record(&mut inner, DynamicMutation::StackPush(key.clone(), value));
        }
        Some(removed)
    }

    pub fn drain_mutations(&self) -> Option<Vec<DynamicMutation>> {
        self.inner
            .borrow_mut()
            .mutations
            .as_mut()
            .map(std::mem::take)
    }
}

impl Trace for DynamicTaskState {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        let Ok(inner) = self.inner.try_borrow() else {
            return false;
        };
        for frame in inner.user_frames.iter().chain(&inner.hidden_frames) {
            for (key, value) in &frame.value {
                sink(GcEdge::Value(key));
                sink(GcEdge::Value(value));
            }
        }
        for (key, values) in &inner.stacks {
            sink(GcEdge::Value(key));
            for value in values {
                sink(GcEdge::Value(&value.value));
            }
        }
        if let Some(mutations) = &inner.mutations {
            for mutation in mutations {
                trace_mutation(mutation, sink);
            }
        }
        true
    }
}

impl TaskLocalValue for DynamicTaskState {
    fn inherit(&self) -> Rc<dyn TaskLocalValue> {
        let mut inner = self.inner.borrow().clone();
        inner.mutations = None;
        Rc::new(Self {
            inner: RefCell::new(inner),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn remove_scope<T>(entries: &mut Vec<Scoped<T>>, id: ScopeId) -> Option<T> {
    entries
        .iter()
        .position(|entry| entry.id == Some(id))
        .map(|index| entries.remove(index).value)
}

fn record(inner: &mut DynamicState, mutation: DynamicMutation) {
    if let Some(mutations) = &mut inner.mutations {
        mutations.push(mutation);
    }
}

fn trace_mutation(mutation: &DynamicMutation, sink: &mut dyn FnMut(GcEdge<'_>)) {
    match mutation {
        DynamicMutation::UserSet(key, value)
        | DynamicMutation::HiddenSet(key, value)
        | DynamicMutation::StackPush(key, value) => {
            sink(GcEdge::Value(key));
            sink(GcEdge::Value(value));
        }
        DynamicMutation::UserRemove(key) | DynamicMutation::StackPop(key) => {
            sink(GcEdge::Value(key));
        }
        DynamicMutation::UserClear => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::rc::Rc;

    use crate::cycle::GcEdge;
    use crate::runtime::{TaskContext, Trace};
    use crate::Value;

    use super::{DynamicMutation, DynamicTaskState, ModuleTaskState};

    #[test]
    fn module_scopes_remove_the_exact_current_file_token_once() {
        let state = ModuleTaskState::default();
        let first = state
            .push_current_file(PathBuf::from("first.sema"))
            .expect("scope ID available");
        let second = state
            .push_current_file(PathBuf::from("second.sema"))
            .expect("scope ID available");

        assert!(state.remove_current_file(first));
        assert!(!state.remove_current_file(first));
        assert_eq!(state.current_file(), Some(PathBuf::from("second.sema")));
        assert!(state.remove_current_file(second));
        assert_eq!(state.current_file(), None);
    }

    #[test]
    fn module_scopes_remove_the_exact_loading_token_once() {
        let state = ModuleTaskState::default();
        let first = state
            .push_loading(PathBuf::from("first.sema"))
            .expect("scope ID available");
        let second = state
            .push_loading(PathBuf::from("second.sema"))
            .expect("scope ID available");

        assert!(state.remove_loading(first));
        assert!(!state.remove_loading(first));
        assert_eq!(state.loading(), vec![PathBuf::from("second.sema")]);
        assert!(state.remove_loading(second));
        assert!(state.loading().is_empty());
    }

    #[test]
    fn module_scopes_remove_the_exact_export_token_once() {
        let state = ModuleTaskState::default();
        let first = state
            .push_exports(Some(vec!["first".to_owned()]))
            .expect("scope ID available");
        let second = state
            .push_exports(Some(vec!["second".to_owned()]))
            .expect("scope ID available");

        assert!(state.remove_exports(first));
        assert!(!state.remove_exports(first));
        assert_eq!(state.exports(), vec![Some(vec!["second".to_owned()])]);
        assert!(state.remove_exports(second));
        assert!(state.exports().is_empty());
    }

    #[test]
    fn module_exports_update_and_take_the_exact_scope() {
        let state = ModuleTaskState::default();
        let outer = state.push_exports(None).expect("scope ID available");
        let inner = state.push_exports(None).expect("scope ID available");

        assert!(state.set_current_exports(vec!["inner".to_owned()]));
        assert_eq!(
            state.take_exports(outer),
            Some(None),
            "taking an outer scope must not consume the current scope"
        );
        assert_eq!(
            state.take_exports(inner),
            Some(Some(vec!["inner".to_owned()]))
        );
        assert_eq!(state.take_exports(inner), None);
    }

    #[test]
    fn module_child_snapshot_is_independent() {
        let parent = Rc::new(ModuleTaskState::from_snapshot(
            vec![PathBuf::from("parent.sema")],
            vec![PathBuf::from("loading.sema")],
            vec![Some(vec!["parent-export".to_owned()])],
        ));
        let mut context = TaskContext::default();
        context.insert(Rc::clone(&parent));

        let child = context.inherit_for_child();
        let child = child
            .get_rc::<ModuleTaskState>()
            .expect("module state inherited");
        let child_file = child
            .push_current_file(PathBuf::from("child.sema"))
            .expect("scope ID available");
        let child_loading = child
            .push_loading(PathBuf::from("child-loading.sema"))
            .expect("scope ID available");
        let child_exports = child.push_exports(None).expect("scope ID available");

        assert_eq!(parent.current_file(), Some(PathBuf::from("parent.sema")));
        assert_eq!(parent.loading(), vec![PathBuf::from("loading.sema")]);
        assert_eq!(
            parent.exports(),
            vec![Some(vec!["parent-export".to_owned()])]
        );
        assert!(child.remove_current_file(child_file));
        assert!(child.remove_loading(child_loading));
        assert!(child.remove_exports(child_exports));
    }

    #[test]
    fn dynamic_scope_cleanup_is_exact_and_idempotent() {
        let state = DynamicTaskState::root(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            BTreeMap::new(),
        );
        let outer_user = state
            .push_user_frame(BTreeMap::from([(
                Value::keyword("frame"),
                Value::string("outer"),
            )]))
            .expect("scope ID available");
        let inner_user = state
            .push_user_frame(BTreeMap::from([(
                Value::keyword("frame"),
                Value::string("inner"),
            )]))
            .expect("scope ID available");
        let outer_hidden = state
            .push_hidden_frame(BTreeMap::new())
            .expect("scope ID available");
        let inner_hidden = state
            .push_hidden_frame(BTreeMap::new())
            .expect("scope ID available");
        let stack_key = Value::keyword("stack");
        let outer_stack = state
            .stack_push(stack_key.clone(), Value::string("outer"))
            .expect("scope ID available");
        let inner_stack = state
            .stack_push(stack_key.clone(), Value::string("inner"))
            .expect("scope ID available");

        assert!(state.remove_user_frame(outer_user));
        assert!(!state.remove_user_frame(outer_user));
        assert_eq!(
            state.user_get(&Value::keyword("frame")),
            Some(Value::string("inner"))
        );
        assert!(state.remove_hidden_frame(outer_hidden));
        assert!(!state.remove_hidden_frame(outer_hidden));
        assert_eq!(
            state.remove_stack_value(&stack_key, outer_stack),
            Some(Value::string("outer"))
        );
        assert_eq!(state.remove_stack_value(&stack_key, outer_stack), None);
        assert_eq!(state.stack_get(&stack_key), vec![Value::string("inner")]);

        assert!(state.remove_user_frame(inner_user));
        assert!(state.remove_hidden_frame(inner_hidden));
        assert_eq!(
            state.remove_stack_value(&stack_key, inner_stack),
            Some(Value::string("inner"))
        );
    }

    #[test]
    fn root_dynamic_state_records_publishable_mutations_in_order() {
        let state = DynamicTaskState::root(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            BTreeMap::new(),
        );
        let user_key = Value::keyword("user");
        let hidden_key = Value::keyword("hidden");
        let stack_key = Value::keyword("stack");

        state.user_set(user_key.clone(), Value::int(1));
        assert_eq!(state.user_remove(&user_key), Some(Value::int(1)));
        state.user_clear();
        state.hidden_set(hidden_key.clone(), Value::int(2));
        state
            .stack_push(stack_key.clone(), Value::int(3))
            .expect("scope ID available");
        assert_eq!(state.stack_pop(&stack_key), Some(Value::int(3)));

        assert_eq!(
            state.drain_mutations(),
            Some(vec![
                DynamicMutation::UserSet(user_key.clone(), Value::int(1)),
                DynamicMutation::UserRemove(user_key),
                DynamicMutation::UserClear,
                DynamicMutation::HiddenSet(hidden_key, Value::int(2)),
                DynamicMutation::StackPush(stack_key.clone(), Value::int(3)),
                DynamicMutation::StackPop(stack_key),
            ])
        );
        assert_eq!(state.drain_mutations(), Some(Vec::new()));
    }

    #[test]
    fn dynamic_user_all_merges_frames_from_outer_to_inner() {
        let outer_only = Value::keyword("outer-only");
        let shadowed = Value::keyword("shadowed");
        let inner_only = Value::keyword("inner-only");
        let state = DynamicTaskState::root(
            vec![
                BTreeMap::from([
                    (outer_only.clone(), Value::int(1)),
                    (shadowed.clone(), Value::int(2)),
                ]),
                BTreeMap::from([
                    (shadowed.clone(), Value::int(3)),
                    (inner_only.clone(), Value::int(4)),
                ]),
            ],
            vec![BTreeMap::new()],
            BTreeMap::new(),
        );

        assert_eq!(
            state.user_all(),
            BTreeMap::from([
                (outer_only, Value::int(1)),
                (shadowed, Value::int(3)),
                (inner_only, Value::int(4)),
            ])
        );
    }

    #[test]
    fn dynamic_child_deep_clones_state_without_publication_authority() {
        let user_key = Value::keyword("user");
        let hidden_key = Value::keyword("hidden");
        let stack_key = Value::keyword("stack");
        let parent = Rc::new(DynamicTaskState::root(
            vec![BTreeMap::from([(user_key.clone(), Value::int(1))])],
            vec![BTreeMap::from([(hidden_key.clone(), Value::int(2))])],
            BTreeMap::from([(stack_key.clone(), vec![Value::int(3)])]),
        ));
        let mut context = TaskContext::default();
        context.insert(Rc::clone(&parent));

        let child = context.inherit_for_child();
        let child = child
            .get_rc::<DynamicTaskState>()
            .expect("dynamic state inherited");
        child.user_set(user_key.clone(), Value::int(10));
        child.hidden_set(hidden_key.clone(), Value::int(20));
        child
            .stack_push(stack_key.clone(), Value::int(30))
            .expect("scope ID available");

        assert_eq!(parent.user_get(&user_key), Some(Value::int(1)));
        assert_eq!(parent.hidden_get(&hidden_key), Some(Value::int(2)));
        assert_eq!(parent.stack_get(&stack_key), vec![Value::int(3)]);
        assert_eq!(child.user_get(&user_key), Some(Value::int(10)));
        assert_eq!(child.hidden_get(&hidden_key), Some(Value::int(20)));
        assert_eq!(
            child.stack_get(&stack_key),
            vec![Value::int(3), Value::int(30)]
        );
        assert_eq!(child.drain_mutations(), None);
        assert_eq!(parent.drain_mutations(), Some(Vec::new()));
    }

    #[test]
    fn dynamic_trace_reports_every_stored_value_edge_exactly_once() {
        let state = DynamicTaskState::root(
            vec![BTreeMap::from([(Value::keyword("user"), Value::int(1))])],
            vec![BTreeMap::from([(Value::keyword("hidden"), Value::int(2))])],
            BTreeMap::from([(Value::keyword("stack"), vec![Value::int(3), Value::int(4)])]),
        );
        state.user_set(Value::keyword("published"), Value::int(5));
        state.hidden_set(Value::keyword("private"), Value::int(6));
        state
            .stack_push(Value::keyword("other-stack"), Value::int(7))
            .expect("scope ID available");
        state.user_remove(&Value::keyword("published"));

        let mut edges = 0;
        assert!(state.trace(&mut |edge| {
            assert!(matches!(edge, GcEdge::Value(_)));
            edges += 1;
        }));

        // Live state: user 2, hidden 4, stacks 5. Journal: 2 + 2 + 2 + 1.
        assert_eq!(edges, 18);
    }
}
