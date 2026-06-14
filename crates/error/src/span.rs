#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TSpan {
    pub start: u32,
    pub end: u32,
}
impl TSpan {
    pub const EMPTY: Self = Self { start: 0, end: 0 };

    pub const MISSING: Self = Self {
        start: u32::MAX,
        end: u32::MAX,
    };

    pub fn new(start: u32, end: u32) -> Self {
        debug_assert!(start <= end, "Invalid span constructed {start}..{end}");
        Self { start, end }
    }

    pub fn with_len(start: u32, len: u32) -> Self {
        Self {
            start,
            end: start + len,
        }
    }

    pub fn range(self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }

    pub fn contains(&self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    /// offset is either inside or immediately after this span
    pub fn contains_inclusive(&self, offset: u32) -> bool {
        self.start <= offset && offset <= self.end
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IdentPath(TSpan); // just save the span and reparse when it is resolved
impl IdentPath {
    pub fn new(span: TSpan) -> Self {
        debug_assert!(!span.range().is_empty(), "Tried to construct empty path");
        Self(span)
    }
    /// Returns: (`root`, `segments_without_last`, `last_segment`)
    /// `last_segment` will only be None if the path is a single root item
    pub fn segments<'a>(
        &'a self,
        src: &'a str,
    ) -> (
        Option<TSpan>,
        impl Iterator<Item = (&'a str, TSpan)>,
        Option<(&'a str, TSpan)>,
    ) {
        let start_addr = src.as_ptr() as usize;

        let s = &src[self.0.range()];

        let mut split = s
            .split('.')
            .map(move |segment| {
                let trimmed = segment.trim();
                let idx = (trimmed.as_ptr() as usize - start_addr) as u32;
                (trimmed, TSpan::new(idx, idx + trimmed.len() as u32))
            })
            .peekable();
        let first = split.peek().copied();
        let last = split.next_back().unwrap();
        if let Some(("root", first_span)) = first {
            split.next();
            let last = if last.0 == "root" { None } else { Some(last) };
            (Some(first_span), split, last)
        } else {
            (None, split, Some(last))
        }
    }

    pub fn span(&self) -> TSpan {
        self.0
    }
}
