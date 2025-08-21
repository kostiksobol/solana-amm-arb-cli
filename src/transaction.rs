use anyhow::Result;
use raydium_cpmm::instructions::SwapBaseInputBuilder;
use solana_client::{rpc_client::RpcClient, rpc_config::RpcSimulateTransactionConfig};
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    instruction::Instruction,
    message::Message,
    pubkey::Pubkey,
    signer::{Signer, keypair::Keypair},
    system_instruction,
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

fn get_pool_authority() -> Result<Pubkey> {
    let program_id = raydium_cpmm::RAYDIUM_CP_SWAP_ID;

    let (authority, _bump) =
        Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &program_id);

    Ok(authority)
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
    let authority = get_pool_authority()?;

    let (input_vault, output_vault, input_mint, output_mint) = if swap_direction {
        // token0 -> token1 (e.g., SOL -> USDC)
        (pool.vault0, pool.vault1, pool.mint0, pool.mint1)
    } else {
        // token1 -> token0 (e.g., USDC -> SOL)
        (pool.vault1, pool.vault0, pool.mint1, pool.mint0)
    };

    let instruction = SwapBaseInputBuilder::new()
        .payer(*user)
        .authority(authority)
        .amm_config(pool.amm_config)
        .pool_state(pool.pool_id)
        .input_token_account(*user_source_token)
        .output_token_account(*user_dest_token)
        .input_vault(input_vault)
        .output_vault(output_vault)
        .input_token_program(spl_token::id())
        .output_token_program(spl_token::id())
        .input_token_mint(input_mint)
        .output_token_mint(output_mint)
        .observation_state(pool.observation_key)
        .amount_in(amount_in)
        .minimum_amount_out(min_amount_out)
        .instruction();

    Ok(instruction)
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
            instructions.push(system_instruction::transfer(
                &payer_pubkey,
                &user_0_ata,
                amount_in,
            ));
            instructions.push(spl_token::instruction::sync_native(
                &spl_token::id(),
                &user_0_ata,
            )?);
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
    let amount_in_2 = crate::arbitrage::calculate_swap_output_raw(
        amount_in,
        cheap_pool.real_reserve0,
        cheap_pool.real_reserve1,
        cheap_pool.fee,
    );
    let swap2_ix = create_swap_instruction(
        &payer_pubkey,
        &expensive_pool,
        &user_1_ata,
        &user_0_ata,
        amount_in_2,
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
