#[cfg(not(feature = "loom"))]
mod loom {
    pub use std::thread;
    pub use std::sync;

    pub fn model<F>(f: F)
    where
        F: Fn() + Sync + Send + 'static
    {
        f()
    }
}

use std::rc::Rc;
use std::pin::Pin;
use std::cell::RefCell;
use std::future::Future;
use std::collections::VecDeque;
use std::task::{ Context, Waker, RawWaker, Poll };
use futures_oneshot as oneshot;

struct Runtime {
    queue: VecDeque<BoxFuture>,
    incoming: Rc<Incoming>
}

struct Spawner {
    incoming: Rc<Incoming>
}

type BoxFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;
type Incoming = RefCell<Vec<BoxFuture>>;

fn dummy_waker() -> RawWaker {
    use std::task::RawWakerVTable;

    unsafe fn clone(_: *const ()) -> RawWaker {
        dummy_waker()
    }

    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop(_: *const ()) {}

    const VTABLE: &RawWakerVTable = &RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    RawWaker::new(std::ptr::null(), VTABLE)
}

impl Runtime {
    pub fn new() -> Runtime {
        Runtime {
            queue: VecDeque::new(),
            incoming: Rc::new(RefCell::new(Vec::new()))
        }
    }

    pub fn spawner(&self) -> Spawner {
        Spawner { incoming: self.incoming.clone() }
    }

    pub fn block_on<F: Future>(&mut self, fut: F) -> F::Output {
        let mut fut = Box::pin(fut);
        let waker = unsafe { Waker::from_raw(dummy_waker()) };

        loop {
            let mut cx = Context::from_waker(&waker);

            if let Poll::Ready(output) = fut.as_mut().poll(&mut cx) {
                return output;
            }

            {
                let mut incoming = self.incoming.borrow_mut();
                for task in incoming.drain(..) {
                    self.queue.push_back(task);
                }
            }

            for task in self.queue.drain(..) {
                let mut task = task;
                if let Poll::Pending = task.as_mut().poll(&mut cx) {
                    self.incoming.borrow_mut()
                        .push(task);
                }
            }

            loom::sync::atomic::spin_loop_hint();
        }
    }
}

impl Spawner {
    pub fn spawn(&self, fut: impl Future<Output = ()> + 'static) {
        self.incoming.borrow_mut().push(Box::pin(fut));
    }
}


#[test]
fn test_local_oneshot_ok() {
    loom::model(|| {
        let mut runtime = Runtime::new();
        let spawner = runtime.spawner();
        let (tx, rx) = oneshot::channel::<Box<usize>>();

        spawner.spawn(async move {
            let ret = tx.send(Box::new(0x42));
            assert_eq!(Ok(()), ret);
        });

        runtime.block_on(async move {
            let ret = rx.await;
            assert_eq!(Some(&0x42), ret.as_deref());
        });
    });
}

#[test]
fn test_local_oneshot_drop_tx() {
    loom::model(|| {
        let mut runtime = Runtime::new();
        let (tx, rx) = oneshot::channel::<Box<usize>>();

        drop(tx);

        runtime.block_on(async move {
            let ret = rx.await;
            assert_eq!(None, ret);
        });
    })
}

#[test]
fn test_local_oneshot_drop_rx() {
    loom::model(|| {
        let mut runtime = Runtime::new();
        let (tx, rx) = oneshot::channel::<Box<usize>>();

        drop(rx);

        runtime.block_on(async move {
            let ret = tx.send(Box::new(0x42));
            assert_eq!(Some(&0x42), ret.err().as_deref());
        });
    });
}

#[test]
fn test_loom_threaded_ok() {
    use loom::thread;

    loom::model(|| {
        let mut runtime = Runtime::new();
        let (tx, rx) = oneshot::channel::<Box<usize>>();

        thread::spawn(move || {
            let ret = tx.send(Box::new(0x42));
            assert_eq!(Ok(()), ret);
        });

        runtime.block_on(async move {
            let ret = rx.await;
            assert_eq!(Some(&0x42), ret.as_deref());
        });
    });
}

#[test]
fn test_loom_threaded_drop_tx() {
    use loom::thread;

    loom::model(|| {
        let mut runtime = Runtime::new();
        let (tx, rx) = oneshot::channel::<Box<usize>>();

        thread::spawn(move || {
            drop(tx);
        });

        runtime.block_on(async move {
            let ret = rx.await;
            assert_eq!(None, ret);
        });
    });
}

#[test]
fn test_loom_threaded_drop_rx() {
    use loom::thread;

    loom::model(|| {
        let (tx, rx) = oneshot::channel::<Box<usize>>();

        thread::spawn(move || {
            drop(rx);
        });

        let _ = tx.send(Box::new(0x42));
    });
}

#[test]
fn test_loom_threaded_poll_drop_rx() {
    use loom::thread;

    loom::model(|| {
        let (tx, rx) = oneshot::channel::<Box<usize>>();

        thread::spawn(move || {
            let waker = unsafe { Waker::from_raw(dummy_waker()) };
            let mut cx = Context::from_waker(&waker);

            let mut fut = Box::pin(rx);

            let _ = fut.as_mut().poll(&mut cx);

            drop(fut);
        });

        let _ = tx.send(Box::new(0x42));
    });
}

#[test]
fn test_loom_threaded_drop_tx_rx() {
    use loom::thread;

    loom::model(|| {
        let (tx, rx) = oneshot::channel::<Box<usize>>();

        thread::spawn(move || {
            drop(rx);
        });

        drop(tx);
    });
}
