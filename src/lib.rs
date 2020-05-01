#[allow(dead_code)]
#[cfg(not(feature = "loom"))]
mod loom {
    pub use std::sync;

    pub mod cell {
        pub struct UnsafeCell<T>(std::cell::UnsafeCell<T>);

        impl<T> UnsafeCell<T> {
            #[inline]
            pub fn new(t: T) -> UnsafeCell<T> {
                UnsafeCell(std::cell::UnsafeCell::new(t))
            }

            #[inline]
            pub fn with<F, R>(&self, f: F) -> R
            where F: FnOnce(*const T) -> R
            {
                f(self.0.get())
            }

            #[inline]
            pub fn with_mut<F, R>(&self, f: F) -> R
            where F: FnOnce(*mut T) -> R
            {
                f(self.0.get())
            }
        }
    }
}


use std::{ mem, ptr };
use std::pin::Pin;
use std::future::Future;
use std::task::{ Context, Waker, Poll };
use loom::sync::atomic;
use loom::cell::UnsafeCell;


pub struct Sender<T>(ptr::NonNull<Inner<T>>);
pub struct Receiver<T>(ptr::NonNull<Inner<T>>);

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

struct Inner<T> {
    waker: UnsafeCell<mem::MaybeUninit<Waker>>,
    value: UnsafeCell<mem::MaybeUninit<T>>,
    state: atomic::AtomicU8
}

const UNINIT:  u8 = 0;
const LOCKED:  u8 = 1;
const WAITING: u8 = 2;
const READY:   u8 = 3;

const SEND_DEAD: u8 = 4;
const RECV_DEAD: u8 = 5;
const END:       u8 = 6;


pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Box::new(Inner {
        waker: UnsafeCell::new(mem::MaybeUninit::uninit()),
        value: UnsafeCell::new(mem::MaybeUninit::uninit()),
        state: atomic::AtomicU8::new(UNINIT)
    });

    let raw_ptr = Box::into_raw(inner);
    let raw_ptr = unsafe {
        ptr::NonNull::new_unchecked(raw_ptr)
    };

    (Sender(raw_ptr), Receiver(raw_ptr))
}

impl<T> Sender<T> {
    pub fn send(self, entry: T) -> Result<(), T> {
        let this = unsafe { self.0.as_ref() };

        loop {
            match this.state.swap(LOCKED, atomic::Ordering::SeqCst) {
                RECV_DEAD => unsafe {
                    Box::from_raw(self.0.as_ptr());
                    mem::forget(self);
                    return Err(entry);
                },
                UNINIT => unsafe {
                    this.value.with_mut(|ptr| (&mut *ptr).as_mut_ptr()).write(entry);
                    this.state.store(READY, atomic::Ordering::SeqCst);
                },
                LOCKED => {
                    atomic::spin_loop_hint();
                    continue
                },
                WAITING => unsafe {
                    this.waker.with_mut(|ptr| mem::replace(&mut *ptr, mem::MaybeUninit::uninit()))
                        .assume_init()
                        .wake();

                    this.value.with_mut(|ptr| (&mut *ptr).as_mut_ptr()).write(entry);
                    this.state.store(READY, atomic::Ordering::SeqCst);
                },
                _ => unreachable!()
            }

            break
        }

        mem::forget(self);

        Ok(())
    }
}

impl<T> Future for Receiver<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.0.as_ref() };

        loop {
            match this.state.swap(LOCKED, atomic::Ordering::SeqCst) {
                SEND_DEAD | END => {
                    this.state.store(END, atomic::Ordering::SeqCst);
                    return Poll::Ready(None);
                },
                UNINIT => {
                    let waker = cx.waker().clone();

                    unsafe {
                        this.waker.with_mut(|ptr| (&mut *ptr).as_mut_ptr()).write(waker);
                    }

                    this.state.store(WAITING, atomic::Ordering::SeqCst);
                },
                LOCKED => {
                    atomic::spin_loop_hint();
                    continue
                },
                WAITING => {
                    let waker_cx = cx.waker();
                    let waker_ref = unsafe {
                        &mut *this.waker.with_mut(|ptr| (&mut *ptr).as_mut_ptr())
                    };

                    if !waker_ref.will_wake(waker_cx) {
                        let _ = mem::replace(waker_ref, waker_cx.clone());
                    }

                    this.state.store(WAITING, atomic::Ordering::SeqCst);
                },
                READY => {
                    let value = unsafe {
                        this.value.with_mut(|ptr| mem::replace(&mut *ptr, mem::MaybeUninit::uninit()))
                            .assume_init()
                    };

                    this.state.store(END, atomic::Ordering::SeqCst);

                    return Poll::Ready(Some(value));
                },
                _ => unreachable!()
            }

            break
        }

        Poll::Pending
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let this = unsafe { self.0.as_ref() };

        match this.state.swap(SEND_DEAD, atomic::Ordering::SeqCst) {
            RECV_DEAD => unsafe {
                Box::from_raw(self.0.as_ptr());
            },
            UNINIT | LOCKED => (),
            WAITING => unsafe {
                this.waker.with_mut(|ptr| mem::replace(&mut *ptr, mem::MaybeUninit::uninit()))
                    .assume_init()
                    .wake();
            },
            _ => unreachable!()
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let this = unsafe { self.0.as_ref() };

        match this.state.swap(RECV_DEAD, atomic::Ordering::SeqCst) {
            SEND_DEAD | END => unsafe {
                Box::from_raw(self.0.as_ptr());
            },
            UNINIT | LOCKED => (),
            WAITING => unsafe {
                this.waker.with_mut(|ptr| mem::replace(&mut *ptr, mem::MaybeUninit::uninit()))
                    .assume_init();
            },
            READY => unsafe {
                this.value.with_mut(|ptr| mem::replace(&mut *ptr, mem::MaybeUninit::uninit()))
                    .assume_init();

                Box::from_raw(self.0.as_ptr());
            },
            _ => unreachable!()
        }
    }
}
