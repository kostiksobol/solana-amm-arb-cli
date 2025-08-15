use anyhow::{anyhow, Result};
use carbon_core::account::AccountDecoder;
use carbon_raydium_cpmm_decoder::{
    RaydiumCpmmDecoder,
    accounts::{
        pool_state::PoolState,
        amm_config::AmmConfig,
        RaydiumCpmmAccount,
    },
};
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_program::program_pack::Pack;
use spl_token::state::Account as TokenAccount;

pub fn load_pool_data(
    rpc: &RpcClient, 
    pool_address: &str, 
    decoder: &RaydiumCpmmDecoder
) -> Result<(PoolState, AmmConfig, u128, u128)> {
    let pool_pk: Pubkey = pool_address.parse()?;
    let pool_acc = rpc.get_account(&pool_pk)?;
    
    let pool_state = match decoder.decode_account(&pool_acc).ok_or(anyhow!("Failed to decode pool account"))?.data {
        RaydiumCpmmAccount::PoolState(state) => state,
        _ => return Err(anyhow!("Invalid pool account type")),
    };

    let config_acc = rpc.get_account(&pool_state.amm_config)?;
    let amm_config = match decoder.decode_account(&config_acc).ok_or(anyhow!("Failed to decode config account"))?.data {
        RaydiumCpmmAccount::AmmConfig(config) => config,
        _ => return Err(anyhow!("Invalid config account type")),
    };

    let vault0_acc = rpc.get_account(&pool_state.token0_vault)?;
    let vault1_acc = rpc.get_account(&pool_state.token1_vault)?;
    
    let vault0_data = TokenAccount::unpack(&vault0_acc.data)?;
    let vault1_data = TokenAccount::unpack(&vault1_acc.data)?;

    Ok((pool_state, amm_config, vault0_data.amount.into(), vault1_data.amount.into()))
}

pub fn normalize_pools(
    pool_a: &PoolState, cfg_a: &AmmConfig, res_a0: u128, res_a1: u128,
    pool_b: &PoolState, cfg_b: &AmmConfig, res_b0: u128, res_b1: u128,
) -> Result<(PoolState, AmmConfig, u128, u128, PoolState, AmmConfig, u128, u128)> {
    
    if pool_a.token0_mint == pool_b.token0_mint && pool_a.token1_mint == pool_b.token1_mint {
        return Ok((pool_a.clone(), cfg_a.clone(), res_a0, res_a1, pool_b.clone(), cfg_b.clone(), res_b0, res_b1));
    }

    if pool_a.token0_mint == pool_b.token1_mint && pool_a.token1_mint == pool_b.token0_mint {
        let pool_b_reversed = PoolState {
            token0_mint: pool_b.token1_mint,
            token1_mint: pool_b.token0_mint,
            mint0_decimals: pool_b.mint1_decimals,
            mint1_decimals: pool_b.mint0_decimals,
            token0_vault: pool_b.token1_vault,
            token1_vault: pool_b.token0_vault,
            ..pool_b.clone()
        };
        return Ok((pool_a.clone(), cfg_a.clone(), res_a0, res_a1, pool_b_reversed, cfg_b.clone(), res_b1, res_b0));
    }

    Err(anyhow!("Pools have incompatible token pairs"))
}

pub fn calculate_price(reserve0: u128, reserve1: u128, decimals0: u8, decimals1: u8) -> f64 {
    if reserve0 == 0 { return 0.0; }
    (reserve1 as f64 / 10f64.powi(decimals1 as i32)) / (reserve0 as f64 / 10f64.powi(decimals0 as i32))
}
