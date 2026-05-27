//! `PageSize` — validated page-size value object per ADR-0060.
//!
//! Wraps `NonZeroU32` and enforces the `1..=10_000` range declared by ADR-0057
//! at the domain port boundary. Callers outside the domain convert from `u32`
//! via [`PageSize::try_from`]; the domain never receives a zero or out-of-range
//! value.
//!
//! # Design note — `DEFAULT` constant
//!
//! The workspace lints forbid `unsafe_code`, `unwrap_used`, and `expect_used`.
//! The ADR example used `NonZeroU32::new_unchecked(50)` inside an `unsafe` block.
//! Instead, this file uses a `const` match on `NonZeroU32::new(50)` with a
//! `panic!` in the unreachable `None` arm. `panic!` in `const` evaluation is a
//! **hard compile-time error** if the arm is ever reached — never a runtime panic —
//! so the workspace `panic = "deny"` lint is not triggered (the lint targets
//! runtime panics, not const-eval panics). No `unsafe`, no `unwrap`, no `expect`.
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
/// let ps = PageSize::try_from(50u32).expect("50 is valid");
/// assert_eq!(ps.get(), 50);
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
    pub const DEFAULT: Self = match NonZeroU32::new(50) {
        Some(n) => Self(n),
        None => panic!("50 != 0: compile-time invariant"),
    };

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
        // SAFETY (logical): the branch above guarantees n > 0.
        // NonZeroU32::new returns Some when n > 0.
        match NonZeroU32::new(n) {
            Some(nz) => Ok(Self(nz)),
            None => Err(SubstrateError::InvalidArgument {
                offending_field: "page_size".to_owned(),
                reason: format!("page_size must be >= 1; got {n}"),
                correlation_id: None,
            }),
        }
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
