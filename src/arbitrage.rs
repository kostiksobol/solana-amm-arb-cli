use crate::pool::PoolData;

pub struct Arbitrage {
    pub amount_in: f64,
    pub amount_out: f64,
    pub pnl: f64,
}

pub fn calculate_pnl(
    amount_in: f64,
    expensive_pool: &PoolData,
    cheap_pool: &PoolData,
    rent: u64,
    priority_fee: u64,
) -> Arbitrage {
    // Convert rent and priority fee from lamports to SOL (assuming costs are in SOL)
    let total_fees = (rent + priority_fee / 1_000) as f64 / 1_000_000_000.0;

    // Convert amount_in to raw token units for precise calculation
    let amount_in_raw = (amount_in * 10_f64.powi(cheap_pool.decimals0 as i32)) as u64;

    // Step 1: Buy from cheap pool (convert token0 to token1)
    let amount_out_cheap = calculate_swap_output_raw(
        amount_in_raw,
        cheap_pool.real_reserve0,
        cheap_pool.real_reserve1,
        cheap_pool.fee,
    );

    // Step 2: Sell on expensive pool (convert token1 back to token0)
    let amount_out_expensive = calculate_swap_output_raw(
        amount_out_cheap,
        expensive_pool.real_reserve1,
        expensive_pool.real_reserve0,
        expensive_pool.fee,
    );

    // Convert back to human-readable units
    let final_amount = amount_out_expensive as f64 / 10_f64.powi(expensive_pool.decimals0 as i32);

    // Calculate gross profit (difference between final amount and initial amount)
    let gross_profit = final_amount - amount_in;

    // Calculate net PnL after transaction costs
    let net_pnl = gross_profit - total_fees;

    Arbitrage {
        amount_in,
        amount_out: final_amount,
        pnl: net_pnl,
    }
}

// Raw token calculation using exact Raydium math
pub fn calculate_swap_output_raw(
    amount_in: u64,
    reserve_in: u64,
    reserve_out: u64,
    trade_fee_rate: u64,
) -> u64 {
    // Calculate fee using exact Raydium method
    let den: u128 = 1_000_000;
    let fee: u128 = (amount_in as u128) * (trade_fee_rate as u128) / den;
    let net_in: u128 = (amount_in as u128) - fee;

    // Constant product formula with integer math
    let numerator = net_in * (reserve_out as u128);
    let denominator = (reserve_in as u128) + net_in;
    let amount_out = numerator / denominator;

    amount_out as u64
}

pub fn calculate_price(reserve0: u64, reserve1: u64, decimals0: u8, decimals1: u8) -> f64 {
    if reserve0 == 0 {
        return 0.0;
    }

    let r0 = reserve0 as f64 / 10f64.powi(decimals0 as i32);
    let r1 = reserve1 as f64 / 10f64.powi(decimals1 as i32);
    r1 / r0
}

pub fn spread_bps(price_a: f64, price_b: f64) -> f64 {
    let spread = (price_b - price_a) / price_a;
    (spread * 10000.0) as f64
}

pub fn calculate_min_out(amount_out: f64, slippage_bps: u32) -> f64 {
    let slippage_factor = 1.0 - (slippage_bps as f64 / 10000.0);
    amount_out * slippage_factor
}
