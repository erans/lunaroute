//! Statistics calculations and helpers

/// Calculate estimated cost based on token usage
pub fn calculate_cost(
    input_tokens: i64,
    output_tokens: i64,
    thinking_tokens: i64,
    model: &str,
) -> f64 {
    // Pricing as of Jan 2025 (per million tokens)
    // Source: https://www.anthropic.com/pricing and https://openai.com/api/pricing/
    let (input_rate, output_rate, thinking_rate) = match model {
        // Anthropic Claude Models
        m if m.contains("claude-sonnet-4") => {
            (3.0 / 1_000_000.0, 15.0 / 1_000_000.0, 3.0 / 1_000_000.0)
        }
        m if m.contains("claude-3-5-sonnet") => {
            (3.0 / 1_000_000.0, 15.0 / 1_000_000.0, 3.0 / 1_000_000.0)
        }
        m if m.contains("claude-opus-4") => {
            (15.0 / 1_000_000.0, 75.0 / 1_000_000.0, 15.0 / 1_000_000.0)
        }
        m if m.contains("claude-opus") => {
            (15.0 / 1_000_000.0, 75.0 / 1_000_000.0, 15.0 / 1_000_000.0)
        }
        m if m.contains("claude-3-5-haiku") => (0.80 / 1_000_000.0, 4.0 / 1_000_000.0, 0.0),
        m if m.contains("claude-3-haiku") => (0.25 / 1_000_000.0, 1.25 / 1_000_000.0, 0.0),
        m if m.contains("claude-haiku") => (0.80 / 1_000_000.0, 4.0 / 1_000_000.0, 0.0), // Default to 3.5 pricing

        // OpenAI Models
        m if m.contains("gpt-4o-mini") => (0.15 / 1_000_000.0, 0.60 / 1_000_000.0, 0.0),
        m if m.contains("gpt-4o") => (2.50 / 1_000_000.0, 10.0 / 1_000_000.0, 0.0),
        m if m.contains("gpt-4-turbo") => (10.0 / 1_000_000.0, 30.0 / 1_000_000.0, 0.0),
        m if m.contains("gpt-4") => (30.0 / 1_000_000.0, 60.0 / 1_000_000.0, 0.0),
        m if m.contains("gpt-5") => (5.0 / 1_000_000.0, 15.0 / 1_000_000.0, 0.0),
        m if m.contains("o1-mini") => (3.0 / 1_000_000.0, 12.0 / 1_000_000.0, 0.0),
        m if m.contains("o1") => (15.0 / 1_000_000.0, 60.0 / 1_000_000.0, 0.0),

        // Default fallback (conservative estimate)
        _ => (1.0 / 1_000_000.0, 3.0 / 1_000_000.0, 0.0),
    };

    let input_cost = input_tokens as f64 * input_rate;
    let output_cost = output_tokens as f64 * output_rate;
    let thinking_cost = thinking_tokens as f64 * thinking_rate;

    input_cost + output_cost + thinking_cost
}

/// Project monthly cost based on daily average
pub fn project_monthly_cost(daily_costs: &[f64]) -> f64 {
    if daily_costs.is_empty() {
        return 0.0;
    }

    let avg_daily = daily_costs.iter().sum::<f64>() / daily_costs.len() as f64;
    avg_daily * 30.0 // Approximate month
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_3_5_haiku_pricing() {
        // Claude 3.5 Haiku: $0.80 input, $4.00 output per million
        let cost = calculate_cost(1_000_000, 1_000_000, 0, "claude-3-5-haiku-20241022");
        assert!((cost - 4.80).abs() < 0.01, "Expected ~$4.80, got ${}", cost);
    }

    #[test]
    fn test_claude_sonnet_4_5_pricing() {
        // Claude Sonnet 4.5: $3.00 input, $15.00 output, $3.00 thinking per million
        let cost = calculate_cost(
            1_000_000,
            1_000_000,
            1_000_000,
            "claude-sonnet-4-5-20250929",
        );
        assert!(
            (cost - 21.0).abs() < 0.01,
            "Expected ~$21.00, got ${}",
            cost
        );
    }

    #[test]
    fn test_gpt_4o_mini_pricing() {
        // GPT-4o-mini: $0.15 input, $0.60 output per million
        let cost = calculate_cost(1_000_000, 1_000_000, 0, "gpt-4o-mini-2024-07-18");
        assert!((cost - 0.75).abs() < 0.01, "Expected ~$0.75, got ${}", cost);
    }

    #[test]
    fn test_actual_haiku_usage() {
        // Actual usage from database: 8,634,555 input, 12,100 output
        let cost = calculate_cost(8_634_555, 12_100, 0, "claude-3-5-haiku-20241022");
        // Expected: 8.635 * $0.80 + 0.0121 * $4.00 = $6.908 + $0.048 = $6.956
        assert!(
            (cost - 6.956).abs() < 0.01,
            "Expected ~$6.96, got ${}",
            cost
        );
    }

    #[test]
    fn test_actual_sonnet_usage() {
        // Actual usage from database: 27,579 input, 171,256 output
        let cost = calculate_cost(27_579, 171_256, 0, "claude-sonnet-4-5-20250929");
        // Expected: 0.028 * $3.00 + 0.171 * $15.00 = $0.083 + $2.569 = $2.652
        assert!(
            (cost - 2.652).abs() < 0.01,
            "Expected ~$2.65, got ${}",
            cost
        );
    }

    #[test]
    fn test_claude_3_haiku_pricing() {
        // Claude 3 Haiku: $0.25 input, $1.25 output per million
        let cost = calculate_cost(1_000_000, 1_000_000, 0, "claude-3-haiku-20240307");
        assert!((cost - 1.50).abs() < 0.01, "Expected ~$1.50, got ${}", cost);
    }

    #[test]
    fn test_gpt_4o_pricing() {
        // GPT-4o: $2.50 input, $10.00 output per million
        let cost = calculate_cost(1_000_000, 1_000_000, 0, "gpt-4o");
        assert!(
            (cost - 12.50).abs() < 0.01,
            "Expected ~$12.50, got ${}",
            cost
        );
    }
}
