use carbon_raydium_cpmm_decoder::accounts::{pool_state::PoolState, amm_config::AmmConfig};
use crate::cli::Calculations;

pub fn calculate_swap_out(amount_in: f64, reserve_in: f64, reserve_out: f64, fee_bps: u64) -> f64 {
    let fee_multiplier = 10000 - fee_bps;
    let numerator = amount_in * reserve_out * fee_multiplier as f64;
    let denominator = (reserve_in * 10000.0) + (amount_in * fee_multiplier as f64);
    numerator / denominator
}

pub fn calculate_pnl(
    amount_in: f64,
    expensive_pool: &PoolState, expensive_cfg: &AmmConfig, expensive_res0: u128, expensive_res1: u128,
    cheap_pool: &PoolState, cheap_cfg: &AmmConfig, cheap_res0: u128, cheap_res1: u128,
    missing_accounts_count: u32,
    token_account_rent: f64,
    priority_fee: f64
) -> Calculations {
    // Convert reserves to normalized values
    let exp_res0 = expensive_res0 as f64 / 10f64.powi(expensive_pool.mint0_decimals as i32);
    let exp_res1 = expensive_res1 as f64 / 10f64.powi(expensive_pool.mint1_decimals as i32);
    let cheap_res0 = cheap_res0 as f64 / 10f64.powi(cheap_pool.mint0_decimals as i32);
    let cheap_res1 = cheap_res1 as f64 / 10f64.powi(cheap_pool.mint1_decimals as i32);
    
    // Step 1: Buy token1 in cheap pool
    let token1_received = calculate_swap_out(amount_in, cheap_res0, cheap_res1, cheap_cfg.trade_fee_rate);
    
    // Step 2: Sell token1 in expensive pool
    let token0_received = calculate_swap_out(token1_received, exp_res1, exp_res0, expensive_cfg.trade_fee_rate);
    
    // Calculate costs breakdown
    let gross_profit = token0_received - amount_in;
    let rent_cost = token_account_rent * missing_accounts_count as f64;
    let total_costs = priority_fee + rent_cost;
    let net_pnl = gross_profit - total_costs;
    
    Calculations {
        amount_in,
        expected_out: token0_received,
        gross_profit,
        total_costs,
        net_pnl,
        rent_cost,
        min_out: 0.0, // Will be set later
    }
}
