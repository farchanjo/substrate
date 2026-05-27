//! `PageSize` — validated page-size value object per ADR-0060.
//!
//! Wraps `NonZeroU32` and enforces the `1..=10_000` range declared by ADR-0057
//! at the domain port boundary. Callers outside the domain convert from `u32`
//! via [`PageSize::try_from`]; the domain never receives a zero or out-of-range
//! value.
//!
//! # Design note — `DEFAULT` constant and `new_static`
//!
//! The workspace lints forbid `unsafe_code`, `unwrap_used`, and `expect_used`.
//! The ADR example used `NonZeroU32::new_unchecked(50)` inside an `unsafe` block.
//! Instead, this file uses a `const` match on `NonZeroU32::new(n)` with a
//! `panic!` in the unreachable `None` arm. `panic!` in `const` evaluation is a
//! **hard compile-time error** if the arm is ever reached — never a runtime panic.
//!
//! `new_static` carries `#[expect(clippy::panic, ...)]` because the workspace
//! lint `panic = "deny"` targets runtime panics in production paths. Here the
//! function is designed for compile-time use with literal constants; a zero
//! literal produces a compile-time error, not a runtime panic.
//!
//! References: ADR-0057, ADR-0060.

use std::num::NonZeroU32;

use crate::errors::SubstrateError;

/// Validated page size: 1..=10 000 (per ADR-0057 + ADR-0060).
///
/// # Examples
///
/// ```rust
/// use substrate_domain::value_objects::PageSize;
///
/// let ps = PageSize::try_from(50u32);
/// assert!(ps.is_ok());
///
/// assert!(PageSize::try_from(0u32).is_err());
/// assert!(PageSize::try_from(10_001u32).is_err());
///
/// assert_eq!(PageSize::default().get(), 50);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PageSize(NonZeroU32);

impl PageSize {
    /// Minimum valid page size.
    pub const MIN: u32 = 1;

    /// Maximum valid page size.
    pub const MAX: u32 = 10_000;

    /// Default page size (50), matching the handler-level default from ADR-0061.
    ///
    /// Constructed via a `const` match so neither `unsafe` nor `.unwrap()` is
    /// required. The `None` arm `panic!` is a compile-time error, never reached.
    pub const DEFAULT: Self = Self::new_static(50);

    /// Constructs a [`PageSize`] from a compile-time-known constant.
    ///
    /// Intended for use in `const` initializers where the argument is a
    /// literal. If `n == 0`, the compiler rejects the program at compile time
    /// rather than producing a runtime panic.
    ///
    /// For values known only at runtime, use [`PageSize::try_from`] instead.
    ///
    /// # Panics
    ///
    /// Panics at **compile time** (const-eval) when `n == 0`. This is
    /// intentional: a zero literal at a `const` call site is a hard compile
    /// error, not a runtime panic.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use substrate_domain::value_objects::PageSize;
    /// const MY_PAGE_SIZE: PageSize = PageSize::new_static(100);
    /// assert_eq!(MY_PAGE_SIZE.get(), 100);
    /// ```
    #[must_use]
    #[expect(
        clippy::panic,
        reason = "new_static is designed for const expressions with literal \
                  arguments; a zero literal is a compile-time error, never a \
                  runtime panic — the lint target for `panic = deny` is production \
                  runtime code, not const-eval branches"
    )]
    pub const fn new_static(n: u32) -> Self {
        match NonZeroU32::new(n) {
            Some(nz) => Self(nz),
            None => panic!("PageSize::new_static: n must be > 0"),
        }
    }

    /// Returns the page size as a plain `u32`.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl TryFrom<u32> for PageSize {
    type Error = SubstrateError;

    /// Converts `n` into a [`PageSize`].
    ///
    /// # Errors
    ///
    /// Returns [`SubstrateError::InvalidArgument`] when `n` is `0` or exceeds
    /// [`PageSize::MAX`].
    fn try_from(n: u32) -> Result<Self, Self::Error> {
        if n == 0 || n > Self::MAX {
            return Err(SubstrateError::InvalidArgument {
                offending_field: "page_size".to_owned(),
                reason: format!(
                    "page_size must be in [{}, {}]; got {}",
                    Self::MIN,
                    Self::MAX,
                    n
                ),
                correlation_id: None,
            });
        }
        // n > 0 is guaranteed by the guard above; NonZeroU32::new returns Some.
        NonZeroU32::new(n).map_or_else(
            || {
                Err(SubstrateError::InvalidArgument {
                    offending_field: "page_size".to_owned(),
                    reason: format!("page_size must be >= 1; got {n}"),
                    correlation_id: None,
                })
            },
            |nz| Ok(Self(nz)),
        )
    }
}

impl Default for PageSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "test code: idiomatic panicking assertions are intentional"
    )]

    use super::*;

    #[test]
    fn try_from_zero_is_err() {
        let result = PageSize::try_from(0_u32);
        assert!(result.is_err(), "page_size=0 must be rejected");
        let err = result.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
    }

    #[test]
    fn try_from_above_max_is_err() {
        let result = PageSize::try_from(10_001_u32);
        assert!(result.is_err(), "page_size=10001 must be rejected");
        let err = result.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
    }

    #[test]
    fn try_from_one_is_ok() {
        let ps = PageSize::try_from(1_u32);
        assert!(ps.is_ok(), "page_size=1 must be accepted");
        assert_eq!(ps.unwrap().get(), 1);
    }

    #[test]
    fn try_from_max_is_ok() {
        let ps = PageSize::try_from(10_000_u32);
        assert!(ps.is_ok(), "page_size=10000 must be accepted");
        assert_eq!(ps.unwrap().get(), 10_000);
    }

    #[test]
    fn default_is_fifty() {
        assert_eq!(
            PageSize::default().get(),
            50,
            "Default::default().get() must equal 50"
        );
    }

    #[test]
    fn default_constant_matches_default_impl() {
        assert_eq!(PageSize::DEFAULT.get(), PageSize::default().get());
    }

    #[test]
    fn try_from_mid_range_is_ok() {
        for n in [1_u32, 50, 100, 500, 9_999, 10_000] {
            let ps = PageSize::try_from(n);
            assert!(ps.is_ok(), "page_size={n} must be accepted");
            assert_eq!(ps.unwrap().get(), n);
        }
    }
}
