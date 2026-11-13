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
}
