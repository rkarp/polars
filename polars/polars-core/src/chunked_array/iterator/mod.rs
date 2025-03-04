use crate::chunked_array::ops::downcast::Chunks;
use crate::datatypes::CategoricalChunked;
use crate::prelude::{
    BooleanChunked, ChunkedArray, ListChunked, PolarsNumericType, Series, Utf8Chunked,
};
use crate::utils::CustomIterTools;
use arrow::array::{Array, ArrayData, ArrayRef, BooleanArray, LargeListArray, LargeStringArray};
use std::convert::TryFrom;
use std::ops::Deref;

// If parallel feature is enable, then, activate the parallel module.
#[cfg(feature = "parallel")]
#[cfg_attr(docsrs, doc(cfg(feature = "parallel")))]
pub mod par;

/// A `PolarsIterator` is an iterator over a `ChunkedArray` which contains polars types. A `PolarsIterator`
/// must implement `ExactSizeIterator` and `DoubleEndedIterator`.
pub trait PolarsIterator: ExactSizeIterator + DoubleEndedIterator + Send + Sync {}

/// Implement PolarsIterator for every iterator that implements the needed traits.
impl<T: ?Sized> PolarsIterator for T where T: ExactSizeIterator + DoubleEndedIterator + Send + Sync {}

impl<'a, T> IntoIterator for &'a ChunkedArray<T>
where
    T: PolarsNumericType,
{
    type Item = Option<T::Native>;
    type IntoIter = Box<dyn PolarsIterator<Item = Self::Item> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.downcast_iter().flatten().trust_my_length(self.len()))
    }
}

impl<'a> IntoIterator for &'a CategoricalChunked {
    type Item = Option<u32>;
    type IntoIter = Box<dyn PolarsIterator<Item = Self::Item> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.deref().into_iter()
    }
}

impl<'a> IntoIterator for &'a BooleanChunked {
    type Item = Option<bool>;
    type IntoIter = Box<dyn PolarsIterator<Item = Self::Item> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.downcast_iter().flatten().trust_my_length(self.len()))
    }
}

/// The no null iterator for a BooleanArray
pub struct BoolIterNoNull<'a> {
    array: &'a BooleanArray,
    current: usize,
    current_end: usize,
}

impl<'a> BoolIterNoNull<'a> {
    /// create a new iterator
    pub fn new(array: &'a BooleanArray) -> Self {
        BoolIterNoNull {
            array,
            current: 0,
            current_end: array.len(),
        }
    }
}

impl<'a> Iterator for BoolIterNoNull<'a> {
    type Item = bool;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current == self.current_end {
            None
        } else {
            let old = self.current;
            self.current += 1;
            unsafe { Some(self.array.value_unchecked(old)) }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (
            self.array.len() - self.current,
            Some(self.array.len() - self.current),
        )
    }
}

impl<'a> DoubleEndedIterator for BoolIterNoNull<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.current_end == self.current {
            None
        } else {
            self.current_end -= 1;
            unsafe { Some(self.array.value_unchecked(self.current_end)) }
        }
    }
}

/// all arrays have known size.
impl<'a> ExactSizeIterator for BoolIterNoNull<'a> {}

impl BooleanChunked {
    #[allow(clippy::wrong_self_convention)]
    pub fn into_no_null_iter(
        &self,
    ) -> impl Iterator<Item = bool> + '_ + Send + Sync + ExactSizeIterator + DoubleEndedIterator
    {
        self.downcast_iter()
            .map(|bool_arr| BoolIterNoNull::new(bool_arr))
            .flatten()
            .trust_my_length(self.len())
    }
}

/// Trait for ChunkedArrays that don't have null values.
/// The result is the most efficient implementation `Iterator`, according to the number of chunks.
pub trait IntoNoNullIterator {
    type Item;
    type IntoIter: Iterator<Item = Self::Item>;

    fn into_no_null_iter(self) -> Self::IntoIter;
}

/// Wrapper struct to convert an iterator of type `T` into one of type `Option<T>`.  It is useful to make the
/// `IntoIterator` trait, in which every iterator shall return an `Option<T>`.
pub struct SomeIterator<I>(I)
where
    I: Iterator;

impl<I> Iterator for SomeIterator<I>
where
    I: Iterator,
{
    type Item = Option<I::Item>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(Some)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<I> DoubleEndedIterator for SomeIterator<I>
where
    I: DoubleEndedIterator,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(Some)
    }
}

impl<I> ExactSizeIterator for SomeIterator<I> where I: ExactSizeIterator {}

impl CategoricalChunked {
    #[allow(clippy::wrong_self_convention)]
    pub fn into_no_null_iter(
        &self,
    ) -> impl Iterator<Item = u32> + '_ + Send + Sync + ExactSizeIterator + DoubleEndedIterator
    {
        self.deref().into_no_null_iter()
    }
}

/// Creates and implement a iterator for chunked arrays with a single chunks and no null values, so
/// it iterates over the only chunk without performing null checks. Returns `iter_item`, as elements
/// cannot be null.
///
/// It also implements, for the created iterator, the following traits:
/// - Iterator
/// - DoubleEndedIterator
/// - ExactSizeIterator
///
/// # Input
///
/// ca_type: The chunked array for which the single chunks iterator is implemented.
/// arrow_array: The arrow type of the chunked array chunks.
/// iterator_name: The name of the iterator struct to be implemented for a `SingleChunk` iterator.
/// iter_item: The iterator `Item`, the type which is going to be returned by the iterator.
/// (Optional) return_function: The function to apply to the each value of the chunked array before returning
///     the value.
macro_rules! impl_single_chunk_iterator {
    ($ca_type:ident, $arrow_array:ident, $iterator_name:ident, $iter_item:ty $(, $return_function:ident)?) => {
        impl<'a> ExactSizeIterator for $iterator_name<'a> {}

        /// Iterator for chunked arrays with just one chunk.
        /// The chunk cannot have null values so it does NOT perform null checks.
        ///
        /// The return type is `$iter_item`.
        pub struct $iterator_name<'a> {
            current_array: &'a $arrow_array,
            idx_left: usize,
            idx_right: usize,
        }

        impl<'a> $iterator_name<'a> {
            fn new(ca: &'a $ca_type) -> Self {
                let current_array = ca.downcast_iter().next().unwrap();
                let idx_left = 0;
                let idx_right = current_array.len();

                Self {
                    current_array,
                    idx_left,
                    idx_right,
                }
            }

            // TODO: Move this function to `impl Iterator` when the feature is stabilized.
            /// Advance the iterator by `n` positions in constant time *O(n)*. It is an eager function.
            fn advance_by(&mut self, n: usize) -> Result<(), usize> {
                // Compute the total available elements in the iterator.
                let total_elements = self.idx_right - self.idx_left;

                // If skip more than the total number of elements in the iterator, then, move the iterator to
                // the last element and return an error with the number of elements in the iterator.
                if n > total_elements {
                    self.idx_left = self.idx_right;
                    return Err(total_elements)
                }

                // If n is less than the total number of elements then advance the left index by n elements
                // and return success.
                self.idx_left += n;
                Ok(())
            }
        }

        impl<'a> Iterator for $iterator_name<'a> {
            type Item = $iter_item;

            fn next(&mut self) -> Option<Self::Item> {
                // end of iterator or meet reversed iterator in the middle
                if self.idx_left == self.idx_right {
                    return None;
                }

                // Safety:
                // Bounds are checked
                let v = unsafe { self.current_array.value_unchecked(self.idx_left) };
                self.idx_left += 1;

                $(
                    // If return function is provided, apply the next function to the value.
                    // The result value will shadow, the `v` variable.
                    let v = $return_function("next", v);
                )?

                Some(v)
            }

            // TODO: Remove this method once `advance_by` is stable method and it is implemented
            // as an `Iterator` method.
            /// Return the `nth` element in the iterator.
            fn nth(&mut self, n: usize) -> Option<Self::Item> {
                self.advance_by(n).ok()?;
                self.next()
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                let len = self.current_array.len();
                (len, Some(len))
            }

        }

        impl<'a> DoubleEndedIterator for $iterator_name<'a> {
            fn next_back(&mut self) -> Option<Self::Item> {
                // end of iterator or meet reversed iterator in the middle
                if self.idx_left == self.idx_right {
                    return None;
                }
                self.idx_right -= 1;
                // Safety:
                // Bounds are checked
                let v = unsafe { self.current_array.value_unchecked(self.idx_right) };

                $(
                    // If return function is provided, apply the next_back function to the value.
                    // The result value will shadow, the `v` variable.
                    let v = $return_function("next_back", v);
                )?

                Some(v)
            }
        }
    };
}

/// Creates and implement a iterator for chunked arrays with a single chunks and null values, so
/// it iterates over the only chunk and performing null checks. Returns `Option<iter_item>`, as elements
/// can be null.
///
/// It also implements, for the created iterator, the following traits:
/// - Iterator
/// - DoubleEndedIterator
/// - ExactSizeIterator
///
/// # Input
///
/// ca_type: The chunked array for which the which the single chunks with null check iterator is implemented.
/// arrow_array: The arrow type of the chunked array chunks.
/// iterator_name: The name of the iterator struct to be implemented for a `SingleChunkNullCheck` iterator.
/// iter_item: The iterator `Item`, the type which is going to be returned by the iterator.
/// (Optional) return_function: The function to apply to the each value of the chunked array before returning
///     the value.
macro_rules! impl_single_chunk_null_check_iterator {
    ($ca_type:ident, $arrow_array:ident, $iterator_name:ident, $iter_item:ty $(, $return_function:ident)?) => {
        impl<'a> ExactSizeIterator for $iterator_name<'a> {}

        /// Iterator for chunked arrays with just one chunk.
        /// The chunk have null values so it DOES perform null checks.
        ///
        /// The return type is `Option<$iter_item>`.
        pub struct $iterator_name<'a> {
            current_data: &'a ArrayData,
            current_array: &'a $arrow_array,
            idx_left: usize,
            idx_right: usize,
        }

        impl<'a> $iterator_name<'a> {
            fn new(ca: &'a $ca_type) -> Self {
                let current_array  = ca.downcast_iter().next().unwrap();
                let current_data = current_array.data();
                let idx_left = 0;
                let idx_right = current_array.len();

                Self {
                    current_data,
                    current_array,
                    idx_left,
                    idx_right,
                }
            }

            // TODO: Move this function to `impl Iterator` when the feature is stabilized.
            /// Advance the iterator by `n` positions in constant time *O(n)*. It is an eager function.
            fn advance_by(&mut self, n: usize) -> Result<(), usize> {
                // Compute the total available elements in the iterator.
                let total_elements = self.idx_right - self.idx_left;

                // If skip more than the total number of elements in the iterator, then, move the iterator to
                // the last element and return an error with the number of elements in the iterator.
                if n > total_elements {
                    self.idx_left = self.idx_right;
                    return Err(total_elements)
                }

                // If n is less than the total number of elements then advance the left index by n elements
                // and return success.
                self.idx_left += n;
                Ok(())
            }
        }

        impl<'a> Iterator for $iterator_name<'a> {
            type Item = Option<$iter_item>;

            fn next(&mut self) -> Option<Self::Item> {
                // end of iterator or meet reversed iterator in the middle
                if self.idx_left == self.idx_right {
                    return None;
                }
                let ret = if self.current_data.is_null(self.idx_left) {
                    Some(None)
                } else {
                    let v = unsafe { self.current_array.value_unchecked(self.idx_left) };

                    $(
                        // If return function is provided, apply the next function to the value.
                        // The result value will shadow, the `v` variable.
                        let v = $return_function("next", v);
                    )?

                    Some(Some(v))
                };
                self.idx_left += 1;
                ret
            }

            // TODO: Remove this method once `advance_by` is stable method and it is implemented
            // as an `Iterator` method.
            /// Return the `nth` element in the iterator.
            fn nth(&mut self, n: usize) -> Option<Self::Item> {
                self.advance_by(n).ok()?;
                self.next()
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                let len = self.current_array.len();
                (len, Some(len))
            }
        }

        impl<'a> DoubleEndedIterator for $iterator_name<'a> {
            fn next_back(&mut self) -> Option<Self::Item> {
                // end of iterator or meet reversed iterator in the middle
                if self.idx_left == self.idx_right {
                    return None;
                }
                self.idx_right -= 1;
                if self.current_data.is_null(self.idx_right) {
                    Some(None)
                } else {
                    let v = unsafe { self.current_array.value_unchecked(self.idx_right) };

                    $(
                        // If return function is provided, apply the next_back function to the value.
                        // The result value will shadow, the `v` variable.
                        let v = $return_function("next_back", v);
                    )?

                    Some(Some(v))
                }
            }
        }
    };
}

/// Creates and implement a iterator for chunked arrays with many chunks and no null values, so
/// it iterates over several chunks without performing null checks. Returns `iter_item`, as elements
/// cannot be null.
///
/// It also implements, for the created iterator, the following traits:
/// - Iterator
/// - DoubleEndedIterator
/// - ExactSizeIterator
///
/// # Input
///
/// ca_type: The chunked array for which the which the many chunks iterator is implemented.
/// arrow_array: The arrow type of the chunked array chunks.
/// iterator_name: The name of the iterator struct to be implemented for a `ManyChunk` iterator.
/// iter_item: The iterator `Item`, the type which is going to be returned by the iterator.
/// (Optional) return_function: The function to apply to the each value of the chunked array before returning
///     the value.
macro_rules! impl_many_chunk_iterator {
    ($ca_type:ident, $arrow_array:ident, $iterator_name:ident, $iter_item:ty $(, $return_function:ident)?) => {
        impl<'a> ExactSizeIterator for $iterator_name<'a> {}

        /// Iterator for chunked arrays with many chunks.
        /// The chunks cannot have null values so it does NOT perform null checks.
        ///
        /// The return type is `$iter_item`.
        pub struct $iterator_name<'a> {
            ca: &'a $ca_type,
            chunks: Chunks<'a, $arrow_array>,
            current_array_left: &'a $arrow_array,
            current_array_right: &'a $arrow_array,
            current_array_idx_left: usize,
            current_array_idx_right: usize,
            current_array_left_len: usize,
            idx_left: usize,
            idx_right: usize,
            chunk_idx_left: usize,
            chunk_idx_right: usize,
        }

        impl<'a> $iterator_name<'a> {
            fn new(ca: &'a $ca_type) -> Self {
                let chunks = ca.downcast_chunks();
                let current_array_left = chunks.get(0).unwrap();
                let idx_left = 0;
                let chunk_idx_left = 0;
                let chunk_idx_right = chunks.len() - 1;
                let current_array_right = chunks.get(chunk_idx_right).unwrap();
                let idx_right = ca.len();
                let current_array_idx_left = 0;
                let current_array_idx_right = current_array_right.len();
                let current_array_left_len = current_array_left.len();

                Self {
                    ca,
                    chunks,
                    current_array_left,
                    current_array_right,
                    current_array_idx_left,
                    current_array_idx_right,
                    current_array_left_len,
                    idx_left,
                    idx_right,
                    chunk_idx_left,
                    chunk_idx_right,
                }
            }

            /// Update the left index as well as the data regarding the left chunk.
            fn update_left_index(&mut self, idx_left: usize) {
                self.idx_left = idx_left;
                let (chunk_idx_left, current_array_idx_left) = self.ca.index_to_chunked_index(idx_left);
                self.chunk_idx_left = chunk_idx_left;
                self.current_array_idx_left = current_array_idx_left;
                self.current_array_left = self.chunks.get(chunk_idx_left).unwrap();
                self.current_array_left_len = self.current_array_left.len();
            }

            // TODO: Move this function to `impl Iterator` when the feature is stabilized.
            /// Advance the iterator by `n` positions in constant time *O(n)*. It is an eager function.
            fn advance_by(&mut self, n: usize) -> Result<(), usize> {
                // Compute the total available elements in the iterator.
                let total_elements = self.idx_right - self.idx_left;

                // If skip more than the total number of elements in the iterator, then, move the iterator to
                // the last element and return an error with the number of elements in the iterator.
                if n > total_elements {
                    self.idx_left = self.idx_right;
                    return Err(total_elements)
                }

                // If n is less than the total number of elements then advance the left index by n elements
                // and return success.
                let new_idx_left = self.idx_left + n;
                self.update_left_index(new_idx_left);

                Ok(())
            }
        }

        impl<'a> Iterator for $iterator_name<'a> {
            type Item = $iter_item;

            fn next(&mut self) -> Option<Self::Item> {
                // end of iterator or meet reversed iterator in the middle
                if self.idx_left == self.idx_right {
                    return None;
                }

                // return value
                // Safety:
                // Bounds are checked
                let ret = unsafe { self.current_array_left.value_unchecked(self.current_array_idx_left) };

                // increment index pointers
                self.idx_left += 1;
                self.current_array_idx_left += 1;

                // we've reached the end of the chunk
                if self.current_array_idx_left == self.current_array_left_len {
                    // Set a new chunk as current data
                    self.chunk_idx_left += 1;

                    // if this evaluates to False, next call will be end of iterator
                    if self.chunk_idx_left < self.chunks.len() {
                        // reset to new array
                        self.current_array_idx_left = 0;
                        self.current_array_left = self.chunks.get(self.chunk_idx_left).unwrap();
                        self.current_array_left_len = self.current_array_left.len();
                    }
                }

                $(
                    // If return function is provided, apply the next function to the value.
                    // The result value will shadow, the `ret` variable.
                    let ret = $return_function("next", ret);
                )?

                Some(ret)
            }

            // TODO: Remove this method once `advance_by` is stable method and it is implemented
            // as an `Iterator` method.
            /// Return the `nth` element in the iterator.
            fn nth(&mut self, n: usize) -> Option<Self::Item> {
                self.advance_by(n).ok()?;
                self.next()
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                let len = self.ca.len();
                (len, Some(len))
            }
        }

        impl<'a> DoubleEndedIterator for $iterator_name<'a> {
            fn next_back(&mut self) -> Option<Self::Item> {
                // end of iterator or meet reversed iterator in the middle
                if self.idx_left == self.idx_right {
                    return None;
                }
                self.idx_right -= 1;
                self.current_array_idx_right -= 1;

                // Safety:
                // Bounds are checked
                let ret = unsafe { self.current_array_right.value_unchecked(self.current_array_idx_right) };

                // we've reached the end of the chunk from the right
                if self.current_array_idx_right == 0 && self.idx_right > 0 {
                    // set a new chunk as current data
                    self.chunk_idx_right -= 1;
                    // reset to new array
                    self.current_array_right = self.chunks.get(self.chunk_idx_right).unwrap();
                    self.current_array_idx_right = self.current_array_right.len();
                }

                $(
                    // If return function is provided, apply the next_back function to the value.
                    // The result value will shadow, the `ret` variable.
                    let ret = $return_function("next_back", ret);
                )?

                Some(ret)
            }
        }
    };
}

/// Creates and implement a iterator for chunked arrays with many chunks and null values, so
/// it iterates over several chunks and perform null checks. Returns `Option<iter_item>`, as elements
/// can be null.
///
/// It also implements, for the created iterator, the following traits:
/// - Iterator
/// - DoubleEndedIterator
/// - ExactSizeIterator
///
/// # Input
///
/// ca_type: The chunked array for which the which the many chunks with null check iterator is implemented.
/// arrow_array: The arrow type of the chunked array chunks.
/// iterator_name: The name of the iterator struct to be implemented for a `ManyChunkNullCheck` iterator.
/// iter_item: The iterator `Item`, the type which is going to be returned by the iterator, wrapped
///     into and `Option`, as null check is performed by this iterator.
/// (Optional) return_function: The function to apply to the each value of the chunked array before returning
///     the value.
macro_rules! impl_many_chunk_null_check_iterator {
    ($ca_type:ident, $arrow_array:ident, $iterator_name:ident, $iter_item:ty $(, $return_function:ident)? ) => {
        impl<'a> ExactSizeIterator for $iterator_name<'a> {}

        /// Iterator for chunked arrays with many chunks.
        /// The chunks have null values so it DOES perform null checks.
        ///
        /// The return type is `Option<$iter_item>`.
        pub struct $iterator_name<'a> {
            ca: &'a $ca_type,
            chunks: Chunks<'a, $arrow_array>,
            #[allow(dead_code)]
            current_data_left: &'a ArrayData,
            current_array_left: &'a $arrow_array,
            current_data_right: &'a ArrayData,
            current_array_right: &'a $arrow_array,
            current_array_idx_left: usize,
            current_array_idx_right: usize,
            current_array_left_len: usize,
            idx_left: usize,
            idx_right: usize,
            chunk_idx_left: usize,
            chunk_idx_right: usize,
        }

        impl<'a> $iterator_name<'a> {
            fn new(ca: &'a $ca_type) -> Self {
                let chunks = ca.downcast_chunks();
                let current_array_left = chunks.get(0).unwrap();
                let current_data_left = current_array_left.data();
                let idx_left = 0;
                let chunk_idx_left = 0;
                let chunk_idx_right = chunks.len() - 1;
                let current_array_right = chunks.get(chunk_idx_right).unwrap();
                let current_data_right = current_array_right.data();
                let idx_right = ca.len();
                let current_array_idx_left = 0;
                let current_array_idx_right = current_data_right.len();
                let current_array_left_len = current_array_left.len();

                Self {
                    ca,
                    chunks,
                    current_data_left,
                    current_array_left,
                    current_data_right,
                    current_array_right,
                    current_array_idx_left,
                    current_array_idx_right,
                    current_array_left_len,
                    idx_left,
                    idx_right,
                    chunk_idx_left,
                    chunk_idx_right,
                }
            }

            /// Update the left index as well as the data regarding the left chunk.
            fn update_left_index(&mut self, idx_left: usize) {
                self.idx_left = idx_left;
                let (chunk_idx_left, current_array_idx_left) = self.ca.index_to_chunked_index(idx_left);
                self.chunk_idx_left = chunk_idx_left;
                self.current_array_idx_left = current_array_idx_left;
                self.current_array_left = self.chunks.get(chunk_idx_left).unwrap();
                self.current_array_left_len = self.current_array_left.len();
                self.current_data_left = self.current_array_left.data();
            }

            // TODO: Move this function to `impl Iterator` when the feature is stabilized.
            /// Advance the iterator by `n` positions in constant time *O(n)*. It is an eager function.
            fn advance_by(&mut self, n: usize) -> Result<(), usize> {
                // Compute the total available elements in the iterator.
                let total_elements = self.idx_right - self.idx_left;

                // If skip more than the total number of elements in the iterator, then, move the iterator to
                // the last element and return an error with the number of elements in the iterator.
                if n > total_elements {
                    self.idx_left = self.idx_right;
                    return Err(total_elements)
                }

                // If n is less than the total number of elements then advance the left index by n elements
                // and return success.
                let new_idx_left = self.idx_left + n;
                self.update_left_index(new_idx_left);

                Ok(())
            }
        }

        impl<'a> Iterator for $iterator_name<'a> {
            type Item = Option<$iter_item>;

            fn next(&mut self) -> Option<Self::Item> {
                // end of iterator or meet reversed iterator in the middle
                if self.idx_left == self.idx_right {
                    return None;
                }

                // return value
                let ret;
                if self.current_array_left.is_null(self.current_array_idx_left) {
                    ret = None
                } else {
                    // Safety:
                    // Bounds are checked
                    let v = unsafe { self.current_array_left.value_unchecked(self.current_array_idx_left) };

                    $(
                        // If return function is provided, apply the next function to the value.
                        // The result value will shadow, the `v` variable.
                        let v = $return_function("next", v);
                    )?

                    ret = Some(v);
                }

                // increment index pointers
                self.idx_left += 1;
                self.current_array_idx_left += 1;

                // we've reached the end of the chunk
                if self.current_array_idx_left == self.current_array_left_len {
                    // Set a new chunk as current data
                    self.chunk_idx_left += 1;

                    // if this evaluates to False, next call will be end of iterator
                    if self.chunk_idx_left < self.chunks.len() {
                        // reset to new array
                        self.current_array_idx_left = 0;
                        self.current_array_left = self.chunks.get(self.chunk_idx_left).unwrap();
                        self.current_data_left = self.current_array_left.data();
                        self.current_array_left_len = self.current_array_left.len();
                    }
                }

                Some(ret)
            }

            // TODO: Remove this method once `advance_by` is stable method and it is implemented
            // as an `Iterator` method.
            /// Return the `nth` element in the iterator.
            fn nth(&mut self, n: usize) -> Option<Self::Item> {
                self.advance_by(n).ok()?;
                self.next()
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                let len = self.ca.len();
                (len, Some(len))
            }
        }

        impl<'a> DoubleEndedIterator for $iterator_name<'a> {
            fn next_back(&mut self) -> Option<Self::Item> {
                // end of iterator or meet reversed iterator in the middle
                if self.idx_left == self.idx_right {
                    return None;
                }
                self.idx_right -= 1;
                self.current_array_idx_right -= 1;

                let ret = if self
                    .current_data_right
                    .is_null(self.current_array_idx_right)
                {
                    Some(None)
                } else {
                    // Safety:
                    // Bounds are checked
                    let v = unsafe { self.current_array_right.value_unchecked(self.current_array_idx_right) };

                    $(
                        // If return function is provided, apply the next_back function to the value.
                        // The result value will shadow, the `v` variable.
                        let v = $return_function("next_back", v);
                    )?

                    Some(Some(v))
                };

                // we've reached the end of the chunk from the right
                if self.current_array_idx_right == 0 && self.idx_right > 0 {
                    // set a new chunk as current data
                    self.chunk_idx_right -= 1;
                    // reset to new array
                    self.current_array_right = self.chunks.get(self.chunk_idx_right).unwrap();
                    self.current_data_right = self.current_array_right.data();
                    self.current_array_idx_right = self.current_array_right.len();
                }
                ret
            }
        }
    };
}

/// Implement the `IntoIterator` to convert a given chunked array type into a `PolarsIterator`
/// with null checks.
///
/// # Input
///
/// ca_type: The chunked array for which the `IntoIterator` trait is implemented.
/// iter_item: The iterator `Item`, the type which is going to be returned by the iterator, wrapped
///     into and `Option`, as null check is performed.
/// single_chunk_ident: Identifier for the struct representing a single chunk without null
///     check iterator. Which returns `iter_item`.
/// single_chunk_null_ident: Identifier for the struct representing a single chunk with null
///     check iterator. Which returns `Option<iter_item>`.
/// many_chunk_ident: Identifier for the struct representing a many chunk without null
///     check iterator. Which returns `iter_item`.
/// many_chunk_null_ident: Identifier for the struct representing a many chunk with null
///     check iterator. Which returns `Option<iter_item>`.
macro_rules! impl_into_polars_iterator {
    ($ca_type:ident, $iter_item:ty, $single_chunk_ident:ident, $single_chunk_null_ident:ident, $many_chunk_ident:ident, $many_chunk_null_ident:ident) => {
        impl<'a> IntoIterator for &'a $ca_type {
            type Item = Option<$iter_item>;
            type IntoIter = Box<dyn PolarsIterator<Item = Self::Item> + 'a>;

            /// Decides which iterator fits best the current chunked array. The decision are based
            /// on the number of chunks and the existence of null values.
            fn into_iter(self) -> Self::IntoIter {
                match self.chunks.len() {
                    1 => {
                        if self.null_count() == 0 {
                            Box::new(SomeIterator($single_chunk_ident::new(self)))
                        } else {
                            Box::new($single_chunk_null_ident::new(self))
                        }
                    }
                    _ => {
                        if self.null_count() == 0 {
                            Box::new(SomeIterator($many_chunk_ident::new(self)))
                        } else {
                            Box::new($many_chunk_null_ident::new(self))
                        }
                    }
                }
            }
        }
    };
}

/// Implement the `IntoNoNullIterator` to convert a given chunked array type into a `PolarsIterator`
/// without null checks.
///
/// # Input
///
/// ca_type: The chunked array for which the `IntoNoNull` trait is implemented.
/// iter_item: The iterator `Item`, the type which is going to be returned by the iterator.
///     The return type is not wrapped into `Option` as the chunked array shall not have
///     null values.
/// single_chunk_ident: Identifier for the struct representing a single chunk without null
///     check iterator. Which returns `iter_item`.
/// many_chunk_ident: Identifier for the struct representing a many chunk without null
///     check iterator. Which returns `iter_item`.
macro_rules! impl_into_no_null_polars_iterator {
    ($ca_type:ident, $iter_item:ty, $single_chunk_ident:ident, $many_chunk_ident:ident) => {
        impl<'a> IntoNoNullIterator for &'a $ca_type {
            type Item = $iter_item;
            type IntoIter = Box<dyn PolarsIterator<Item = Self::Item> + 'a>;

            /// Decides which iterator fits best the current no null chunked array. The decision are based
            /// on the number of chunks.
            fn into_no_null_iter(self) -> Self::IntoIter {
                match self.chunks.len() {
                    1 => Box::new($single_chunk_ident::new(self)),
                    _ => Box::new($many_chunk_ident::new(self)),
                }
            }
        }
    };
}

/// Generates all the iterators and implements its traits. Also implement the `IntoIterator` and `IntoNoNullIterator`.
/// - SingleChunkIterator
/// - SingleChunkIteratorNullCheck
/// - ManyChunkIterator
/// - ManyChunkIteratorNullCheck
/// - IntoIterator
/// - IntoNoNullIterator
///
/// # Input
///
/// ca_type: The chunked array for which the iterators are implemented. The `IntoIterator` and `IntoNoNullIterator`
///     traits are going to be implemented for this chunked array.
/// arrow_array: The arrow type of the chunked array chunks.
/// single_chunk_ident: The name of the `SingleChunkIterator` to create.
/// single_chunk_null_ident: The name of the `SingleChunkIteratorNullCheck` iterator to create.
/// many_chunk_ident: The name of the `ManyChunkIterator` to create.
/// many_chunk_null_ident: The name of the `ManyChunkIteratorNullCheck` iterator to create.
/// iter_item: The iterator item. `NullCheck` iterators and `IntoIterator` will wrap this iter into an `Option`.
/// (Optional) return_function: The function to apply to the each value of the chunked array before returning
///     the value.
macro_rules! impl_all_iterators {
    ($ca_type:ident,
     $arrow_array:ident,
     $single_chunk_ident:ident,
     $single_chunk_null_ident:ident,
     $many_chunk_ident:ident,
     $many_chunk_null_ident:ident,
     $iter_item:ty
     $(, $return_function: ident )?
    ) => {
        // Generate single chunk iterator.
        impl_single_chunk_iterator!(
            $ca_type,
            $arrow_array,
            $single_chunk_ident,
            $iter_item
            $(, $return_function )? // Optional argument, only used if provided
        );

        // Generate single chunk iterator with null checks.
        impl_single_chunk_null_check_iterator!(
            $ca_type,
            $arrow_array,
            $single_chunk_null_ident,
            $iter_item
            $(, $return_function )? // Optional argument, only used if provided
        );

        // Generate many chunk iterator.
        impl_many_chunk_iterator!(
            $ca_type,
            $arrow_array,
            $many_chunk_ident,
            $iter_item
            $(, $return_function )? // Optional argument, only used if provided
        );

        // Generate many chunk iterator with null checks.
        impl_many_chunk_null_check_iterator!(
            $ca_type,
            $arrow_array,
            $many_chunk_null_ident,
            $iter_item
            $(, $return_function )? // Optional argument, only used if provided
        );

        // Generate into iterator function.
        impl_into_polars_iterator!(
            $ca_type,
            $iter_item,
            $single_chunk_ident,
            $single_chunk_null_ident,
            $many_chunk_ident,
            $many_chunk_null_ident
        );

        // Generate into no null iterator function.
        impl_into_no_null_polars_iterator!(
            $ca_type,
            $iter_item,
            $single_chunk_ident,
            $many_chunk_ident
        );
    }
}

impl_all_iterators!(
    Utf8Chunked,
    LargeStringArray,
    Utf8IterSingleChunk,
    Utf8IterSingleChunkNullCheck,
    Utf8IterManyChunk,
    Utf8IterManyChunkNullCheck,
    &'a str
);

// used for macro
fn return_from_list_iter(method_name: &str, v: ArrayRef) -> Series {
    let s = Series::try_from((method_name, v));
    s.unwrap()
}

impl_all_iterators!(
    ListChunked,
    LargeListArray,
    ListIterSingleChunk,
    ListIterSingleChunkNullCheck,
    ListIterManyChunk,
    ListIterManyChunkNullCheck,
    Series,
    return_from_list_iter
);

#[cfg(test)]
mod test {
    use crate::prelude::*;

    #[test]
    fn out_of_bounds() {
        let mut a = UInt32Chunked::new_from_slice("a", &[1, 2, 3]);
        let b = UInt32Chunked::new_from_slice("a", &[1, 2, 3]);
        a.append(&b);

        let v = a.into_iter().collect::<Vec<_>>();
        assert_eq!(
            vec![Some(1u32), Some(2), Some(3), Some(1), Some(2), Some(3)],
            v
        )
    }

    /// Generate test for `IntoIterator` trait for chunked arrays with just one chunk and no null values.
    /// The expected return value of the iterator generated by `IntoIterator` trait is `Option<T>`, where
    /// `T` is the chunked array type.
    ///
    /// # Input
    ///
    /// test_name: The name of the test to generate.
    /// ca_type: The chunked array to use for this test. Ex: `Utf8Chunked`, `UInt32Chunked` ...
    /// first_val: The first value contained in the chunked array.
    /// second_val: The second value contained in the chunked array.
    /// third_val: The third value contained in the chunked array.
    macro_rules! impl_test_iter_single_chunk {
        ($test_name:ident, $ca_type:ty, $first_val:expr, $second_val:expr, $third_val:expr) => {
            #[test]
            fn $test_name() {
                let a = <$ca_type>::new_from_slice("test", &[$first_val, $second_val, $third_val]);

                // normal iterator
                let mut it = a.into_iter();
                assert_eq!(it.next(), Some(Some($first_val)));
                assert_eq!(it.next(), Some(Some($second_val)));
                assert_eq!(it.next(), Some(Some($third_val)));
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // reverse iterator
                let mut it = a.into_iter();
                assert_eq!(it.next_back(), Some(Some($third_val)));
                assert_eq!(it.next_back(), Some(Some($second_val)));
                assert_eq!(it.next_back(), Some(Some($first_val)));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);

                // iterators should not cross
                let mut it = a.into_iter();
                assert_eq!(it.next_back(), Some(Some($third_val)));
                assert_eq!(it.next(), Some(Some($first_val)));
                assert_eq!(it.next(), Some(Some($second_val)));
                // should stop here as we took this one from the back
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // do the same from the right side
                let mut it = a.into_iter();
                assert_eq!(it.next(), Some(Some($first_val)));
                assert_eq!(it.next_back(), Some(Some($third_val)));
                assert_eq!(it.next_back(), Some(Some($second_val)));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);
            }
        };
    }

    impl_test_iter_single_chunk!(num_iter_single_chunk, UInt32Chunked, 1, 2, 3);
    impl_test_iter_single_chunk!(utf8_iter_single_chunk, Utf8Chunked, "a", "b", "c");
    impl_test_iter_single_chunk!(bool_iter_single_chunk, BooleanChunked, true, true, false);

    /// Generate test for `IntoIterator` trait for chunked arrays with just one chunk and null values.
    /// The expected return value of the iterator generated by `IntoIterator` trait is `Option<T>`, where
    /// `T` is the chunked array type.
    ///
    /// # Input
    ///
    /// test_name: The name of the test to generate.
    /// ca_type: The chunked array to use for this test. Ex: `Utf8Chunked`, `UInt32Chunked` ...
    /// first_val: The first value contained in the chunked array. Must be an `Option<T>`.
    /// second_val: The second value contained in the chunked array. Must be an `Option<T>`.
    /// third_val: The third value contained in the chunked array. Must be an `Option<T>`.
    macro_rules! impl_test_iter_single_chunk_null_check {
        ($test_name:ident, $ca_type:ty, $first_val:expr, $second_val:expr, $third_val:expr) => {
            #[test]
            fn $test_name() {
                let a =
                    <$ca_type>::new_from_opt_slice("test", &[$first_val, $second_val, $third_val]);

                // normal iterator
                let mut it = a.into_iter();
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next(), Some($second_val));
                assert_eq!(it.next(), Some($third_val));
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // reverse iterator
                let mut it = a.into_iter();
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next_back(), Some($second_val));
                assert_eq!(it.next_back(), Some($first_val));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);

                // iterators should not cross
                let mut it = a.into_iter();
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next(), Some($second_val));
                // should stop here as we took this one from the back
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // do the same from the right side
                let mut it = a.into_iter();
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next_back(), Some($second_val));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);
            }
        };
    }

    impl_test_iter_single_chunk_null_check!(
        num_iter_single_chunk_null_check,
        UInt32Chunked,
        Some(1),
        None,
        Some(3)
    );
    impl_test_iter_single_chunk_null_check!(
        utf8_iter_single_chunk_null_check,
        Utf8Chunked,
        Some("a"),
        None,
        Some("c")
    );
    impl_test_iter_single_chunk_null_check!(
        bool_iter_single_chunk_null_check,
        BooleanChunked,
        Some(true),
        None,
        Some(false)
    );

    /// Generate test for `IntoIterator` trait for chunked arrays with many chunks and no null values.
    /// The expected return value of the iterator generated by `IntoIterator` trait is `Option<T>`, where
    /// `T` is the chunked array type.
    ///
    /// # Input
    ///
    /// test_name: The name of the test to generate.
    /// ca_type: The chunked array to use for this test. Ex: `Utf8Chunked`, `UInt32Chunked` ...
    /// first_val: The first value contained in the chunked array.
    /// second_val: The second value contained in the chunked array.
    /// third_val: The third value contained in the chunked array.
    macro_rules! impl_test_iter_many_chunk {
        ($test_name:ident, $ca_type:ty, $first_val:expr, $second_val:expr, $third_val:expr) => {
            #[test]
            fn $test_name() {
                let mut a = <$ca_type>::new_from_slice("test", &[$first_val, $second_val]);
                let a_b = <$ca_type>::new_from_slice("", &[$third_val]);
                a.append(&a_b);

                // normal iterator
                let mut it = a.into_iter();
                assert_eq!(it.next(), Some(Some($first_val)));
                assert_eq!(it.next(), Some(Some($second_val)));
                assert_eq!(it.next(), Some(Some($third_val)));
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // reverse iterator
                let mut it = a.into_iter();
                assert_eq!(it.next_back(), Some(Some($third_val)));
                assert_eq!(it.next_back(), Some(Some($second_val)));
                assert_eq!(it.next_back(), Some(Some($first_val)));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);

                // iterators should not cross
                let mut it = a.into_iter();
                assert_eq!(it.next_back(), Some(Some($third_val)));
                assert_eq!(it.next(), Some(Some($first_val)));
                assert_eq!(it.next(), Some(Some($second_val)));
                // should stop here as we took this one from the back
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // do the same from the right side
                let mut it = a.into_iter();
                assert_eq!(it.next(), Some(Some($first_val)));
                assert_eq!(it.next_back(), Some(Some($third_val)));
                assert_eq!(it.next_back(), Some(Some($second_val)));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);
            }
        };
    }

    impl_test_iter_many_chunk!(num_iter_many_chunk, UInt32Chunked, 1, 2, 3);
    impl_test_iter_many_chunk!(utf8_iter_many_chunk, Utf8Chunked, "a", "b", "c");
    impl_test_iter_many_chunk!(bool_iter_many_chunk, BooleanChunked, true, true, false);

    /// Generate test for `IntoIterator` trait for chunked arrays with many chunk and null values.
    /// The expected return value of the iterator generated by `IntoIterator` trait is `Option<T>`, where
    /// `T` is the chunked array type.
    ///
    /// # Input
    ///
    /// test_name: The name of the test to generate.
    /// ca_type: The chunked array to use for this test. Ex: `Utf8Chunked`, `UInt32Chunked` ...
    /// first_val: The first value contained in the chunked array. Must be an `Option<T>`.
    /// second_val: The second value contained in the chunked array. Must be an `Option<T>`.
    /// third_val: The third value contained in the chunked array. Must be an `Option<T>`.
    macro_rules! impl_test_iter_many_chunk_null_check {
        ($test_name:ident, $ca_type:ty, $first_val:expr, $second_val:expr, $third_val:expr) => {
            #[test]
            fn $test_name() {
                let mut a = <$ca_type>::new_from_opt_slice("test", &[$first_val, $second_val]);
                let a_b = <$ca_type>::new_from_opt_slice("", &[$third_val]);
                a.append(&a_b);

                // normal iterator
                let mut it = a.into_iter();
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next(), Some($second_val));
                assert_eq!(it.next(), Some($third_val));
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // reverse iterator
                let mut it = a.into_iter();
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next_back(), Some($second_val));
                assert_eq!(it.next_back(), Some($first_val));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);

                // iterators should not cross
                let mut it = a.into_iter();
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next(), Some($second_val));
                // should stop here as we took this one from the back
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // do the same from the right side
                let mut it = a.into_iter();
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next_back(), Some($second_val));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);
            }
        };
    }

    impl_test_iter_many_chunk_null_check!(
        num_iter_many_chunk_null_check,
        UInt32Chunked,
        Some(1),
        None,
        Some(3)
    );
    impl_test_iter_many_chunk_null_check!(
        utf8_iter_many_chunk_null_check,
        Utf8Chunked,
        Some("a"),
        None,
        Some("c")
    );
    impl_test_iter_many_chunk_null_check!(
        bool_iter_many_chunk_null_check,
        BooleanChunked,
        Some(true),
        None,
        Some(false)
    );

    /// Generate test for `IntoNoNullIterator` trait for chunked arrays with just one chunk and no null values.
    /// The expected return value of the iterator generated by `IntoNoNullIterator` trait is `T`, where
    /// `T` is the chunked array type.
    ///
    /// # Input
    ///
    /// test_name: The name of the test to generate.
    /// ca_type: The chunked array to use for this test. Ex: `Utf8Chunked`, `UInt32Chunked` ...
    /// first_val: The first value contained in the chunked array.
    /// second_val: The second value contained in the chunked array.
    /// third_val: The third value contained in the chunked array.
    macro_rules! impl_test_no_null_iter_single_chunk {
        ($test_name:ident, $ca_type:ty, $first_val:expr, $second_val:expr, $third_val:expr) => {
            #[test]
            fn $test_name() {
                let a = <$ca_type>::new_from_slice("test", &[$first_val, $second_val, $third_val]);

                // normal iterator
                let mut it = a.into_no_null_iter();
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next(), Some($second_val));
                assert_eq!(it.next(), Some($third_val));
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // reverse iterator
                let mut it = a.into_no_null_iter();
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next_back(), Some($second_val));
                assert_eq!(it.next_back(), Some($first_val));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);

                // iterators should not cross
                let mut it = a.into_no_null_iter();
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next(), Some($second_val));
                // should stop here as we took this one from the back
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // do the same from the right side
                let mut it = a.into_no_null_iter();
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next_back(), Some($second_val));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);
            }
        };
    }

    impl_test_no_null_iter_single_chunk!(num_no_null_iter_single_chunk, UInt32Chunked, 1, 2, 3);
    impl_test_no_null_iter_single_chunk!(
        utf8_no_null_iter_single_chunk,
        Utf8Chunked,
        "a",
        "b",
        "c"
    );
    impl_test_no_null_iter_single_chunk!(
        bool_no_null_iter_single_chunk,
        BooleanChunked,
        true,
        true,
        false
    );

    /// Generate test for `IntoNoNullIterator` trait for chunked arrays with many chunks and no null values.
    /// The expected return value of the iterator generated by `IntoNoNullIterator` trait is `T`, where
    /// `T` is the chunked array type.
    ///
    /// # Input
    ///
    /// test_name: The name of the test to generate.
    /// ca_type: The chunked array to use for this test. Ex: `Utf8Chunked`, `UInt32Chunked` ...
    /// first_val: The first value contained in the chunked array.
    /// second_val: The second value contained in the chunked array.
    /// third_val: The third value contained in the chunked array.
    macro_rules! impl_test_no_null_iter_many_chunk {
        ($test_name:ident, $ca_type:ty, $first_val:expr, $second_val:expr, $third_val:expr) => {
            #[test]
            fn $test_name() {
                let mut a = <$ca_type>::new_from_slice("test", &[$first_val, $second_val]);
                let a_b = <$ca_type>::new_from_slice("", &[$third_val]);
                a.append(&a_b);

                // normal iterator
                let mut it = a.into_no_null_iter();
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next(), Some($second_val));
                assert_eq!(it.next(), Some($third_val));
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // reverse iterator
                let mut it = a.into_no_null_iter();
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next_back(), Some($second_val));
                assert_eq!(it.next_back(), Some($first_val));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);

                // iterators should not cross
                let mut it = a.into_no_null_iter();
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next(), Some($second_val));
                // should stop here as we took this one from the back
                assert_eq!(it.next(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next_back(), None);

                // do the same from the right side
                let mut it = a.into_no_null_iter();
                assert_eq!(it.next(), Some($first_val));
                assert_eq!(it.next_back(), Some($third_val));
                assert_eq!(it.next_back(), Some($second_val));
                assert_eq!(it.next_back(), None);
                // ensure both sides are consumes.
                assert_eq!(it.next(), None);
            }
        };
    }

    impl_test_no_null_iter_many_chunk!(num_no_null_iter_many_chunk, UInt32Chunked, 1, 2, 3);
    impl_test_no_null_iter_many_chunk!(utf8_no_null_iter_many_chunk, Utf8Chunked, "a", "b", "c");
    impl_test_no_null_iter_many_chunk!(
        bool_no_null_iter_many_chunk,
        BooleanChunked,
        true,
        true,
        false
    );

    /// The size of the skip iterator.
    const SKIP_ITERATOR_SIZE: usize = 10;

    /// Generates tests to verify the correctness of the `skip` method.
    ///
    /// # Input
    ///
    /// test_name: The name of the test to implement, it is a function name so it shall be unique.
    /// skip_values: The number of values to skip. Keep in mind that is the number of values to skip
    ///   after performing the first next, then, skip_values = 8, will skip until index 1 + skip_values = 9.
    /// first_val: The value before skip.
    /// second_val: The value after skip.
    /// ca_init_block: The block which initialize the chunked array. It shall return the chunked array.
    macro_rules! impl_test_iter_skip {
        ($test_name:ident, $skip_values:expr, $first_val:expr, $second_val:expr, $ca_init_block:block) => {
            #[test]
            fn $test_name() {
                let a = $ca_init_block;

                // Consume first position of iterator.
                let mut it = a.into_iter();
                assert_eq!(it.next(), Some($first_val));

                // Consume `$skip_values` and check the result.
                let mut it = it.skip($skip_values);
                assert_eq!(it.next(), Some($second_val));

                // Consume more values than available and check result is None.
                let mut it = it.skip(SKIP_ITERATOR_SIZE);
                assert_eq!(it.next(), None);
            }
        };
    }

    /// Generates a `Vec` of `Strings`, where every position is the `String` representation of its index.
    fn generate_utf8_vec(size: usize) -> Vec<String> {
        (0..size).map(|n| n.to_string()).collect()
    }

    /// Generate a `Vec` of `Option<String>`, where even indexes are `Some("{idx}")` and odd indexes are `None`.
    fn generate_opt_utf8_vec(size: usize) -> Vec<Option<String>> {
        (0..size)
            .map(|n| {
                if n % 2 == 0 {
                    Some(n.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    impl_test_iter_skip!(utf8_iter_single_chunk_skip, 8, Some("0"), Some("9"), {
        Utf8Chunked::new_from_slice("test", &generate_utf8_vec(SKIP_ITERATOR_SIZE))
    });

    impl_test_iter_skip!(
        utf8_iter_single_chunk_null_check_skip,
        8,
        Some("0"),
        None,
        { Utf8Chunked::new_from_opt_slice("test", &generate_opt_utf8_vec(SKIP_ITERATOR_SIZE)) }
    );

    impl_test_iter_skip!(utf8_iter_many_chunk_skip, 18, Some("0"), Some("9"), {
        let mut a = Utf8Chunked::new_from_slice("test", &generate_utf8_vec(SKIP_ITERATOR_SIZE));
        let a_b = Utf8Chunked::new_from_slice("test", &generate_utf8_vec(SKIP_ITERATOR_SIZE));
        a.append(&a_b);
        a
    });

    impl_test_iter_skip!(utf8_iter_many_chunk_null_check_skip, 18, Some("0"), None, {
        let mut a =
            Utf8Chunked::new_from_opt_slice("test", &generate_opt_utf8_vec(SKIP_ITERATOR_SIZE));
        let a_b =
            Utf8Chunked::new_from_opt_slice("test", &generate_opt_utf8_vec(SKIP_ITERATOR_SIZE));
        a.append(&a_b);
        a
    });

    /// Generates a `Vec` of `bool`, with even indexes are true, and odd indexes are false.
    fn generate_boolean_vec(size: usize) -> Vec<bool> {
        (0..size).map(|n| n % 2 == 0).collect()
    }

    /// Generate a `Vec` of `Option<bool>`, where:
    /// - If the index is divisible by 3, then, the value is `None`.
    /// - If the index is not divisible by 3 and it is even, then, the value is `Some(true)`.
    /// - Otherwise, the value is `Some(false)`.
    fn generate_opt_boolean_vec(size: usize) -> Vec<Option<bool>> {
        (0..size)
            .map(|n| if n % 3 == 0 { None } else { Some(n % 2 == 0) })
            .collect()
    }

    impl_test_iter_skip!(bool_iter_single_chunk_skip, 8, Some(true), Some(false), {
        BooleanChunked::new_from_slice("test", &generate_boolean_vec(SKIP_ITERATOR_SIZE))
    });

    impl_test_iter_skip!(bool_iter_single_chunk_null_check_skip, 8, None, None, {
        BooleanChunked::new_from_opt_slice("test", &generate_opt_boolean_vec(SKIP_ITERATOR_SIZE))
    });

    impl_test_iter_skip!(bool_iter_many_chunk_skip, 18, Some(true), Some(false), {
        let mut a =
            BooleanChunked::new_from_slice("test", &generate_boolean_vec(SKIP_ITERATOR_SIZE));
        let a_b = BooleanChunked::new_from_slice("test", &generate_boolean_vec(SKIP_ITERATOR_SIZE));
        a.append(&a_b);
        a
    });

    impl_test_iter_skip!(bool_iter_many_chunk_null_check_skip, 18, None, None, {
        let mut a = BooleanChunked::new_from_opt_slice(
            "test",
            &generate_opt_boolean_vec(SKIP_ITERATOR_SIZE),
        );
        let a_b = BooleanChunked::new_from_opt_slice(
            "test",
            &generate_opt_boolean_vec(SKIP_ITERATOR_SIZE),
        );
        a.append(&a_b);
        a
    });
}
