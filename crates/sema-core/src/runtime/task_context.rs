pub struct TaskContext {
    _private: (),
}

impl TaskContext {
    #[doc(hidden)]
    pub fn empty() -> Self {
        Self { _private: () }
    }
}

impl Default for TaskContext {
    fn default() -> Self {
        Self::empty()
    }
}
