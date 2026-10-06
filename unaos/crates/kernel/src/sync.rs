// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — kernel-by-ruling
//! LOCKREG (rmbp-ledger B414) — ONE spin lock type for the kernel. Design: docs/dev/evidence/rmbp-1005/lockreg.md.
//!
//! [`Mutex<T>`] is spin's mutex with the same API (`new` const, `lock`, `try_lock`, `is_locked`, `force_unlock`,
//! `get_mut`, `into_inner`), built as a `spin::Mutex<()>` raw lock beside an `UnsafeCell<T>` so the raw lock can
//! be released WITHOUT knowing `T` (a dead task's locks, by address). `new` is `#[track_caller]`: the
//! construction site is the lock's NAME (its class). Every kernel `spin::Mutex` is this type (the sweep).

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::panic::Location;

/// The kernel's spin lock. Same API as `spin::Mutex`.
pub struct Mutex<T: ?Sized> {
    raw: spin::Mutex<()>,
    site: &'static Location<'static>,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}

/// The held lock. `R` only mirrors spin's relax parameter so `MutexGuard<'_, T, spin::relax::Spin>` reads as before.
pub struct MutexGuard<'a, T: ?Sized + 'a, R = spin::relax::Spin> {
    _raw: spin::MutexGuard<'a, ()>,
    data: *mut T,
    _p: PhantomData<(&'a mut T, R)>,
}

unsafe impl<T: ?Sized + Sync, R> Sync for MutexGuard<'_, T, R> {}

impl<T> Mutex<T> {
    #[track_caller]
    #[inline]
    pub const fn new(value: T) -> Self {
        Mutex { raw: spin::Mutex::new(()), site: Location::caller(), data: UnsafeCell::new(value) }
    }

    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    #[inline]
    fn guard<'a>(&'a self, raw: spin::MutexGuard<'a, ()>) -> MutexGuard<'a, T> {
        MutexGuard { _raw: raw, data: self.data.get(), _p: PhantomData }
    }

    #[track_caller]
    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        self.guard(self.raw.lock())
    }

    #[track_caller]
    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.raw.try_lock().map(|g| self.guard(g))
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    /// # Safety
    /// As `spin::Mutex::force_unlock`: no live guard may still be used.
    #[inline]
    pub unsafe fn force_unlock(&self) {
        unsafe { self.raw.force_unlock() }
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    /// The construction site — the lock's name.
    #[inline]
    pub fn site(&self) -> &'static Location<'static> {
        self.site
    }
}

impl<T: Default> Default for Mutex<T> {
    #[track_caller]
    fn default() -> Self {
        Mutex::new(T::default())
    }
}

impl<T: ?Sized + core::fmt::Debug> core::fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.try_lock() {
            Some(g) => write!(f, "Mutex {{ data: {:?} }}", &*g),
            None => f.write_str("Mutex { <locked> }"),
        }
    }
}

impl<T: ?Sized, R> Deref for MutexGuard<'_, T, R> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.data }
    }
}

impl<T: ?Sized, R> DerefMut for MutexGuard<'_, T, R> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.data }
    }
}

impl<T: ?Sized + core::fmt::Debug, R> core::fmt::Debug for MutexGuard<'_, T, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + core::fmt::Display, R> core::fmt::Display for MutexGuard<'_, T, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}
