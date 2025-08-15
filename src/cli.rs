use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(name = "solana-amm-arb-cli")]
#[command(about = "Solana AMM Arbitrage CLI")]
pub struct Cli {
    #[arg(long, default_value = "https://api.mainnet-beta.solana.com")]
    pub rpc_url: String,
    
    #[arg(long, default_value = "keypair.json")]
    pub keypair: String,
    
    #[arg(long, default_value = "0.01")]
    pub amount_in: f64,
    
    #[arg(long, default_value = "100")]
    pub spread_threshold_bps: u32,
    
    #[arg(long, default_value = "500")]
    pub slippage_bps: u16,
    
    #[arg(long, default_value = "100")]
    pub priority_fee: u64,
    
    #[arg(long)]
    pub simulate_only: bool,
    
    #[arg(long, default_value = "4jgpwmuwaUrZgTvUjio8aBVNQJ6HcsF3YKAekpwwxTou")]
    pub pool_a: String,
    
    #[arg(long, default_value = "7JuwJuNU88gurFnyWeiyGKbFmExMWcmRZntn9imEzdny")]
    pub pool_b: String,
}

#[derive(Serialize, Deserialize)]
pub struct ArbitrageReport {
    pub cli_args: CliArgs,
    pub decision: String,
    pub reason: String,
    pub pools: PoolsData,
    pub calculations: Calculations,
    pub transaction: Option<TransactionData>,
}

#[derive(Serialize, Deserialize)]
pub struct CliArgs {
    pub rpc_url: String,
    pub keypair: String,
    pub amount_in: f64,
    pub spread_threshold_bps: u32,
    pub slippage_bps: u16,
    pub priority_fee: u64,
    pub simulate_only: bool,
    pub pool_a: String,
    pub pool_b: String,
}

#[derive(Serialize, Deserialize)]
pub struct PoolsData {
    pub pool_a: PoolInfo,
    pub pool_b: PoolInfo,
    pub spread_bps: u32,
}

#[derive(Serialize, Deserialize)]
pub struct PoolInfo {
    pub address: String,
    pub token0: String,
    pub token1: String,
    pub price: f64,
    pub fee_bps: u64,
    pub reserve0: f64,
    pub reserve1: f64,
}

#[derive(Serialize, Deserialize)]
pub struct Calculations {
    pub amount_in: f64,
    pub expected_out: f64,
    pub gross_profit: f64,
    pub total_costs: f64,
    pub net_pnl: f64,
    pub rent_cost: f64,
    pub min_out: f64,
}

#[derive(Serialize, Deserialize)]
pub struct TransactionData {
    pub signature: Option<String>,
    pub instructions_count: usize,
    pub size_bytes: usize,
    pub compute_units_used: Option<u64>,
}
