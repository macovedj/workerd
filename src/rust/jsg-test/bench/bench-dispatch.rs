// Copyright (c) 2026 Cloudflare, Inc.
// Licensed under the Apache 2.0 license found in the LICENSE file or at:
//     https://opensource.org/licenses/Apache-2.0

//! Slow vs. fast callback dispatch to a Rust resource. Rust counterpart of
//! `src/workerd/jsg/bench-dispatch.c++`.

use std::cell::Cell;
use std::time::Instant;

use jsg::Number;
use jsg::ToJS;
use jsg_macros::jsg_method;
use jsg_macros::jsg_resource;

#[jsg_resource]
struct DispatchTarget {
    value: Cell<u32>,
}

#[jsg_resource]
impl DispatchTarget {
    #[jsg_method]
    pub fn call(&self) -> Number {
        self.value.set(self.value.get().wrapping_add(1));
        Number::from(self.value.get())
    }

    #[jsg_method]
    pub fn call_with(&self, increment: u32) -> Number {
        self.value.set(self.value.get().wrapping_add(increment));
        Number::from(self.value.get())
    }
}

const CALLS_PER_BATCH: u32 = 1_000_000;
const BATCHES: usize = 25;

/// Median nanoseconds per call of `call` from an optimized loop.
fn run(fast_api_enabled: bool, call: &str) -> f64 {
    let harness = if fast_api_enabled {
        jsg_test::Harness::with_fast_api()
    } else {
        jsg_test::Harness::new()
    };
    let mut per_call = 0.0;
    harness.run_in_context(|lock, ctx| {
        let target = jsg::Rc::new(DispatchTarget {
            value: Cell::new(0),
        });
        let wrapped = target.to_js(lock);
        ctx.set_global("target", wrapped);
        ctx.eval::<Number>(
            lock,
            &format!(
                "function dispatchBatch(count) {{
                   let result;
                   for (let i = 0; i < count; ++i) {{
                     result = target.{call};
                   }}
                   return result;
                 }}
                 %PrepareFunctionForOptimization(dispatchBatch);
                 dispatchBatch(10000);
                 %OptimizeFunctionOnNextCall(dispatchBatch);
                 dispatchBatch(10000)"
            ),
        )
        .expect("warmup failed");

        let calls_before = jsg::fast_api::call_count();
        let batch = format!("dispatchBatch({CALLS_PER_BATCH})");
        let mut samples = Vec::with_capacity(BATCHES);
        for _ in 0..BATCHES {
            let start = Instant::now();
            ctx.eval::<Number>(lock, &batch).expect("batch failed");
            samples.push(start.elapsed().as_secs_f64() * 1e9 / f64::from(CALLS_PER_BATCH));
        }
        samples.sort_by(f64::total_cmp);
        per_call = samples[BATCHES / 2];

        let took_fast_path = jsg::fast_api::call_count() > calls_before;
        assert_eq!(
            took_fast_path, fast_api_enabled,
            "{call}: fast path taken = {took_fast_path}"
        );
        Ok(())
    });
    per_call
}

fn main() {
    println!(
        "{:<24} {:>12} {:>12} {:>9}",
        "benchmark", "slow", "fast", "speedup"
    );
    for (name, call) in [
        ("RustDispatch", "call()"),
        ("RustDispatchArg", "callWith(1)"),
    ] {
        let slow = run(false, call);
        let fast = run(true, call);
        println!(
            "{name:<24} {slow:>9.2} ns {fast:>9.2} ns {:>8.2}x",
            slow / fast
        );
    }
}
