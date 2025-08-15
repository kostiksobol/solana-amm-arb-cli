use anyhow::Result;
use clap::Parser;
use solana_client::rpc_client::RpcClient;
use solana_sdk::signer::Signer;
use carbon_raydium_cpmm_decoder::RaydiumCpmmDecoder;

use solana_amm_arb_cli::{
    cli::{Cli, ArbitrageReport, PoolsData, PoolInfo, TransactionData, CliArgs},
    utils::{load_keypair, get_token_account_rent, count_missing_token_accounts, calculate_min_out},
    pool::{load_pool_data, normalize_pools, calculate_price},
    arbitrage::calculate_pnl,
    transaction::{create_arbitrage_transaction, simulate_transaction},
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Step 1: Initialize
    eprintln!("Step 1: Initializing RPC client and keypair");
    let rpc = RpcClient::new(cli.rpc_url.clone());
    let decoder = RaydiumCpmmDecoder;
    let keypair = load_keypair(&cli.keypair)?;
    let token_account_rent = get_token_account_rent(&rpc)?;
    let priority_fee = cli.priority_fee as f64 / 1_000_000_000.0; // Convert micro-lamports to SOL

    // Step 2: Load pool data
    eprintln!("Step 2: Loading pool data");
    let (pool_a, cfg_a, reserve_a0, reserve_a1) = load_pool_data(&rpc, &cli.pool_a, &decoder)?;
    let (pool_b, cfg_b, reserve_b0, reserve_b1) = load_pool_data(&rpc, &cli.pool_b, &decoder)?;

    // Step 3: Normalize pools
    eprintln!("Step 3: Normalizing pool data");
    let (pool_a_norm, cfg_a_norm, res_a0_norm, res_a1_norm, pool_b_norm, cfg_b_norm, res_b0_norm, res_b1_norm) = 
        normalize_pools(&pool_a, &cfg_a, reserve_a0, reserve_a1, &pool_b, &cfg_b, reserve_b0, reserve_b1)?;

    // Step 4: Calculate prices and spread
    eprintln!("Step 4: Calculating prices and spread");
    let price_a = calculate_price(res_a0_norm, res_a1_norm, pool_a_norm.mint0_decimals, pool_a_norm.mint1_decimals);
    let price_b = calculate_price(res_b0_norm, res_b1_norm, pool_b_norm.mint0_decimals, pool_b_norm.mint1_decimals);
    let spread_bps = ((price_a - price_b).abs() / price_a.min(price_b) * 10000.0) as u32;

    // Step 5: Check token accounts
    eprintln!("Step 5: Checking token accounts");
    let missing_accounts = count_missing_token_accounts(&rpc, &keypair.pubkey(), &pool_a_norm.token0_mint, &pool_a_norm.token1_mint)?;

    // Step 6: Calculate arbitrage
    eprintln!("Step 6: Calculating arbitrage opportunity");
    let mut calculations = if price_a > price_b {
        calculate_pnl(cli.amount_in, &pool_a_norm, &cfg_a_norm, res_a0_norm, res_a1_norm, 
                     &pool_b_norm, &cfg_b_norm, res_b0_norm, res_b1_norm, missing_accounts, token_account_rent, priority_fee)
    } else {
        calculate_pnl(cli.amount_in, &pool_b_norm, &cfg_b_norm, res_b0_norm, res_b1_norm,
                     &pool_a_norm, &cfg_a_norm, res_a0_norm, res_a1_norm, missing_accounts, token_account_rent, priority_fee)
    };

    calculations.min_out = calculate_min_out(calculations.expected_out, cli.slippage_bps);

    // Decision logic
    let (decision, reason) = if spread_bps < cli.spread_threshold_bps {
        ("SKIP".to_string(), format!("Spread {} bps below threshold {} bps", spread_bps, cli.spread_threshold_bps))
    } else if calculations.net_pnl <= 0.0 {
        ("SKIP".to_string(), format!("Not profitable: PnL {:.6}", calculations.net_pnl))
    } else {
        ("EXECUTE".to_string(), format!("Profitable: PnL {:.6}", calculations.net_pnl))
    };

    // Step 7: Create transaction if profitable or simulate_only
    let transaction_data = if decision == "EXECUTE" || cli.simulate_only {
        eprintln!("Step 7: Creating atomic arbitrage transaction");
        let amount_in_lamports = (cli.amount_in * 1_000_000_000.0) as u64;
        
        let transaction = create_arbitrage_transaction(
            &rpc,
            &keypair,
            amount_in_lamports,
            &pool_a_norm,
            &pool_b_norm,
            calculations.min_out,
            cli.priority_fee
        )?;

        let mut tx_data = TransactionData {
            signature: None,
            instructions_count: transaction.message.instructions.len(),
            size_bytes: transaction.message_data().len(),
            compute_units_used: None,
        };

        if cli.simulate_only {
            eprintln!("Step 8: Simulating transaction");
            if let Ok(response) = simulate_transaction(&rpc, &transaction).await {
                tx_data.compute_units_used = response.units_consumed;
            }
        } else {
            eprintln!("Step 8: Sending transaction");
            match rpc.send_and_confirm_transaction(&transaction) {
                Ok(signature) => {
                    tx_data.signature = Some(signature.to_string());
                    eprintln!("Transaction successful: {}", signature);
                }
                Err(e) => {
                    eprintln!("Transaction failed: {}", e);
                }
            }
        }

        Some(tx_data)
    } else {
        None
    };

    // Generate report
    let report = ArbitrageReport {
        cli_args: CliArgs {
            rpc_url: cli.rpc_url.clone(),
            keypair: cli.keypair.clone(),
            amount_in: cli.amount_in,
            spread_threshold_bps: cli.spread_threshold_bps,
            slippage_bps: cli.slippage_bps,
            priority_fee: cli.priority_fee,
            simulate_only: cli.simulate_only,
            pool_a: cli.pool_a.clone(),
            pool_b: cli.pool_b.clone(),
        },
        decision: decision.clone(),
        reason: reason.clone(),
        pools: PoolsData {
            pool_a: PoolInfo {
                address: cli.pool_a.clone(),
                token0: pool_a_norm.token0_mint.to_string(),
                token1: pool_a_norm.token1_mint.to_string(),
                price: price_a,
                fee_bps: cfg_a_norm.trade_fee_rate,
                reserve0: res_a0_norm as f64 / 10f64.powi(pool_a_norm.mint0_decimals as i32),
                reserve1: res_a1_norm as f64 / 10f64.powi(pool_a_norm.mint1_decimals as i32),
            },
            pool_b: PoolInfo {
                address: cli.pool_b.clone(),
                token0: pool_b_norm.token0_mint.to_string(),
                token1: pool_b_norm.token1_mint.to_string(),
                price: price_b,
                fee_bps: cfg_b_norm.trade_fee_rate,
                reserve0: res_b0_norm as f64 / 10f64.powi(pool_b_norm.mint0_decimals as i32),
                reserve1: res_b1_norm as f64 / 10f64.powi(pool_b_norm.mint1_decimals as i32),
            },
            spread_bps,
        },
        calculations,
        transaction: transaction_data,
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}