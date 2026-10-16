use sqlx::PgPool;
use uuid::Uuid;

use crate::blockchain::{keypair, wallet_crypto};
use crate::models::{NewWallet, Wallet};

#[derive(Debug, thiserror::Error)]
pub enum CreateWalletError {
    #[error("failed to encrypt wallet secret: {0}")]
    Encryption(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub async fn create_wallet(
    db: &PgPool,
    merchant_id: Uuid,
    network: &str,
    encryption_key: &[u8; 32],
) -> Result<Wallet, CreateWalletError> {
    let generated = keypair::generate_keypair();
    let secret_key_encrypted = wallet_crypto::encrypt(encryption_key, &generated.secret_seed)
        .map_err(CreateWalletError::Encryption)?;

    let wallet = NewWallet {
        merchant_id,
        address: generated.public_address,
        network: network.to_string(),
        secret_key_encrypted,
    };
    sqlx::query_as::<_, Wallet>(
        "INSERT INTO wallets (merchant_id, address, network, secret_key_encrypted)
         VALUES ($1, $2, $3, $4)
         RETURNING id, merchant_id, address, network, created_at",
    )
    .bind(wallet.merchant_id)
    .bind(&wallet.address)
    .bind(&wallet.network)
    .bind(&wallet.secret_key_encrypted)
    .fetch_one(db)
    .await
    .map_err(CreateWalletError::from)
}

pub async fn wallet_by_merchant(
    db: &PgPool,
    merchant_id: Uuid,
) -> Result<Option<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at
           FROM wallets
          WHERE merchant_id = $1
          ORDER BY created_at DESC
          LIMIT 1",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

/// Fetch a single page of Stellar wallets using keyset (cursor) pagination.
///
/// `after_id` is the exclusive lower bound on `id`; pass `None` for the first
/// page. Results are ordered by `id ASC` and capped at `limit` rows, so each
/// query is bounded regardless of how many wallets exist. Callers should loop
/// until an empty (or short) page is returned.
///
/// Expected scale limit: with a batch size of 500 the deposit worker issues
/// `ceil(N / 500)` queries per poll cycle instead of materializing all `N`
/// wallet rows at once, keeping memory usage flat as the platform grows.
pub async fn all_wallets_page(
    db: &PgPool,
    after_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at
           FROM wallets
          WHERE network = 'stellar'
            AND ($1::uuid IS NULL OR id > $1)
          ORDER BY id ASC
          LIMIT $2",
    )
    .bind(after_id)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Convenience wrapper that loads every Stellar wallet by draining
/// [`all_wallets_page`] in batches. Prefer the paginated variant in hot paths
/// such as the deposit poll loop to avoid unbounded memory use.
pub async fn all_wallets(db: &PgPool) -> Result<Vec<Wallet>, sqlx::Error> {
    const BATCH_SIZE: i64 = 500;

    let mut wallets = Vec::new();
    let mut after_id: Option<Uuid> = None;

    loop {
        let page = all_wallets_page(db, after_id, BATCH_SIZE).await?;
        if page.is_empty() {
            break;
        }
        after_id = page.last().map(|wallet| wallet.id);
        let is_last_page = page.len() < BATCH_SIZE as usize;
        wallets.extend(page);
        if is_last_page {
            break;
        }
    }

    Ok(wallets)
}

pub async fn wallet_by_id(db: &PgPool, id: Uuid) -> Result<Option<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at FROM wallets WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn wallet_by_address(db: &PgPool, address: &str) -> Result<Option<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at FROM wallets WHERE address = $1",
    )
    .bind(address)
    .fetch_optional(db)
    .await
}
