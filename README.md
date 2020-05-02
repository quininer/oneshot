# M(mlti)-unsafe lockless asynchronous oneshot channel

A lightweight lockless asynchronous oneshot channel.

This crate is similar to tokio's oneshot channel,
but it removes the `Sender::poll_closed` method
and merges reference count into inner state (like https://github.com/tokio-rs/tokio/pull/1710).

## Tests

* [x] `asan` + `tsan`
* [x] loom
* [ ] miri
  + TODO https://github.com/rust-lang/miri/issues/1038
