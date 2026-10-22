//! Host-function helpers for embedders: the context passed to
//! [`Interpreter::register_async_fn_cx`](crate::Interpreter::register_async_fn_cx).

use std::future::Future;
use std::sync::Arc;

use sema_core::runtime::{NativeOutcome, NativeResult};
use sema_core::{SemaError, Value};

pub use sema_io::host_async::CancellationToken;

/// Context for one call of a function registered with
/// [`register_async_fn_cx`](crate::Interpreter::register_async_fn_cx).
///
/// It lives on the interpreter thread for the duration of the closure.
pub struct AsyncCx {
    name: Arc<str>,
    token: Option<CancellationToken>,
}

impl AsyncCx {
    pub(crate) fn new(name: Arc<str>) -> Self {
        Self { name, token: None }
    }

    /// The registered function name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// A token that fires when this call's [`await_future`](Self::await_future)
    /// is cancelled (`async/cancel`, `async/with-timeout`, interpreter
    /// shutdown). Clone it into spawned tasks so they stop with the caller.
    /// It never fires if the call returns a value without awaiting.
    pub fn cancellation_token(&mut self) -> CancellationToken {
        self.token
            .get_or_insert_with(CancellationToken::new)
            .clone()
    }

    /// Spawn a fire-and-forget task on Sema's I/O pool. `make` runs on the
    /// pool, so it may construct Tokio timers and I/O resources. The task is
    /// not cancelled when the call ends; pass it a
    /// [`cancellation_token`](Self::cancellation_token) for that.
    pub fn spawn<F, Fut>(&self, make: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let _abort = sema_io::io_spawn(async move { make().await });
    }

    /// Suspend the calling Sema task until the future built by `make`
    /// resolves, then convert its output with `finish` on the interpreter
    /// thread. `make` runs on Sema's I/O pool, so Tokio resources it creates
    /// are in runtime context. A host error is raised as
    /// `SemaError::Io("<name>: <text>")`, exactly as for
    /// [`register_async_fn`](crate::Interpreter::register_async_fn).
    pub fn await_future<Output, Error, F, Fut, Finish>(
        &mut self,
        make: F,
        finish: Finish,
    ) -> HostReturn
    where
        Output: Send + 'static,
        Error: std::error::Error + Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<Output, Error>> + Send + 'static,
        Finish: FnOnce(Output) -> Result<Value, SemaError> + 'static,
    {
        let name = Arc::clone(&self.name);
        let token = self.cancellation_token();
        HostReturn(Inner::Pending(Box::new(move || {
            sema_io::host_async::suspend_host_job(&name, token, make, Box::new(finish))
        })))
    }
}

/// What a [`register_async_fn_cx`](crate::Interpreter::register_async_fn_cx)
/// closure returns: a value now (`value.into()`), or a pending future from
/// [`AsyncCx::await_future`].
pub struct HostReturn(Inner);

enum Inner {
    Ready(Value),
    Pending(Box<dyn FnOnce() -> NativeResult>),
}

impl From<Value> for HostReturn {
    fn from(value: Value) -> Self {
        Self(Inner::Ready(value))
    }
}

impl HostReturn {
    /// Runtime ABI: return the value, or suspend on the pending future.
    pub(crate) fn into_native(self) -> NativeResult {
        match self.0 {
            Inner::Ready(value) => Ok(NativeOutcome::Return(value)),
            Inner::Pending(suspend) => suspend(),
        }
    }

    /// Value ABI, reached where the caller cannot suspend: a ready value is
    /// returned; a pending future is an error.
    pub(crate) fn into_value(self, name: &str) -> Result<Value, SemaError> {
        match self.0 {
            Inner::Ready(value) => Ok(value),
            Inner::Pending(_) => Err(SemaError::eval(format!(
                "{name}: async host function cannot run here: the caller cannot \
                 suspend (macro expansion, a synchronous apply, or debug evaluation)"
            ))),
        }
    }
}

// ── Typed registration ──────────────────────────────────────────

use sema_core::{FromValue, IntoValue};

fn check_arity(name: &str, expected: usize, args: &[Value]) -> Result<(), SemaError> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(SemaError::arity(name, expected.to_string(), args.len()))
    }
}

fn arg<T: FromValue>(name: &str, args: &[Value], index: usize) -> Result<T, SemaError> {
    T::from_value(&args[index])
        .map_err(|error| error.with_note(format!("argument {} of {name}", index + 1)))
}

/// A Rust closure usable with
/// [`register_fn_typed`](crate::Interpreter::register_fn_typed): any
/// `Fn(A1, …, An) -> Result<R, SemaError>` with `Ai: FromValue` and
/// `R: IntoValue`, for up to six arguments.
pub trait TypedFn<Args>: 'static {
    #[doc(hidden)]
    fn invoke(&self, name: &str, args: &[Value]) -> Result<Value, SemaError>;
}

/// A Rust closure usable with
/// [`register_async_fn_typed`](crate::Interpreter::register_async_fn_typed):
/// any `Fn(A1, …, An) -> Fut` with `Ai: FromValue + Send` and
/// `Fut: Future<Output = Result<R, E>> + Send`, `R: IntoValue + Send`,
/// `E: Error + Send`, for up to six arguments.
pub trait AsyncTypedFn<Args>: Send + Sync + 'static {
    #[doc(hidden)]
    type Output: IntoValue + Send + 'static;
    #[doc(hidden)]
    type Error: std::error::Error + Send + 'static;
    #[doc(hidden)]
    type Fut: Future<Output = Result<Self::Output, Self::Error>> + Send + 'static;
    #[doc(hidden)]
    fn extract(name: &str, args: &[Value]) -> Result<Args, SemaError>;
    #[doc(hidden)]
    fn call(&self, args: Args) -> Self::Fut;
}

macro_rules! typed_fns {
    ($arity:expr; $($A:ident $i:tt),*) => {
        impl<F, R, $($A,)*> TypedFn<($($A,)*)> for F
        where
            F: Fn($($A),*) -> Result<R, SemaError> + 'static,
            R: IntoValue,
            $($A: FromValue,)*
        {
            #[allow(unused_variables)]
            fn invoke(&self, name: &str, args: &[Value]) -> Result<Value, SemaError> {
                check_arity(name, $arity, args)?;
                Ok((self)($(arg::<$A>(name, args, $i)?),*)?.into_value())
            }
        }

        impl<F, Fut, R, E, $($A,)*> AsyncTypedFn<($($A,)*)> for F
        where
            F: Fn($($A),*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Result<R, E>> + Send + 'static,
            R: IntoValue + Send + 'static,
            E: std::error::Error + Send + 'static,
            $($A: FromValue + Send + 'static,)*
        {
            type Output = R;
            type Error = E;
            type Fut = Fut;

            #[allow(unused_variables)]
            fn extract(name: &str, args: &[Value]) -> Result<($($A,)*), SemaError> {
                check_arity(name, $arity, args)?;
                Ok(($(arg::<$A>(name, args, $i)?,)*))
            }

            #[allow(non_snake_case)]
            fn call(&self, args: ($($A,)*)) -> Fut {
                let ($($A,)*) = args;
                (self)($($A),*)
            }
        }
    };
}

typed_fns!(0;);
typed_fns!(1; A0 0);
typed_fns!(2; A0 0, A1 1);
typed_fns!(3; A0 0, A1 1, A2 2);
typed_fns!(4; A0 0, A1 1, A2 2, A3 3);
typed_fns!(5; A0 0, A1 1, A2 2, A3 3, A4 4);
typed_fns!(6; A0 0, A1 1, A2 2, A3 3, A4 4, A5 5);
