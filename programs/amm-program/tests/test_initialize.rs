// Run `anchor build` first (tests load target/deploy/amm_program.so), then `cargo test`.
//
// Mints / token accounts are built with hand-crafted SPL instructions, so no
// extra dev-dependency (litesvm-token etc.) is needed.

use {
    anchor_lang::{
        prelude::Pubkey,
        solana_program::{
            instruction::{AccountMeta, Instruction},
            system_program,
        },
        AccountDeserialize, Id, InstructionData, ToAccountMetas,
    },
    anchor_spl::{associated_token::AssociatedToken, token::Token},
    litesvm::LiteSVM,
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
};

const DECIMALS: u8 = 6;
const MINT_RENT: u64 = 1_461_600; // rent-exempt minimum for an 82 byte mint
const FEE_BPS: u16 = 30;
const PROTOCOL_FEE_BPS: u16 = 10;

const INIT_LP: u64 = 141_421_356; // isqrt(INIT_X * INIT_Y), what the first deposit mints
const INIT_X: u64 = 100_000_000;
const INIT_Y: u64 = 200_000_000;

// ───────────────────────── low-level helpers ─────────────────────────

fn send(
    svm: &mut LiteSVM,
    ixs: &[Instruction],
    payer: &Keypair,
    extra: &[&Keypair],
) -> Result<(), String> {
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(ixs, Some(&payer.pubkey()), &blockhash);
    let mut signers: Vec<&Keypair> = vec![payer];
    signers.extend_from_slice(extra);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &signers[..]).unwrap();
    let res = svm
        .send_transaction(tx)
        .map(|_| ())
        .map_err(|e| format!("{:?}", e));
    // avoids AlreadyProcessed when the same ix is sent twice
    svm.expire_blockhash();
    res
}

fn assert_err(res: Result<(), String>, code: &str) {
    let e = res.expect_err("tx should have failed");
    if !code.is_empty() {
        assert!(e.contains(code), "expected `{code}`, got: {e}");
    }
}

fn ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[wallet.as_ref(), Token::id().as_ref(), mint.as_ref()],
        &AssociatedToken::id(),
    )
    .0
}

fn create_ata_ix(payer: Pubkey, wallet: Pubkey, mint: Pubkey) -> Instruction {
    Instruction {
        program_id: AssociatedToken::id(),
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(ata(&wallet, &mint), false),
            AccountMeta::new_readonly(wallet, false),
            AccountMeta::new_readonly(mint, false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(Token::id(), false),
        ],
        data: vec![1], // CreateIdempotent
    }
}

fn mint_to_ix(mint: Pubkey, dest: Pubkey, authority: Pubkey, amount: u64) -> Instruction {
    let mut data = vec![7u8]; // MintTo
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: Token::id(),
        accounts: vec![
            AccountMeta::new(mint, false),
            AccountMeta::new(dest, false),
            AccountMeta::new_readonly(authority, true),
        ],
        data,
    }
}

// ───────────────────────── test environment ─────────────────────────

struct Pool {
    seed: u64,
    config: Pubkey,
    mint_lp: Pubkey,
    vault_x: Pubkey,
    vault_y: Pubkey,
    treasury_x: Pubkey,
    treasury_y: Pubkey,
}

struct Env {
    svm: LiteSVM,
    admin: Keypair,
    treasury: Pubkey, // wallet that owns the treasury token accounts
    mint_x: Pubkey,
    mint_y: Pubkey,
}

impl Env {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        let bytes = include_bytes!(concat!(
            env!("CARGO_TARGET_TMPDIR"),
            "/../deploy/amm_program.so"
        ));
        svm.add_program(amm_program::id(), bytes).unwrap();

        let admin = Keypair::new();
        svm.airdrop(&admin.pubkey(), 100_000_000_000).unwrap();

        let mut env = Env {
            svm,
            admin,
            treasury: Keypair::new().pubkey(),
            mint_x: Pubkey::default(),
            mint_y: Pubkey::default(),
        };
        env.mint_x = env.create_mint();
        env.mint_y = env.create_mint();
        env
    }

    fn create_mint(&mut self) -> Pubkey {
        let mint = Keypair::new();

        // system program CreateAccount: u32 index 0, lamports, space, owner
        let mut data = vec![0u8; 4];
        data.extend_from_slice(&MINT_RENT.to_le_bytes());
        data.extend_from_slice(&82u64.to_le_bytes());
        data.extend_from_slice(Token::id().as_ref());
        let create = Instruction {
            program_id: system_program::ID,
            accounts: vec![
                AccountMeta::new(self.admin.pubkey(), true),
                AccountMeta::new(mint.pubkey(), true),
            ],
            data,
        };

        // token InitializeMint2: tag 20, decimals, authority, no freeze authority
        let mut init_data = vec![20u8, DECIMALS];
        init_data.extend_from_slice(self.admin.pubkey().as_ref());
        init_data.push(0);
        let init = Instruction {
            program_id: Token::id(),
            accounts: vec![AccountMeta::new(mint.pubkey(), false)],
            data: init_data,
        };

        send(&mut self.svm, &[create, init], &self.admin, &[&mint]).unwrap();
        mint.pubkey()
    }

    /// New funded user with ATAs for X and Y holding the given balances.
    fn fund_user(&mut self, x: u64, y: u64) -> Keypair {
        let user = Keypair::new();
        self.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
        let a = self.admin.pubkey();
        let u = user.pubkey();
        let ixs = vec![
            create_ata_ix(a, u, self.mint_x),
            create_ata_ix(a, u, self.mint_y),
            mint_to_ix(self.mint_x, ata(&u, &self.mint_x), a, x),
            mint_to_ix(self.mint_y, ata(&u, &self.mint_y), a, y),
        ];
        send(&mut self.svm, &ixs, &self.admin, &[]).unwrap();
        user
    }

    fn token_balance(&self, addr: &Pubkey) -> u64 {
        match self.svm.get_account(addr) {
            Some(acc) if acc.data.len() >= 72 => {
                u64::from_le_bytes(acc.data[64..72].try_into().unwrap())
            }
            _ => 0,
        }
    }

    fn lp_supply(&self, pool: &Pool) -> u64 {
        match self.svm.get_account(&pool.mint_lp) {
            Some(acc) if acc.data.len() >= 44 => {
                u64::from_le_bytes(acc.data[36..44].try_into().unwrap())
            }
            _ => 0,
        }
    }

    fn bal_x(&self, owner: &Pubkey) -> u64 {
        self.token_balance(&ata(owner, &self.mint_x))
    }
    fn bal_y(&self, owner: &Pubkey) -> u64 {
        self.token_balance(&ata(owner, &self.mint_y))
    }
    fn bal_lp(&self, pool: &Pool, owner: &Pubkey) -> u64 {
        self.token_balance(&ata(owner, &pool.mint_lp))
    }

    // ───── pool ─────

    fn pool_addrs(&self, seed: u64) -> Pool {
        let config =
            Pubkey::find_program_address(&[b"config", &seed.to_le_bytes()], &amm_program::id()).0;
        let mint_lp = Pubkey::find_program_address(&[b"lp", config.as_ref()], &amm_program::id()).0;
        Pool {
            seed,
            config,
            mint_lp,
            vault_x: ata(&config, &self.mint_x),
            vault_y: ata(&config, &self.mint_y),
            treasury_x: ata(&self.treasury, &self.mint_x),
            treasury_y: ata(&self.treasury, &self.mint_y),
        }
    }

    fn try_init_pool(
        &mut self,
        seed: u64,
        fee: u16,
        protocol_fee: u16,
        authority: Option<Pubkey>,
    ) -> Result<(), String> {
        let pool = self.pool_addrs(seed);
        let ix = Instruction::new_with_bytes(
            amm_program::id(),
            &amm_program::instruction::Initialize {
                seed,
                fee,
                protocol_fee,
                authority,
            }
            .data(),
            amm_program::accounts::Initialize {
                initializer: self.admin.pubkey(),
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                mint_lp: pool.mint_lp,
                vault_x: pool.vault_x,
                vault_y: pool.vault_y,
                config: pool.config,
                treasury: self.treasury,
                treasury_x: pool.treasury_x,
                treasury_y: pool.treasury_y,
                token_program: Token::id(),
                associated_token_program: AssociatedToken::id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        );
        send(&mut self.svm, &[ix], &self.admin, &[])
    }

    fn init_pool(
        &mut self,
        seed: u64,
        fee: u16,
        protocol_fee: u16,
        authority: Option<Pubkey>,
    ) -> Pool {
        self.try_init_pool(seed, fee, protocol_fee, authority)
            .unwrap();
        self.pool_addrs(seed)
    }

    fn lp_accounts(&self, pool: &Pool, user: &Pubkey) -> Vec<AccountMeta> {
        amm_program::accounts::LiquidityPool {
            user: *user,
            mint_x: self.mint_x,
            mint_y: self.mint_y,
            config: pool.config,
            mint_lp: pool.mint_lp,
            vault_x: pool.vault_x,
            vault_y: pool.vault_y,
            user_x: ata(user, &self.mint_x),
            user_y: ata(user, &self.mint_y),
            user_lp: ata(user, &pool.mint_lp),
            token_program: Token::id(),
            system_program: system_program::ID,
            associated_token_program: AssociatedToken::id(),
        }
        .to_account_metas(None)
    }

    fn deposit(
        &mut self,
        pool: &Pool,
        user: &Keypair,
        amount: u64,
        max_x: u64,
        max_y: u64,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            amm_program::id(),
            &amm_program::instruction::Deposit {
                amount,
                max_x,
                max_y,
            }
            .data(),
            self.lp_accounts(pool, &user.pubkey()),
        );
        send(&mut self.svm, &[ix], user, &[])
    }

    fn withdraw(
        &mut self,
        pool: &Pool,
        user: &Keypair,
        amount: u64,
        min_x: u64,
        min_y: u64,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            amm_program::id(),
            &amm_program::instruction::Withdraw {
                amount,
                min_x,
                min_y,
            }
            .data(),
            self.lp_accounts(pool, &user.pubkey()),
        );
        send(&mut self.svm, &[ix], user, &[])
    }

    fn swap(
        &mut self,
        pool: &Pool,
        user: &Keypair,
        amount: u64,
        min_amount: u64,
        is_x: bool,
    ) -> Result<(), String> {
        let u = user.pubkey();
        let ix = Instruction::new_with_bytes(
            amm_program::id(),
            &amm_program::instruction::Swap {
                amount,
                min_amount,
                is_x,
            }
            .data(),
            amm_program::accounts::Swap {
                user: u,
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                config: pool.config,
                mint_lp: pool.mint_lp,
                vault_x: pool.vault_x,
                vault_y: pool.vault_y,
                user_x: ata(&u, &self.mint_x),
                user_y: ata(&u, &self.mint_y),
                treasury_x: pool.treasury_x,
                treasury_y: pool.treasury_y,
                token_program: Token::id(),
                system_program: system_program::ID,
                associated_token_program: AssociatedToken::id(),
            }
            .to_account_metas(None),
        );
        send(&mut self.svm, &[ix], user, &[])
    }

    fn set_lock(&mut self, pool: &Pool, signer: &Keypair, lock: bool) -> Result<(), String> {
        let metas = amm_program::accounts::Update {
            authority: signer.pubkey(),
            config: pool.config,
        }
        .to_account_metas(None);
        let data = if lock {
            amm_program::instruction::Lock {}.data()
        } else {
            amm_program::instruction::Unlock {}.data()
        };
        let ix = Instruction::new_with_bytes(amm_program::id(), &data, metas);
        send(&mut self.svm, &[ix], signer, &[])
    }
}

/// Env + initialized pool (authority = admin) + a liquidity provider who made the first deposit.
fn seeded_pool() -> (Env, Pool, Keypair) {
    let mut env = Env::new();
    let admin = env.admin.pubkey();
    let pool = env.init_pool(1, FEE_BPS, PROTOCOL_FEE_BPS, Some(admin));
    let lp = env.fund_user(1_000_000_000, 1_000_000_000);
    env.deposit(&pool, &lp, INIT_LP, INIT_X, INIT_Y).unwrap();
    (env, pool, lp)
}

// ───────────────────────── initialize ─────────────────────────

#[test]
fn initialize_stores_config() {
    let mut env = Env::new();
    let admin = env.admin.pubkey();
    let pool = env.init_pool(7, FEE_BPS, PROTOCOL_FEE_BPS, Some(admin));

    let acc = env.svm.get_account(&pool.config).unwrap();
    let mut data: &[u8] = &acc.data;
    let cfg = amm_program::state::Config::try_deserialize(&mut data).unwrap();

    assert_eq!(cfg.seed, 7);
    assert_eq!(cfg.fee, FEE_BPS);
    assert_eq!(cfg.protocol_fee, PROTOCOL_FEE_BPS);
    assert_eq!(cfg.treasury, env.treasury);
    assert_eq!(cfg.authority, Some(admin));
    assert_eq!(cfg.mint_x, env.mint_x);
    assert_eq!(cfg.mint_y, env.mint_y);
    assert!(!cfg.locked);

    // vaults + LP mint exist and are empty
    assert_eq!(env.token_balance(&pool.vault_x), 0);
    assert_eq!(env.token_balance(&pool.vault_y), 0);
    assert_eq!(env.lp_supply(&pool), 0);
    assert_eq!(env.token_balance(&pool.treasury_x), 0);
    assert_eq!(env.token_balance(&pool.treasury_y), 0);
}

#[test]
fn initialize_rejects_fee_over_100_percent() {
    let mut env = Env::new();
    let res = env.try_init_pool(1, 10_001, 0, None);
    assert_err(res, "InvalidFee");
}

#[test]
fn initialize_rejects_combined_fees_over_100_percent() {
    let mut env = Env::new();
    // 9_990 + 11 = 10_001 bps
    let res = env.try_init_pool(1, 9_990, 11, None);
    assert_err(res, "InvalidFee");
}

// ───────────────────────── deposit ─────────────────────────

#[test]
fn first_deposit_fills_vaults_and_mints_lp() {
    let (env, pool, lp) = seeded_pool();
    let u = lp.pubkey();

    assert_eq!(env.token_balance(&pool.vault_x), INIT_X);
    assert_eq!(env.token_balance(&pool.vault_y), INIT_Y);
    assert_eq!(env.bal_lp(&pool, &u), INIT_LP);
    assert_eq!(env.lp_supply(&pool), INIT_LP);
    assert_eq!(env.bal_x(&u), 1_000_000_000 - INIT_X);
    assert_eq!(env.bal_y(&u), 1_000_000_000 - INIT_Y);
}

#[test]
fn first_deposit_cannot_claim_more_lp_than_sqrt() {
    let mut env = Env::new();
    let admin = env.admin.pubkey();
    let pool = env.init_pool(9, FEE_BPS, PROTOCOL_FEE_BPS, Some(admin));
    let lp = env.fund_user(1_000_000_000, 1_000_000_000);
    // claiming 10x the LP the deposit is worth must fail
    let res = env.deposit(&pool, &lp, INIT_LP * 10, INIT_X, INIT_Y);
    assert_err(res, "SlippageExceeded");
    assert_eq!(env.lp_supply(&pool), 0);
}

#[test]
fn second_deposit_follows_pool_ratio() {
    let (mut env, pool, lp) = seeded_pool();
    let u = lp.pubkey();
    let (x0, y0) = (env.bal_x(&u), env.bal_y(&u));

    let lp_out = 10_000_000u64;
    env.deposit(&pool, &lp, lp_out, 20_000_000, 40_000_000)
        .unwrap();

    // required amounts round UP so the pool never loses
    let ex = (INIT_X as u128 * lp_out as u128).div_ceil(INIT_LP as u128) as u64;
    let ey = (INIT_Y as u128 * lp_out as u128).div_ceil(INIT_LP as u128) as u64;
    let dx = x0 - env.bal_x(&u);
    let dy = y0 - env.bal_y(&u);
    assert_eq!(dx, ex);
    assert_eq!(dy, ey);
    assert_eq!(env.bal_lp(&pool, &u), INIT_LP + 10_000_000);
    assert_eq!(env.lp_supply(&pool), INIT_LP + 10_000_000);
    assert_eq!(env.token_balance(&pool.vault_x), INIT_X + dx);
    assert_eq!(env.token_balance(&pool.vault_y), INIT_Y + dy);
}

#[test]
fn deposit_fails_on_slippage() {
    let (mut env, pool, lp) = seeded_pool();
    // needs ~10M X but we only allow 1M
    let res = env.deposit(&pool, &lp, 10_000_000, 1_000_000, 40_000_000);
    assert_err(res, "SlippageExceeded");
}

#[test]
fn deposit_fails_on_zero_amount() {
    let (mut env, pool, lp) = seeded_pool();
    let res = env.deposit(&pool, &lp, 0, 1_000, 1_000);
    assert_err(res, "InvalidAmount");
}

// ───────────────────────── withdraw ─────────────────────────

#[test]
fn withdraw_burns_lp_and_returns_tokens() {
    let (mut env, pool, lp) = seeded_pool();
    let u = lp.pubkey();
    let (x0, y0) = (env.bal_x(&u), env.bal_y(&u));

    env.withdraw(&pool, &lp, INIT_LP / 2, 0, 0).unwrap();

    let dx = env.bal_x(&u) - x0;
    let dy = env.bal_y(&u) - y0;
    assert!((49_999_000..=50_001_000).contains(&dx), "dx = {dx}");
    assert!((99_998_000..=100_002_000).contains(&dy), "dy = {dy}");
    assert_eq!(env.bal_lp(&pool, &u), INIT_LP / 2);
    assert_eq!(env.lp_supply(&pool), INIT_LP / 2);
    assert_eq!(env.token_balance(&pool.vault_x), INIT_X - dx);
    assert_eq!(env.token_balance(&pool.vault_y), INIT_Y - dy);
}

#[test]
fn withdraw_ten_percent_is_exact() {
    let (mut env, pool, lp) = seeded_pool();
    let u = lp.pubkey();
    let (x0, y0) = (env.bal_x(&u), env.bal_y(&u));
    let lp_in = INIT_LP / 10;
    env.withdraw(&pool, &lp, lp_in, 0, 0).unwrap();
    // payouts round DOWN so the pool never loses
    let ex = (INIT_X as u128 * lp_in as u128 / INIT_LP as u128) as u64;
    let ey = (INIT_Y as u128 * lp_in as u128 / INIT_LP as u128) as u64;
    assert_eq!(env.bal_x(&u) - x0, ex);
    assert_eq!(env.bal_y(&u) - y0, ey);
}

#[test]
fn withdraw_fails_on_slippage() {
    let (mut env, pool, lp) = seeded_pool();
    // half the pool is ~50M X, asking for at least 60M must fail
    let res = env.withdraw(&pool, &lp, INIT_LP / 2, 60_000_000, 0);
    assert_err(res, "SlippageExceeded");
}

#[test]
fn withdraw_fails_on_zero_amount() {
    let (mut env, pool, lp) = seeded_pool();
    let res = env.withdraw(&pool, &lp, 0, 0, 0);
    assert_err(res, "InvalidAmount");
}

#[test]
fn withdraw_more_than_owned_lp_fails() {
    let (mut env, pool, lp) = seeded_pool();
    let res = env.withdraw(&pool, &lp, INIT_LP + 1, 0, 0);
    assert_err(res, "");

    // pool untouched
    assert_eq!(env.token_balance(&pool.vault_x), INIT_X);
    assert_eq!(env.lp_supply(&pool), INIT_LP);
}

// ───────────────────────── swap ─────────────────────────

#[test]
fn swap_x_for_y() {
    let (mut env, pool, _lp) = seeded_pool();
    let trader = env.fund_user(10_000_000, 0);
    let t = trader.pubkey();
    let amount = 1_000_000u64;

    let k_before = env.token_balance(&pool.vault_x) as u128 * env.token_balance(&pool.vault_y) as u128;

    env.swap(&pool, &trader, amount, 0, true).unwrap();

    let cut = amount * PROTOCOL_FEE_BPS as u64 / 10_000; // protocol share of the input
    assert_eq!(env.bal_x(&t), 10_000_000 - amount);
    let got_y = env.bal_y(&t);
    // spot price is 2 Y per X; fee + price impact => strictly less than 2x, but > 0
    assert!(got_y > 0 && got_y < amount * 2, "got_y = {got_y}");
    assert_eq!(env.token_balance(&pool.vault_x), INIT_X + amount - cut);
    assert_eq!(env.token_balance(&pool.vault_y), INIT_Y - got_y);
    // protocol fee lands in treasury, in the INPUT token only
    assert_eq!(env.token_balance(&pool.treasury_x), cut);
    assert_eq!(env.token_balance(&pool.treasury_y), 0);

    let k_after = env.token_balance(&pool.vault_x) as u128 * env.token_balance(&pool.vault_y) as u128;
    assert!(k_after >= k_before, "k decreased: {k_before} -> {k_after}");
}

#[test]
fn swap_y_for_x() {
    let (mut env, pool, _lp) = seeded_pool();
    let trader = env.fund_user(0, 10_000_000);
    let t = trader.pubkey();
    let amount = 2_000_000u64;

    let k_before = env.token_balance(&pool.vault_x) as u128 * env.token_balance(&pool.vault_y) as u128;

    env.swap(&pool, &trader, amount, 0, false).unwrap();

    let cut = amount * PROTOCOL_FEE_BPS as u64 / 10_000;
    assert_eq!(env.bal_y(&t), 10_000_000 - amount);
    let got_x = env.bal_x(&t);
    // spot price is 0.5 X per Y
    assert!(got_x > 0 && got_x < amount / 2, "got_x = {got_x}");
    assert_eq!(env.token_balance(&pool.vault_y), INIT_Y + amount - cut);
    assert_eq!(env.token_balance(&pool.vault_x), INIT_X - got_x);
    assert_eq!(env.token_balance(&pool.treasury_y), cut);
    assert_eq!(env.token_balance(&pool.treasury_x), 0);

    let k_after = env.token_balance(&pool.vault_x) as u128 * env.token_balance(&pool.vault_y) as u128;
    assert!(k_after >= k_before, "k decreased: {k_before} -> {k_after}");
}

#[test]
fn swap_fails_on_slippage() {
    let (mut env, pool, _lp) = seeded_pool();
    let trader = env.fund_user(10_000_000, 0);
    // 1M X can never return 5M Y at this price
    let res = env.swap(&pool, &trader, 1_000_000, 5_000_000, true);
    assert_err(res, "SlippageExceeded");
    assert_eq!(env.bal_x(&trader.pubkey()), 10_000_000); // nothing moved
}

#[test]
fn swap_fails_on_zero_amount() {
    let (mut env, pool, _lp) = seeded_pool();
    let trader = env.fund_user(10_000_000, 0);
    let res = env.swap(&pool, &trader, 0, 0, true);
    assert_err(res, "InvalidAmount");
}

#[test]
fn treasury_accumulates_across_swaps() {
    let (mut env, pool, _lp) = seeded_pool();
    let trader = env.fund_user(10_000_000, 10_000_000);

    env.swap(&pool, &trader, 1_000_000, 0, true).unwrap();
    env.swap(&pool, &trader, 3_000_000, 0, true).unwrap();
    env.swap(&pool, &trader, 2_000_000, 0, false).unwrap();

    let cut = |a: u64| a * PROTOCOL_FEE_BPS as u64 / 10_000;
    assert_eq!(
        env.token_balance(&pool.treasury_x),
        cut(1_000_000) + cut(3_000_000)
    );
    assert_eq!(env.token_balance(&pool.treasury_y), cut(2_000_000));
}

#[test]
fn zero_protocol_fee_sends_nothing_to_treasury() {
    let mut env = Env::new();
    let admin = env.admin.pubkey();
    let pool = env.init_pool(5, FEE_BPS, 0, Some(admin));
    let lp = env.fund_user(1_000_000_000, 1_000_000_000);
    env.deposit(&pool, &lp, INIT_LP, INIT_X, INIT_Y).unwrap();
    let trader = env.fund_user(10_000_000, 0);

    env.swap(&pool, &trader, 1_000_000, 0, true).unwrap();

    assert_eq!(env.token_balance(&pool.treasury_x), 0);
    assert_eq!(env.token_balance(&pool.vault_x), INIT_X + 1_000_000);
}

// ───────────────────────── lock / unlock ─────────────────────────

#[test]
fn locked_pool_blocks_everything_until_unlocked() {
    let mut env = Env::new();
    let auth = env.fund_user(0, 0); // separate keypair that holds lock authority
    let pool = env.init_pool(3, FEE_BPS, PROTOCOL_FEE_BPS, Some(auth.pubkey()));
    let lp = env.fund_user(1_000_000_000, 1_000_000_000);
    env.deposit(&pool, &lp, INIT_LP, INIT_X, INIT_Y).unwrap();
    let trader = env.fund_user(10_000_000, 0);

    env.set_lock(&pool, &auth, true).unwrap();

    assert_err(
        env.deposit(&pool, &lp, 1_000_000, 10_000_000, 10_000_000),
        "PoolLocked",
    );
    assert_err(env.withdraw(&pool, &lp, 1_000_000, 0, 0), "PoolLocked");
    assert_err(env.swap(&pool, &trader, 1_000_000, 0, true), "PoolLocked");

    env.set_lock(&pool, &auth, false).unwrap();
    env.swap(&pool, &trader, 1_000_000, 0, true).unwrap();
}

#[test]
fn lock_fails_for_wrong_signer() {
    let (mut env, pool, _lp) = seeded_pool();
    let stranger = env.fund_user(0, 0);
    let res = env.set_lock(&pool, &stranger, true);
    assert_err(res, "InvalidAuthority");
}

#[test]
fn lock_fails_when_pool_has_no_authority() {
    let mut env = Env::new();
    let pool = env.init_pool(2, FEE_BPS, PROTOCOL_FEE_BPS, None);
    let stranger = env.fund_user(0, 0);
    let res = env.set_lock(&pool, &stranger, true);
    assert_err(res, "NoAuthoritySet");
}