use solana_sdk::pubkey::Pubkey;

use crate::pool::PoolValues;

const ESTIMATED_COMPUTE_UNITS: u64 = 100_000;
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
const MICRO_LAMPORTS_PER_LAMPORTS: u64 = 1_000_000;
pub const SOL_MINT: &str = "So11111111111111111111111111111111111111112";
const UNITS_PER_TRADE_FEE_RATE: u128 = 1_000_000;

pub struct Arbitrage {
    pub amount_in: f64,
    pub amount_in_raw: u64,
    pub amount_out_1: f64,
    pub amount_out_1_raw: u64,
    pub amount_out_2: f64,
    pub amount_out_2_raw: u64,
    pub gross_profit: f64,
    pub gross_profit_raw: i64,
    pub total_fees: f64,
    pub total_fees_raw: u64,
    pub rent: f64,
    pub rent_raw: u64,
    pub pnl: Option<f64>,
}

pub fn calculate_pnl(
    amount_in: f64,
    pool_in: &PoolValues,
    pool_out: &PoolValues,
    rent_raw: u64,
    priority_fee: u64,
) -> Arbitrage {
    let total_fees_raw = rent_raw + priority_fee * ESTIMATED_COMPUTE_UNITS / MICRO_LAMPORTS_PER_LAMPORTS;
    let amount_in_raw = (amount_in * 10_f64.powi(pool_in.token0_decimals as i32)) as u64;

    let amount_out_raw_1 = calculate_swap_output_raw(
        amount_in_raw,
        pool_in.reserve0,
        pool_in.reserve1,
        pool_in.trade_fee_rate,
    );

    let amount_out_raw_2 = calculate_swap_output_raw(
        amount_out_raw_1,
        pool_out.reserve1,
        pool_out.reserve0,
        pool_out.trade_fee_rate,
    );

    let gross_profit_raw = (amount_out_raw_2 as i128 - amount_in_raw as i128) as i64;

    let total_fees = total_fees_raw as f64 / LAMPORTS_PER_SOL as f64;
    let amount_out_1 = amount_out_raw_1 as f64 / 10_f64.powi(pool_in.token1_decimals as i32);
    let amount_out_2 = amount_out_raw_2 as f64 / 10_f64.powi(pool_out.token0_decimals as i32);
    let gross_profit = gross_profit_raw as f64 / 10_f64.powi(pool_out.token0_decimals as i32);
    let rent = rent_raw as f64 / LAMPORTS_PER_SOL as f64;

    let mut pnl = None;

    let sol_mint = SOL_MINT.parse::<Pubkey>().unwrap();
    if pool_out.mint0 == sol_mint {
        pnl = Some(gross_profit - total_fees);
    }

    Arbitrage {
        amount_in,
        amount_in_raw,
        amount_out_1,
        amount_out_1_raw: amount_out_raw_1,
        amount_out_2,
        amount_out_2_raw: amount_out_raw_2,
        gross_profit,
        gross_profit_raw,
        total_fees,
        total_fees_raw,
        rent,
        rent_raw,
        pnl,
    }
}

// Raw token calculation using exact Raydium math
pub fn calculate_swap_output_raw(
    amount_in: u64,
    reserve_in: u64,
    reserve_out: u64,
    trade_fee_rate: u64,
) -> u64 {
    let fees: u128 = (amount_in as u128) * (trade_fee_rate as u128) / UNITS_PER_TRADE_FEE_RATE;
    let net_in: u128 = (amount_in as u128) - fees;

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

pub fn calculate_min_out(amount_out: u64, slippage_bps: u32) -> u64 {
    let slippage_factor = 1.0 - (slippage_bps as f64 / 10000.0);
    (amount_out as f64 * slippage_factor) as u64
}
