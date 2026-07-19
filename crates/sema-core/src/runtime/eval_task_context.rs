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

/// Stable identity for one published dynamic-stack entry.
///
/// Pointer identity is deliberate: a mutation journal keeps the old token
/// alive, so an allocator cannot recycle its address while a stale pop can
/// still compare against it. Equal replacement values therefore remain
/// distinguishable without a wrapping numeric generation.
#[derive(Clone)]
pub(crate) struct DynamicStackEntryId(Rc<()>);

impl DynamicStackEntryId {
    fn fresh() -> Self {
        Self(Rc::new(()))
    }
}

impl std::fmt::Debug for DynamicStackEntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DynamicStackEntryId(..)")
    }
}

impl PartialEq for DynamicStackEntryId {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for DynamicStackEntryId {}

/// Interpreter-wide identity sidecar for dynamic context stacks.
///
/// The public stack values retain their legacy shape. This sidecar gives each
/// entry an opaque identity that root snapshots clone and publication checks.
#[derive(Clone, Debug, Default)]
pub(crate) struct DynamicStackIdentities {
    entries: BTreeMap<Value, Vec<DynamicStackEntryId>>,
}

impl DynamicStackIdentities {
    pub(crate) fn from_stacks(stacks: &BTreeMap<Value, Vec<Value>>) -> Self {
        Self {
            entries: stacks
                .iter()
                .map(|(key, values)| {
                    (
                        key.clone(),
                        values
                            .iter()
                            .map(|_| DynamicStackEntryId::fresh())
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    pub(crate) fn matches_stacks(&self, stacks: &BTreeMap<Value, Vec<Value>>) -> bool {
        self.entries.len() == stacks.len()
            && stacks.iter().all(|(key, values)| {
                self.entries
                    .get(key)
                    .is_some_and(|identities| identities.len() == values.len())
            })
    }

    pub(crate) fn push(&mut self, key: Value) {
        self.entries
            .entry(key)
            .or_default()
            .push(DynamicStackEntryId::fresh());
    }

    pub(crate) fn pop(&mut self, key: &Value) -> bool {
        let Some(entries) = self.entries.get_mut(key) else {
            return false;
        };
        if entries.pop().is_none() {
            return false;
        }
        if entries.is_empty() {
            self.entries.remove(key);
        }
        true
    }

    pub(crate) fn remove(&mut self, key: &Value) {
        self.entries.remove(key);
    }

    fn entries_for(&self, key: &Value, len: usize) -> Vec<DynamicStackEntryId> {
        let identities = self.entries.get(key).map(Vec::as_slice).unwrap_or(&[]);
        assert_eq!(
            identities.len(),
            len,
            "dynamic stack identities must match their value entries"
        );
        identities.to_vec()
    }

    fn push_exact(&mut self, key: Value, identity: DynamicStackEntryId) {
        self.entries.entry(key).or_default().push(identity);
    }

    fn pop_exact(&mut self, key: &Value, expected: &DynamicStackEntryId) -> bool {
        let Some(entries) = self.entries.get_mut(key) else {
            return false;
        };
        if entries.last() != Some(expected) {
            return false;
        }
        entries.pop();
        if entries.is_empty() {
            self.entries.remove(key);
        }
        true
    }
}

#[derive(Clone, Debug)]
struct DynamicStackEntry {
    scope: Option<ScopeId>,
    identity: DynamicStackEntryId,
    value: Value,
}

impl DynamicStackEntry {
    fn inherited(identity: DynamicStackEntryId, value: Value) -> Self {
        Self {
            scope: None,
            identity,
            value,
        }
    }

    fn owned(scope: ScopeId, value: Value) -> Self {
        Self {
            scope: Some(scope),
            identity: DynamicStackEntryId::fresh(),
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
        let mut inner = self.inner.borrow().clone();
        strip_scope_ids(&mut inner.current_files);
        strip_scope_ids(&mut inner.loading);
        strip_scope_ids(&mut inner.exports);
        Rc::new(Self {
            inner: RefCell::new(inner),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DynamicMutation {
    UserSet(Value, Value),
    UserRemove(Value),
    UserClear,
    HiddenSet(Value, Value),
    StackPush {
        key: Value,
        value: Value,
        scope: ScopeId,
        identity: DynamicStackEntryId,
    },
    /// Removes an inherited top only if publication still sees that exact
    /// entry. An equal value recreated later carries a different identity.
    StackPop {
        key: Value,
        expected: DynamicStackEntryId,
    },
}

#[derive(Clone, Debug)]
struct DynamicState {
    scope_ids: IdCounter<ScopeId>,
    user_frames: Vec<Scoped<BTreeMap<Value, Value>>>,
    hidden_frames: Vec<Scoped<BTreeMap<Value, Value>>>,
    stacks: BTreeMap<Value, Vec<DynamicStackEntry>>,
    mutations: Option<Vec<DynamicMutation>>,
}

#[derive(Debug)]
pub struct DynamicTaskState {
    inner: RefCell<DynamicState>,
}

impl DynamicTaskState {
    pub fn root(
        user_frames: Vec<BTreeMap<Value, Value>>,
        hidden_frames: Vec<BTreeMap<Value, Value>>,
        stacks: BTreeMap<Value, Vec<Value>>,
    ) -> Self {
        let identities = DynamicStackIdentities::from_stacks(&stacks);
        Self::root_with_stack_identities(user_frames, hidden_frames, stacks, &identities)
    }

    pub(crate) fn root_with_stack_identities(
        mut user_frames: Vec<BTreeMap<Value, Value>>,
        mut hidden_frames: Vec<BTreeMap<Value, Value>>,
        stacks: BTreeMap<Value, Vec<Value>>,
        identities: &DynamicStackIdentities,
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
                    .map(|(key, values)| {
                        let entries = identities
                            .entries_for(&key, values.len())
                            .into_iter()
                            .zip(values)
                            .map(|(identity, value)| DynamicStackEntry::inherited(identity, value))
                            .collect();
                        (key, entries)
                    })
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
        let publishes = inner
            .user_frames
            .last()
            .is_some_and(|frame| frame.id.is_none());
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
            .iter()
            .any(|frame| frame.id.is_none() && frame.value.contains_key(key));
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

    pub fn pop_user_frame(&self) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner.user_frames.len() <= 1 {
            return false;
        }
        inner.user_frames.pop();
        true
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
        let publishes = inner
            .hidden_frames
            .last()
            .is_some_and(|frame| frame.id.is_none());
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

    pub fn pop_hidden_frame(&self) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner.hidden_frames.len() <= 1 {
            return false;
        }
        inner.hidden_frames.pop();
        true
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
        let entry = DynamicStackEntry::owned(id, value.clone());
        let identity = entry.identity.clone();
        inner.stacks.entry(key.clone()).or_default().push(entry);
        record(
            &mut inner,
            DynamicMutation::StackPush {
                key,
                value,
                scope: id,
                identity,
            },
        );
        Ok(id)
    }

    pub fn stack_pop(&self, key: &Value) -> Option<Value> {
        let mut inner = self.inner.borrow_mut();
        let entry = inner.stacks.get_mut(key)?.pop()?;
        if inner.stacks.get(key).is_some_and(Vec::is_empty) {
            inner.stacks.remove(key);
        }
        if entry
            .scope
            .is_some_and(|scope| cancel_pending_stack_push(&mut inner, scope))
        {
            return Some(entry.value);
        }
        record(
            &mut inner,
            DynamicMutation::StackPop {
                key: key.clone(),
                expected: entry.identity,
            },
        );
        Some(entry.value)
    }

    pub fn remove_stack_value(&self, key: &Value, id: ScopeId) -> Option<Value> {
        let mut inner = self.inner.borrow_mut();
        let stack = inner.stacks.get_mut(key)?;
        let index = stack.iter().position(|entry| entry.scope == Some(id))?;
        let was_top = index + 1 == stack.len();
        let removed = stack.remove(index);
        if stack.is_empty() {
            inner.stacks.remove(key);
        }
        if !cancel_pending_stack_push(&mut inner, id) && was_top {
            record(
                &mut inner,
                DynamicMutation::StackPop {
                    key: key.clone(),
                    expected: removed.identity.clone(),
                },
            );
        }
        Some(removed.value)
    }

    pub(crate) fn drain_mutations(&self) -> Option<Vec<DynamicMutation>> {
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
        strip_scope_ids(&mut inner.user_frames);
        strip_scope_ids(&mut inner.hidden_frames);
        for stack in inner.stacks.values_mut() {
            for entry in stack {
                entry.scope = None;
            }
        }
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

fn strip_scope_ids<T>(entries: &mut [Scoped<T>]) {
    for entry in entries {
        entry.id = None;
    }
}

fn record(inner: &mut DynamicState, mutation: DynamicMutation) {
    if let Some(mutations) = &mut inner.mutations {
        mutations.push(mutation);
    }
}

fn cancel_pending_stack_push(inner: &mut DynamicState, scope: ScopeId) -> bool {
    let Some(mutations) = inner.mutations.as_mut() else {
        return false;
    };
    let Some(index) = mutations.iter().rposition(
        |mutation| matches!(mutation, DynamicMutation::StackPush { scope: candidate, .. } if *candidate == scope),
    ) else {
        return false;
    };
    mutations.remove(index);
    true
}

/// Applies a root's settlement journal to the interpreter-wide dynamic state.
pub(crate) fn apply_dynamic_mutations(
    user_frames: &mut Vec<BTreeMap<Value, Value>>,
    hidden_frames: &mut Vec<BTreeMap<Value, Value>>,
    stacks: &mut BTreeMap<Value, Vec<Value>>,
    stack_identities: &mut DynamicStackIdentities,
    mutations: &[DynamicMutation],
) {
    assert!(
        stack_identities.matches_stacks(stacks),
        "dynamic stack identities must match their value entries"
    );
    if user_frames.is_empty() {
        user_frames.push(BTreeMap::new());
    }
    if hidden_frames.is_empty() {
        hidden_frames.push(BTreeMap::new());
    }
    for mutation in mutations {
        match mutation {
            DynamicMutation::UserSet(key, value) => {
                user_frames
                    .last_mut()
                    .expect("user context is normalized above")
                    .insert(key.clone(), value.clone());
            }
            DynamicMutation::UserRemove(key) => {
                for frame in user_frames.iter_mut().rev() {
                    frame.remove(key);
                }
            }
            DynamicMutation::UserClear => {
                user_frames.clear();
                user_frames.push(BTreeMap::new());
            }
            DynamicMutation::HiddenSet(key, value) => {
                hidden_frames
                    .last_mut()
                    .expect("hidden context is normalized above")
                    .insert(key.clone(), value.clone());
            }
            DynamicMutation::StackPush {
                key,
                value,
                identity,
                ..
            } => {
                stacks.entry(key.clone()).or_default().push(value.clone());
                stack_identities.push_exact(key.clone(), identity.clone());
            }
            DynamicMutation::StackPop { key, expected } => {
                if stack_identities.pop_exact(key, expected) {
                    let stack = stacks
                        .get_mut(key)
                        .expect("stack identity must have a matching value stack");
                    stack
                        .pop()
                        .expect("stack identity must have a matching value entry");
                    if stack.is_empty() {
                        stacks.remove(key);
                    }
                }
            }
        }
    }
}

fn trace_mutation(mutation: &DynamicMutation, sink: &mut dyn FnMut(GcEdge<'_>)) {
    match mutation {
        DynamicMutation::UserSet(key, value)
        | DynamicMutation::HiddenSet(key, value)
        | DynamicMutation::StackPush { key, value, .. } => {
            sink(GcEdge::Value(key));
            sink(GcEdge::Value(value));
        }
        DynamicMutation::UserRemove(key) => {
            sink(GcEdge::Value(key));
        }
        DynamicMutation::StackPop { key, .. } => {
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

    use super::{
        apply_dynamic_mutations, DynamicMutation, DynamicStackIdentities, DynamicTaskState,
        ModuleTaskState,
    };

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
    fn module_child_strips_parent_scope_tokens() {
        let parent = Rc::new(ModuleTaskState::default());
        let file = parent
            .push_current_file(PathBuf::from("parent.sema"))
            .expect("scope ID available");
        let loading = parent
            .push_loading(PathBuf::from("loading.sema"))
            .expect("scope ID available");
        let exports = parent
            .push_exports(Some(vec!["parent-export".to_owned()]))
            .expect("scope ID available");
        let mut context = TaskContext::default();
        context.insert(Rc::clone(&parent));

        let child = context
            .inherit_for_child()
            .get_rc::<ModuleTaskState>()
            .expect("module state inherited");

        assert!(!child.remove_current_file(file));
        assert!(!child.remove_loading(loading));
        assert!(!child.remove_exports(exports));
        assert_eq!(child.current_file(), Some(PathBuf::from("parent.sema")));
        assert_eq!(child.loading(), vec![PathBuf::from("loading.sema")]);
        assert_eq!(
            child.exports(),
            vec![Some(vec!["parent-export".to_owned()])]
        );
        let child_scope = child
            .push_current_file(PathBuf::from("child.sema"))
            .expect("scope ID available");
        assert!(child_scope > exports);
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

        state
            .push_user_frame(BTreeMap::new())
            .expect("scope ID available");
        state
            .push_hidden_frame(BTreeMap::new())
            .expect("scope ID available");
        assert!(state.pop_user_frame());
        assert!(state.pop_hidden_frame());
        assert!(!state.pop_user_frame());
        assert!(!state.pop_hidden_frame());
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
            ])
        );
        assert_eq!(state.drain_mutations(), Some(Vec::new()));
    }

    #[test]
    fn inherited_stack_pop_cannot_remove_another_roots_equal_later_push() {
        let key = Value::keyword("stack");
        let initial = BTreeMap::from([(key.clone(), vec![Value::keyword("a")])]);
        let mut identities = DynamicStackIdentities::from_stacks(&initial);
        let first = DynamicTaskState::root_with_stack_identities(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            initial.clone(),
            &identities,
        );
        let second = DynamicTaskState::root_with_stack_identities(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            initial.clone(),
            &identities,
        );

        first
            .stack_push(key.clone(), Value::keyword("a"))
            .expect("scope ID available");
        assert_eq!(second.stack_pop(&key), Some(Value::keyword("a")));

        let mut user = vec![BTreeMap::new()];
        let mut hidden = vec![BTreeMap::new()];
        let mut published = initial;
        apply_dynamic_mutations(
            &mut user,
            &mut hidden,
            &mut published,
            &mut identities,
            &first.drain_mutations().expect("root journal"),
        );
        apply_dynamic_mutations(
            &mut user,
            &mut hidden,
            &mut published,
            &mut identities,
            &second.drain_mutations().expect("root journal"),
        );

        assert_eq!(
            published.get(&key),
            Some(&vec![Value::keyword("a"), Value::keyword("a")])
        );
    }

    #[test]
    fn stale_pop_cannot_remove_an_equal_entry_recreated_by_a_later_root() {
        let key = Value::keyword("stack");
        let value = Value::keyword("same");
        let initial = BTreeMap::from([(key.clone(), vec![value.clone()])]);
        let mut identities = DynamicStackIdentities::from_stacks(&initial);
        let stale = DynamicTaskState::root_with_stack_identities(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            initial.clone(),
            &identities,
        );
        let recreating = DynamicTaskState::root_with_stack_identities(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            initial.clone(),
            &identities,
        );

        assert_eq!(stale.stack_pop(&key), Some(value.clone()));
        assert_eq!(recreating.stack_pop(&key), Some(value.clone()));
        recreating
            .stack_push(key.clone(), value.clone())
            .expect("scope ID available");

        let mut user = vec![BTreeMap::new()];
        let mut hidden = vec![BTreeMap::new()];
        let mut published = initial;
        apply_dynamic_mutations(
            &mut user,
            &mut hidden,
            &mut published,
            &mut identities,
            &recreating.drain_mutations().expect("root journal"),
        );
        apply_dynamic_mutations(
            &mut user,
            &mut hidden,
            &mut published,
            &mut identities,
            &stale.drain_mutations().expect("root journal"),
        );

        assert_eq!(published.get(&key), Some(&vec![value]));
    }

    #[test]
    fn one_root_can_replay_multiple_inherited_pops_in_order() {
        let key = Value::keyword("stack");
        let initial = BTreeMap::from([(
            key.clone(),
            vec![Value::keyword("bottom"), Value::keyword("top")],
        )]);
        let mut identities = DynamicStackIdentities::from_stacks(&initial);
        let root = DynamicTaskState::root_with_stack_identities(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            initial.clone(),
            &identities,
        );

        assert_eq!(root.stack_pop(&key), Some(Value::keyword("top")));
        assert_eq!(root.stack_pop(&key), Some(Value::keyword("bottom")));

        let mut user = vec![BTreeMap::new()];
        let mut hidden = vec![BTreeMap::new()];
        let mut published = initial;
        apply_dynamic_mutations(
            &mut user,
            &mut hidden,
            &mut published,
            &mut identities,
            &root.drain_mutations().expect("root journal"),
        );

        assert!(!published.contains_key(&key));
    }

    #[test]
    fn owned_stack_cleanup_cancels_only_its_exact_push_with_duplicate_values() {
        let key = Value::keyword("stack");
        let state = DynamicTaskState::root(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            BTreeMap::new(),
        );
        let first = state
            .stack_push(key.clone(), Value::keyword("same"))
            .expect("scope ID available");
        let second = state
            .stack_push(key.clone(), Value::keyword("same"))
            .expect("scope ID available");

        assert_eq!(
            state.remove_stack_value(&key, first),
            Some(Value::keyword("same"))
        );
        let mutations = state.drain_mutations().expect("root journal");
        assert!(matches!(
            mutations.as_slice(),
            [DynamicMutation::StackPush {
                key: mutation_key,
                value,
                scope,
                ..
            }] if mutation_key == &key && value == &Value::keyword("same") && *scope == second
        ));

        let mut user = vec![BTreeMap::new()];
        let mut hidden = vec![BTreeMap::new()];
        let mut published = BTreeMap::new();
        let mut identities = DynamicStackIdentities::default();
        apply_dynamic_mutations(
            &mut user,
            &mut hidden,
            &mut published,
            &mut identities,
            &mutations,
        );
        assert_eq!(published.get(&key), Some(&vec![Value::keyword("same")]));
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
    fn inherited_top_frames_record_user_and_hidden_sets_for_publication() {
        let user_key = Value::keyword("user");
        let hidden_key = Value::keyword("hidden");
        let state = DynamicTaskState::root(
            vec![
                BTreeMap::from([(user_key.clone(), Value::int(1))]),
                BTreeMap::new(),
            ],
            vec![
                BTreeMap::from([(hidden_key.clone(), Value::int(2))]),
                BTreeMap::new(),
            ],
            BTreeMap::new(),
        );

        state.user_set(user_key.clone(), Value::int(10));
        state.hidden_set(hidden_key.clone(), Value::int(20));

        assert_eq!(
            state.drain_mutations(),
            Some(vec![
                DynamicMutation::UserSet(user_key, Value::int(10)),
                DynamicMutation::HiddenSet(hidden_key, Value::int(20)),
            ])
        );
    }

    #[test]
    fn removing_a_key_from_any_inherited_frame_records_publication() {
        let key = Value::keyword("removed");
        let state = DynamicTaskState::root(
            vec![
                BTreeMap::new(),
                BTreeMap::from([(key.clone(), Value::int(1))]),
            ],
            vec![BTreeMap::new()],
            BTreeMap::new(),
        );

        assert_eq!(state.user_remove(&key), Some(Value::int(1)));
        assert_eq!(
            state.drain_mutations(),
            Some(vec![DynamicMutation::UserRemove(key)])
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
    fn dynamic_child_strips_parent_scope_tokens() {
        let parent = Rc::new(DynamicTaskState::root(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            BTreeMap::new(),
        ));
        let user = parent
            .push_user_frame(BTreeMap::from([(Value::keyword("user"), Value::int(1))]))
            .expect("scope ID available");
        let hidden = parent
            .push_hidden_frame(BTreeMap::from([(Value::keyword("hidden"), Value::int(2))]))
            .expect("scope ID available");
        let stack_key = Value::keyword("stack");
        let stack = parent
            .stack_push(stack_key.clone(), Value::int(3))
            .expect("scope ID available");
        let mut context = TaskContext::default();
        context.insert(Rc::clone(&parent));

        let child = context
            .inherit_for_child()
            .get_rc::<DynamicTaskState>()
            .expect("dynamic state inherited");

        assert!(!child.remove_user_frame(user));
        assert!(!child.remove_hidden_frame(hidden));
        assert_eq!(child.remove_stack_value(&stack_key, stack), None);
        assert_eq!(child.user_get(&Value::keyword("user")), Some(Value::int(1)));
        assert_eq!(
            child.hidden_get(&Value::keyword("hidden")),
            Some(Value::int(2))
        );
        assert_eq!(child.stack_get(&stack_key), vec![Value::int(3)]);
        let child_scope = child
            .push_user_frame(BTreeMap::new())
            .expect("scope ID available");
        assert!(child_scope > stack);
    }

    #[test]
    fn dynamic_child_preserves_inherited_stack_entry_identity() {
        let key = Value::keyword("stack");
        let parent = Rc::new(DynamicTaskState::root(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            BTreeMap::from([(key.clone(), vec![Value::keyword("value")])]),
        ));
        let mut context = TaskContext::default();
        context.insert(Rc::clone(&parent));

        let child = context
            .inherit_for_child()
            .get_rc::<DynamicTaskState>()
            .expect("dynamic state inherited");
        let parent_identity = parent.inner.borrow().stacks[&key][0].identity.clone();
        let child_entry = child.inner.borrow().stacks[&key][0].clone();

        assert_eq!(child_entry.identity, parent_identity);
        assert_eq!(child_entry.scope, None);
    }

    #[test]
    fn conditional_stack_pop_traces_its_key() {
        let key = Value::keyword("stack");
        let state = DynamicTaskState::root(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            BTreeMap::from([(key.clone(), vec![Value::int(1)])]),
        );
        assert_eq!(state.stack_pop(&key), Some(Value::int(1)));

        let mut edges = 0;
        assert!(state.trace(&mut |edge| {
            assert!(matches!(edge, GcEdge::Value(_)));
            edges += 1;
        }));
        assert_eq!(edges, 1);
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
