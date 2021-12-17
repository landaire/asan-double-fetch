use core::cmp::Ordering;
use core::fmt;
use core::ops::Range;

use crate::Address;

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct Span(Range<Address>);

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum SpanRelation {
    None,
    AdjacentStart,
    AdjacentEnd,
    OverlapStart,
    OverlapEnd,
    Engulf,
    Break,
}

impl Span {
    pub const fn new(start: Address, end: Address) -> Self {
        Self(start..end)
    }

    pub const fn with_len(start: Address, sz: usize) -> Self {
        Self(start..start.saturating_add(sz))
    }

    pub const fn start(&self) -> Address {
        self.0.start
    }

    pub const fn end(&self) -> Address {
        self.0.end
    }

    pub const fn len(&self) -> usize {
        self.end() - self.start()
    }

    pub fn relation(&self, other: &Self) -> SpanRelation {
        if other.start() <= self.start() && other.end() >= self.end() {
            // other span is engulfs redzone span
            SpanRelation::Engulf
        } else if other.end() == self.start() {
            // other span is adjacent to left edge
            SpanRelation::AdjacentStart
        } else if other.start() == self.end() {
            // other span is adjacent to right edge
            SpanRelation::AdjacentEnd
        } else if other.start() < self.start() && other.end() > self.start() {
            // other span overlaps left edge
            SpanRelation::OverlapStart
        } else if other.end() > self.end() && other.start() < self.end() {
            // other span overlaps right edge
            SpanRelation::OverlapEnd
        } else if other.start() > self.start() && other.end() < self.end() {
            // other span breaks an existing
            SpanRelation::Break
        } else {
            // span does not overlap
            SpanRelation::None
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "0x{:016x}..0x{:016x}", self.start(), self.end())
    }
}

impl PartialOrd for Span {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Span {
    fn cmp(&self, other: &Self) -> Ordering {
        self.start().cmp(&other.start())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_len() {
        let a = Span::new(0x4141, 0x4142);
        let b = Span::with_len(0x4141, 1);

        assert_eq!(a, b);
    }

    #[test]
    fn start() {
        let s = Span::new(0x4141, 0x4242);

        assert_eq!(s.start(), 0x4141);
    }

    #[test]
    fn end() {
        let s = Span::new(0x4141, 0x4242);

        assert_eq!(s.end(), 0x4242);
    }

    #[test]
    fn ord() {
        let a = Span::new(0x4040, 0x4141);
        let b = Span::new(0x4141, 0x4242);

        assert!(a < b);
        assert!(b > a);
    }

    #[test]
    fn len() {
        assert_eq!(Span::new(0x4141, 0x4242).len(), 0x4242 - 0x4141);
    }

    #[test]
    fn break_engulf() {
        let a = Span::new(0, 0xffff);
        let b = Span::new(0x4141, 0x4242);

        assert_eq!(a.relation(&b), SpanRelation::Break);
        assert_eq!(b.relation(&a), SpanRelation::Engulf);
    }

    #[test]
    fn adjacent() {
        let a = Span::new(0x4040, 0x4141);
        let b = Span::new(0x4141, 0x4242);

        assert_eq!(a.relation(&b), SpanRelation::AdjacentEnd);
        assert_eq!(b.relation(&a), SpanRelation::AdjacentStart);
    }

    #[test]
    fn overlap() {
        let a = Span::new(0x4040, 0x4141 + 1);
        let b = Span::new(0x4141 - 1, 0x4242);

        assert_eq!(a.relation(&b), SpanRelation::OverlapEnd);
        assert_eq!(b.relation(&a), SpanRelation::OverlapStart);
    }

    #[test]
    fn na() {
        let a = Span::new(0x4141, 0x4242);
        let b = Span::new(0x5151, 0x5252);

        assert_eq!(a.relation(&b), SpanRelation::None);
        assert_eq!(b.relation(&a), SpanRelation::None);
    }
}
