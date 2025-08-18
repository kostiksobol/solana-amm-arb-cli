use anyhow::Result;
use solana_client::{rpc_client::RpcClient, rpc_config::RpcSimulateTransactionConfig};
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    message::Message,
    pubkey::Pubkey,
    signer::{Signer, keypair::Keypair},
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;

use crate::pool::PoolData;

const COMPUTE_UNIT_LIMIT: u32 = 400_000;

pub fn create_ata_instruction(
    payer: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey,
) -> Result<Option<Instruction>> {
    let create_ata_ix = spl_associated_token_account::instruction::create_associated_token_account(
        payer,
        wallet,
        mint,
        &spl_token::ID,
    );
    Ok(Some(create_ata_ix))
}

pub fn create_swap_instruction(
    user: &Pubkey,
    pool: &PoolData,
    user_source_token: &Pubkey,
    user_dest_token: &Pubkey,
    amount_in: u64,
    min_amount_out: u64,
    swap_direction: bool, // true = token0->token1, false = token1->token0
) -> Result<Instruction> {
    let program_id: Pubkey = carbon_raydium_cpmm_decoder::PROGRAM_ID;

    // Derive required PDAs
    let (pool_authority, _) =
        Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &program_id);

    // Build accounts vector согласно SwapBaseInputInstructionAccounts
    let mut accounts = Vec::new();

    // Core accounts
    accounts.push(AccountMeta::new(*user, true)); // 0. Payer/User wallet
    accounts.push(AccountMeta::new_readonly(pool_authority, false)); // 1. Pool authority PDA
    accounts.push(AccountMeta::new_readonly(pool.amm_config, false)); // 2. AMM config
    accounts.push(AccountMeta::new(pool.pool_id, false)); // 3. Pool state

    // User token accounts
    accounts.push(AccountMeta::new(*user_source_token, false)); // 4. User source token account
    accounts.push(AccountMeta::new(*user_dest_token, false)); // 5. User destination token account

    // Pool vaults (меняем порядок в зависимости от направления)
    if swap_direction {
        // token0 -> token1
        accounts.push(AccountMeta::new(pool.vault0, false)); // 6. Input vault
        accounts.push(AccountMeta::new(pool.vault1, false)); // 7. Output vault
    } else {
        // token1 -> token0
        accounts.push(AccountMeta::new(pool.vault1, false)); // 6. Input vault
        accounts.push(AccountMeta::new(pool.vault0, false)); // 7. Output vault
    }

    // Token programs (same for both source and dest if both are SPL tokens)
    accounts.push(AccountMeta::new_readonly(spl_token::ID, false)); // 8. Token program (source)
    accounts.push(AccountMeta::new_readonly(spl_token::ID, false)); // 9. Token program (dest)

    // Token mints (меняем порядок в зависимости от направления)
    if swap_direction {
        // token0 -> token1
        accounts.push(AccountMeta::new_readonly(pool.mint0, false)); // 10. Input token mint
        accounts.push(AccountMeta::new_readonly(pool.mint1, false)); // 11. Output token mint
    } else {
        // token1 -> token0
        accounts.push(AccountMeta::new_readonly(pool.mint1, false)); // 10. Input token mint
        accounts.push(AccountMeta::new_readonly(pool.mint0, false)); // 11. Output token mint
    }
    accounts.push(AccountMeta::new(pool.observation_key, false)); // 12. Observation state

    // Build instruction data
    let instruction_data = build_swap_instruction_data(amount_in, min_amount_out)?;

    Ok(Instruction {
        program_id,
        accounts,
        data: instruction_data,
    })
}

fn build_swap_instruction_data(amount_in: u64, min_amount_out: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();

    // Discriminator for swap instruction (8 bytes)
    data.extend_from_slice(&[0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde]);
    // data.extend_from_slice(&[0xde, 0x33, 0x1e, 0xc4, 0xda, 0x5a, 0xbe, 0x8f]);

    // Параметры инструкции
    data.extend_from_slice(&amount_in.to_le_bytes()); // 8 bytes
    data.extend_from_slice(&min_amount_out.to_le_bytes()); // 8 bytes

    Ok(data)
}

pub fn create_arbitrage_transaction(
    rpc: &RpcClient,
    payer: &Keypair,
    expensive_pool: &PoolData,
    cheap_pool: &PoolData,
    amount_in: u64,
    min_out: u64,
    priority_fee: u64,
) -> Result<Transaction> {
    let mut instructions = Vec::new();
    let payer_pubkey = payer.pubkey();

    // 1. Add compute budget instructions
    instructions.push(ComputeBudgetInstruction::set_compute_unit_limit(
        COMPUTE_UNIT_LIMIT,
    ));
    instructions.push(ComputeBudgetInstruction::set_compute_unit_price(
        priority_fee,
    ));

    // 2. Create ATA instructions if needed (for BOTH token mints)
    let token0_mint = &cheap_pool.mint0;
    let token1_mint = &cheap_pool.mint1;

    let user_0_ata = get_associated_token_address(&payer_pubkey, token0_mint);
    let user_1_ata = get_associated_token_address(&payer_pubkey, token1_mint);

    if rpc.get_account(&user_0_ata).is_err() {
        if let Some(ata_ix) = create_ata_instruction(&payer_pubkey, &payer_pubkey, token0_mint)? {
            instructions.push(ata_ix);
        }
    }

    if rpc.get_account(&user_1_ata).is_err() {
        if let Some(ata_ix) = create_ata_instruction(&payer_pubkey, &payer_pubkey, token1_mint)? {
            instructions.push(ata_ix);
        }
    }

    // 3. First swap in cheap pool: token0 -> token1
    let swap1_ix = create_swap_instruction(
        &payer_pubkey,
        &cheap_pool,
        &user_0_ata,
        &user_1_ata,
        amount_in,
        0,
        true, // token0 -> token1
    )?;
    instructions.push(swap1_ix);

    // 4. Second swap in expensive pool: token1 -> token0
    let swap2_ix = create_swap_instruction(
        &payer_pubkey,
        &expensive_pool,
        &user_1_ata,
        &user_0_ata,
        u64::MAX, // Use all available tokens from first swap
        min_out,
        false, // token1 -> token0
    )?;
    instructions.push(swap2_ix);

    // 5. Create transaction
    let recent_blockhash = rpc.get_latest_blockhash()?;
    let message = Message::new(&instructions, Some(&payer_pubkey));
    let transaction = Transaction::new(&[payer], message, recent_blockhash);

    Ok(transaction)
}

pub fn simulate_transaction(
    rpc: &RpcClient,
    transaction: &Transaction,
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

    Ok(rpc
        .simulate_transaction_with_config(transaction, config)?
        .value)
}
