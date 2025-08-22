use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{pubkey::Pubkey, signer::keypair::Keypair};
use spl_associated_token_account::get_associated_token_address;
use std::{fs, path::Path};

const TOKEN_ACCOUNT_SIZE: usize = 165;

pub fn load_keypair(keypair_path: &Path) -> Result<Keypair> {
    let json_string = fs::read_to_string(keypair_path)?;
    let bytes: Vec<u8> = serde_json::from_str(&json_string)?;
    Ok(Keypair::from_bytes(&bytes)?)
}

pub fn get_token_account_rent(rpc: &RpcClient) -> Result<u64> {
    Ok(rpc.get_minimum_balance_for_rent_exemption(TOKEN_ACCOUNT_SIZE)?)
}

#[derive(Clone)]
pub struct TokenAccount {
    pub mint: Pubkey,
    pub ata: Pubkey,
    pub exists: bool,
}

pub fn get_missing_token_account(
    rpc: &RpcClient,
    wallet: &Pubkey,
    token_mint: &Pubkey,
) -> TokenAccount {
    let ata = get_associated_token_address(wallet, token_mint);

    if rpc.get_account(&ata).is_err() {
        TokenAccount {
            mint: *token_mint,
            ata,
            exists: false,
        }
    } else {
        TokenAccount {
            mint: *token_mint,
            ata,
            exists: true,
        }
    }
}
