use core::{fmt, mem, slice};

use super::String;

/// An iterator which uses a closure to determine if a character should be removed.
///
/// This struct is created by [`String::extract_if`].
/// See its documentation for more.
///
/// # Example
///
/// ```
/// #![feature(string_extract_if)]
/// let mut s = "Hello! Привет!　你好！".to_string();
/// let iter: std::string::ExtractIf<'_, _> = s.extract_if(|c| c.len_utf8() > 2);
/// ```
#[unstable(feature = "string_extract_if", issue = "154318")]
#[must_use = "iterators are lazy and do nothing unless consumed; \
    use `retain` or `extract_if().for_each(drop)` to remove and discard characters"]
pub struct ExtractIf<'a, F> {
    /// The underlying bytes of the original [`String`].
    /// The length of this slice is equal to the (byte) length of the original [`String`] prior to draining.
    ///
    /// During the iteration, this slice consists of:
    /// - A valid UTF-8 prefix (`*valid_prefix_len` bytes)
    ///   of characters that we iterated over and didn't extract.
    /// - A middle portion of `bytes_removed` initialized bytes that might not be valid UTF-8.
    /// - A valid UTF-8 suffix (`bytes.len() - (*valid_prefix_len + bytes_removed)` bytes)
    ///   of characters that we have not iterated over yet.
    ///
    /// The above (together with the fact that `valid_prefix.len() + bytes_removed <= old_len`,
    /// and that the `bytes` reference *itself* is never changed)
    /// is essentially this structure's invariant.
    bytes: &'a mut [u8],

    /// A mutable reference to the `length` field of the original [`String`]'s underlying `Vec<u8>`.
    /// To maintain the [`String`]'s invariant, even in the face `panic`s or this structure being leaked
    /// or dropped before the iteration is over, we must at all times ensure that:
    /// - `*valid_prefix_len <= bytes.len()`, where `bytes.len()` is the original `String`'s length.
    /// - `bytes[..*valid_prefix_len]` is valid UTF-8.
    ///
    /// To do so, we set this values to zero in [`ExtractIf::new`]
    /// and then gradually increase it as `bytes`'s prefix keeps filling with valid UTF-8.
    /// It is finally adjusted in [`ExtractIf::drop`].
    valid_prefix_len: &'a mut usize,

    /// The number of UTF-8 bytes we have removed from the original [`String`] so far.
    bytes_removed: usize,

    /// The filter test predicate.
    pred: F,
}

impl<'a, F> ExtractIf<'a, F> {
    pub(super) fn new(string: &'a mut String, pred: F) -> Self {
        let (bytes, valid_prefix_len, _) = unsafe { string.vec.as_raw_parts() };
        let old_len = mem::replace(valid_prefix_len, 0);
        let bytes = unsafe { slice::from_raw_parts_mut(bytes.as_ptr(), old_len) };
        ExtractIf { bytes, valid_prefix_len, bytes_removed: 0, pred }
    }
}

#[unstable(feature = "string_extract_if", issue = "154318")]
impl<F> Iterator for ExtractIf<'_, F>
where
    F: FnMut(char) -> bool,
{
    type Item = char;

    fn next(&mut self) -> Option<char> {
        loop {
            let valid_prefix_len = *self.valid_prefix_len;

            // SAFETY: by our invariant, `valid_prefix_len <= bytes.len()`.
            let tail = unsafe { self.bytes.get_unchecked_mut(valid_prefix_len..) };

            let c = unsafe {
                // SAFETY: by our invariant, `bytes_removed <= bytes.len() - valid_prefix_len`.
                let valid_tail = tail.get_unchecked(self.bytes_removed..);
                // SAFETY: we have not touched these bytes before, so they remain valid UTF-8.
                str::from_utf8_unchecked(valid_tail)
            }
            .chars() // FIXME(str_first_last_char): replace this with `first_char`
            .next()?;

            let char_len = c.len_utf8();
            if (self.pred)(c) {
                self.bytes_removed += char_len;
                return Some(c);
            } else {
                tail.copy_within(self.bytes_removed..(self.bytes_removed + char_len), 0);
                *self.valid_prefix_len += char_len;
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.bytes.len() - *self.valid_prefix_len - self.bytes_removed))
    }
}

#[unstable(feature = "string_extract_if", issue = "154318")]
impl<F> Drop for ExtractIf<'_, F> {
    fn drop(&mut self) {
        let valid_prefix_len = *self.valid_prefix_len;
        self.bytes.copy_within((valid_prefix_len + self.bytes_removed).., valid_prefix_len);
        *self.valid_prefix_len = self.bytes.len() - self.bytes_removed;
    }
}

#[unstable(feature = "string_extract_if", issue = "154318")]
impl<F> fmt::Debug for ExtractIf<'_, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: by invariant, `valid_prefix_len <= bytes.len()`.
        let (valid_prefix, tail) = unsafe { self.bytes.split_at_unchecked(*self.valid_prefix_len) };
        // SAFETY: by our invariant, `bytes_removed <= bytes.len() - valid_prefix_len`.
        let (_hole, valid_suffix) = unsafe { tail.split_at_unchecked(self.bytes_removed) };
        // SAFETY: by our invariant, this prefix is valid UTF-8.
        let valid_prefix = unsafe { str::from_utf8_unchecked(valid_prefix) };
        // SAFETY: by our invariant, this suffix is valid UTF-8.
        let valid_suffix = unsafe { str::from_utf8_unchecked(valid_suffix) };
        f.debug_struct("ExtractIf")
            .field("retained", &valid_prefix)
            .field("remainder", &valid_suffix)
            .finish_non_exhaustive()
    }
}
