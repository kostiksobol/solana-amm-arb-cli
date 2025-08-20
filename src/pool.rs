use anyhow::{Result, anyhow};
use carbon_core::account::AccountDecoder;
use carbon_raydium_cpmm_decoder::{RaydiumCpmmDecoder, accounts::RaydiumCpmmAccount};
use solana_client::rpc_client::RpcClient;
use solana_program::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use spl_token::state::Account as TokenAccount;

pub struct PoolData {
    pub pool_id: Pubkey,
    pub amm_config: Pubkey,
    pub observation_key: Pubkey,
    pub reserve0: u64,
    pub protocol_fees_token0: u64,
    pub fund_fees_token0: u64,
    pub real_reserve0: u64,
    pub reserve1: u64,
    pub protocol_fees_token1: u64,
    pub fund_fees_token1: u64,
    pub real_reserve1: u64,
    pub fee: u64,
    pub decimals0: u8,
    pub decimals1: u8,
    pub mint0: Pubkey,
    pub mint1: Pubkey,
    pub vault0: Pubkey,
    pub vault1: Pubkey,
}

pub fn load_pool_data(
    rpc: &RpcClient,
    pool_address: &str,
    decoder: &RaydiumCpmmDecoder,
) -> Result<PoolData> {
    let pool_pk: Pubkey = pool_address.parse()?;
    let pool_acc = rpc.get_account(&pool_pk)?;

    let pool_state = match decoder
        .decode_account(&pool_acc)
        .ok_or(anyhow!("Failed to decode pool account"))?
        .data
    {
        RaydiumCpmmAccount::PoolState(state) => state,
        _ => return Err(anyhow!("Invalid pool account type")),
    };

    let config_acc = rpc.get_account(&pool_state.amm_config)?;
    let amm_config = match decoder
        .decode_account(&config_acc)
        .ok_or(anyhow!("Failed to decode config account"))?
        .data
    {
        RaydiumCpmmAccount::AmmConfig(config) => config,
        _ => return Err(anyhow!("Invalid config account type")),
    };

    let vault0_acc = rpc.get_account(&pool_state.token0_vault)?;
    let vault1_acc = rpc.get_account(&pool_state.token1_vault)?;

    let vault0_data = TokenAccount::unpack(&vault0_acc.data)?;
    let vault1_data = TokenAccount::unpack(&vault1_acc.data)?;

    Ok(PoolData {
        pool_id: pool_pk,
        amm_config: pool_state.amm_config,
        observation_key: pool_state.observation_key,
        reserve0: vault0_data.amount,
        protocol_fees_token0: pool_state.protocol_fees_token0,
        fund_fees_token0: pool_state.fund_fees_token0,
        real_reserve0: vault0_data.amount - pool_state.protocol_fees_token0 - pool_state.fund_fees_token0,
        reserve1: vault1_data.amount,
        protocol_fees_token1: pool_state.protocol_fees_token1,
        fund_fees_token1: pool_state.fund_fees_token1,
        real_reserve1: vault1_data.amount - pool_state.protocol_fees_token1 - pool_state.fund_fees_token1,
        fee: amm_config.trade_fee_rate,
        decimals0: pool_state.mint0_decimals,
        decimals1: pool_state.mint1_decimals,
        mint0: pool_state.token0_mint,
        mint1: pool_state.token1_mint,
        vault0: pool_state.token0_vault,
        vault1: pool_state.token1_vault,
    })
}

pub fn normalize_pools(pool_a: &PoolData, pool_b: &mut PoolData) -> Result<()> {
    if pool_a.mint0 == pool_b.mint0 && pool_a.mint1 == pool_b.mint1 {
        return Ok(());
    }
    if pool_a.mint0 == pool_b.mint1 && pool_a.mint1 == pool_b.mint0 {
        let pool = PoolData {
            pool_id: pool_b.pool_id,
            amm_config: pool_b.amm_config,
            observation_key: pool_b.observation_key,
            reserve0: pool_b.reserve1,
            protocol_fees_token0: pool_b.protocol_fees_token1,
            fund_fees_token0: pool_b.fund_fees_token1,
            real_reserve0: pool_b.real_reserve1,
            reserve1: pool_b.reserve0,
            protocol_fees_token1: pool_b.protocol_fees_token0,
            fund_fees_token1: pool_b.fund_fees_token0,
            real_reserve1: pool_b.real_reserve0,
            fee: pool_b.fee,
            decimals0: pool_b.decimals1,
            decimals1: pool_b.decimals0,
            mint0: pool_b.mint1,
            mint1: pool_b.mint0,
            vault0: pool_b.vault1,
            vault1: pool_b.vault0,
        };
        *pool_b = pool;
        return Ok(());
    }
    Err(anyhow!("Pools have incompatible token pairs"))
}
