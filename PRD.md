# Aframp — Product Requirements Document

**Status:** Living document · **Last updated:** 2026-08-10 · **Scope:** Aframp Pay backend (this repository)

This PRD exists because the backend has now reached a point where we have real, verified clarity on what works — not just what was planned. Every status claim below reflects something exercised end-to-end (real Postgres writes, a real Stellar testnet transaction), not a reading of the code. Where we don't yet have an answer, that's marked as an open decision rather than papered over.

---

## 1. Overview

Aframp is a merchant point-of-sale acceptance network for Stellar-powered payments, starting in Nigeria. The insight: Nigerians already understand POS terminals and bank-transfer payments ("give me your account number," "use the POS"). Aframp adds **scan and pay** as a familiar-feeling option, settled on Stellar, without merchants or customers needing to understand blockchain concepts.

```
Customer → Scan → Pay → Confirmed
```

From the merchant's point of view, that's the whole product. Stellar is the settlement layer underneath; Aframp is the experience on top.

**Planned product layers** (long-term — this repo is the backend under the first one):

| Layer | Purpose |
|---|---|
| Aframp Pay | Merchant-facing: payment requests, QR codes, receive Stellar payments, receipts, revenue tracking |
| Aframp Wallet | Consumer wallet built for spending, not just holding |
| Aframp Business | Merchant dashboard: analytics, reconciliation, multi-location, invoices |
| Aframp API | Infrastructure layer for other African fintechs to integrate Stellar payments |

## 2. Problem

Cross-border commerce in Africa is fragmented across national payment systems — multiple currencies, high fees, slow settlement, and merchant onboarding that doesn't travel across borders. Stablecoins and blockchain rails already move value globally, fast and cheap. What's missing is the everyday, physical-commerce bridge between the two.

Aframp's bet: onboard merchants first with a familiar payment experience, and let consumer demand for Stellar wallets follow — merchant acceptance drives adoption, not the other way around.

## 3. Users

| User | Role in MVP | Notes |
|---|---|---|
| **Merchant** | Primary user of this backend | Signs up, receives a Stellar wallet Aframp custodies on their behalf, watches balance grow as customers pay, withdraws to a bank account |
| **Customer** | Pays a merchant | Not a user of this backend directly — assumed to bring any Stellar-compatible wallet. No Aframp Wallet app exists yet (post-MVP) |
| **Aframp (platform)** | Custodian + settlement operator | Holds encrypted keys for every merchant wallet; will eventually sweep funds to a platform settlement wallet (not built yet — see [§9](#9-open-decisions)) |

## 4. Goals — MVP scope

The MVP is scoped to exactly seven capabilities, in the order they occur in the payment flow. Status reflects what's been verified in this repository as of the date above.

| # | Capability | Requirement | Status |
|---|---|---|---|
| 1 | Merchant account | A merchant can create an account and authenticate | ✅ **Done** — signup/login, Argon2-hashed passwords, JWT sessions. Every subsequent endpoint requires a valid token; verified live that no-token and garbage-token requests both return `401`. |
| 2 | Payment request generation | A merchant can generate a request for a specific amount | ✅ **Done** — `POST /payment-requests` creates a request tied to the merchant's wallet, with a unique correlation memo and a configurable expiry (15min default). `GET /payment-requests/{id}` is deliberately public (no auth) so a customer's wallet can read it before paying. |
| 3 | QR-based payment | A customer can scan a QR code to pay | ✅ **Done for XLM** — the request response includes a SEP-0007 `web+stellar:pay` URI any Stellar wallet can open directly. Verified live: correct destination, correct amount conversion (25,000,000 stroops → `2.5000000` in the URI). **Not yet for cNGN** — no real cNGN issuer address is configured (see [§9.3](#9-open-decisions)), so a credit-asset request returns `sep7_uri: null` rather than a guessed/wrong issuer. Detection also now correlates a specific incoming payment to its request via Stellar memo, not just "something arrived." |
| 4 | Stellar transaction creation | The system can create a Stellar-native receiving address | ✅ **Done, narrower than the name implies** — `/wallet/create` generates a real ed25519 keypair, encoded as a genuine Stellar `G...` address. Verified against Horizon directly. The platform does **not** yet build or sign outbound Stellar transactions anywhere (no XDR construction, no submission) — this row covers wallet/address creation, not general transaction authoring. |
| 5 | Transaction monitoring | The system detects incoming Stellar payments | ✅ **Done** — a background worker polls Horizon per merchant wallet on a timer, handling both `create_account` (how a brand-new wallet is always first funded on Stellar) and `payment`/`path_payment` ops. Proven with a real testnet transaction: funded a generated wallet via Stellar's friendbot and watched the exact transaction hash and amount land in the system within one poll cycle. |
| 6 | Payment confirmation | A detected payment is confirmed back to the merchant | 🚧 **Working, but simplified** — detected deposits move `detected → verified → confirmed` and update the merchant's balance immediately. There is no real "wait N ledger confirmations" threshold yet (open TODO in `blockchain/worker.rs`). |
| 7 | Merchant transaction history | A merchant can see their payment history | ✅ **Done** — `GET /transactions` returns real detected payments, paginated. |

**Also implemented, one layer past the original MVP list:** fiat withdrawal requests. `POST /withdraw` atomically debits a merchant's available balance and records a withdrawal in the same DB transaction, with insufficient-balance and validation checks enforced server-side. **However:** no money actually leaves the system yet — there's no real payout provider wired in (see [§9](#9-open-decisions)). This is ledger accounting, not a working cash-out.

## 5. Non-goals for MVP

Explicitly out of scope right now (per founder direction, not an oversight):

- KYC/AML and compliance depth
- Multi-region / multi-currency infrastructure beyond cNGN/XLM on Stellar testnet
- Analytics dashboards (Aframp Business layer)
- Physical POS hardware
- Cross-border payment corridors
- Public-facing API for third-party fintechs (Aframp API layer)
- A consumer-facing Aframp Wallet app

## 6. Core user flows

**Merchant onboarding**
1. Merchant signs up (`POST /signup`) → gets a JWT session.
2. Merchant creates a wallet (`POST /wallet/create`) → Aframp generates a real Stellar keypair, returns the public address, encrypts and stores the private key server-side (custodial model — not "bring your own wallet").

**Payment (deposit) flow — as it works today**
1. Merchant creates a payment request for an amount (`POST /payment-requests`) → gets back a destination address, a correlation memo, and (for XLM) a scannable SEP-0007 URI.
2. Customer opens the request (`GET /payment-requests/{id}`, no auth needed) or scans the QR, and pays with any Stellar-compatible wallet, including the memo.
3. The background worker's next poll cycle (default every 60s, configurable) detects the payment via Horizon — now fetched with the parent transaction joined in, so the memo comes back with it.
4. The payment is recorded, moved through `detected → verified → confirmed`, and the merchant's balance updates. If the memo matches a pending request, that request is marked `paid` and linked to the real payment — not just a raw balance change.
5. Merchant sees the confirmed payment in `GET /balance` and `GET /transactions`; anyone with the request ID sees it flip to `paid` in `GET /payment-requests/{id}`.

A payment can still land without going through a request at all (manually, or via friendbot), and it will still be detected and credited — request correlation is an enhancement, not a requirement for detection.

## 7. Planned features (not yet implemented)

These are tracked here so the schema and models that anticipate them aren't mistaken for dead code. Nothing in this section is wired to an endpoint yet.

### 7.1 API key authentication

The `0001_init.sql` migration creates an `api_keys` table and `src/models/api_key.rs` defines an `ApiKey` model, but **no API key endpoints exist yet** — there is no way to create, list, or revoke a key, and no request path authenticates via API key. This is planned infrastructure for the Aframp API layer (third-party fintech integrations), not a working feature.

Planned scope (tracked separately, including multi-environment keys):

- `POST /api-keys` to mint a key for the authenticated merchant
- `GET /api-keys` to list a merchant's keys (without exposing the secret)
- `DELETE /api-keys/{id}` to revoke a key
- API-key authentication as an alternative to JWT on merchant endpoints
- Multi-environment keys (e.g. test vs. live) — see the related issue

Until then, `WEBHOOK_SECRET` is used only for signing outbound webhooks; it is **not** an API key authentication mechanism.

## 8. Success metrics

- A merchant can go from signup to a confirmed, memo-correlated payment without leaving the API.
- Every confirmed payment is traceable to a real Stellar transaction hash.
- No merchant ever sees a balance that isn't backed by a detected on-chain deposit minus recorded withdrawals.

## 9. Open decisions
## 11. Risks

- **Custodial key risk.** Aframp holds every merchant's private key. `WALLET_ENCRYPTION_KEY` compromise would expose every merchant's funds. Key management (rotation, HSM/KMS vs. env var) needs to mature before real merchant funds are at stake.
- **No real payout yet.** If withdrawal UX ships before a real payout provider is wired in, merchants would see a debited balance with no way to actually receive funds — a trust-destroying gap if exposed prematurely.
- **Poll-interval latency.** A 60-second default poll interval means a merchant could wait up to a minute to see a payment confirmed — worth deciding if that's acceptable for a POS "instant" feel, or needs tightening.
- **Regulatory scope creep.** Custodying every merchant's private key, converting crypto to fiat through a platform-controlled treasury, and fanning fiat back out to individual merchant bank accounts is, functionally, the deposit-taking and payment-transmission core of a bank — not just a payments API. Worth an explicit read on Nigerian licensing exposure (money transmission / PSP / MFB territory) before Stage A/B in §9.1 goes live with real merchant funds, not after.

## 12. Roadmap — near-term, ordered

1. Settlement & payout pipeline: sweep merchant wallets → redeem cNGN via the issuer → last-mile payout via Paystack/Flutterwave (closes the "ledger says X, bank account has 0" gap — see §9.1). Full implementation plan in [`docs/settlement-sweep-wallet.md`](docs/settlement-sweep-wallet.md) (issue [#1276](https://github.com/kellymusk/Aframp-backend/issues/1276))
2. Source a real cNGN issuer Stellar address (unblocks cNGN QR codes and cNGN-denominated payment requests — see §9.4)
3. Confirmation-depth policy decision + implementation
4. Cleanup: remove or repurpose `src/stellar/mod.rs`

---

1. **Settlement sweeping** — when and how funds move from per-merchant wallets to a platform settlement wallet.
2. **Confirmation depth** — how many ledger confirmations count as "confirmed" (currently immediate).
3. **cNGN issuer** — which issuer address to trust for cNGN on testnet/mainnet.
4. **Payout provider** — which provider actually moves fiat for withdrawals.
5. **API key environments** — whether keys are scoped per environment or per merchant (see [§7.1](#71-api-key-authentication)).
