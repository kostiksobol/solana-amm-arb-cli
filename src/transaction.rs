use anyhow::Result;
use carbon_raydium_cpmm_decoder::accounts::pool_state::PoolState;
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

use crate::{arbitrage::SOL_MINT, pool::PoolData, utils::TokenAccount};

const COMPUTE_UNIT_LIMIT: u32 = 400_000;

pub fn create_ata_instruction(payer: &Pubkey, wallet: &Pubkey, mint: &Pubkey) -> Instruction {
    let create_ata_ix = spl_associated_token_account::instruction::create_associated_token_account(
        payer,
        wallet,
        mint,
        &spl_token::ID,
    );
    create_ata_ix
}

fn get_pool_authority() -> Pubkey {
    let program_id = raydium_cpmm::RAYDIUM_CP_SWAP_ID;

    let (authority, _bump) =
        Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &program_id);

    authority
}

pub fn create_swap_instruction(
    user: &Pubkey,
    pool_id: &Pubkey,
    pool_state: &PoolState,
    user_source_token: &Pubkey,
    user_dest_token: &Pubkey,
    amount_in: u64,
    min_amount_out: u64,
    swap_direction: bool, // true = token0->token1, false = token1->token0
) -> Result<Instruction> {
    let authority = get_pool_authority();

    let (input_vault, output_vault, input_mint, output_mint) = if swap_direction {
        // token0 -> token1 (e.g., SOL -> USDC)
        (
            pool_state.token0_vault,
            pool_state.token1_vault,
            pool_state.token0_mint,
            pool_state.token1_mint,
        )
    } else {
        // token1 -> token0 (e.g., USDC -> SOL)
        (
            pool_state.token1_vault,
            pool_state.token0_vault,
            pool_state.token1_mint,
            pool_state.token0_mint,
        )
    };

    let instruction = SwapBaseInputBuilder::new()
        .payer(*user)
        .authority(authority)
        .amm_config(pool_state.amm_config)
        .pool_state(*pool_id)
        .input_token_account(*user_source_token)
        .output_token_account(*user_dest_token)
        .input_vault(input_vault)
        .output_vault(output_vault)
        .input_token_program(spl_token::id())
        .output_token_program(spl_token::id())
        .input_token_mint(input_mint)
        .output_token_mint(output_mint)
        .observation_state(pool_state.observation_key)
        .amount_in(amount_in)
        .minimum_amount_out(min_amount_out)
        .instruction();

    Ok(instruction)
}

pub fn create_arbitrage_transaction(
    rpc: &RpcClient,
    payer: &Keypair,
    pool_in: &PoolData,
    pool_out: &PoolData,
    amount_in: u64,
    amount_in_1: u64,
    atas: Vec<TokenAccount>,
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

    for ata in &atas {
        if !ata.exists {
            if ata.mint == SOL_MINT.parse::<Pubkey>().unwrap() {
                instructions.push(create_ata_instruction(
                    &payer_pubkey,
                    &payer_pubkey,
                    &ata.mint,
                ));
                instructions.push(system_instruction::transfer(
                    &payer_pubkey,
                    &ata.ata,
                    amount_in,
                ));
                instructions.push(spl_token::instruction::sync_native(
                    &spl_token::id(),
                    &ata.ata,
                )?);
            } else {
                instructions.push(create_ata_instruction(
                    &payer_pubkey,
                    &payer_pubkey,
                    &ata.mint,
                ));
            }
        }
    }
    let swap_direction = pool_in.state.token0_mint == atas[0].mint;
    let swap1_ix = create_swap_instruction(
        &payer_pubkey,
        &pool_in.pool_id,
        &pool_in.state,
        &atas[0].ata,
        &atas[1].ata,
        amount_in,
        0,
        swap_direction,
    )?;
    instructions.push(swap1_ix);

    let swap_direction = pool_out.state.token0_mint == atas[1].mint;
    let swap2_ix = create_swap_instruction(
        &payer_pubkey,
        &pool_out.pool_id,
        &pool_out.state,
        &atas[1].ata,
        &atas[0].ata,
        amount_in_1,
        min_out,
        swap_direction,
    )?;
    instructions.push(swap2_ix);

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
