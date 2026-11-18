use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{burn, mint_to, transfer, Burn, Mint, MintTo, Token, TokenAccount, Transfer},
};

use crate::error::AmmError;
use crate::state::Config;

fn isqrt(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

fn mul_div_floor(a: u64, b: u64, d: u64) -> Result<u64> {
    let r = (a as u128)
        .checked_mul(b as u128)
        .ok_or(AmmError::Overflow)?
        .checked_div(d as u128)
        .ok_or(AmmError::Overflow)?;
    Ok(u64::try_from(r).map_err(|_| AmmError::Overflow)?)
}

fn mul_div_ceil(a: u64, b: u64, d: u64) -> Result<u64> {
    let r = (a as u128)
        .checked_mul(b as u128)
        .ok_or(AmmError::Overflow)?
        .checked_add(d as u128 - 1)
        .ok_or(AmmError::Overflow)?
        .checked_div(d as u128)
        .ok_or(AmmError::Overflow)?;
    Ok(u64::try_from(r).map_err(|_| AmmError::Overflow)?)
}

#[derive(Accounts)]
pub struct LiquidityPool<'info> {
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
        init_if_needed,
        payer = user,
        associated_token::mint = mint_lp,
        associated_token::authority = user
    )]
    pub user_lp: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

impl<'info> LiquidityPool<'info> {
    pub fn deposit(&mut self, amount: u64, max_x: u64, max_y: u64) -> Result<()> {
        require!(!self.config.locked, AmmError::PoolLocked);
        require_neq!(amount, 0, AmmError::InvalidAmount);

        let supply = self.mint_lp.supply;
        let (x, y, lp) = if supply == 0 && self.vault_x.amount == 0 && self.vault_y.amount == 0 {
            // first deposit: LP is derived from the deposit, not chosen by the user.
            // `amount` acts as the minimum LP the user will accept.
            require!(max_x > 0 && max_y > 0, AmmError::InvalidAmount);
            let minted = u64::try_from(isqrt(max_x as u128 * max_y as u128))
                .map_err(|_| AmmError::Overflow)?;
            require!(minted >= amount, AmmError::SlippageExceeded);
            (max_x, max_y, minted)
        } else {
            require!(supply > 0, AmmError::NoLiquidityInPool);
            // required amounts round UP so the pool never loses
            let x = mul_div_ceil(self.vault_x.amount, amount, supply)?;
            let y = mul_div_ceil(self.vault_y.amount, amount, supply)?;
            require!(x > 0 && y > 0, AmmError::InvalidAmount);
            require!(x <= max_x && y <= max_y, AmmError::SlippageExceeded);
            (x, y, amount)
        };

        self.deposit_tokens(true, x)?;
        self.deposit_tokens(false, y)?;
        self.mint_lp_tokens(lp)
    }

    pub fn withdraw(&mut self, amount: u64, min_x: u64, min_y: u64) -> Result<()> {
        require!(!self.config.locked, AmmError::PoolLocked);
        require!(amount > 0, AmmError::InvalidAmount);

        let supply = self.mint_lp.supply;
        require!(amount <= supply, AmmError::InsufficientBalance);

        // payouts round DOWN so the pool never loses, computed BEFORE the burn
        let x = mul_div_floor(self.vault_x.amount, amount, supply)?;
        let y = mul_div_floor(self.vault_y.amount, amount, supply)?;
        require!(x >= min_x && y >= min_y, AmmError::SlippageExceeded);

        self.burn_lp_tokens(amount)?;
        self.return_tokens(true, x)?;
        self.return_tokens(false, y)
    }

    pub fn burn_lp_tokens(&self, amount: u64) -> Result<()> {
        let cpi_accounts = Burn {
            mint: self.mint_lp.to_account_info(),
            from: self.user_lp.to_account_info(),
            authority: self.user.to_account_info(),
        };

        let ctx = CpiContext::new(self.token_program.key(), cpi_accounts);

        burn(ctx, amount)
    }
    pub fn return_tokens(&self, is_x: bool, amount: u64) -> Result<()> {
        let (from, to) = match is_x {
            true => (
                self.vault_x.to_account_info(),
                self.user_x.to_account_info(),
            ),
            false => (
                self.vault_y.to_account_info(),
                self.user_y.to_account_info(),
            ),
        };

        let cpi_accounts = Transfer {
            from,
            to,
            authority: self.config.to_account_info(),
        };

        let signer_seeds: &[&[&[u8]]] = &[&[
            b"config",
            &self.config.seed.to_le_bytes(),
            &[self.config.config_bump],
        ]];

        let ctx = CpiContext::new_with_signer(self.token_program.key(), cpi_accounts, signer_seeds);

        transfer(ctx, amount)
    }

    pub fn deposit_tokens(&self, is_x: bool, amount: u64) -> Result<()> {
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

        let cpi_program = self.token_program.key();

        let cpi_accounts = Transfer {
            from,
            to,
            authority: self.user.to_account_info(),
        };

        let ctx = CpiContext::new(cpi_program, cpi_accounts);

        transfer(ctx, amount)
    }

    pub fn mint_lp_tokens(&self, amount: u64) -> Result<()> {
        let cpi_program = self.token_program.key();

        let cpi_accounts = MintTo {
            mint: self.mint_lp.to_account_info(),
            to: self.user_lp.to_account_info(),
            authority: self.config.to_account_info(),
        };

        let signer_seeds: &[&[&[u8]]] = &[&[
            b"config",
            &self.config.seed.to_le_bytes(),
            &[self.config.config_bump],
        ]];

        let ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);

        mint_to(ctx, amount)
    }
}
