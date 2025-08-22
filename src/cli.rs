use anyhow::{Context, Result, bail};
use carbon_raydium_cpmm_decoder::RaydiumCpmmDecoder;
use clap::{Parser, Subcommand};
use dialoguer::{Confirm, Input, Select};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use shellexpand;
use solana_client::rpc_client::RpcClient;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use crate::{pool::PoolData, utils::load_keypair};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AppState {
    // Pools & mints
    pub pool_a: Option<String>,
    pub pool_b: Option<String>,
    pub mint_in: Option<String>, // e.g., So11111111111111111111111111111111111111112
    pub mint_out: Option<String>,

    // Trading params
    pub amount_in: Option<f64>, // decimal units of chosen mint
    pub spread_threshold_bps: Option<u32>,
    pub slippage_bps: Option<u32>,
    pub priority_fee_microlamports: Option<u64>,
    pub simulate_only: Option<bool>,

    // Infra
    pub rpc_url: Option<String>,
    pub keypair_path: Option<PathBuf>,
}

// ======== Programmer-editable defaults (initial install state) ========
pub fn default_state() -> AppState {
    // Edit to your desired shipped defaults
    AppState {
        pool_a: Some("4jgpwmuwaUrZgTvUjio8aBVNQJ6HcsF3YKAekpwwxTou".to_string()),
        pool_b: Some("7JuwJuNU88gurFnyWeiyGKbFmExMWcmRZntn9imEzdny".to_string()),
        mint_in: Some("So11111111111111111111111111111111111111112".to_string()),
        mint_out: Some("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string()),
        amount_in: Some(0.00001),
        spread_threshold_bps: Some(100),
        slippage_bps: Some(500),
        priority_fee_microlamports: Some(100000),
        simulate_only: Some(true),
        rpc_url: Some("https://api.mainnet-beta.solana.com".to_string()),
        keypair_path: Some("/home/coolman/solana-amm-arb-cli/keypair.json".into()),
    }
}

pub fn parse_non_negative_f64(s: &str) -> Result<f64, String> {
    let v: f64 = s
        .parse()
        .map_err(|e: std::num::ParseFloatError| e.to_string())?;
    if v >= 0.0 {
        Ok(v)
    } else {
        Err("amount-in must be >= 0".into())
    }
}

// ======================= CLI =======================

#[derive(Parser, Debug)]
#[command(
    name = "solana-cpmm-arb-cli",
    version,
    about = "Stateful Solana CPMM arbitrage CLI (skeleton)"
)]
pub struct Cli {
    // Runtime flags (no subcommand) — main path prints ONLY requested params
    #[arg(long)]
    pub rpc_url: Option<String>,
    #[arg(long)]
    pub keypair: Option<PathBuf>,
    #[arg(long, value_parser = parse_non_negative_f64)]
    pub amount_in: Option<f64>,
    #[arg(long, value_name = "U32")]
    pub spread_threshold_bps: Option<u32>,
    #[arg(long, value_name = "U32")]
    pub slippage_bps: Option<u32>,
    /// Priority fee in MICRO-lamports (1_000 µlamports = 1 lamport)
    #[arg(long, value_name = "U64")]
    pub priority_fee: Option<u64>,
    #[arg(long, value_name = "BOOL")]
    pub simulate_only: Option<bool>,

    #[command(subcommand)]
    pub cmd: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Grouped config commands
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Show persisted state
    Show,
    /// Reset persisted state to programmer-defined defaults
    ResetDefaults,
    /// Interactively set pools; recompute mints; if mints changed, ask amount-in & which mint
    SetPools,
    /// Interactively set RPC URL (checks connectivity)
    SetRpcUrl,
    /// Interactively set keypair path (validates)
    SetKeypair,
    /// Interactively set amount-in (first choose for which mint)
    SetAmountIn,
    /// Interactively set spread-threshold-bps
    SetSpreadThresholdBps,
    /// Interactively set slippage-bps
    SetSlippageBps,
    /// Interactively set priority-fee (micro-lamports)
    SetPriorityFee,
    /// Interactively set simulate-only flag
    SetSimulate,
}

// ======================= Config flows =======================

pub fn config_set_pools(state_path: &Path, state: &mut AppState) -> Result<()> {
    println!(
        "Current poolA: {}",
        state.pool_a.clone().unwrap_or("-unset-".into())
    );
    let change_a = Confirm::new()
        .with_prompt("Change poolA?")
        .default(false)
        .interact()?;
    if change_a {
        let new_a: String = Input::new()
            .with_prompt("Enter poolA address")
            .validate_with(|s: &String| {
                if s.trim().is_empty() {
                    Err("poolA cannot be empty")
                } else {
                    Ok(())
                }
            })
            .interact_text()?;
        state.pool_a = Some(new_a);
    }

    println!(
        "Current poolB: {}",
        state.pool_b.clone().unwrap_or("-unset-".into())
    );
    let change_b = Confirm::new()
        .with_prompt("Change poolB?")
        .default(false)
        .interact()?;
    if change_b {
        let new_b: String = Input::new()
            .with_prompt("Enter poolB address")
            .validate_with(|s: &String| {
                if s.trim().is_empty() {
                    Err("poolB cannot be empty")
                } else {
                    Ok(())
                }
            })
            .interact_text()?;
        state.pool_b = Some(new_b);
    }

    // Need rpc + both pools to compute mints
    let rpc = match &state.rpc_url {
        Some(u) => u.clone(),
        None => {
            println!(
                "rpc-url not set in state; skipping mint computation. Use `config set-rpc-url`."
            );
            save_state(state_path, state)?;
            return Ok(());
        }
    };
    let (a, b) = match (&state.pool_a, &state.pool_b) {
        (Some(a), Some(b)) => (a.clone(), b.clone()),
        _ => {
            println!("poolA/poolB missing; saved changes.");
            save_state(state_path, state)?;
            return Ok(());
        }
    };

    // Compute pool mints (order doesn’t matter yet)
    let (m0, m1) = compute_mints(&rpc, &a, &b)?;
    println!("Computed pool mints:\n  - {}\n  - {}", m0, m1);

    // Did the mint set change compared to current mint_in/mint_out?
    let have_current_pair = state.mint_in.is_some() && state.mint_out.is_some();
    let changed = if have_current_pair {
        let cur_in = state.mint_in.as_ref().unwrap();
        let cur_out = state.mint_out.as_ref().unwrap();
        !((cur_in == &m0 && cur_out == &m1) || (cur_in == &m1 && cur_out == &m0))
    } else {
        true
    };

    if changed {
        println!(
            "Detected mint change or unset mints; choose which mint is the INPUT (amount-in token)."
        );
        let items = vec![m0.clone(), m1.clone()];
        // Default to current mint_in if it matches one of them
        let default_idx = match state.mint_in.as_ref() {
            Some(mi) if mi == &m1 => 1,
            _ => 0,
        };
        let choice = Select::new()
            .with_prompt("Choose input mint (by address)")
            .items(&items)
            .default(default_idx)
            .interact()?;

        if choice == 0 {
            state.mint_in = Some(m0.clone());
            state.mint_out = Some(m1.clone());
        } else {
            state.mint_in = Some(m1.clone());
            state.mint_out = Some(m0.clone());
        }

        // Ask for amount-in
        let amt: f64 = Input::new()
            .with_prompt("Enter amount-in (decimal)")
            .validate_with(|v: &String| {
                v.parse::<f64>()
                    .map(|x| {
                        if x >= 0.0 {
                            Ok(())
                        } else {
                            Err("must be >= 0.0")
                        }
                    })
                    .unwrap_or(Err("invalid number"))
            })
            .interact_text()?
            .parse::<f64>()?;
        state.amount_in = Some(amt);

        println!(
            "Set amount-in = {} (mint_in: {})",
            amt,
            state.mint_in.as_ref().unwrap()
        );
    } else {
        println!(
            "Mint set unchanged; keeping mint_in = {}, mint_out = {}",
            state.mint_in.as_ref().unwrap(),
            state.mint_out.as_ref().unwrap()
        );
    }

    save_state(state_path, state)?;
    println!(
        "Saved pools & mints to {} (mint_in={}, mint_out={})",
        state_path.display(),
        state.mint_in.as_deref().unwrap_or("-unset-"),
        state.mint_out.as_deref().unwrap_or("-unset-")
    );
    Ok(())
}

pub fn config_set_rpc(state_path: &Path, state: &mut AppState) -> Result<()> {
    let cur = state.rpc_url.clone().unwrap_or("-unset-".into());
    println!("Current rpc-url: {cur}");
    let new_url: String = Input::new().with_prompt("Enter rpc-url").interact_text()?;
    check_rpc_url(&new_url)?; // plug your real checker
    state.rpc_url = Some(new_url.clone());
    save_state(state_path, state)?;
    println!("Saved rpc-url = {}", new_url);
    Ok(())
}

pub fn config_set_keypair(state_path: &Path, state: &mut AppState) -> Result<()> {
    let cur = state.keypair_path.clone().unwrap_or("-unset-".into());
    println!("Current keypair path: {:?}", cur);
    let path_str: String = Input::new()
        .with_prompt("Enter keypair file path")
        .interact_text()?;
    let expanded = shellexpand::tilde(&path_str).to_string();
    validate_keypair_path(Path::new(&expanded))?; // plug your real validator
    state.keypair_path = Some(expanded.clone().into());
    save_state(state_path, state)?;
    println!("Saved keypair path = {}", expanded);
    Ok(())
}

pub fn config_set_amount_in(state_path: &Path, state: &mut AppState) -> Result<()> {
    // We need two mint candidates to choose from:
    // Prefer computing from pools; fall back to current (mint_in, mint_out) if pools/rpc missing.
    let maybe_pair_from_state = state.mint_in.clone().zip(state.mint_out.clone());

    let pair: (String, String) = if state.pool_a.is_some() && state.pool_b.is_some() {
        match &state.rpc_url {
            Some(rpc) => {
                let (a, b) = (
                    state.pool_a.as_ref().unwrap(),
                    state.pool_b.as_ref().unwrap(),
                );
                compute_mints(rpc, a, b)?
            }
            None => match maybe_pair_from_state {
                Some(p) => p,
                None => bail!("Need rpc-url or existing (mint_in, mint_out). Set pools/rpc first."),
            },
        }
    } else {
        match maybe_pair_from_state {
            Some(p) => p,
            None => bail!(
                "Mint addresses are unknown. Set pools first: `solana-cpmm-arb-cli config set-pools`"
            ),
        }
    };

    let (m0, m1) = pair;
    let items = vec![m0.clone(), m1.clone()];

    // Let user pick which is the input mint (amount-in token)
    let default_idx = match state.mint_in.as_ref() {
        Some(mi) if mi == &m1 => 1,
        _ => 0,
    };
    let choice = Select::new()
        .with_prompt("Choose input mint (by address)")
        .items(&items)
        .default(default_idx)
        .interact()?;

    if choice == 0 {
        state.mint_in = Some(m0.clone());
        state.mint_out = Some(m1.clone());
    } else {
        state.mint_in = Some(m1.clone());
        state.mint_out = Some(m0.clone());
    }

    // Ask for amount
    let amt: f64 = Input::new()
        .with_prompt("Enter amount-in (decimal)")
        .validate_with(|v: &String| {
            v.parse::<f64>()
                .map(|x| {
                    if x >= 0.0 {
                        Ok(())
                    } else {
                        Err("must be >= 0.0")
                    }
                })
                .unwrap_or(Err("invalid number"))
        })
        .interact_text()?
        .parse::<f64>()?;
    state.amount_in = Some(amt);

    save_state(state_path, state)?;
    println!(
        "Saved amount-in = {} (mint_in: {}, mint_out: {})",
        amt,
        state.mint_in.as_deref().unwrap_or("-unset-"),
        state.mint_out.as_deref().unwrap_or("-unset-")
    );
    Ok(())
}

pub fn config_set_spread_threshold_bps(state_path: &Path, state: &mut AppState) -> Result<()> {
    let cur = state.spread_threshold_bps.unwrap_or(0);
    println!("Current spread-threshold-bps: {cur}");
    let val: u32 = Input::new()
        .with_prompt("Enter spread-threshold-bps (u32, e.g., 50 = 0.50%)")
        .validate_with(|s: &String| s.parse::<u32>().map(|_| ()).map_err(|_| "invalid u32"))
        .interact_text()?
        .parse::<u32>()?;
    state.spread_threshold_bps = Some(val);
    save_state(state_path, state)?;
    println!("Saved spread-threshold-bps = {val}");
    Ok(())
}

pub fn config_set_slippage_bps(state_path: &Path, state: &mut AppState) -> Result<()> {
    let cur = state.slippage_bps.unwrap_or(0);
    println!("Current slippage-bps: {cur}");
    let val: u32 = Input::new()
        .with_prompt("Enter slippage-bps (u32, e.g., 100 = 1.00%)")
        .validate_with(|s: &String| s.parse::<u32>().map(|_| ()).map_err(|_| "invalid u32"))
        .interact_text()?
        .parse::<u32>()?;
    state.slippage_bps = Some(val);
    save_state(state_path, state)?;
    println!("Saved slippage-bps = {val}");
    Ok(())
}

pub fn config_set_priority_fee(state_path: &Path, state: &mut AppState) -> Result<()> {
    let cur = state.priority_fee_microlamports.unwrap_or(0);
    println!("Current priority-fee (µlamports): {cur}");
    let val: u64 = Input::new()
        .with_prompt("Enter priority-fee in MICRO-lamports (u64)")
        .validate_with(|s: &String| s.parse::<u64>().map(|_| ()).map_err(|_| "invalid u64"))
        .interact_text()?
        .parse::<u64>()?;
    state.priority_fee_microlamports = Some(val);
    save_state(state_path, state)?;
    println!("Saved priority-fee = {val} µlamports");
    Ok(())
}

pub fn config_set_simulate(state_path: &Path, state: &mut AppState) -> Result<()> {
    let cur = state.simulate_only.unwrap_or(true);
    println!("Current simulate-only: {cur}");
    let newv = Confirm::new()
        .with_prompt("Enable simulate-only?")
        .default(cur)
        .interact()?;
    state.simulate_only = Some(newv);
    save_state(state_path, state)?;
    println!("Saved simulate-only = {newv}");
    Ok(())
}

// ======================= Helpers =======================

pub fn take_or_panic<T: Clone>(flag: Option<T>, stored: Option<T>, name: &str) -> T {
    if let Some(v) = flag {
        return v;
    }
    if let Some(v) = stored {
        return v;
    }
    panic!(
        "Missing required parameter `{name}`: not provided as a flag and not found in state. (Set defaults in `default_state()` or configure via `config` commands.)"
    );
}

pub fn state_file_path() -> Result<PathBuf> {
    let pd = ProjectDirs::from("com", "yourorg", "solana-amm-arb-cli")
        .context("cannot determine platform-specific dirs")?;
    let dir: &Path = pd.state_dir().unwrap_or_else(|| pd.config_dir());
    Ok(dir.join("state.json"))
}

pub fn load_state(path: &Path) -> Result<AppState> {
    if !path.exists() {
        let s = default_state();
        save_state(path, &s)?;
        return Ok(s);
    }
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let st: AppState = serde_json::from_slice(&data)
        .with_context(|| format!("parse JSON in {}", path.display()))?;
    Ok(st)
}

pub fn save_state(path: &Path, st: &AppState) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(st)?;
    {
        let mut f =
            fs::File::create(&tmp).with_context(|| format!("create temp {}", tmp.display()))?;
        f.write_all(&data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("atomic rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

// ======================= External hooks (plug your real ones) =======================

/// Compute mint0/mint1 from pools via the given RPC.
/// Replace with your actual function that returns `Result<(String,String)>`.
pub fn compute_mints(rpc_url: &str, pool_a: &str, pool_b: &str) -> Result<(String, String)> {
    let rpc = RpcClient::new(rpc_url);
    let decoder = RaydiumCpmmDecoder;
    let pool_a_state = PoolData::new(&rpc, pool_a, &decoder)?;
    let pool_b_state = PoolData::new(&rpc, pool_b, &decoder)?;
    if pool_a_state.state.token0_mint == pool_b_state.state.token0_mint
        && pool_a_state.state.token1_mint == pool_b_state.state.token1_mint
    {
        Ok((
            pool_a_state.state.token0_mint.to_string(),
            pool_a_state.state.token1_mint.to_string(),
        ))
    } else if pool_a_state.state.token0_mint == pool_b_state.state.token1_mint
        && pool_a_state.state.token1_mint == pool_b_state.state.token0_mint
    {
        Ok((
            pool_a_state.state.token0_mint.to_string(),
            pool_a_state.state.token1_mint.to_string(),
        ))
    } else {
        bail!(
            "Incompatible pools: poolA mints: {} {}, poolB mints: {} {}",
            pool_a_state.state.token0_mint,
            pool_a_state.state.token1_mint,
            pool_b_state.state.token0_mint,
            pool_b_state.state.token1_mint
        );
    }
}

pub fn check_rpc_url(rpc_url: &str) -> Result<()> {
    let rpc = RpcClient::new(rpc_url);
    rpc.get_health()?;
    Ok(())
}

pub fn validate_keypair_path(path: &Path) -> Result<()> {
    load_keypair(path)?;
    Ok(())
}
