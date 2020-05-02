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
use loom::sync::atomic::{ AtomicU8, Ordering };
use loom::cell::UnsafeCell;


pub struct Sender<T>(InlineRc<T>);
pub struct Receiver<T>(InlineRc<T>);

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

struct InlineRc<T>(ptr::NonNull<Inner<T>>);

struct Inner<T> {
    state: AtomicU8,
    waker: UnsafeCell<mem::MaybeUninit<Waker>>,
    value: UnsafeCell<mem::MaybeUninit<T>>,
}

const WAKER_READY: u8 = 0b00000001;
const VALUE_READY: u8 = 0b00000010;
const CLOSED:      u8 = 0b00000100;


pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Box::new(Inner {
        waker: UnsafeCell::new(mem::MaybeUninit::uninit()),
        value: UnsafeCell::new(mem::MaybeUninit::uninit()),
        state: AtomicU8::new(0)
    });

    let raw_ptr = Box::into_raw(inner);
    let raw_ptr = unsafe {
        ptr::NonNull::new_unchecked(raw_ptr)
    };

    (Sender(InlineRc(raw_ptr)), Receiver(InlineRc(raw_ptr)))
}

impl<T> InlineRc<T> {
    #[inline]
    pub unsafe fn as_ref(&self) -> &Inner<T> {
        self.0.as_ref()
    }
}

impl<T> Sender<T> {
    pub fn send(self, entry: T) -> Result<(), T> {
        let this = unsafe { self.0.as_ref() };

        unsafe {
            this.value.with_mut(|ptr| (&mut *ptr).as_mut_ptr())
                .write(entry);
        }

        let state = this.state.fetch_or(VALUE_READY, Ordering::AcqRel);

        if state & CLOSED == CLOSED {
            this.state.fetch_and(!VALUE_READY, Ordering::AcqRel);

            let value = unsafe { take(&this.value) };

            return Err(value);
        }

        if state & WAKER_READY == WAKER_READY {
            // take waker
            let state = this.state.fetch_and(!WAKER_READY, Ordering::AcqRel);

            if state & WAKER_READY == WAKER_READY {
                unsafe {
                    take(&this.waker).wake();
                }
            }
        }

        Ok(())
    }
}

impl<T> Future for Receiver<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.0.as_ref() };

        // take waker
        let state = this.state.fetch_and(!WAKER_READY, Ordering::AcqRel);

        if state & VALUE_READY == VALUE_READY {
            let value = unsafe { take(&this.value) };

            this.state.fetch_and(!VALUE_READY, Ordering::AcqRel);

            return Poll::Ready(Some(value));
        }

        if state & CLOSED == CLOSED {
            return Poll::Ready(None);
        }

        if state & WAKER_READY == WAKER_READY {
            let waker_cx = cx.waker();
            let waker_ref = unsafe {
                let waker_ptr = this.waker
                    .with_mut(|ptr| (&mut *ptr).as_mut_ptr());
                &mut *waker_ptr
            };

            if !waker_ref.will_wake(waker_cx) {
                let _ = mem::replace(waker_ref, waker_cx.clone());
            }
        } else {
            let waker_cx = cx.waker().clone();

            unsafe {
                this.waker.with_mut(|ptr| &mut *ptr)
                    .as_mut_ptr()
                    .write(waker_cx);
            }
        }

        this.state.fetch_or(WAKER_READY, Ordering::AcqRel);

        Poll::Pending
    }
}

impl<T> Drop for InlineRc<T> {
    fn drop(&mut self) {
        let this = unsafe { self.0.as_ref() };

        let state = this.state.fetch_or(CLOSED, Ordering::AcqRel);

        if state & CLOSED == CLOSED {
            unsafe {
                Box::from_raw(self.0.as_ptr());
            }
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        let state = load_u8(&mut self.state);

        if state & WAKER_READY == WAKER_READY {
            unsafe { take(&self.waker) };
        }

        if state & VALUE_READY == VALUE_READY {
            unsafe { take(&self.value) };
        }
    }
}

#[inline]
unsafe fn take<T>(target: &UnsafeCell<mem::MaybeUninit<T>>) -> T {
    target.with_mut(|ptr| mem::replace(&mut *ptr, mem::MaybeUninit::uninit()))
        .assume_init()
}

#[cfg(feature = "loom")]
#[inline]
pub fn load_u8(t: &mut AtomicU8) -> u8 {
    unsafe {
        t.unsync_load()
    }
}

#[cfg(not(feature = "loom"))]
#[inline]
pub fn load_u8(t: &mut AtomicU8) -> u8 {
    *t.get_mut()
}
