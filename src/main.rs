use anyhow::{Context, Result};
use carbon_raydium_cpmm_decoder::RaydiumCpmmDecoder;
use chrono::Utc;
use clap::Parser;
use log::{error, info, warn};
use serde_json::json;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{pubkey::Pubkey, signer::Signer};
use spl_associated_token_account::get_associated_token_address;
use std::fs;
use std::time::Instant;

use solana_amm_arb_cli::{
    arbitrage::{calculate_min_out, calculate_pnl, calculate_price, spread_bps},
    cli::{
        config_set_amount_in, config_set_keypair, config_set_pools, config_set_priority_fee,
        config_set_rpc, config_set_simulate, config_set_slippage_bps,
        config_set_spread_threshold_bps, default_state, load_state, save_state, state_file_path,
        take_or_panic, Cli, Command, ConfigCmd,
    },
    pool::PoolData,
    transaction::{create_arbitrage_transaction, simulate_transaction},
    utils::{get_missing_token_account, get_token_account_rent, load_keypair},
};

macro_rules! step {
    ($steps:expr, $($arg:tt)*) => {{
        let s = format!($($arg)*);
        $steps.push(s.clone());
        info!("{}", s);
    }};
}

fn pk_s(p: &Pubkey) -> String {
    p.to_string()
}

fn main() -> Result<()> {
    // Initialize logging
    env_logger::init();
    let start_time = Instant::now();
    info!("════════════════════════════════════════════");
    info!("🚀 Starting solana-cpmm-arb-cli");
    info!("════════════════════════════════════════════");

    let cli = Cli::parse();

    // Ensure state dir exists
    let state_path = state_file_path()?;
    fs::create_dir_all(state_path.parent().unwrap())
        .with_context(|| format!("create state dir: {}", state_path.display()))?;

    // Load or initialize defaults
    let mut state = load_state(&state_path).unwrap_or_else(|_| default_state());

    // --- Subcommands (config) ---
    if let Some(Command::Config { cmd }) = cli.cmd {
        match cmd {
            ConfigCmd::Show => {
                println!("{}", serde_json::to_string_pretty(&state)?);
            }
            ConfigCmd::ResetDefaults => {
                state = default_state();
                save_state(&state_path, &state)?;
                println!(
                    "State reset to defaults and saved to {}",
                    state_path.display()
                );
            }
            ConfigCmd::SetPools => config_set_pools(&state_path, &mut state)?,
            ConfigCmd::SetRpcUrl => config_set_rpc(&state_path, &mut state)?,
            ConfigCmd::SetKeypair => config_set_keypair(&state_path, &mut state)?,
            ConfigCmd::SetAmountIn => config_set_amount_in(&state_path, &mut state)?,
            ConfigCmd::SetSpreadThresholdBps => {
                config_set_spread_threshold_bps(&state_path, &mut state)?
            }
            ConfigCmd::SetSlippageBps => config_set_slippage_bps(&state_path, &mut state)?,
            ConfigCmd::SetPriorityFee => config_set_priority_fee(&state_path, &mut state)?,
            ConfigCmd::SetSimulate => config_set_simulate(&state_path, &mut state)?,
        }
        return Ok(());
    }

    // --- Resolve runtime params from flags OR state ---
    let rpc_url = take_or_panic(cli.rpc_url, state.rpc_url.clone(), "rpc-url");
    let keypair_path = take_or_panic(cli.keypair, state.keypair_path.clone(), "keypair");
    let amount_in = take_or_panic(cli.amount_in, state.amount_in, "amount-in");
    let spread_threshold_bps = take_or_panic(
        cli.spread_threshold_bps,
        state.spread_threshold_bps,
        "spread-threshold-bps",
    );
    let slippage_bps = take_or_panic(cli.slippage_bps, state.slippage_bps, "slippage-bps");
    let priority_fee_microlamports =
        take_or_panic(cli.priority_fee, state.priority_fee_microlamports, "priority-fee");
    let simulate_only = take_or_panic(cli.simulate_only, state.simulate_only, "simulate-only");

    info!("📋 CONFIG");
    info!("  • RPC URL: {}", rpc_url);
    info!("  • Keypair: {:?}", keypair_path);
    info!("  • Amount In: {}", amount_in);
    info!("  • Spread Threshold: {} bps", spread_threshold_bps);
    info!("  • Slippage: {} bps", slippage_bps);
    info!("  • Priority Fee: {} µlamports", priority_fee_microlamports);
    info!("  • Simulate Only: {}", simulate_only);

    let rpc = RpcClient::new(rpc_url.clone());
    let keypair = load_keypair(&keypair_path)?;
    info!("🔑 Keypair loaded: {}", keypair.pubkey());

    // Mints + pools from state
    let mint_in = state
        .mint_in
        .as_ref()
        .expect("mint-in is required")
        .parse::<Pubkey>()?;
    let mint_out = state
        .mint_out
        .as_ref()
        .expect("mint-out is required")
        .parse::<Pubkey>()?;

    let pool_a_addr = state.pool_a.take().expect("pool-a is required");
    let pool_b_addr = state.pool_b.take().expect("pool-b is required");

    info!("🔄 Loading pools…");
    let decoder = RaydiumCpmmDecoder;
    let pool_a = PoolData::new(&rpc, &pool_a_addr, &decoder)?;
    let pool_b = PoolData::new(&rpc, &pool_b_addr, &decoder)?;

    // Raw values, then normalized so that token0 == mint_in for BOTH pools
    let mut pool_a_values = pool_a.get_values(&rpc)?;
    let mut pool_b_values = pool_b.get_values(&rpc)?;
    pool_a_values.normalize_pool_values(&mint_in);
    pool_b_values.normalize_pool_values(&mint_in);

    // ---------- Detailed Pool Logging ----------
    info!("──────── Pool A [{}] ────────", pool_a_addr);
    info!("  • token0 (mint_in): {}", pk_s(&pool_a_values.mint0));
    info!("    - decimals: {}", pool_a_values.token0_decimals);
    info!("    - reserve0: {}", pool_a_values.reserve0);
    info!("    - vault_amount0: {}", pool_a_values.vault_amount0);
    info!("    - protocol_fees_token0: {}", pool_a_values.protocol_fees_token0);
    info!("    - fund_fees_token0: {}", pool_a_values.fund_fees_token0);
    info!("  • token1 (mint_out): {}", pk_s(&pool_a_values.mint1));
    info!("    - decimals: {}", pool_a_values.token1_decimals);
    info!("    - reserve1: {}", pool_a_values.reserve1);
    info!("    - vault_amount1: {}", pool_a_values.vault_amount1);
    info!("    - protocol_fees_token1: {}", pool_a_values.protocol_fees_token1);
    info!("    - fund_fees_token1: {}", pool_a_values.fund_fees_token1);
    info!("  • trade_fee_rate (ppm/bps-style raw): {}", pool_a_values.trade_fee_rate);

    info!("──────── Pool B [{}] ────────", pool_b_addr);
    info!("  • token0 (mint_in): {}", pk_s(&pool_b_values.mint0));
    info!("    - decimals: {}", pool_b_values.token0_decimals);
    info!("    - reserve0: {}", pool_b_values.reserve0);
    info!("    - vault_amount0: {}", pool_b_values.vault_amount0);
    info!("    - protocol_fees_token0: {}", pool_b_values.protocol_fees_token0);
    info!("    - fund_fees_token0: {}", pool_b_values.fund_fees_token0);
    info!("  • token1 (mint_out): {}", pk_s(&pool_b_values.mint1));
    info!("    - decimals: {}", pool_b_values.token1_decimals);
    info!("    - reserve1: {}", pool_b_values.reserve1);
    info!("    - vault_amount1: {}", pool_b_values.vault_amount1);
    info!("    - protocol_fees_token1: {}", pool_b_values.protocol_fees_token1);
    info!("    - fund_fees_token1: {}", pool_b_values.fund_fees_token1);
    info!("  • trade_fee_rate (ppm/bps-style raw): {}", pool_b_values.trade_fee_rate);

    // Prices: both pools are oriented as mint_in -> mint_out (0 -> 1)
    let price_a = calculate_price(
        pool_a_values.reserve0,
        pool_a_values.reserve1,
        pool_a_values.token0_decimals,
        pool_a_values.token1_decimals,
    );
    let price_b = calculate_price(
        pool_b_values.reserve0,
        pool_b_values.reserve1,
        pool_b_values.token0_decimals,
        pool_b_values.token1_decimals,
    );
    let spread_bps_val = spread_bps(price_a, price_b);

    info!("──────── Prices (0→1 i.e., {} → {}) ────────", pk_s(&mint_in), pk_s(&mint_out));
    info!("  • Pool A price: {:.12}", price_a);
    info!("  • Pool B price: {:.12}", price_b);
    info!("  • Spread: {:.4} bps", spread_bps_val);

    let mut steps: Vec<String> = Vec::new();
    step!(steps, "Pools: A={}  B={}", pool_a_addr, pool_b_addr);
    step!(steps, "Mints: mint_in={}  mint_out={}", pk_s(&mint_in), pk_s(&mint_out));
    step!(steps, "Prices: A={:.12}  B={:.12}  spread_bps={:.4}", price_a, price_b, spread_bps_val);

    // ---------- Token accounts & rent ----------
    let ata_in_addr = get_associated_token_address(&keypair.pubkey(), &mint_in);
    let ata_out_addr = get_associated_token_address(&keypair.pubkey(), &mint_out);
    let atas = vec![
        get_missing_token_account(&rpc, &keypair.pubkey(), &mint_in),
        get_missing_token_account(&rpc, &keypair.pubkey(), &mint_out),
    ];
    let rent_per_ata = get_token_account_rent(&rpc)?;
    let rent_raw = (!atas[0].exists as u64 + !atas[1].exists as u64) * rent_per_ata;

    info!("──────── Token Accounts ────────");
    info!("  • Owner: {}", keypair.pubkey());
    info!(
        "  • {} → ATA: {}  (exists: {})",
        pk_s(&mint_in),
        ata_in_addr,
        atas[0].exists
    );
    info!(
        "  • {} → ATA: {}  (exists: {})",
        pk_s(&mint_out),
        ata_out_addr,
        atas[1].exists
    );
    info!("  • Rent per ATA: {} lamports", rent_per_ata);
    info!(
        "  • Rent to be paid now (if creating): {} lamports",
        rent_raw
    );
    step!(
        steps,
        "ATAs: in={} (exists={}), out={} (exists={}), rent_per_ata={}, rent_raw={}",
        ata_in_addr,
        atas[0].exists,
        ata_out_addr,
        atas[1].exists,
        rent_per_ata,
        rent_raw
    );

    // ---------- PnL both directions ----------
    let arb_a_b = calculate_pnl(
        amount_in,
        &pool_a_values,
        &pool_b_values,
        rent_raw,
        priority_fee_microlamports,
    );
    let arb_b_a = calculate_pnl(
        amount_in,
        &pool_b_values,
        &pool_a_values,
        rent_raw,
        priority_fee_microlamports,
    );

    // Choose direction with higher net PnL (fallback to higher gross)
    let (arb, pool_in, pool_out, in_vals, out_vals, price_in, price_out) =
        if arb_a_b.pnl.is_some() && arb_b_a.pnl.is_some() {
            if arb_a_b.pnl.unwrap() > arb_b_a.pnl.unwrap() {
                (arb_a_b, &pool_a, &pool_b, &pool_a_values, &pool_b_values, price_a, price_b)
            } else {
                (arb_b_a, &pool_b, &pool_a, &pool_b_values, &pool_a_values, price_b, price_a)
            }
        } else if arb_a_b.gross_profit > arb_b_a.gross_profit {
            (arb_a_b, &pool_a, &pool_b, &pool_a_values, &pool_b_values, price_a, price_b)
        } else {
            (arb_b_a, &pool_b, &pool_a, &pool_b_values, &pool_a_values, price_b, price_a)
        };

    info!("──────── Direction ────────");
    info!("  • mint_in:  {}", pk_s(&mint_in));
    info!("  • mint_out: {}", pk_s(&mint_out));
    info!("  • First pool (0→1): {}", pool_in.pool_id);
    info!("  • Second pool (0→1): {}", pool_out.pool_id);
    info!("  • Price first:  {:.12}", price_in);
    info!("  • Price second: {:.12}", price_out);

    // Log the flow amounts across both swaps
    // amount_in is in mint_in units; arb.amount_out_1_raw should be in mint_out raw units;
    // arb.amount_out_2_raw should be back in mint_in raw units.
    let out1 = arb.amount_out_1;       // mint_out (decimal)
    let out2 = arb.amount_out_2;       // mint_in (decimal)

    info!("──────── Swap Path Amounts ────────");
    info!("  • Start: {} (mint_in {})", amount_in, pk_s(&mint_in));
    info!(
        "  • After first swap ({}): {} (mint_out {})",
        pool_in.pool_id, out1, pk_s(&mint_out)
    );
    info!(
        "  • After second swap ({}): {} (back to mint_in {})",
        pool_out.pool_id, out2, pk_s(&mint_in)
    );

    step!(
        steps,
        "Direction: first={}, second={}, out1={} {}, out2={} {}",
        pool_in.pool_id,
        pool_out.pool_id,
        out1,
        pk_s(&mint_out),
        out2,
        pk_s(&mint_in)
    );

    // ---------- Decision ----------
    let is_profitable = if let Some(p) = arb.pnl {
        p > 0.0
    } else {
        arb.gross_profit > 0.0
    };
    let meets_spread_threshold = spread_bps_val >= spread_threshold_bps as f64;
    let should_execute = is_profitable && meets_spread_threshold;

    if !is_profitable {
        warn!("❌ Not profitable after fees (pnl {:?}, gross {})", arb.pnl, arb.gross_profit);
        step!(steps, "Not profitable after fees");
    }
    if !meets_spread_threshold {
        warn!(
            "❌ Spread below threshold: {:.4} < {}",
            spread_bps_val, spread_threshold_bps
        );
        step!(
            steps,
            "Spread below threshold: {:.4} < {}",
            spread_bps_val,
            spread_threshold_bps
        );
    }
    info!("✔ Decision: should_execute={}", should_execute);
    step!(steps, "Decision should_execute={}", should_execute);

    // ---------- Slippage & tx build ----------
    let min_out = calculate_min_out(arb.amount_out_2_raw, slippage_bps);
    info!(
        "🛡  Slippage protection: min_out(raw)={} (slippage_bps={})",
        min_out, slippage_bps
    );
    step!(steps, "min_out (slippage_bps={}) = {}", slippage_bps, min_out);

    let tx = create_arbitrage_transaction(
        &rpc,
        &keypair,
        pool_in,
        pool_out,
        arb.amount_in_raw,
        arb.amount_out_1_raw,
        atas.clone(),
        min_out,
        priority_fee_microlamports,
    )?;

    // Prepare token-account creation result flags
    let planned_create_in = !atas[0].exists;
    let planned_create_out = !atas[1].exists;

    // ---------- Execute or simulate ----------
    let mut tx_signature: Option<String> = None;
    let mut simulate_result: Option<String> = None;
    let mut tx_error: Option<String> = None;

    if simulate_only {
        info!("🧪 Simulating transaction…");
        step!(steps, "simulate_only=true → simulate");
        match simulate_transaction(&rpc, &tx) {
            Ok(result) => {
                simulate_result = Some(format!("{:?}", result));
                info!("✅ Simulation OK");
                step!(steps, "simulation OK");
            }
            Err(e) => {
                tx_error = Some(format!("{:?}", e));
                error!("❌ Simulation error: {:?}", e);
                step!(steps, "simulation ERROR: {:?}", e);
            }
        }
    } else if should_execute {
        info!("📡 Sending transaction…");
        step!(steps, "simulate_only=false & should_execute=true → send");
        match rpc.send_and_confirm_transaction(&tx) {
            Ok(sig) => {
                tx_signature = Some(sig.to_string());
                info!("✅ Send OK: {}", sig);
                step!(steps, "send OK: {}", sig);
            }
            Err(e) => {
                tx_error = Some(e.to_string());
                error!("❌ Send error: {}", e);
                step!(steps, "send ERROR: {}", e);
            }
        }
    } else {
        info!("⏭️  Skipping execution");
        step!(steps, "skip execution");
    }

    // Whether ATAs actually created now (only true if planned && we actually sent successfully)
    let actually_created_in = planned_create_in && !simulate_only && tx_signature.is_some();
    let actually_created_out = planned_create_out && !simulate_only && tx_signature.is_some();

    // ---------- JSON report ----------
    let execution_time_ms = start_time.elapsed().as_millis() as u64;

    let report = json!({
        "timestamp": Utc::now().to_rfc3339(),
        "execution_time_ms": execution_time_ms,
        "inputs": {
            "rpc_url": rpc_url,
            "keypair_path": keypair_path,
            "amount_in": amount_in,
            "spread_threshold_bps": spread_threshold_bps,
            "slippage_bps": slippage_bps,
            "priority_fee_microlamports": priority_fee_microlamports,
            "simulate_only": simulate_only,
        },
        "mints": { "mint_in": mint_in.to_string(), "mint_out": mint_out.to_string() },
        "pools": {
            "pool_a": pool_a_addr,
            "pool_b": pool_b_addr,
            "first": pool_in.pool_id.to_string(),
            "second": pool_out.pool_id.to_string()
        },
        "prices": { "first": price_in, "second": price_out, "spread_bps": spread_bps_val },
        "pool_values": {
            "first": {
                "mint0": in_vals.mint0.to_string(),
                "mint1": in_vals.mint1.to_string(),
                "reserve0": in_vals.reserve0,
                "reserve1": in_vals.reserve1,
                "vault_amount0": in_vals.vault_amount0,
                "vault_amount1": in_vals.vault_amount1,
                "protocol_fees_token0": in_vals.protocol_fees_token0,
                "protocol_fees_token1": in_vals.protocol_fees_token1,
                "fund_fees_token0": in_vals.fund_fees_token0,
                "fund_fees_token1": in_vals.fund_fees_token1,
                "token0_decimals": in_vals.token0_decimals,
                "token1_decimals": in_vals.token1_decimals,
                "trade_fee_rate": in_vals.trade_fee_rate
            },
            "second": {
                "mint0": out_vals.mint0.to_string(),
                "mint1": out_vals.mint1.to_string(),
                "reserve0": out_vals.reserve0,
                "reserve1": out_vals.reserve1,
                "vault_amount0": out_vals.vault_amount0,
                "vault_amount1": out_vals.vault_amount1,
                "protocol_fees_token0": out_vals.protocol_fees_token0,
                "protocol_fees_token1": out_vals.protocol_fees_token1,
                "fund_fees_token0": out_vals.fund_fees_token0,
                "fund_fees_token1": out_vals.fund_fees_token1,
                "token0_decimals": out_vals.token0_decimals,
                "token1_decimals": out_vals.token1_decimals,
                "trade_fee_rate": out_vals.trade_fee_rate
            }
        },
        "flow": {
            "amount_in": amount_in,                       // mint_in
            "amount_out_after_first": out1,               // mint_out
            "amount_out_after_second": out2               // back to mint_in
        },
        "calculations": {
            "amount_in_raw": arb.amount_in_raw,
            "amount_out_1_raw": arb.amount_out_1_raw,
            "amount_out_2_raw": arb.amount_out_2_raw,
            "gross_profit": arb.gross_profit,
            "gross_profit_raw": arb.gross_profit_raw,
            "total_fees": arb.total_fees,
            "total_fees_raw": arb.total_fees_raw,
            "rent": arb.rent,
            "rent_raw": arb.rent_raw,
            "pnl": arb.pnl,
            "min_out_raw": min_out
        },
        "decision": {
            "is_profitable": is_profitable,
            "meets_spread_threshold": meets_spread_threshold,
            "should_execute": should_execute
        },
        "token_accounts": [
            {
                "mint": mint_in.to_string(),
                "owner": keypair.pubkey().to_string(),
                "ata": ata_in_addr.to_string(),
                "existed_before": atas[0].exists,
                "planned_to_create_now": planned_create_in,
                "actually_created_now": actually_created_in
            },
            {
                "mint": mint_out.to_string(),
                "owner": keypair.pubkey().to_string(),
                "ata": ata_out_addr.to_string(),
                "existed_before": atas[1].exists,
                "planned_to_create_now": planned_create_out,
                "actually_created_now": actually_created_out
            }
        ],
        "tx": {
            "mode": if simulate_only { "simulate" } else if should_execute { "send" } else { "skip" },
            "signature": tx_signature,
            "simulate_result": simulate_result,
            "error": tx_error
        },
        "steps": steps
    });

    // Save & print JSON report
    let json_str = serde_json::to_string_pretty(&report)?;
    fs::write("arbitrage_result.json", &json_str)?;
    info!("📄 Detailed report saved to: arbitrage_result.json");
    println!("{}", json_str);

    info!("⏱  Total execution time: {} ms", execution_time_ms);
    info!("════════════════════════════════════════════");
    Ok(())
}
