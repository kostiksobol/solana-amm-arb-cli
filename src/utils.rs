use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{pubkey::Pubkey, signer::keypair::Keypair};
use std::fs;

pub fn load_keypair(keypair_path: &str) -> Result<Keypair> {
    if std::path::Path::new(keypair_path).exists() {
        let json_string = fs::read_to_string(keypair_path)?;
        let bytes: Vec<u8> = serde_json::from_str(&json_string)?;
        Ok(Keypair::from_bytes(&bytes)?)
    } else {
        let new_keypair = Keypair::new();
        let bytes = new_keypair.to_bytes();
        let json = serde_json::to_string(&bytes.to_vec())?;
        fs::write(keypair_path, json)?;
        Ok(new_keypair)
    }
}

pub fn get_token_account_rent(rpc: &RpcClient) -> Result<f64> {
    let account_size = 165; // Standard token account size
    let rent_exemption_lamports = rpc.get_minimum_balance_for_rent_exemption(account_size)?;
    Ok(rent_exemption_lamports as f64 / 1_000_000_000.0)
}

pub fn get_associated_token_address_manual(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    let ata_program_id: Pubkey = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL".parse().unwrap();
    let spl_token_program_id = spl_token::ID;
    
    let seeds = &[
        wallet.as_ref(),
        spl_token_program_id.as_ref(),
        mint.as_ref(),
    ];
    
    Pubkey::find_program_address(seeds, &ata_program_id).0
}

pub fn count_missing_token_accounts(
    rpc: &RpcClient, 
    wallet: &Pubkey, 
    token0_mint: &Pubkey, 
    token1_mint: &Pubkey
) -> Result<u32> {
    let mut missing_count = 0;
    
    let ata0 = get_associated_token_address_manual(wallet, token0_mint);
    if rpc.get_account(&ata0).is_err() {
        missing_count += 1;
    }
    
    let ata1 = get_associated_token_address_manual(wallet, token1_mint);
    if rpc.get_account(&ata1).is_err() {
        missing_count += 1;
    }
    
    Ok(missing_count)
}

pub fn calculate_min_out(amount_out: f64, slippage_bps: u16) -> f64 {
    let slippage_factor = 1.0 - (slippage_bps as f64 / 10000.0);
    amount_out * slippage_factor
}
