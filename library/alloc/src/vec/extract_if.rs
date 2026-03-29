use core::ops::{Range, RangeBounds};
use core::{fmt, ptr, slice};

use super::Vec;
use crate::alloc::{Allocator, Global};

/// An iterator which uses a closure to determine if an element should be removed.
///
/// This struct is created by [`Vec::extract_if`].
/// See its documentation for more.
///
/// # Example
///
/// ```
/// let mut v = vec![0, 1, 2];
/// let iter: std::vec::ExtractIf<'_, _, _> = v.extract_if(.., |x| *x % 2 == 0);
/// ```
#[stable(feature = "extract_if", since = "1.87.0")]
#[must_use = "iterators are lazy and do nothing unless consumed; \
    use `retain_mut` or `extract_if().for_each(drop)` to remove and discard elements"]
pub struct ExtractIf<
    'a,
    T,
    F,
    #[unstable(feature = "allocator_api", issue = "32838")] A: Allocator = Global,
> {
    valid_prefix: &'a mut Vec<T, A>,

    /// The number of items that have been drained (removed) thus far.
    hole_size: usize,

    /// Elements at and beyond this point will be retained. Must be equal or smaller than `old_len`.
    end: usize,
    /// The original length of `vec` prior to draining.
    old_len: usize,
    /// The filter test predicate.
    pred: F,
}

impl<'a, T, F, A: Allocator> ExtractIf<'a, T, F, A> {
    pub(super) fn new<R: RangeBounds<usize>>(vec: &'a mut Vec<T, A>, pred: F, range: R) -> Self {
        let old_len = vec.len();
        let Range { start, end } = slice::range(range, ..old_len);

        // Guard against the vec getting leaked (leak amplification)
        unsafe { vec.set_len(start) };
        ExtractIf { valid_prefix: vec, hole_size: 0, end, old_len, pred }
    }

    /// Returns a reference to the underlying allocator.
    #[unstable(feature = "allocator_api", issue = "32838")]
    #[inline]
    pub fn allocator(&self) -> &A {
        self.valid_prefix.allocator()
    }
}

#[stable(feature = "extract_if", since = "1.87.0")]
impl<T, F, A: Allocator> Iterator for ExtractIf<'_, T, F, A>
where
    F: FnMut(&mut T) -> bool,
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        loop {
            let valid_prefix_len = self.valid_prefix.len();
            let hole_size = self.hole_size;
            let start = self.valid_prefix.as_mut_ptr();
            let hole = unsafe { start.add(valid_prefix_len) };
            let tail = unsafe { hole.add(hole_size) };

            if (self.pred)(
                unsafe {
                    slice::from_raw_parts_mut(tail, self.end - (hole_size + valid_prefix_len))
                }
                .first_mut()?,
            ) {
                self.hole_size += 1;
                // SAFETY: We never touch this element again after returning it.
                return Some(unsafe { ptr::read(tail) });
            } else {
                if self.hole_size > 0 {
                    unsafe { ptr::copy_nonoverlapping(tail, hole, 1) };
                }
                unsafe { self.valid_prefix.set_len(valid_prefix_len + 1) };
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.end - self.valid_prefix.len() - self.hole_size))
    }
}

#[stable(feature = "extract_if", since = "1.87.0")]
impl<T, F, A: Allocator> Drop for ExtractIf<'_, T, F, A> {
    fn drop(&mut self) {
        let hole_size = self.hole_size;
        if hole_size > 0 {
            let valid_prefix_len = self.valid_prefix.len();
            let valid_tail_len = self.old_len - valid_prefix_len - hole_size;
            let start = self.valid_prefix.as_mut_ptr();

            // SAFETY: Trailing unchecked items must be valid since we never touch them.
            unsafe {
                let hole = start.add(valid_prefix_len);
                ptr::copy(hole.add(hole_size), hole, valid_tail_len);
            }
        }
        // SAFETY: After filling holes, all items are in contiguous memory.
        unsafe {
            self.valid_prefix.set_len(self.old_len - self.hole_size);
        }
    }
}

#[stable(feature = "extract_if", since = "1.87.0")]
impl<T, F, A> fmt::Debug for ExtractIf<'_, T, F, A>
where
    T: fmt::Debug,
    A: Allocator,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // We have to use pointer arithmetics here,
        // because the length of `self.vec` is temporarily set to 0.
        let start = self.valid_prefix.as_ptr();

        let retained = self.valid_prefix.as_slice();

        let tail_start = self.valid_prefix.len() + self.hole_size;

        let valid_tail =
            unsafe { slice::from_raw_parts(start.add(tail_start), self.old_len - tail_start) };

        let (remainder, skipped_tail) =
            unsafe { valid_tail.split_at_unchecked(self.end - tail_start) };

        f.debug_struct("ExtractIf")
            .field("retained", &retained)
            .field("remainder", &remainder)
            .field("skipped_tail", &skipped_tail)
            .finish_non_exhaustive()
    }
}
