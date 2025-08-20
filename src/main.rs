use anyhow::Result;
use carbon_raydium_cpmm_decoder::RaydiumCpmmDecoder;
use chrono::Utc;
use clap::Parser;
use log::{error, info, warn};
use solana_client::rpc_client::RpcClient;
use solana_amm_arb_cli::{
    arbitrage::{Arbitrage, calculate_min_out, calculate_pnl, calculate_price, spread_bps},
    cli::{
        ArbitrageDecision, ArbitrageResult, Cli, CliInputs, ComputedValues, PoolInfo, PoolStates,
        TransactionResult,
    },
    pool::{load_pool_data, normalize_pools},
    transaction::{create_arbitrage_transaction, simulate_transaction},
    utils::{count_missing_token_accounts, get_token_account_rent, load_keypair},
};
use solana_sdk::signer::Signer;
use std::time::Instant;

fn main() -> Result<()> {
    // Initialize logging
    env_logger::init();
    let start_time = Instant::now();

    info!("🚀 Starting Solana AMM Arbitrage CLI");

    let Cli {
        rpc_url,
        keypair: keypair_path,
        amount_in,
        spread_threshold_bps,
        slippage_bps,
        priority_fee,
        simulate_only,
        pool_a: pool_a_addr,
        pool_b: pool_b_addr,
    } = Cli::parse();

    info!("📋 Configuration loaded:");
    info!("  • RPC URL: {}", rpc_url);
    info!("  • Amount In: {} tokens", amount_in);
    info!("  • Spread Threshold: {} bps", spread_threshold_bps);
    info!("  • Slippage: {} bps", slippage_bps);
    info!("  • Priority Fee: {} micro-lamports", priority_fee);
    info!("  • Simulate Only: {}", simulate_only);
    info!("  • Pool A: {}", pool_a_addr);
    info!("  • Pool B: {}", pool_b_addr);

    let rpc = RpcClient::new(rpc_url.clone());
    let keypair = load_keypair(&keypair_path)?;
    info!("🔑 Keypair loaded: {}", keypair.pubkey());

    let token_account_rent = get_token_account_rent(&rpc)?;
    info!("💰 Token account rent: {} lamports", token_account_rent);

    let decoder = RaydiumCpmmDecoder;

    info!("🔄 Loading pool data...");
    let pool_a = load_pool_data(&rpc, &pool_a_addr, &decoder)?;
    let mut pool_b = load_pool_data(&rpc, &pool_b_addr, &decoder)?;
    normalize_pools(&pool_a, &mut pool_b)?;

    info!("✅ Pool data loaded and normalized");

    info!("📊 Pool A state:");
    info!("  • Reserve 0: {} ({})", pool_a.reserve0, pool_a.mint0);
    info!("  • Reserve 1: {} ({})", pool_a.reserve1, pool_a.mint1);
    info!("  • Real reserve 0: {} ({})", pool_a.real_reserve0, pool_a.mint0);
    info!("  • Real reserve 1: {} ({})", pool_a.real_reserve1, pool_a.mint1);
    info!("  • Protocol fees token 0: {} ({})", pool_a.protocol_fees_token0, pool_a.mint0);
    info!("  • Fund fees token 0: {} ({})", pool_a.fund_fees_token0, pool_a.mint0);
    info!("  • Protocol fees token 1: {} ({})", pool_a.protocol_fees_token1, pool_a.mint1);
    info!("  • Fund fees token 1: {} ({})", pool_a.fund_fees_token1, pool_a.mint1);
    info!(
        "  • Fee rate: {} ({}%)",
        pool_a.fee,
        pool_a.fee as f64 / 10000.0
    );

    info!("📊 Pool B state:");
    info!("  • Reserve 0: {} ({})", pool_b.reserve0, pool_b.mint0);
    info!("  • Reserve 1: {} ({})", pool_b.reserve1, pool_b.mint1);
    info!("  • Real reserve 0: {} ({})", pool_b.real_reserve0, pool_b.mint0);
    info!("  • Real reserve 1: {} ({})", pool_b.real_reserve1, pool_b.mint1);
    info!("  • Protocol fees token 0: {} ({})", pool_a.protocol_fees_token0, pool_a.mint0);
    info!("  • Fund fees token 0: {} ({})", pool_a.fund_fees_token0, pool_a.mint0);
    info!("  • Protocol fees token 1: {} ({})", pool_a.protocol_fees_token1, pool_a.mint1);
    info!("  • Fund fees token 1: {} ({})", pool_a.fund_fees_token1, pool_a.mint1);
    info!(
        "  • Fee rate: {} ({}%)",
        pool_b.fee,
        pool_b.fee as f64 / 10000.0
    );

    let price_a = calculate_price(
        pool_a.real_reserve0,
        pool_b.real_reserve1,
        pool_a.decimals0,
        pool_a.decimals1,
    );
    let price_b = calculate_price(
        pool_b.real_reserve0,
        pool_a.real_reserve1,
        pool_b.decimals0,
        pool_b.decimals1,
    );
    let spread = spread_bps(price_a, price_b);

    info!("💱 Price analysis:");
    info!("  • Pool A price: {:.6}", price_a);
    info!("  • Pool B price: {:.6}", price_b);
    info!("  • Spread: {:.2} bps", spread);

    let rent = count_missing_token_accounts(&rpc, &keypair.pubkey(), &pool_a.mint0, &pool_b.mint1)?
        as u64
        * token_account_rent;
    info!("💸 Transaction costs:");
    info!("  • Token account rent: {} lamports", rent);
    info!("  • Priority fee: {} micro-lamports", priority_fee);

    let arbitrage_direction = if price_a > price_b { "A->B" } else { "B->A" };
    info!("🔄 Arbitrage direction: {}", arbitrage_direction);

    let Arbitrage {
        amount_out, pnl, ..
    } = if price_a > price_b {
        calculate_pnl(amount_in, &pool_a, &pool_b, rent, priority_fee)
    } else {
        calculate_pnl(amount_in, &pool_b, &pool_a, rent, priority_fee)
    };

    let min_out = calculate_min_out(amount_out, slippage_bps);
    let amount_in_raw = (amount_in * 10_f64.powi(pool_a.decimals0 as i32)) as u64;
    let min_out_raw = (min_out * 10_f64.powi(pool_b.decimals0 as i32)) as u64;

    let rent_cost = rent as f64 / 1_000_000_000.0;
    let priority_fee_cost = priority_fee as f64 / 1_000_000_000_000.0; // Convert micro-lamports to SOL
    let total_fees = rent_cost + priority_fee_cost;
    let gross_profit = amount_out - amount_in;
    let is_profitable = pnl > 0.0;
    let meets_spread_threshold = spread.abs() >= spread_threshold_bps as f64;

    info!("📈 Arbitrage calculations:");
    info!("  • Amount in: {} tokens", amount_in);
    info!("  • Expected amount out: {} tokens", amount_out);
    info!("  • Minimum amount out (with slippage): {} tokens", min_out);
    info!("  • Gross profit: {} tokens", gross_profit);
    info!("  • Total fees: {} SOL", total_fees);
    info!("  • Net PnL: {} tokens", pnl);

    // Decision logic
    let mut reasons = Vec::new();
    let mut warnings = Vec::new();
    let should_execute = is_profitable && meets_spread_threshold;

    if !is_profitable {
        reasons.push("Not profitable after fees".to_string());
        warn!("❌ Trade not profitable: PnL = {} tokens", pnl);
    }

    if !meets_spread_threshold {
        reasons.push(format!(
            "Spread ({:.2} bps) below threshold ({} bps)",
            spread.abs(),
            spread_threshold_bps
        ));
        warn!(
            "❌ Spread too low: {:.2} bps < {} bps",
            spread.abs(),
            spread_threshold_bps
        );
    }

    if should_execute {
        info!("✅ Arbitrage opportunity detected!");
        reasons.push("Profitable arbitrage opportunity found".to_string());
    }

    if simulate_only {
        warnings.push("Running in simulation mode only".to_string());
        info!("⚠️  Simulation mode enabled - no actual trades will be executed");
    }

    // Transaction execution
    let mut transaction_result = None;

    if should_execute {
        info!("🔄 Creating arbitrage transaction...");
        let tx = if price_a > price_b {
            create_arbitrage_transaction(
                &rpc,
                &keypair,
                &pool_a,
                &pool_b,
                amount_in_raw,
                min_out_raw,
                priority_fee,
            )?
        } else {
            create_arbitrage_transaction(
                &rpc,
                &keypair,
                &pool_b,
                &pool_a,
                amount_in_raw,
                min_out_raw,
                priority_fee,
            )?
        };

        if simulate_only {
            info!("🧪 Simulating transaction...");
            match simulate_transaction(&rpc, &tx) {
                Ok(sim_result) => {
                    if sim_result.err.is_none() {
                        info!("✅ Transaction simulation successful");
                        transaction_result = Some(TransactionResult {
                            success: true,
                            transaction_signature: None,
                            simulation_result: Some(format!("{:?}", sim_result)),
                            error_message: None,
                            compute_units_consumed: sim_result.units_consumed,
                        });
                    } else {
                        error!("❌ Transaction simulation failed: {:?}", sim_result.err);
                        transaction_result = Some(TransactionResult {
                            success: false,
                            transaction_signature: None,
                            simulation_result: Some(format!("{:?}", sim_result)),
                            error_message: sim_result.err.map(|e| format!("{:?}", e)),
                            compute_units_consumed: sim_result.units_consumed,
                        });
                    }
                }
                Err(e) => {
                    error!("❌ Simulation error: {}", e);
                    transaction_result = Some(TransactionResult {
                        success: false,
                        transaction_signature: None,
                        simulation_result: None,
                        error_message: Some(e.to_string()),
                        compute_units_consumed: None,
                    });
                }
            }
        } else {
            info!("📡 Sending transaction to blockchain...");
            match rpc.send_transaction(&tx) {
                Ok(tx_hash) => {
                    info!("✅ Transaction sent successfully: {}", tx_hash);
                    transaction_result = Some(TransactionResult {
                        success: true,
                        transaction_signature: Some(tx_hash.to_string()),
                        simulation_result: None,
                        error_message: None,
                        compute_units_consumed: None,
                    });
                }
                Err(e) => {
                    error!("❌ Transaction failed: {}", e);
                    transaction_result = Some(TransactionResult {
                        success: false,
                        transaction_signature: None,
                        simulation_result: None,
                        error_message: Some(e.to_string()),
                        compute_units_consumed: None,
                    });
                }
            }
        }
    } else {
        info!("⏭️  Skipping transaction execution");
    }

    let execution_time = start_time.elapsed().as_millis() as u64;
    info!("⏱️  Total execution time: {} ms", execution_time);

    // Create comprehensive result
    let result = ArbitrageResult {
        timestamp: Utc::now().to_rfc3339(),
        execution_time_ms: execution_time,
        cli_inputs: CliInputs {
            rpc_url: rpc_url.clone(),
            keypair: keypair_path.clone(),
            amount_in,
            spread_threshold_bps,
            slippage_bps,
            priority_fee,
            simulate_only,
            pool_a: pool_a_addr.clone(),
            pool_b: pool_b_addr.clone(),
        },
        pool_states: PoolStates {
            pool_a: PoolInfo {
                address: pool_a_addr.clone(),
                reserve0: pool_a.reserve0,
                reserve1: pool_a.reserve1,
                real_reserve0: pool_a.real_reserve0,
                real_reserve1: pool_a.real_reserve1,
                protocol_fees_token0: pool_a.protocol_fees_token0,
                fund_fees_token0: pool_a.fund_fees_token0,
                protocol_fees_token1: pool_a.protocol_fees_token1,
                fund_fees_token1: pool_a.fund_fees_token1,
                price: price_a,
                fee_rate: pool_a.fee,
                mint0: pool_a.mint0.to_string(),
                mint1: pool_a.mint1.to_string(),
                decimals0: pool_a.decimals0,
                decimals1: pool_a.decimals1,
            },
            pool_b: PoolInfo {
                address: pool_b_addr.clone(),
                reserve0: pool_b.reserve0,
                reserve1: pool_b.reserve1,
                real_reserve0: pool_b.real_reserve0,
                real_reserve1: pool_b.real_reserve1,
                protocol_fees_token0: pool_b.protocol_fees_token0,
                fund_fees_token0: pool_b.fund_fees_token0,
                protocol_fees_token1: pool_b.protocol_fees_token1,
                fund_fees_token1: pool_b.fund_fees_token1,
                price: price_b,
                fee_rate: pool_b.fee,
                mint0: pool_b.mint0.to_string(),
                mint1: pool_b.mint1.to_string(),
                decimals0: pool_b.decimals0,
                decimals1: pool_b.decimals1,
            },
        },
        computed_values: ComputedValues {
            amount_out,
            min_amount_out: min_out,
            pnl,
            spread_bps: spread,
            rent_cost,
            priority_fee_cost,
            total_fees,
            gross_profit,
            price_a,
            price_b,
            is_profitable,
            meets_spread_threshold,
            arbitrage_direction: arbitrage_direction.to_string(),
        },
        decision: ArbitrageDecision {
            should_execute,
            reasons,
            warnings,
        },
        transaction_result,
    };

    // Convert to JSON and save to file
    let json_output = serde_json::to_string_pretty(&result)?;
    std::fs::write("arbitrage_result.json", &json_output)?;
    info!("📄 Detailed report saved to: arbitrage_result.json");

    // Print summary to console
    println!("\n📊 ARBITRAGE ANALYSIS SUMMARY");
    println!("═══════════════════════════════════════════════");
    println!("🕒 Timestamp: {}", result.timestamp);
    println!("⏱️  Execution Time: {} ms", result.execution_time_ms);
    println!("💱 Spread: {:.2} bps", result.computed_values.spread_bps);
    println!("📈 PnL: {:.6} tokens", result.computed_values.pnl);
    println!("✅ Profitable: {}", result.computed_values.is_profitable);
    println!(
        "🎯 Meets Threshold: {}",
        result.computed_values.meets_spread_threshold
    );
    println!(
        "🔄 Direction: {}",
        result.computed_values.arbitrage_direction
    );
    println!("⚡ Should Execute: {}", result.decision.should_execute);

    if let Some(tx_result) = &result.transaction_result {
        println!("📡 Transaction Success: {}", tx_result.success);
        if let Some(signature) = &tx_result.transaction_signature {
            println!("📝 Transaction Signature: {}", signature);
        }
    }

    println!("═══════════════════════════════════════════════");

    if result.decision.should_execute
        && result
            .transaction_result
            .as_ref()
            .map_or(false, |t| t.success)
    {
        info!("🎉 Arbitrage completed successfully!");
    } else if !result.decision.should_execute {
        info!("ℹ️  No arbitrage opportunity found");
    } else {
        warn!("⚠️  Arbitrage opportunity found but execution failed");
    }

    Ok(())
}
