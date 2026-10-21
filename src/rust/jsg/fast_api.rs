// Copyright (c) 2026 Cloudflare, Inc.
// Licensed under the Apache 2.0 license found in the LICENSE file or at:
//     https://opensource.org/licenses/Apache-2.0

//! V8 Fast API callbacks for `#[jsg_method]`. See the README.

#![allow(
    unsafe_code,
    reason = "fast callbacks receive raw V8 pointers and resolve resources through them"
)]

use std::panic::AssertUnwindSafe;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use crate::Number;
use crate::Resource;
use crate::v8::ffi;
pub use crate::v8::ffi::FastApiType;

mod private {
    pub trait Sealed {}
}

/// An argument type a fast callback can take directly from V8, as the C type `Raw`.
pub trait FastApiArg: private::Sealed {
    type Raw: Copy;
    const TYPE: FastApiType;
    fn from_raw(raw: Self::Raw) -> Self;
}

/// A return type a fast callback can hand directly back to V8, as the C type `Raw`.
pub trait FastApiReturn: private::Sealed {
    type Raw: Default;
    const TYPE: FastApiType;
    fn into_raw(self) -> Self::Raw;
}

macro_rules! impl_fast_api_arg {
    ($($ty:ty => $variant:ident),* $(,)?) => {
        $(
            impl private::Sealed for $ty {}
            impl FastApiArg for $ty {
                type Raw = Self;
                const TYPE: FastApiType = FastApiType::$variant;
                fn from_raw(raw: Self) -> Self {
                    raw
                }
            }
        )*
    };
}

impl_fast_api_arg!(bool => Bool, i32 => Int32, u32 => Uint32);

impl FastApiReturn for bool {
    type Raw = Self;
    const TYPE: FastApiType = FastApiType::Bool;
    fn into_raw(self) -> Self {
        self
    }
}

impl private::Sealed for Number {}
impl FastApiArg for Number {
    type Raw = f64;
    const TYPE: FastApiType = FastApiType::Float64;
    fn from_raw(raw: f64) -> Self {
        Self::new(raw)
    }
}

impl FastApiReturn for Number {
    type Raw = f64;
    const TYPE: FastApiType = FastApiType::Float64;
    fn into_raw(self) -> f64 {
        self.value()
    }
}

impl private::Sealed for () {}
impl FastApiReturn for () {
    type Raw = Self;
    const TYPE: FastApiType = FastApiType::Void;
    fn into_raw(self) {}
}

/// Runs a `&self` method from its generated fast callback.
///
/// # Safety
/// `options` and `receiver` must be the ones V8 passed to the fast callback.
#[doc(hidden)]
pub unsafe fn method<R: Resource, T: FastApiReturn>(
    options: *mut ffi::FastApiCallbackOptions,
    receiver: ffi::Local,
    f: impl FnOnce(&R) -> T,
) -> T::Raw {
    // SAFETY: `options` and `receiver` come from V8 (caller contract).
    let trait_ptr = unsafe { ffi::fast_api_unwrap_receiver(options, receiver) };
    let Some(resource) = trait_ptr.resolve::<R>() else {
        // SAFETY: as above.
        unsafe { ffi::fast_api_throw_illegal_invocation(options) };
        return T::Raw::default();
    };
    // SAFETY: The receiver keeps the resource alive for the call. A fast callback cannot run
    // JavaScript, so nothing else can reach the resource while this borrow lives.
    let resource = unsafe { resource.as_ref() };
    // SAFETY: as above.
    unsafe { run(options, || f(resource)) }
}

/// Runs a static method from its generated fast callback.
///
/// # Safety
/// `options` must be the one V8 passed to the fast callback.
#[doc(hidden)]
pub unsafe fn static_method<T: FastApiReturn>(
    options: *mut ffi::FastApiCallbackOptions,
    f: impl FnOnce() -> T,
) -> T::Raw {
    // SAFETY: `options` comes from V8 (caller contract).
    unsafe { run(options, f) }
}

/// Mirrors [`crate::catch_panic`]: a panic becomes an internal error and terminates execution.
unsafe fn run<T: FastApiReturn>(
    options: *mut ffi::FastApiCallbackOptions,
    f: impl FnOnce() -> T,
) -> T::Raw {
    record_call();
    // AssertUnwindSafe: nothing observes the closure's state after a panic.
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value.into_raw(),
        Err(payload) => {
            // SAFETY: `options` comes from V8 (caller contract).
            unsafe { ffi::fast_api_throw_internal_error(options, crate::panic_message(&*payload)) };
            T::Raw::default()
        }
    }
}

static COUNT_CALLS: AtomicBool = AtomicBool::new(false);
static CALL_COUNT: AtomicU64 = AtomicU64::new(0);

#[inline]
fn record_call() {
    if COUNT_CALLS.load(Ordering::Relaxed) {
        CALL_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

/// Fast callbacks run since the first call to this function. For tests.
#[doc(hidden)]
pub fn call_count() -> u64 {
    COUNT_CALLS.store(true, Ordering::Relaxed);
    CALL_COUNT.load(Ordering::Relaxed)
}
