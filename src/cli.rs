use std::path::PathBuf;

use clap::Parser;
use serde::Serialize;

#[derive(Parser)]
#[command(name = "solana-amm-arb-cli")]
#[command(about = "Solana AMM Arbitrage CLI")]
pub struct Cli {
    #[arg(long, default_value = "https://api.mainnet-beta.solana.com")]
    pub rpc_url: String,

    #[arg(long, default_value = "keypair.json")]
    pub keypair: PathBuf,

    #[arg(long, default_value = "0.01")]
    pub amount_in: f64,

    #[arg(long, default_value = "100")]
    pub spread_threshold_bps: u32,

    #[arg(long, default_value = "500")]
    pub slippage_bps: u32,

    #[arg(long, default_value = "100000")]
    pub priority_fee: u64,

    #[arg(long, default_value = "true")]
    pub simulate_only: bool,

    #[arg(long, default_value = "4jgpwmuwaUrZgTvUjio8aBVNQJ6HcsF3YKAekpwwxTou")]
    pub pool_a: String,

    #[arg(long, default_value = "7JuwJuNU88gurFnyWeiyGKbFmExMWcmRZntn9imEzdny")]
    pub pool_b: String,
}

// Create arbitrage result structure
#[derive(Serialize)]
pub struct ArbitrageResult {
    // Execution metadata
    pub timestamp: String,
    pub execution_time_ms: u64,
    // CLI inputs
    pub cli_inputs: CliInputs,
    // Pool states
    pub pool_states: PoolStates,
    // Computed values
    pub computed_values: ComputedValues,
    // Decision and execution
    pub decision: ArbitrageDecision,
    // Transaction result (if executed)
    pub transaction_result: Option<TransactionResult>,
}

#[derive(Serialize)]
pub struct CliInputs {
    pub rpc_url: String,
    pub keypair: PathBuf,
    pub amount_in: f64,
    pub spread_threshold_bps: u32,
    pub slippage_bps: u32,
    pub priority_fee: u64,
    pub simulate_only: bool,
    pub pool_a: String,
    pub pool_b: String,
}

#[derive(Serialize)]
pub struct PoolStates {
    pub pool_a: PoolInfo,
    pub pool_b: PoolInfo,
}

#[derive(Serialize)]
pub struct PoolInfo {
    pub address: String,
    pub reserve0: u64,
    pub reserve1: u64,
    pub real_reserve0: u64,
    pub real_reserve1: u64,
    pub protocol_fees_token0: u64,
    pub fund_fees_token0: u64,
    pub protocol_fees_token1: u64,
    pub fund_fees_token1: u64,
    pub price: f64,
    pub fee_rate: u64,
    pub mint0: String,
    pub mint1: String,
    pub decimals0: u8,
    pub decimals1: u8,
}

#[derive(Serialize)]
pub struct ComputedValues {
    pub amount_out: f64,
    pub min_amount_out: f64,
    pub pnl: f64,
    pub spread_bps: f64,
    pub rent_cost: f64,
    pub priority_fee_cost: f64,
    pub total_fees: f64,
    pub gross_profit: f64,
    pub price_a: f64,
    pub price_b: f64,
    pub is_profitable: bool,
    pub meets_spread_threshold: bool,
    pub arbitrage_direction: String, // "A->B" or "B->A"
}

#[derive(Serialize)]
pub struct ArbitrageDecision {
    pub should_execute: bool,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct TransactionResult {
    pub success: bool,
    pub transaction_signature: Option<String>,
    pub simulation_result: Option<String>,
    pub error_message: Option<String>,
    pub compute_units_consumed: Option<u64>,
}
