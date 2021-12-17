#[cfg(feature = "no_std")]
use alloc::collections::BTreeSet;
#[cfg(not(feature = "no_std"))]
use std::collections::BTreeSet;
use core::fmt;
use core::ops::Bound::{Excluded, Included};

use crate::span::{Span, SpanRelation};
use crate::Address;

/// A redzone based on a BTreeSet
///
/// This is a BTree based implementation. This means a few things:
/// - The perf scales entirely with the number of redzones tracked, not with
///   their size. This means you're free to create 64bit redzones with
///   impunity.
/// - This make pretty heavy use of a specific BTree query pattern to emulate
///   an interval tree. Let's say you want to see if any intervals overlap
///   with an 4 byte span starting at 0x4142, or [0x4142, 0x4146). First,
///   query the range [0..0x4146). This returns a sorted double ended iterator
///   over any interval below 0x4146. Because it's a double ended, it can be
///   reversed for free, and then only take elements from the new head that
///   fall into our target range.
///
///   This ends up looking like this:
///   ```
///   # use std::collections::BTreeMap;
///   # use std::ops::Bound::{Excluded, Included};
///   # use std::ops::Range;
///
///   let mut tree: BTreeMap<usize, Range<usize>> = BTreeMap::new();
///
///   // set up some dummy ranges
///   tree.insert(0x3131, 0x3131..0x3139);
///   tree.insert(0x4141, 0x4141..0x4149);
///   tree.insert(0x5151, 0x5151..0x5159);
///
///   // set up the query values
///   let target: usize = 0x4142;
///   let sz: usize = 4;
///
///   let val = tree
///       .range((Included(0), Excluded(target.saturating_add(sz))))
///       .rev()
///       .take_while(move |(_, ii)| target < ii.end)
///       .map(|(_, ii)| ii)
///       .cloned()
///       .next();
///
///   // see if our range came out
///   assert_eq!(val, Some(0x4141usize..0x4149));
///   ```
///
///   This example is with a BTreeMap, however the concept can also be applied
///   to BTreeSets if one extends ranges to be comparable, which is exactly
///   what this code does.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct MemoryTracker(BTreeSet<Span>);

impl Default for MemoryTracker {
    fn default() -> Self {
        Self(BTreeSet::new())
    }
}

impl fmt::Display for MemoryTracker {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "{{")?;
        for span in &self.0 {
            writeln!(f, "\t{}", span)?;
        }
        writeln!(f, "}}")
    }
}

impl MemoryTracker {
    /// New redzone span
    ///
    /// Takes a base address and size, and creates a redzone for it. If the
    /// new redzone overlaps with any existing redzones, they are merged.
    ///
    /// # Examples
    ///
    /// ```
    /// # use penumbra::Redzone;
    /// #
    /// let mut rz = Redzone::default();
    ///
    /// rz.red_span(0x4141, 8);
    ///
    /// assert!(rz.check(0x5151, 1).is_ok());
    /// assert!(rz.check(0x4144, 1).is_err());
    /// ```
    pub fn track_access(&mut self, a: Address, sz: usize) {
        let new = Span::with_len(a, sz);

        // we want to merge with adjacent spans, so we need to broaden the range
        // by 1 byte on each side to make us overlap
        let overlapped: Vec<Span> = self
            .lookup_range(a.saturating_sub(1), sz.saturating_add(1))
            .cloned()
            .collect();

        let mut start: Option<Address> = None;
        let mut end: Option<Address> = None;

        for span in overlapped {
            self.0.remove(&span);

            match new.relation(&span) {
                SpanRelation::Break => (),
                SpanRelation::Engulf => {
                    start = Some(span.start());
                    end = Some(span.end());
                }
                SpanRelation::AdjacentStart | SpanRelation::OverlapStart => {
                    start = Some(span.start())
                }
                SpanRelation::AdjacentEnd | SpanRelation::OverlapEnd => end = Some(span.end()),
                SpanRelation::None => panic!(format!(
                    "error merging span: requested merge of {} overlapping with {}",
                    new, span
                )),
            }
        }

        let new_start = start.unwrap_or_else(|| new.start());
        let new_end = end.unwrap_or_else(|| new.end());

        self.0.insert(Span::new(new_start, new_end));
    }

    /// Clear redzone span
    ///
    /// Clears the red from existing span. If the address and sz and size are
    /// not currently red, this is a no-op.
    ///
    /// # Examples
    ///
    /// ```
    /// # use penumbra::Redzone;
    /// #
    /// let mut rz = Redzone::default();
    ///
    /// rz.red_span(0x4141, 8);
    /// // redzone is from [0x4141..0x4149)
    ///
    /// assert!(rz.check(0x4141, 1).is_err());
    ///
    /// rz.clear_span(0x4141, 1);
    /// // redzone is from [0x4142..0x4149)
    ///
    /// assert!(rz.check(0x4141, 1).is_ok());
    /// assert!(rz.check(0x4142, 1).is_err());
    /// ```
    pub fn remove_access(&mut self, a: Address, sz: usize) {
        let overlap: Vec<Span> = self.lookup_range(a, sz).cloned().collect();

        let clear = Span::with_len(a, sz);

        for span in overlap {
            self.0.remove(&span);

            match clear.relation(&span) {
                SpanRelation::Break => (),
                SpanRelation::OverlapEnd => {
                    self.0.insert(Span::new(clear.end(), span.end()));
                }
                SpanRelation::OverlapStart => {
                    self.0.insert(Span::new(span.start(), clear.start()));
                }
                SpanRelation::Engulf => {
                    let a = Span::new(span.start(), clear.start());
                    let b = Span::new(clear.end(), span.end());

                    if a.len() > 0 {
                        self.0.insert(a);
                    }
                    if b.len() > 0 {
                        self.0.insert(b);
                    }
                }
                _ => panic!(format!(
                    "error clearing span: requested clear of {} overlapping with {}",
                    clear, span
                )),
            }
        }
    }

    /// Spans in the redzone
    ///
    /// The number of spans in the redzone. Note: due to merging and splitting
    /// this may be greater or less than the number the user inserted.
    ///
    /// # Examples
    ///
    /// ```
    /// # use penumbra::Redzone;
    /// #
    /// let mut rz = Redzone::default();
    ///
    /// rz.red_span(0x4141, 8);
    /// // redzone is from [0x4141..0x4149)
    ///
    /// assert_eq!(rz.len(), 1);
    ///
    /// rz.clear_span(0x4145, 2);
    /// // redzones are from:
    /// //   [0x4141..0x4145)
    /// //   [0x4147..0x4149)
    ///
    /// assert_eq!(rz.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if the Redzone is empty
    ///
    /// # Examples
    ///
    /// ```
    /// # use penumbra::Redzone;
    /// #
    /// let mut rz = Redzone::default();
    ///
    /// assert_eq!(rz.is_empty(), true);
    ///
    /// rz.red_span(0x4141, 8);
    ///
    /// assert_eq!(rz.is_empty(), false);
    /// ```
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Clear all redzones
    ///
    /// # Examples
    ///
    /// ```
    /// # use penumbra::Redzone;
    /// #
    /// let mut rz = Redzone::default();
    ///
    /// rz.red_span(0x4141, 8);
    ///
    /// assert!(rz.check(0x4144, 1).is_err());
    ///
    /// rz.clear();
    ///
    /// assert!(rz.check(0x4144, 1).is_ok());
    /// ```
    pub fn clear(&mut self) {
        self.0.clear()
    }

    /// Iterate over redzones
    ///
    /// Iterate over a tuples of (redzone, size), sorted by the redzone base
    ///
    /// # Examples
    ///
    ///
    /// ```
    /// # use penumbra::Redzone;
    /// #
    /// let mut rz = Redzone::default();
    ///
    /// rz.red_span(0x4141, 8);
    /// rz.red_span(0x5151, 8);
    ///
    /// let mut ii = rz.redzones();
    ///
    /// assert_eq!(ii.next(), Some((0x4141, 8)));
    /// assert_eq!(ii.next(), Some((0x5151, 8)));
    /// assert_eq!(ii.next(), None);
    /// ```
    pub fn redzones(&self) -> impl Iterator<Item = (Address, usize)> {
        self.0
            .iter()
            .map(|span| (span.start(), span.len()))
            .collect::<Vec<(Address, usize)>>()
            .into_iter()
    }

    /// Check Address
    ///
    /// Checks if any part of a given address and size overlap with an
    /// existing redzone.
    ///
    /// # Errors
    ///
    /// On redzone violation, `Err(fault)` is returned, where `fault` is the
    /// start address of the _last_ offending span.
    ///
    /// # Examples
    ///
    /// ```
    /// # use penumbra::Redzone;
    /// #
    /// let mut rz = Redzone::default();
    ///
    /// rz.red_span(0x4141, 8);
    ///
    /// assert!(rz.check(0x5151, 1).is_ok());
    /// assert!(rz.check(0x4144, 1).is_err());
    /// ```
    ///
    /// ```
    /// # use penumbra::Redzone;
    /// #
    /// let mut rz = Redzone::default();
    ///
    /// rz.red_span(0x4141, 8);
    /// rz.clear_span(0x4143, 4);
    ///
    /// assert_eq!(rz.check(0x4141, 8), Err(0x4147));
    /// ```
    pub fn check(&self, a: Address, sz: usize) -> Result<(), Address> {
        match self.lookup_range(a, sz).next() {
            None => Ok(()),
            Some(span) => Err(span.start()),
        }
    }

    fn lookup_range(&self, a: Address, sz: usize) -> impl Iterator<Item = &Span> {
        self.0
            .range((
                Included(Span::new(0, 0)),
                Excluded(Span::new(a.saturating_add(sz), 0)),
            ))
            .rev()
            .take_while(move |span| a < span.end())
    }
}
