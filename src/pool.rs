use anyhow::{Result, anyhow};
use carbon_core::account::AccountDecoder;
use carbon_raydium_cpmm_decoder::{
    RaydiumCpmmDecoder,
    accounts::{RaydiumCpmmAccount, amm_config::AmmConfig, pool_state::PoolState},
};
use solana_client::rpc_client::RpcClient;
use solana_program::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use spl_token::state::Account;

pub struct PoolData {
    pub pool_id: Pubkey,
    pub state: PoolState,
    pub config: AmmConfig,
}

pub struct PoolValues {
    pub mint0: Pubkey,
    pub mint1: Pubkey,
    pub vault_amount0: u64,
    pub vault_amount1: u64,
    pub protocol_fees_token0: u64,
    pub protocol_fees_token1: u64,
    pub fund_fees_token0: u64,
    pub fund_fees_token1: u64,
    pub reserve0: u64,
    pub reserve1: u64,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
    pub trade_fee_rate: u64,
}

impl PoolData {
    pub fn new(rpc: &RpcClient, pool_address: &str, decoder: &RaydiumCpmmDecoder) -> Result<Self> {
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

        Ok(Self {
            pool_id: pool_pk,
            state: pool_state,
            config: amm_config,
        })
    }

    pub fn get_values(&self, rpc: &RpcClient) -> Result<PoolValues> {
        let vault0_acc = rpc.get_account(&self.state.token0_vault)?;
        let vault1_acc = rpc.get_account(&self.state.token1_vault)?;

        let vault0_data = Account::unpack(&vault0_acc.data)?;
        let vault1_data = Account::unpack(&vault1_acc.data)?;

        let vault_amount0 = vault0_data.amount;
        let vault_amount1 = vault1_data.amount;

        let protocol_fees_token0 = self.state.protocol_fees_token0;
        let protocol_fees_token1 = self.state.protocol_fees_token1;
        let fund_fees_token0 = self.state.fund_fees_token0;
        let fund_fees_token1 = self.state.fund_fees_token1;

        let reserve0 = vault_amount0 - protocol_fees_token0 - fund_fees_token0;
        let reserve1 = vault_amount1 - protocol_fees_token1 - fund_fees_token1;

        let token0_decimals = self.state.mint0_decimals;
        let token1_decimals = self.state.mint1_decimals;
        let trade_fee_rate = self.config.trade_fee_rate;

        Ok(PoolValues {
            mint0: self.state.token0_mint,
            mint1: self.state.token1_mint,
            vault_amount0,
            vault_amount1,
            protocol_fees_token0,
            protocol_fees_token1,
            fund_fees_token0,
            fund_fees_token1,
            reserve0,
            reserve1,
            token0_decimals,
            token1_decimals,
            trade_fee_rate,
        })
    }
}

impl PoolValues {
    pub fn normalize_pool_values(&mut self, first_mint: &Pubkey) {
        if self.mint0 != *first_mint {
            let pool_val = PoolValues {
                mint0: self.mint1,
                mint1: self.mint0,
                vault_amount0: self.vault_amount1,
                vault_amount1: self.vault_amount0,
                protocol_fees_token0: self.protocol_fees_token1,
                protocol_fees_token1: self.protocol_fees_token0,
                fund_fees_token0: self.fund_fees_token1,
                fund_fees_token1: self.fund_fees_token0,
                reserve0: self.reserve1,
                reserve1: self.reserve0,
                token0_decimals: self.token1_decimals,
                token1_decimals: self.token0_decimals,
                trade_fee_rate: self.trade_fee_rate,
            };
            *self = pool_val;
        }
    }
}
