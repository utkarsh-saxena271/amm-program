pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("EWCVQJwM5rh2nQnPCRXLT74xFWCtpJC7n9svCW8BYZsL");

#[program]
pub mod amm_program {
    use super::*;
    pub fn initialize(
        ctx: Context<Initialize>,
        seed: u64,
        fee: u16,
        authority: Option<Pubkey>,
    ) -> Result<()> {
        ctx.accounts.initialize(seed, fee, authority, ctx.bumps)
    }

     pub fn deposit(ctx: Context<LiquidityPool>, amount: u64, max_x: u64, max_y: u64) -> Result<()> {
        ctx.accounts.deposit(amount, max_x, max_y)
    }

    pub fn withdraw(ctx: Context<LiquidityPool>, amount: u64, min_x: u64, min_y: u64) -> Result<()> {
        ctx.accounts.withdraw(amount, min_x, min_y)
    }

    pub fn swap(ctx: Context<Swap>, amount: u64, min_amount: u64, is_x: bool)->Result<()> {
        ctx.accounts.swap(amount, min_amount, is_x)
    }
}

