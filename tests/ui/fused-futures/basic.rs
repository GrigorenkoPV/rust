//@ run-pass
//@ edition: 2024

#![feature(fused_futures)]

use std::pin::pin;
use std::task::{Context, Poll, Waker};

async fn fused() -> &'static str {
    "done"
} do fuse {
    Poll::Ready("fused")
}

fn main() {
    let mut fut = pin!(fused());
    let cx = &mut Context::from_waker(Waker::noop());
    assert_eq!(fut.as_mut().poll(cx), Poll::Ready("done"));
    assert_eq!(fut.as_mut().poll(cx), Poll::Ready("fused"));
    assert_eq!(fut.as_mut().poll(cx), Poll::Ready("fused"));
    assert_eq!(fut.as_mut().poll(cx), Poll::Ready("fused"));
}
