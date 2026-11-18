use anchor_lang::prelude::*;

use crate::error::AmmError;
use crate::state::Config;

#[derive(Accounts)]
pub struct Update<'info> {
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [b"config", config.seed.to_le_bytes().as_ref()],
        bump = config.config_bump
    )]
    pub config: Account<'info, Config>,
}

impl<'info> Update<'info> {
    fn check_authority(&self) -> Result<()> {
        require!(self.config.authority.is_some(), AmmError::NoAuthoritySet);
        require!(
            self.config.authority == Some(self.authority.key()),
            AmmError::InvalidAuthority
        );
        Ok(())
    }

    pub fn lock(&mut self) -> Result<()> {
        self.check_authority()?;
        self.config.locked = true;
        Ok(())
    }

    pub fn unlock(&mut self) -> Result<()> {
        self.check_authority()?;
        self.config.locked = false;
        Ok(())
    }
}