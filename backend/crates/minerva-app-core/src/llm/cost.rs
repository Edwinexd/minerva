//! USD cost computation for LLM calls.
//!
//! Money is `rust_decimal::Decimal` end-to-end (NUMERIC in Postgres);
//! never `f64`. Rates are USD per 1,000,000 tokens, admin-entered per
//! model in the `chat_models` catalog. A rate of `0` is a valid "free"
//! (on-prem) price; an *unknown* rate is represented upstream as `None`
//! (the call must not be billed or run), never as a silent `0` here.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Cost in USD of a call that consumed `prompt` input tokens and
/// `completion` output tokens, given the per-million-token input and
/// output rates.
pub fn cost_usd(prompt: i64, completion: i64, in_rate: Decimal, out_rate: Decimal) -> Decimal {
    (Decimal::from(prompt) * in_rate + Decimal::from(completion) * out_rate) / dec!(1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_matches_hand_computed_value() {
        // 1000 input @ $0.35/Mtok + 500 output @ $0.75/Mtok
        //   = 0.00035 + 0.000375 = 0.000725
        let c = cost_usd(1000, 500, dec!(0.35), dec!(0.75));
        assert_eq!(c, dec!(0.000725));
    }

    #[test]
    fn zero_rate_is_free_not_an_error() {
        // On-prem model priced at $0: a real, billable-at-zero cost.
        assert_eq!(cost_usd(1_000_000, 1_000_000, dec!(0), dec!(0)), dec!(0));
    }

    #[test]
    fn large_counts_do_not_lose_precision() {
        // 10M input @ $0.35 = $3.50 exactly (no float drift).
        assert_eq!(cost_usd(10_000_000, 0, dec!(0.35), dec!(0)), dec!(3.5));
    }
}
