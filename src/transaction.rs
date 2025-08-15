use anyhow::Result;
use carbon_raydium_cpmm_decoder::accounts::pool_state::PoolState;
use solana_client::{
    rpc_client::RpcClient,
    rpc_config::RpcSimulateTransactionConfig,
};
use solana_sdk::{
    pubkey::Pubkey,
    signer::keypair::Keypair,
    signer::Signer,
    transaction::Transaction,
    message::Message,
    instruction::Instruction,
    compute_budget::ComputeBudgetInstruction,
};
use crate::utils::get_associated_token_address_manual;

const COMPUTE_UNIT_LIMIT: u32 = 400_000;
const RAYDIUM_CPMM_PROGRAM_ID: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";

pub fn create_ata_instruction(
    payer: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey
) -> Result<Option<Instruction>> {
    let ata_address = get_associated_token_address_manual(wallet, mint);
    
    let create_ata_ix = Instruction {
        program_id: "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL".parse()?,
        accounts: vec![
            solana_sdk::instruction::AccountMeta::new(*payer, true),
            solana_sdk::instruction::AccountMeta::new(ata_address, false),
            solana_sdk::instruction::AccountMeta::new_readonly(*wallet, false),
            solana_sdk::instruction::AccountMeta::new_readonly(*mint, false),
            solana_sdk::instruction::AccountMeta::new_readonly("11111111111111111111111111111112".parse()?, false),
            solana_sdk::instruction::AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data: vec![],
    };
    
    Ok(Some(create_ata_ix))
}

pub fn create_swap_instruction(
    user: &Pubkey,
    pool_id: &Pubkey,
    user_source_token: &Pubkey,
    user_dest_token: &Pubkey,
    pool_vault_0: &Pubkey, 
    pool_vault_1: &Pubkey,
    amount_in: u64,
    min_amount_out: u64
) -> Result<Instruction> {
    let swap_ix = Instruction {
        program_id: RAYDIUM_CPMM_PROGRAM_ID.parse()?,
        accounts: vec![
            solana_sdk::instruction::AccountMeta::new(*user, true),
            solana_sdk::instruction::AccountMeta::new(*pool_id, false),
            solana_sdk::instruction::AccountMeta::new_readonly("Config1111111111111111111111111111111111111".parse()?, false),
            solana_sdk::instruction::AccountMeta::new(*user_source_token, false),
            solana_sdk::instruction::AccountMeta::new(*user_dest_token, false),
            solana_sdk::instruction::AccountMeta::new(*pool_vault_0, false),
            solana_sdk::instruction::AccountMeta::new(*pool_vault_1, false),
            solana_sdk::instruction::AccountMeta::new_readonly("So11111111111111111111111111111111111111112".parse()?, false),
            solana_sdk::instruction::AccountMeta::new_readonly("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".parse()?, false),
            solana_sdk::instruction::AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data: {
            let mut data = Vec::new();
            data.extend_from_slice(&[0x90, 0x6a, 0xc9, 0x5a, 0x25, 0x67, 0x89, 0x0b]); 
            data.extend_from_slice(&amount_in.to_le_bytes());
            data.extend_from_slice(&min_amount_out.to_le_bytes());
            data
        },
    };
    
    Ok(swap_ix)
}

pub fn create_arbitrage_transaction(
    rpc: &RpcClient,
    payer: &Keypair,
    amount_in_lamports: u64,
    pool_a: &PoolState,
    pool_b: &PoolState,
    min_out_final: f64,
    priority_fee_micro_lamports: u64
) -> Result<Transaction> {
    let mut instructions = Vec::new();
    let payer_pubkey = payer.pubkey();
    
    // 1. Add compute budget instructions
    instructions.push(ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT));
    instructions.push(ComputeBudgetInstruction::set_compute_unit_price(priority_fee_micro_lamports));
    
    // 2. Create ATA instructions if needed (for BOTH token mints)
    let token0_mint = &pool_a.token0_mint;
    let token1_mint = &pool_a.token1_mint;

    let user_sol_ata = get_associated_token_address_manual(&payer_pubkey, token0_mint);
    let user_usdc_ata = get_associated_token_address_manual(&payer_pubkey, token1_mint);

    if rpc.get_account(&user_sol_ata).is_err() {
        if let Some(ata_ix) = create_ata_instruction(&payer_pubkey, &payer_pubkey, token0_mint)? {
            instructions.push(ata_ix);
        }
    }

    if rpc.get_account(&user_usdc_ata).is_err() {
        if let Some(ata_ix) = create_ata_instruction(&payer_pubkey, &payer_pubkey, token1_mint)? {
            instructions.push(ata_ix);
        }
    }
    
    // 3. First swap in cheap pool - no min_out check here
    let pool_b_pubkey: Pubkey = "7JuwJuNU88gurFnyWeiyGKbFmExMWcmRZntn9imEzdny".parse()?;
    let swap1_ix = create_swap_instruction(
        &payer_pubkey,
        &pool_b_pubkey,
        &user_sol_ata,
        &user_usdc_ata,
        &pool_b.token0_vault,
        &pool_b.token1_vault,
        amount_in_lamports,
        0 // No minimum check for intermediate swap
    )?;
    instructions.push(swap1_ix);
    
    // 4. Second swap in expensive pool - final min_out check
    let pool_a_pubkey: Pubkey = "4jgpwmuwaUrZgTvUjio8aBVNQJ6HcsF3YKAekpwwxTou".parse()?;
    let min_out_final_lamports = (min_out_final * 10f64.powi(pool_a.mint0_decimals as i32)) as u64;
    let swap2_ix = create_swap_instruction(
        &payer_pubkey,
        &pool_a_pubkey,
        &user_usdc_ata,
        &user_sol_ata,
        &pool_a.token0_vault,
        &pool_a.token1_vault,
        u64::MAX, // Use all available from first swap
        min_out_final_lamports
    )?;
    instructions.push(swap2_ix);
    
    // 5. Create transaction
    let recent_blockhash = rpc.get_latest_blockhash()?;
    let message = Message::new(&instructions, Some(&payer_pubkey));
    let transaction = Transaction::new(&[payer], message, recent_blockhash);
    
    Ok(transaction)
}

pub async fn simulate_transaction(
    rpc: &RpcClient,
    transaction: &Transaction
) -> Result<solana_client::rpc_response::RpcSimulateTransactionResult> {
    let config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(solana_sdk::commitment_config::CommitmentConfig::processed()),
        encoding: None,
        accounts: None,
        min_context_slot: None,
        inner_instructions: true,
    };
    
    Ok(rpc.simulate_transaction_with_config(transaction, config)?.value)
}
