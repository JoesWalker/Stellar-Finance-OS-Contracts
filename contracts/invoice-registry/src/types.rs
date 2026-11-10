//! Data types and errors for the invoice registry. Shapes follow README §4.
//!
//! Only hashes and the payable amount go on-chain: no customer names or
//! personal data.

use soroban_sdk::{contracterror, contracttype, Address, BytesN};

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvoiceStatus {
    Open,
    Paid,
    Void,
    Closed,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invoice {
    /// Backend-generated hash identifying the invoice. Unique per `business`,
    /// not globally.
    pub id: BytesN<32>,
    /// Controller: signs `register`, and is the only address that may `void`
    /// or `close` the invoice.
    pub business: Address,
    /// SAC address of the asset to be paid (e.g. USDC).
    pub token: Address,
    pub amount: i128,
    pub paid: i128,
    pub status: InvoiceStatus,
    /// Backend-generated hash of the invoice document.
    pub content_hash: BytesN<32>,
    /// Receives every payment. May equal `business` or be any other address
    /// or contract (e.g. an approval vault), but never the registry itself.
    pub payee: Address,
}

/// Error codes are part of the public interface: never renumber, only append.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    InvoiceExists = 1,
    InvoiceNotFound = 2,
    InvalidAmount = 3,
    NotOpen = 4,
    Overpayment = 5,
    AlreadyPaid = 6,
    ArithmeticOverflow = 7,
    InvalidToken = 8,
    /// Reserved. No longer returned: invoices are keyed by `(business, id)`,
    /// so another business's invoice is simply `InvoiceNotFound`.
    NotBusiness = 9,
    NothingPaid = 10,
    /// `payee` is the registry's own address. The registry never holds funds.
    InvalidPayee = 11,
}
