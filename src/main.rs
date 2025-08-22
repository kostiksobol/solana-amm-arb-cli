use anyhow::{Context, Result};
use carbon_raydium_cpmm_decoder::RaydiumCpmmDecoder;
use chrono::Utc;
use clap::Parser;
use log::{error, info, warn};
use serde_json::{Value, json};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{pubkey::Pubkey, signer::Signer};
use spl_associated_token_account::get_associated_token_address;
use std::fs;
use std::time::Instant;

use solana_amm_arb_cli::{
    arbitrage::{Arbitrage, calculate_min_out, calculate_pnl, calculate_price, spread_bps},
    cli::{
        Cli, Command, ConfigCmd, config_set_amount_in, config_set_keypair, config_set_pools,
        config_set_priority_fee, config_set_rpc, config_set_simulate, config_set_slippage_bps,
        config_set_spread_threshold_bps, default_state, load_state, save_state, state_file_path,
        take_or_panic,
    },
    pool::{PoolData, PoolValues},
    transaction::{create_arbitrage_transaction, simulate_transaction},
    utils::{get_missing_token_account, get_token_account_rent, load_keypair},
};

const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

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

fn ui_amount(raw: u64, decimals: u8) -> f64 {
    let factor = 10f64.powi(decimals as i32);
    (raw as f64) / factor
}

/* --------------------- Logging helpers --------------------- */

fn log_pool(label: &str, addr: &str, v: &PoolValues) {
    info!("──────── {} [{}] ────────", label, addr);
    info!("  • token0 (mint_in): {}", v.mint0);
    info!("    - decimals: {}", v.token0_decimals);
    info!(
        "    - reserve0: {} (ui {})",
        v.reserve0,
        ui_amount(v.reserve0, v.token0_decimals)
    );
    info!("    - vault_amount0: {}", v.vault_amount0);
    info!("    - protocol_fees_token0: {}", v.protocol_fees_token0);
    info!("    - fund_fees_token0: {}", v.fund_fees_token0);
    info!("  • token1 (mint_out): {}", v.mint1);
    info!("    - decimals: {}", v.token1_decimals);
    info!(
        "    - reserve1: {} (ui {})",
        v.reserve1,
        ui_amount(v.reserve1, v.token1_decimals)
    );
    info!("    - vault_amount1: {}", v.vault_amount1);
    info!("    - protocol_fees_token1: {}", v.protocol_fees_token1);
    info!("    - fund_fees_token1: {}", v.fund_fees_token1);
    info!("  • trade_fee_rate (raw): {}", v.trade_fee_rate);
}

fn log_candidate(tag: &str, arb: &Arbitrage, mint_in: &Pubkey) {
    info!("──── Candidate: {} ────", tag);
    info!("  amount_in: {}", arb.amount_in);
    info!(
        "  after first swap (amount_out_1): {} (raw {})",
        arb.amount_out_1, arb.amount_out_1_raw
    );
    info!(
        "  after second swap (amount_out_2): {} (raw {})",
        arb.amount_out_2, arb.amount_out_2_raw
    );
    info!(
        "  gross_profit: {} (raw {})",
        arb.gross_profit, arb.gross_profit_raw
    );
    info!(
        "  total_fees: {} (raw {})",
        arb.total_fees, arb.total_fees_raw
    );
    info!("  rent: {} (raw {})", arb.rent, arb.rent_raw);
    match arb.pnl {
        Some(p) => info!("  pnl: {}", p),
        None => {
            info!("  pnl: N/A");
            if mint_in.to_string() != SOL_MINT {
                info!(
                    "  note: pnl unavailable because mint_in != SOL ({}); fees are denominated in SOL",
                    mint_in
                );
            }
        }
    }
}

/* --------------------- Decision helper --------------------- */

#[allow(clippy::too_many_arguments)]
fn choose_direction<'a>(
    arb_a_b: &'a Arbitrage,
    arb_b_a: &'a Arbitrage,
    pool_a: &'a PoolData,
    pool_b: &'a PoolData,
    vals_a: &'a PoolValues,
    vals_b: &'a PoolValues,
    price_a: f64,
    price_b: f64,
) -> (
    &'a Arbitrage,  // chosen arbitrage
    &'static str,   // first label: "PoolA" / "PoolB"
    &'static str,   // second label
    &'a PoolData,   // first pool
    &'a PoolData,   // second pool
    &'a PoolValues, // first pool values (normalized)
    &'a PoolValues, // second pool values
    f64,            // first price
    f64,            // second price
) {
    if arb_a_b.pnl.is_some() && arb_b_a.pnl.is_some() {
        if arb_a_b.pnl.unwrap() > arb_b_a.pnl.unwrap() {
            (
                arb_a_b, "PoolA", "PoolB", pool_a, pool_b, vals_a, vals_b, price_a, price_b,
            )
        } else {
            (
                arb_b_a, "PoolB", "PoolA", pool_b, pool_a, vals_b, vals_a, price_b, price_a,
            )
        }
    } else if arb_a_b.gross_profit > arb_b_a.gross_profit {
        (
            arb_a_b, "PoolA", "PoolB", pool_a, pool_b, vals_a, vals_b, price_a, price_b,
        )
    } else {
        (
            arb_b_a, "PoolB", "PoolA", pool_b, pool_a, vals_b, vals_a, price_b, price_a,
        )
    }
}

/* ===================== main ===================== */

fn main() -> Result<()> {
    // Initialize logging
    env_logger::init();
    let start_time = Instant::now();
    info!("==========================================");
    info!("Starting solana-cpmm-arb-cli");
    info!("==========================================");

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
    let priority_fee_microlamports = take_or_panic(
        cli.priority_fee,
        state.priority_fee_microlamports,
        "priority-fee",
    );
    let simulate_only = take_or_panic(cli.simulate_only, state.simulate_only, "simulate-only");

    info!("CONFIG");
    info!("  RPC URL: {}", rpc_url);
    info!("  Keypair: {:?}", keypair_path);
    info!("  Amount In: {}", amount_in);
    info!("  Spread Threshold: {} bps", spread_threshold_bps);
    info!("  Slippage: {} bps", slippage_bps);
    info!("  Priority Fee: {} µlamports", priority_fee_microlamports);
    info!("  Simulate Only: {}", simulate_only);

    let rpc = RpcClient::new(rpc_url.clone());
    let keypair = load_keypair(&keypair_path)?;
    info!("Keypair loaded: {}", keypair.pubkey());

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

    info!("Loading pools…");
    let decoder = RaydiumCpmmDecoder;
    let pool_a = PoolData::new(&rpc, &pool_a_addr, &decoder).map_err(|e| {
        error!("RPC error loading PoolA {}: {}", pool_a_addr, e);
        e
    })?;
    let pool_b = PoolData::new(&rpc, &pool_b_addr, &decoder).map_err(|e| {
        error!("RPC error loading PoolB {}: {}", pool_b_addr, e);
        e
    })?;

    // Raw values, then normalized so that token0 == mint_in for BOTH pools
    let mut pool_a_values = pool_a.get_values(&rpc).map_err(|e| {
        error!("RPC error fetching PoolA values: {}", e);
        e
    })?;
    let mut pool_b_values = pool_b.get_values(&rpc).map_err(|e| {
        error!("RPC error fetching PoolB values: {}", e);
        e
    })?;
    pool_a_values.normalize_pool_values(&mint_in);
    pool_b_values.normalize_pool_values(&mint_in);

    // Detailed Pool Logging (now with UI reserves)
    log_pool("Pool A", &pool_a_addr, &pool_a_values);
    log_pool("Pool B", &pool_b_addr, &pool_b_values);

    // Prices: both pools are oriented as mint_in -> mint_out (token0 -> token1)
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

    info!("Prices ({} -> {}):", pk_s(&mint_in), pk_s(&mint_out));
    info!("  Pool A price: {:.12}", price_a);
    info!("  Pool B price: {:.12}", price_b);
    info!("  Spread: {:.4} bps", spread_bps_val);

    let mut steps: Vec<String> = Vec::new();
    step!(steps, "Pools: A={}  B={}", pool_a_addr, pool_b_addr);
    step!(
        steps,
        "Mints: mint_in={}  mint_out={}",
        pk_s(&mint_in),
        pk_s(&mint_out)
    );
    step!(
        steps,
        "Prices: A={:.12}  B={:.12}  spread_bps={:.4}",
        price_a,
        price_b,
        spread_bps_val
    );

    // ---------- Token accounts & rent ----------
    let ata_in_addr = get_associated_token_address(&keypair.pubkey(), &mint_in);
    let ata_out_addr = get_associated_token_address(&keypair.pubkey(), &mint_out);
    let atas = vec![
        get_missing_token_account(&rpc, &keypair.pubkey(), &mint_in),
        get_missing_token_account(&rpc, &keypair.pubkey(), &mint_out),
    ];
    let rent_per_ata = get_token_account_rent(&rpc).map_err(|e| {
        error!("RPC error fetching token account rent: {}", e);
        e
    })?;
    // pay rent only for accounts that do NOT exist
    let rent_raw = (((!atas[0].exists) as u64) + ((!atas[1].exists) as u64)) * rent_per_ata;

    info!("Token Accounts");
    info!("  Owner: {}", keypair.pubkey());
    info!(
        "  {} → ATA: {}  (exists: {})",
        pk_s(&mint_in),
        ata_in_addr,
        atas[0].exists
    );
    info!(
        "  {} → ATA: {}  (exists: {})",
        pk_s(&mint_out),
        ata_out_addr,
        atas[1].exists
    );
    info!("  Rent per ATA: {} lamports", rent_per_ata);
    info!("  Rent to be paid now (if creating): {} lamports", rent_raw);
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

    info!("Arbitrage candidates (full metrics):");
    log_candidate("A → B (PoolA first, PoolB second)", &arb_a_b, &mint_in);
    log_candidate("B → A (PoolB first, PoolA second)", &arb_b_a, &mint_in);

    // ---------- Choose direction (by reference; no moves) ----------
    let (
        arb_chosen,
        first_label,
        second_label,
        pool_in,
        pool_out,
        in_vals,
        out_vals,
        price_first,
        price_second,
    ) = choose_direction(
        &arb_a_b,
        &arb_b_a,
        &pool_a,
        &pool_b,
        &pool_a_values,
        &pool_b_values,
        price_a,
        price_b,
    );

    info!("Direction");
    info!("  mint_in:  {}", pk_s(&mint_in));
    info!("  mint_out: {}", pk_s(&mint_out));
    info!("  First pool:  {} ({})", first_label, pool_in.pool_id);
    info!("  Second pool: {} ({})", second_label, pool_out.pool_id);
    info!("  Price first:  {:.12}", price_first);
    info!("  Price second: {:.12}", price_second);

    // Flow amounts across both swaps (decimals already computed inside `arb`)
    let out1 = arb_chosen.amount_out_1; // mint_out
    let out2 = arb_chosen.amount_out_2; // back to mint_in

    info!("Swap Path Amounts");
    info!("  Start: {} (mint_in {})", amount_in, pk_s(&mint_in));
    info!(
        "  After first swap ({}): {} (mint_out {})",
        pool_in.pool_id,
        out1,
        pk_s(&mint_out)
    );
    info!(
        "  After second swap ({}): {} (mint_in {})",
        pool_out.pool_id,
        out2,
        pk_s(&mint_in)
    );

    step!(
        steps,
        "Direction: first={} ({}), second={} ({})",
        first_label,
        pool_in.pool_id,
        second_label,
        pool_out.pool_id
    );
    step!(
        steps,
        "Flow: out1={} {}, out2={} {}",
        out1,
        pk_s(&mint_out),
        out2,
        pk_s(&mint_in)
    );

    // ---------- Decision ----------
    let is_profitable = if let Some(p) = arb_chosen.pnl {
        p > 0.0
    } else {
        arb_chosen.gross_profit > 0.0
    };
    let meets_spread_threshold = spread_bps_val >= spread_threshold_bps as f64;
    let should_execute = is_profitable && meets_spread_threshold;

    if !is_profitable {
        warn!(
            "Not profitable (pnl {:?}, gross {})",
            arb_chosen.pnl, arb_chosen.gross_profit
        );
        step!(steps, "Not profitable");
    }
    if !meets_spread_threshold {
        warn!(
            "Spread below threshold: {:.4} < {}",
            spread_bps_val, spread_threshold_bps
        );
        step!(
            steps,
            "Spread below threshold: {:.4} < {}",
            spread_bps_val,
            spread_threshold_bps
        );
    }
    info!("Decision: should_execute={}", should_execute);
    step!(steps, "Decision should_execute={}", should_execute);

    // ---------- Slippage & tx build ----------
    let min_out = calculate_min_out(arb_chosen.amount_out_2_raw, slippage_bps);
    info!(
        "Slippage protection: min_out(raw)={} (slippage_bps={})",
        min_out, slippage_bps
    );
    step!(
        steps,
        "min_out (slippage_bps={}) = {}",
        slippage_bps,
        min_out
    );

    let tx = create_arbitrage_transaction(
        &rpc,
        &keypair,
        pool_in,
        pool_out,
        arb_chosen.amount_in_raw,
        arb_chosen.amount_out_1_raw,
        atas.clone(),
        min_out,
        priority_fee_microlamports,
    )
    .map_err(|e| {
        error!("Error building transaction: {}", e);
        e
    })?;

    // Prepare token-account creation result flags
    let planned_create_in = !atas[0].exists;
    let planned_create_out = !atas[1].exists;

    // ---------- Execute or simulate ----------
    let mut tx_signature: Option<String> = None;
    let mut simulate_result: Option<Value> = None;
    let mut tx_error: Option<String> = None;

    if simulate_only {
        info!("Simulating transaction…");
        step!(steps, "simulate_only=true → simulate");

        match simulate_transaction(&rpc, &tx) {
            Ok(result) => {
                // Store full structured result for the final JSON report
                let result_json = serde_json::to_value(&result).unwrap_or(Value::Null);
                simulate_result = Some(result_json);

                if let Some(err) = result.err {
                    // Concise error logging only (no pretty JSON dump)
                    error!("Simulation error: {:?}", err);
                    if let Some(units) = result.units_consumed {
                        error!("Compute units consumed: {}", units);
                    }
                    if let Some(logs) = result.logs.as_ref().and_then(|v| v.last()) {
                        // Optional: just a single hint line, not the whole payload
                        error!("Last program log: {}", logs);
                    }
                    step!(steps, "simulation ERROR: {:?}", err);
                    tx_error = Some(format!("{:?}", err));
                } else {
                    // Success: concise OK line
                    info!(
                        "Simulation OK (units_consumed: {:?})",
                        result.units_consumed
                    );
                    step!(steps, "simulation OK");
                }
            }
            Err(e) => {
                tx_error = Some(e.to_string());
                error!("Simulation call failed: {}", e);
                step!(steps, "simulation ERROR: {}", e);
            }
        }
    } else if should_execute {
        info!("Sending transaction…");
        step!(steps, "simulate_only=false & should_execute=true → send");
        match rpc.send_and_confirm_transaction(&tx) {
            Ok(sig) => {
                tx_signature = Some(sig.to_string());
                info!("Send OK: {}", sig);
                step!(steps, "send OK: {}", sig);
            }
            Err(e) => {
                tx_error = Some(e.to_string());
                error!("Send error: {}", e);
                step!(steps, "send ERROR: {}", e);
            }
        }
    } else {
        info!("Skipping execution");
        step!(steps, "skip execution");
    }

    // Whether ATAs actually created now (only true if planned && we actually sent successfully)
    let actually_created_in = planned_create_in && !simulate_only && tx_signature.is_some();
    let actually_created_out = planned_create_out && !simulate_only && tx_signature.is_some();

    let creation_status_in = if simulate_only && planned_create_in {
        "would_create_in_simulation"
    } else if !simulate_only && planned_create_in && tx_signature.is_some() {
        "created_now"
    } else if !planned_create_in {
        "existed_before"
    } else {
        "skipped_no_send"
    };

    let creation_status_out = if simulate_only && planned_create_out {
        "would_create_in_simulation"
    } else if !simulate_only && planned_create_out && tx_signature.is_some() {
        "created_now"
    } else if !planned_create_out {
        "existed_before"
    } else {
        "skipped_no_send"
    };

    // ---------- JSON report (now includes reserve*_ui) ----------
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
            "direction": {
                "first_label": first_label,
                "second_label": second_label,
                "first": pool_in.pool_id.to_string(),
                "second": pool_out.pool_id.to_string()
            }
        },
        "prices": { "first": price_first, "second": price_second, "spread_bps": spread_bps_val },
        "pool_values": {
            "first": {
                "mint0": in_vals.mint0.to_string(),
                "mint1": in_vals.mint1.to_string(),
                "reserve0": in_vals.reserve0,
                "reserve1": in_vals.reserve1,
                "reserve0_ui": ui_amount(in_vals.reserve0, in_vals.token0_decimals),
                "reserve1_ui": ui_amount(in_vals.reserve1, in_vals.token1_decimals),
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
                "reserve0_ui": ui_amount(out_vals.reserve0, out_vals.token0_decimals),
                "reserve1_ui": ui_amount(out_vals.reserve1, out_vals.token1_decimals),
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
            "amount_in": amount_in,                // mint_in
            "amount_out_after_first": out1,        // mint_out
            "amount_out_after_second": out2        // back to mint_in
        },
        "arbitrage_candidates": {
            "A_to_B": {
                "amount_out_1": arb_a_b.amount_out_1,
                "amount_out_2": arb_a_b.amount_out_2,
                "gross_profit": arb_a_b.gross_profit,
                "total_fees": arb_a_b.total_fees,
                "rent": arb_a_b.rent,
                "pnl": arb_a_b.pnl
            },
            "B_to_A": {
                "amount_out_1": arb_b_a.amount_out_1,
                "amount_out_2": arb_b_a.amount_out_2,
                "gross_profit": arb_b_a.gross_profit,
                "total_fees": arb_b_a.total_fees,
                "rent": arb_b_a.rent,
                "pnl": arb_b_a.pnl
            }
        },
        "calculations": {
            "amount_in_raw": arb_chosen.amount_in_raw,
            "amount_out_1_raw": arb_chosen.amount_out_1_raw,
            "amount_out_2_raw": arb_chosen.amount_out_2_raw,
            "gross_profit": arb_chosen.gross_profit,
            "gross_profit_raw": arb_chosen.gross_profit_raw,
            "total_fees": arb_chosen.total_fees,
            "total_fees_raw": arb_chosen.total_fees_raw,
            "rent": arb_chosen.rent,
            "rent_raw": arb_chosen.rent_raw,
            "pnl": arb_chosen.pnl,
            "min_out_raw": min_out
        },
        "decision": {
            "is_profitable": is_profitable,
            "meets_spread_threshold": meets_spread_threshold,
            "should_execute": should_execute,
            "chosen_direction": format!("{}→{}", first_label, second_label)
        },
        "token_accounts": [
            {
                "mint": mint_in.to_string(),
                "owner": keypair.pubkey().to_string(),
                "ata": ata_in_addr.to_string(),
                "existed_before": atas[0].exists,
                "planned_to_create_now": planned_create_in,
                "actually_created_now": actually_created_in,
                "creation_status": creation_status_in
            },
            {
                "mint": mint_out.to_string(),
                "owner": keypair.pubkey().to_string(),
                "ata": ata_out_addr.to_string(),
                "existed_before": atas[1].exists,
                "planned_to_create_now": planned_create_out,
                "actually_created_now": actually_created_out,
                "creation_status": creation_status_out
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
    info!("Detailed report saved to: arbitrage_result.json");
    // println!("{}", json_str);

    let execution_time_ms = start_time.elapsed().as_millis() as u64;
    info!("Total execution time: {} ms", execution_time_ms);
    info!("==========================================");
    Ok(())
}
