use crate::boxed::Box;
use crate::traits::*;

use std::fmt::{Debug, Formatter, Result};
use std::ops::{Deref, DerefMut};

///
/// A type for protecting secrets allocated on the heap.
///
/// Heap-allocated secrets have distinct security needs from
/// stack-allocated ones. They provide the following guarantees:
///
/// * any attempt to access the memory without having been borrowed
///   appropriately will result in immediate program termination; the memory is
///   protected with `mprotect(2)` as follows:
///   * `PROT_NONE` when the `SecretVec` has no outstanding borrows
///   * `PROT_READ` when it has outstanding immutable borrows
///   * `PROT_WRITE` when it has an outstanding mutable borrow
/// * the allocated region has guard pages preceding and following it, both
///   set to `PROT_NONE`, ensuring that overflows and (large enough) underflows
///   cause immediate program termination
/// * a canary is placed just before the memory location (and after the guard
///   page) in order to detect smaller underflows; if this memory has been
///   written to (and the canary modified), the program will immediately abort
///   when the `SecretVec` is `drop`ped
/// * `mlock(2)` is called on the underlying memory
/// * `munlock(2)` is called on the underlying memory when no longer in use
/// * the underlying memory is zeroed when no longer in use
/// * they are best-effort compared in constant time
/// * they are best-effort prevented from being printed by `Debug`
/// * they are best-effort protected from `Clone`ing the interior data
///
/// To fulfill these guarantees, `SecretVec` uses an API similar to (but
/// not exactly like) that of `RefCell`. You must call `borrow()` to
/// (immutably) borrow the protected data inside and you must call
/// `borrow_mut()` to access it mutably. Unlike `RefCell` which hides
/// interior mutability with immutable borrows, these two calls follow
/// standard borrowing rules: `borrow_mut` takes a `&mut self`, so the
/// borrow checker statically ensures the exclusivity of mutable
/// borrows.
///
/// These `borrow` and `borrow_mut` calls return a wrapper around the
/// interior that ensures the memory is re-`mprotect`ed when all active
/// borrows leave scope. These wrappers `Deref` to the underlying value
/// so you can to work with them as if they were the underlying type,
/// with a few excepitons: they have specific implementations for
/// `Clone`, `Debug`, `PartialEq`, and `Eq` that try to ensure that the
/// underlying memory isn't copied out of protected area, that the
/// contents are never printed, and that two secrets are only ever
/// compared in constant time.
///
/// Care *must* be taken not to over-aggressively dereference these
/// wrappers, as once you're working with the real underlying type, we
/// can't prevent direct calls to their implementations of these traits.
/// Care must also be taken not to call any other methods on these types
/// that introduce copying.
///
/// # Example: generate a cryptographically-random 128-bit `SecretVec`
///
/// Initialize a `SecretVec` with cryptographically random data:
///
/// ```
/// # use secrets::SecretVec;
/// let secret = SecretVec::<u8>::random(128);
///
/// assert_eq!(secret.len(), 128);
/// ```
///
/// # Example: move mutable data into a `SecretVec`
///
/// Existing data can be moved into a `SecretVec`. When doing so, we
/// make a best-effort attempt to zero out the data in the original
/// location. Any prior copies will be unaffected, so please exercise as
/// much caution as possible when handling data before it can be
/// protected.
///
/// ```
/// # use secrets::SecretVec;
/// let mut value = [1u8, 2, 3, 4];
///
/// // the contents of `value` will be copied into the SecretVec before
/// // being zeroed out
/// let secret = SecretVec::from(&mut value[..]);
///
/// // the contents of `value` have been zeroed
/// assert_eq!(value, [0, 0, 0, 0]);
/// ```
///
/// Example: borrowing a `SecretVec`
///
/// Borrow a `SecretVec` in order to use the wrapped contents.
///
/// ```
/// # use secrets::SecretVec;
/// let secret   = SecretVec::<u8>::from(&mut [1, 2][..]);
/// let secret_r = secret.borrow();
///
/// // use secret_r as if it were a `&[u8]`
///
/// // If uncommented, the line below would prevent compilation due to
/// // the outstanding immutable borrow already held by `secret_r`.
/// // let secret_w = secret.borrow_mut();
/// ```
///
#[derive(Clone, Eq)]
pub struct SecretVec<T: Bytes> {
    boxed: Box<T>,
}

#[derive(Eq)]
pub struct Ref<'a, T: Bytes> {
    boxed: &'a Box<T>,
}

#[derive(Eq)]
pub struct RefMut<'a, T: Bytes> {
    boxed: &'a mut Box<T>,
}

impl<T: Bytes> SecretVec<T> {
    pub fn new<F>(len: usize, f: F) -> Self where F: FnOnce(&mut [T]) {
        Self { boxed: Box::new(len, f) }
    }

    pub fn len(&self) -> usize {
        self.boxed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.boxed.is_empty()
    }

    pub fn size(&self) -> usize {
        self.boxed.size()
    }

    pub fn borrow(&self) -> Ref<'_, T> {
        Ref::new(&self.boxed)
    }

    pub fn borrow_mut(&mut self) -> RefMut<'_, T> {
        RefMut::new(&mut self.boxed)
    }
}

impl<T: Bytes + Randomizable> SecretVec<T> {
    pub fn random(len: usize) -> Self {
        Self { boxed: Box::random(len) }
    }
}

impl<T: Bytes + Zeroable> SecretVec<T> {
    pub fn zero(len: usize) -> Self {
        Self { boxed: Box::zero(len) }
    }
}

impl<T: Bytes + Zeroable> From<&mut [T]> for SecretVec<T> {
    fn from(data: &mut [T]) -> Self {
        Self { boxed: data.into() }
    }
}

impl<T: Bytes> Debug for SecretVec<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result { self.boxed.fmt(f) }
}

impl<T: Bytes + ConstantEq> PartialEq for SecretVec<T> {
    fn eq(&self, rhs: &Self) -> bool {
        self.boxed.eq(&rhs.boxed)
    }
}

impl<'a, T: Bytes> Ref<'a, T> {
    fn new(boxed: &'a Box<T>) -> Self {
        Self { boxed: boxed.unlock() }
    }
}

impl<T: Bytes> Clone for Ref<'_, T> {
    fn clone(&self) -> Self {
        Self { boxed: self.boxed.unlock() }
    }
}

impl<T: Bytes> Drop for Ref<'_, T> {
    fn drop(&mut self) {
        self.boxed.lock();
    }
}

impl<T: Bytes> Deref for Ref<'_, T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.boxed.as_ref()
    }
}

impl<T: Bytes> Debug for Ref<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result { self.boxed.fmt(f) }
}

impl<T: Bytes> PartialEq for Ref<'_, T> {
    fn eq(&self, rhs: &Self) -> bool {
        // technically we could punt to `self.boxed.eq(&other.boxed),
        // but the handler for that performs some extra locks and
        // unlocks which are unnecessary here since we know both sides
        // are already unlocked
        self.as_ref().constant_eq(rhs.as_ref())
    }
}

impl<T: Bytes> PartialEq<RefMut<'_, T>> for Ref<'_, T> {
    fn eq(&self, rhs: &RefMut<'_, T>) -> bool {
        // technically we could punt to `self.boxed.eq(&other.boxed),
        // but the handler for that performs some extra locks and
        // unlocks which are unnecessary here since we know both sides
        // are already unlocked
        self.as_ref().constant_eq(rhs.as_ref())
    }
}

impl<'a, T: Bytes> RefMut<'a, T> {
    fn new(boxed: &'a mut Box<T>) -> Self {
        Self { boxed: boxed.unlock_mut() }
    }
}

impl<T: Bytes> Drop for RefMut<'_, T> {
    fn drop(&mut self) {
        self.boxed.lock();
    }
}

impl<T: Bytes> Deref for RefMut<'_, T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.boxed.as_ref()
    }
}

impl<T: Bytes> DerefMut for RefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.boxed.as_mut()
    }
}

impl<T: Bytes> Debug for RefMut<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result { self.boxed.fmt(f) }
}

impl<T: Bytes> PartialEq for RefMut<'_, T> {
    fn eq(&self, rhs: &Self) -> bool {
        // technically we could punt to `self.boxed.eq(&other.boxed),
        // but the handler for that performs some extra locks and
        // unlocks which are unnecessary here since it's already
        // unlocked
        self.as_ref().constant_eq(rhs.as_ref())
    }
}

impl<T: Bytes> PartialEq<Ref<'_, T>> for RefMut<'_, T> {
    fn eq(&self, rhs: &Ref<'_, T>) -> bool {
        // technically we could punt to `self.boxed.eq(&other.boxed),
        // but the handler for that performs some extra locks and
        // unlocks which are unnecessary here since we know both sides
        // are already unlocked
        self.as_ref().constant_eq(rhs.as_ref())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn it_allows_custom_initialization() {
        let _ = SecretVec::<u64>::new(4, |s| {
            s.clone_from_slice(&[1, 2, 3, 4][..]);

            assert_eq!(*s, [1, 2, 3, 4]);
        });
    }

    #[test]
    fn it_allows_borrowing_immutably() {
        let secret = SecretVec::<u64>::zero(2);
        let s      = secret.borrow();

        assert_eq!(*s, [0, 0]);
    }

    #[test]
    fn it_allows_borrowing_mutably() {
        let mut secret = SecretVec::<u64>::zero(2);
        let mut s      = secret.borrow_mut();

        s.clone_from_slice(&[7, 1][..]);

        assert_eq!(*s, [7, 1]);
    }

    #[test]
    fn it_allows_storing_fixed_size_arrays() {
        let secret = SecretVec::<[u8; 2]>::new(2, |s| {
            s.clone_from_slice(&[[1, 2], [3, 4]][..]);
        });

        assert_eq!(*secret.borrow(), [[1, 2], [3, 4]]);
    }

    #[test]
    fn it_moves_safely() {
        let secret_1 = SecretVec::<u8>::zero(1);
        let secret_2 = secret_1;

        assert_eq!(*secret_2.borrow(), [0]);
    }

    #[test]
    fn it_safely_clones_immutable_references() {
        let secret   = SecretVec::<u8>::random(4);
        let borrow_1 = secret.borrow();
        let borrow_2 = borrow_1.clone();

        assert_eq!(borrow_1, borrow_2);
    }

    #[test]
    fn it_compares_equality() {
        let secret_1 = SecretVec::<u8>::from(&mut [1, 2, 3][..]);
        let secret_2 = secret_1.clone();

        assert_eq!(secret_1, secret_2);
    }

    #[test]
    fn it_compares_inequality() {
        let secret_1 = SecretVec::<[u64; 8]>::random(32);
        let secret_2 = SecretVec::<[u64; 8]>::random(32);

        assert_ne!(secret_1, secret_2);
    }
}
