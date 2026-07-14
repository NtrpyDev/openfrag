use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExactError {
    ZeroDenominator,
    Overflow,
    DivisionByZero,
    OutsideUnitInterval,
    OutsideRatingRange,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Rational {
    numerator: i64,
    denominator: i64,
}

impl Rational {
    pub const ZERO: Self = Self::from_integer(0);
    pub const ONE: Self = Self::from_integer(1);
    pub const HALF: Self = Self {
        numerator: 1,
        denominator: 2,
    };

    pub const fn from_integer(value: i64) -> Self {
        Self {
            numerator: value,
            denominator: 1,
        }
    }

    pub fn new(numerator: i64, denominator: i64) -> Result<Self, ExactError> {
        normalize(i128::from(numerator), i128::from(denominator))
    }

    pub const fn numerator(self) -> i64 {
        self.numerator
    }

    pub const fn denominator(self) -> i64 {
        self.denominator
    }

    pub fn checked_add(self, other: Self) -> Result<Self, ExactError> {
        let numerator = i128::from(self.numerator) * i128::from(other.denominator)
            + i128::from(other.numerator) * i128::from(self.denominator);
        let denominator = i128::from(self.denominator) * i128::from(other.denominator);
        normalize(numerator, denominator)
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, ExactError> {
        self.checked_add(other.checked_neg()?)
    }

    pub fn checked_mul(self, other: Self) -> Result<Self, ExactError> {
        normalize(
            i128::from(self.numerator) * i128::from(other.numerator),
            i128::from(self.denominator) * i128::from(other.denominator),
        )
    }

    pub fn checked_div(self, other: Self) -> Result<Self, ExactError> {
        if other.numerator == 0 {
            return Err(ExactError::DivisionByZero);
        }
        normalize(
            i128::from(self.numerator) * i128::from(other.denominator),
            i128::from(self.denominator) * i128::from(other.numerator),
        )
    }

    pub fn checked_neg(self) -> Result<Self, ExactError> {
        self.numerator
            .checked_neg()
            .map(|numerator| Self { numerator, ..self })
            .ok_or(ExactError::Overflow)
    }

    pub fn abs(self) -> Result<Self, ExactError> {
        if self.numerator < 0 {
            self.checked_neg()
        } else {
            Ok(self)
        }
    }

    pub fn clamp(self, minimum: Self, maximum: Self) -> Self {
        self.max(minimum).min(maximum)
    }

    pub fn round_half_up_nonnegative(self) -> Result<i64, ExactError> {
        if self.numerator < 0 {
            return Err(ExactError::OutsideRatingRange);
        }
        let numerator = i128::from(self.numerator);
        let denominator = i128::from(self.denominator);
        i64::try_from((numerator * 2 + denominator) / (denominator * 2))
            .map_err(|_| ExactError::Overflow)
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        (i128::from(self.numerator) * i128::from(other.denominator))
            .cmp(&(i128::from(other.numerator) * i128::from(self.denominator)))
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UnitInterval(Rational);

impl UnitInterval {
    pub fn new(value: Rational) -> Result<Self, ExactError> {
        if (Rational::ZERO..=Rational::ONE).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ExactError::OutsideUnitInterval)
        }
    }

    pub fn clamped(value: Rational) -> Self {
        Self(value.clamp(Rational::ZERO, Rational::ONE))
    }

    pub const fn exact(self) -> Rational {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RatingBasisPoints(u16);

impl RatingBasisPoints {
    pub const MAX: u16 = 20_000;

    pub fn new(value: u16) -> Result<Self, ExactError> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(ExactError::OutsideRatingRange)
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    pub const fn exact(self) -> Rational {
        Rational {
            numerator: self.0 as i64,
            denominator: 1,
        }
    }

    pub const fn display_centi(self) -> u16 {
        (self.0 + 50) / 100
    }
}

fn normalize(numerator: i128, denominator: i128) -> Result<Rational, ExactError> {
    if denominator == 0 {
        return Err(ExactError::ZeroDenominator);
    }
    let (numerator, denominator) = if denominator < 0 {
        (
            numerator.checked_neg().ok_or(ExactError::Overflow)?,
            denominator.checked_neg().ok_or(ExactError::Overflow)?,
        )
    } else {
        (numerator, denominator)
    };
    let divisor = gcd(numerator.unsigned_abs(), denominator.unsigned_abs());
    let divisor = i128::try_from(divisor).map_err(|_| ExactError::Overflow)?;
    Ok(Rational {
        numerator: i64::try_from(numerator / divisor).map_err(|_| ExactError::Overflow)?,
        denominator: i64::try_from(denominator / divisor).map_err(|_| ExactError::Overflow)?,
    })
}

const fn gcd(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 { 1 } else { left }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_values_reduce_and_order_without_floating_point() {
        let half = Rational::new(2, 4).unwrap();
        assert_eq!(half, Rational::HALF);
        assert!(Rational::new(1, 3).unwrap() < half);
        assert_eq!(half.checked_add(half).unwrap(), Rational::ONE);
    }

    #[test]
    fn rating_display_uses_the_contracts_half_up_rule() {
        assert_eq!(RatingBasisPoints::new(10_249).unwrap().display_centi(), 102);
        assert_eq!(RatingBasisPoints::new(10_250).unwrap().display_centi(), 103);
    }
}
