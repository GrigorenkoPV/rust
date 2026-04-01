use core::alloc::Allocator;
use core::mem::{self, MaybeUninit};
use core::ops::{Range, RangeBounds};
use core::{fmt, ptr, slice};

use super::Vec;

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
pub struct ExtractIf<'a, T, F> {
    elements: &'a mut [MaybeUninit<T>],
    valid_prefix_len: &'a mut usize,

    /// The number of items that have been drained (removed) thus far.
    hole_size: usize,

    /// Elements at and beyond this point will be retained. Must be equal or smaller than `old_len`.
    end: usize,
    /// The filter test predicate.
    pred: F,
}

impl<'a, T, F> ExtractIf<'a, T, F> {
    pub(super) fn new<R, A>(vec: &'a mut Vec<T, A>, pred: F, range: R) -> Self
    where
        R: RangeBounds<usize>,
        A: Allocator,
    {
        let (elements, valid_prefix_len, _, _) = unsafe { vec.as_raw_parts_in() };
        let old_len = *valid_prefix_len;
        let Range { start, end } = slice::range(range, ..old_len);
        let elements = unsafe { slice::from_raw_parts_mut(elements.as_ptr().cast(), old_len) };
        // Guard against the vec getting leaked (leak amplification)
        *valid_prefix_len = start;
        ExtractIf { elements, valid_prefix_len, hole_size: 0, end, pred }
    }
}

#[stable(feature = "extract_if", since = "1.87.0")]
impl<T, F> Iterator for ExtractIf<'_, T, F>
where
    F: FnMut(&mut T) -> bool,
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        loop {
            let valid_prefix_len = *self.valid_prefix_len;

            let tail = unsafe { self.elements.get_unchecked_mut(valid_prefix_len..self.end) };

            let next = tail.get_mut(self.hole_size)?;

            if (self.pred)(unsafe { next.assume_init_mut() }) {
                self.hole_size += 1;
                // SAFETY: We never touch this element again after returning it.
                return Some(unsafe { next.assume_init_read() });
            } else {
                if let Ok([to, from]) = tail.get_disjoint_mut([0, self.hole_size]) {
                    unsafe { to.write(from.assume_init_read()) };
                }
                *self.valid_prefix_len += 1;
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.end - *self.valid_prefix_len - self.hole_size))
    }
}

unsafe fn assume_init_slice<T>(s: &[MaybeUninit<T>]) -> &[T] {
    unsafe { mem::transmute(s) }
}

#[stable(feature = "extract_if", since = "1.87.0")]
impl<T, F> Drop for ExtractIf<'_, T, F> {
    fn drop(&mut self) {
        let hole_size = self.hole_size;
        if self.hole_size > 0 {
            let valid_prefix_len = *self.valid_prefix_len;
            let valid_tail_len = self.elements.len() - valid_prefix_len - hole_size;
            let start = self.elements.as_mut_ptr();

            // SAFETY: Trailing unchecked items must be valid since we never touch them.
            unsafe {
                let hole = start.add(valid_prefix_len);
                ptr::copy(hole.add(hole_size), hole, valid_tail_len);
            }
        }
        *self.valid_prefix_len = self.elements.len() - self.hole_size
    }
}

#[stable(feature = "extract_if", since = "1.87.0")]
impl<T, F> fmt::Debug for ExtractIf<'_, T, F>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (retained, tail) = unsafe { self.elements.split_at_unchecked(*self.valid_prefix_len) };
        let retained = unsafe { assume_init_slice(retained) };

        let valid_tail = unsafe { tail.get_unchecked(self.hole_size..) };
        let valid_tail = unsafe { assume_init_slice(valid_tail) };

        let (remainder, skipped_tail) = unsafe {
            valid_tail.split_at_unchecked(self.end - (*self.valid_prefix_len + self.hole_size))
        };

        f.debug_struct("ExtractIf")
            .field("retained", &retained)
            .field("remainder", &remainder)
            .field("skipped_tail", &skipped_tail)
            .finish_non_exhaustive()
    }
}
