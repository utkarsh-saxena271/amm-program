use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{transfer, Mint, Token, TokenAccount, Transfer},
};

use crate::error::AmmError;
use crate::state::Config;

#[derive(Accounts)]
pub struct Swap<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    pub mint_x: Account<'info, Mint>,
    pub mint_y: Account<'info, Mint>,
    #[account(
        has_one = mint_x,
        has_one = mint_y,
        seeds = [b"config", config.seed.to_le_bytes().as_ref()],
        bump = config.config_bump
    )]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        seeds = [b"lp", config.key().as_ref()],
        bump = config.lp_bump
    )]
    pub mint_lp: Account<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = mint_x,
        associated_token::authority = config
    )]
    pub vault_x: Account<'info, TokenAccount>,
    #[account(
        mut,
        associated_token::mint = mint_y,
        associated_token::authority = config
    )]
    pub vault_y: Account<'info, TokenAccount>,
    #[account(
        init_if_needed,
        payer= user,
        associated_token::mint = mint_x,
        associated_token::authority = user
    )]
    pub user_x: Account<'info, TokenAccount>,
    #[account(
        init_if_needed,
        payer = user,
        associated_token::mint = mint_y,
        associated_token::authority = user
    )]
    pub user_y: Account<'info, TokenAccount>,
    #[account(
    mut,
    associated_token::mint = mint_x,
    associated_token::authority = config.treasury
    )]
    pub treasury_x: Account<'info, TokenAccount>,
    #[account(
        mut,
        associated_token::mint = mint_y,
        associated_token::authority = config.treasury
    )]
    pub treasury_y: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

impl<'info> Swap<'info> {
    pub fn swap(&mut self, amount: u64, min_amount: u64, is_x: bool) -> Result<()> {
        require!(!self.config.locked, AmmError::PoolLocked);
        require!(amount > 0, AmmError::InvalidAmount);

        let (reserve_in, reserve_out) = if is_x {
            (self.vault_x.amount, self.vault_y.amount)
        } else {
            (self.vault_y.amount, self.vault_x.amount)
        };
        require!(
            reserve_in > 0 && reserve_out > 0,
            AmmError::NoLiquidityInPool
        );

        let protocol_cut = (amount as u128)
            .checked_mul(self.config.protocol_fee as u128)
            .ok_or(AmmError::Overflow)?
            .checked_div(10_000)
            .ok_or(AmmError::Overflow)? as u64;
        let net = amount
            .checked_sub(protocol_cut)
            .ok_or(AmmError::Underflow)?;


        let in_after_fee = (net as u128)
            .checked_mul(10_000u128 - self.config.fee as u128)
            .ok_or(AmmError::Overflow)?
            .checked_div(10_000)
            .ok_or(AmmError::Overflow)?;

        let denom = (reserve_in as u128)
            .checked_add(in_after_fee)
            .ok_or(AmmError::Overflow)?;
        let out = (reserve_out as u128)
            .checked_mul(in_after_fee)
            .ok_or(AmmError::Overflow)?
            .checked_div(denom)
            .ok_or(AmmError::Overflow)? as u64;

        require!(out > 0 && out >= min_amount, AmmError::SlippageExceeded);

        if protocol_cut > 0 {
            self.pay_treasury(is_x, protocol_cut)?;
        }
        self.deposit_tokens(is_x, net)?;
        self.withdraw_tokens(is_x, out)?;

        Ok(())
    }

    pub fn pay_treasury(&mut self, is_x: bool, amount: u64) -> Result<()> {
        let (from, to) = match is_x {
            true => (
                self.user_x.to_account_info(),
                self.treasury_x.to_account_info(),
            ),
            false => (
                self.user_y.to_account_info(),
                self.treasury_y.to_account_info(),
            ),
        };

        transfer(
            CpiContext::new(
                self.token_program.key(),
                Transfer {
                    from,
                    to,
                    authority: self.user.to_account_info(),
                },
            ),
            amount,
        )
    }

    pub fn deposit_tokens(&mut self, is_x: bool, amount: u64) -> Result<()> {
        let (from, to) = match is_x {
            true => (
                self.user_x.to_account_info(),
                self.vault_x.to_account_info(),
            ),
            false => (
                self.user_y.to_account_info(),
                self.vault_y.to_account_info(),
            ),
        };

        transfer(
            CpiContext::new(
                self.token_program.key(),
                Transfer {
                    from,
                    to,
                    authority: self.user.to_account_info(),
                },
            ),
            amount,
        )
    }

    pub fn withdraw_tokens(&mut self, is_x: bool, amount: u64) -> Result<()> {
        let (from, to) = match is_x {
            true => (
                self.vault_y.to_account_info(),
                self.user_y.to_account_info(),
            ),
            false => (
                self.vault_x.to_account_info(),
                self.user_x.to_account_info(),
            ),
        };

        transfer(
            CpiContext::new_with_signer(
                self.token_program.key(),
                Transfer {
                    from,
                    to,
                    authority: self.config.to_account_info(),
                },
                &[&[
                    b"config",
                    &self.config.seed.to_le_bytes(),
                    &[self.config.config_bump],
                ]],
            ),
            amount,
        )
    }
}
