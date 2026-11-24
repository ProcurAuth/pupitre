// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared helpers of the tests of the runner. Each test file uses a part of them.
#![allow(dead_code)]

pub mod fake_process;
pub mod fixture;
pub mod harness;
pub mod mem_store;

use std::future::Future;
use std::time::{Duration, Instant};

/// Wait until a condition holds, at most `seconds`. A timeout fails the test with the message.
pub async fn wait_for<F: Fn() -> bool>(what: &str, seconds: u64, condition: F) {
    let end = Instant::now() + Duration::from_secs(seconds);
    while !condition() {
        assert!(Instant::now() < end, "timeout while waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Wait for an async condition.
pub async fn wait_for_async<F, Fut>(what: &str, seconds: u64, condition: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let end = Instant::now() + Duration::from_secs(seconds);
    while !condition().await {
        assert!(Instant::now() < end, "timeout while waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
